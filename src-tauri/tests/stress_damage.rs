//! R 组：损坏文件的检测与恢复（收尾补充）。
//!
//! 磁盘位损坏、外部工具误写或中断的复制，都会让仓库内 snapshot/delta 的字节与数据库
//! 记录的摘要不再一致。库级单测已覆盖函数层行为，本组补上**真实可执行文件**这一层，
//! 验证使用者实际会遇到的三件事：
//!
//! - **内容级校验**（`scrub`，逐块复算摘要链）必须报错；
//! - **数据库级校验**（`verify`，UUID / 相对路径 / 摘要格式的全表检查）不打开文件内容，
//!   因此仍通过——两层校验的分工由此可断言，也解释了为什么「能打开仓库」不等于「数据完好」；
//! - 恢复损坏的节点必须**拒绝**且不留半成品输出；head snapshot 损坏后，一次内容未变化的
//!   提交会重新发布 snapshot 并把它修好（错误恢复路径）。
//!
//! 只改测试自有工作区里文件的字节，不触碰任何产品行为。
//!
//! 运行：`cargo test --features headless --test stress_damage`

mod stress_support;

use std::fs;

use serde_json::json;
use stress_support::*;

/// R1：head snapshot 被改坏 → 内容级校验报错、恢复拒绝，数据库仍可打开；随后一次
/// 内容未变化的提交把它修好。
#[test]
fn r1_damaged_head_snapshot_is_detected_and_repaired() {
    let fixture = ArtworkFixture::new();
    let head = fixture.commit(96 * 1024, 1, "baseline");
    let expected = pattern_bytes(96 * 1024, 1);

    let before = assert_no_stray(&fixture.repository);
    assert_eq!(
        before.snapshots.len(),
        1,
        "单节点分支只持有一枚 snapshot：{:?}",
        before.snapshots
    );
    let damaged_relative = before.snapshots[0].clone();
    let damaged = fixture.repository.join(&damaged_relative);

    // 翻转 snapshot 的最后一个字节：等价于一次位损坏或外部工具误写。
    let mut bytes = fs::read(&damaged).expect("无法读取 snapshot");
    *bytes.last_mut().expect("snapshot 不应为空") ^= 0xff;
    fs::write(&damaged, &bytes).expect("无法改写 snapshot");

    // 内容级校验必须发现损坏。
    let scrub = fixture.command("scrub").finish();
    assert!(
        !scrub.ok() && !scrub.timed_out,
        "损坏的 snapshot 必须让全库校验失败：\n{}",
        scrub.describe()
    );
    assert!(!scrub.error().is_empty(), "{}", scrub.describe());

    // 数据库级校验不读取文件内容，因此仓库仍可打开——「能打开」不等于「数据完好」。
    assert_reopens(&fixture.workspace, &fixture.repository);

    // 恢复该节点必须被拒绝，且不留下半成品输出。
    let output = fixture.workspace.out("damaged.bin");
    let restore = fixture
        .command("restore")
        .arg("history", &head)
        .arg("output", output.to_string_lossy())
        .finish();
    assert!(
        !restore.ok() && !restore.timed_out,
        "损坏节点的恢复必须被拒绝：\n{}",
        restore.describe()
    );
    assert!(
        !output.exists(),
        "被拒绝的恢复不得产生输出：{}",
        output.display()
    );

    record(
        "R1 damaged head snapshot detected",
        "R",
        "small",
        &scrub,
        json!({ "snapshot": damaged_relative }),
    );

    // 正向对照（错误恢复）：内容未变化的提交重新生成并发布 head snapshot，旧的损坏
    // 文件被回收；随后全库校验与恢复都恢复正常。
    write_work_file(&fixture.work, 96 * 1024, 1);
    let repaired = fixture
        .commit_command()
        .arg("note", "repair after damage")
        .finish();
    repaired.expect_ok();
    assert!(
        repaired.data("unchanged").as_bool().unwrap_or(false),
        "内容未变化时应按「内容未变化」结果返回：\n{}",
        repaired.describe()
    );
    let after = assert_no_stray(&fixture.repository);
    assert!(
        !after.snapshots.contains(&damaged_relative),
        "损坏的 snapshot 应被新文件替换：{:?}",
        after.snapshots
    );
    assert_eq!(after.snapshots.len(), 1, "{:?}", after.snapshots);

    fixture.command("scrub").finish().expect_ok();
    assert_eq!(fixture.healthy(), 1, "修复不创建新节点");
    fixture.assert_repository_clean();

    let restored = fixture.workspace.out("repaired.bin");
    fixture
        .command("restore")
        .arg("history", &head)
        .arg("output", restored.to_string_lossy())
        .finish()
        .expect_ok();
    assert_eq!(
        fs::read(&restored).expect("无法读取恢复输出"),
        expected,
        "修复后必须能恢复出与损坏前逐位一致的内容"
    );
    let _ = fs::remove_file(&restored);
}

/// R2：delta 被改坏 → 内容级校验报错（失败原因指向 delta），但数据库级校验与 head 的
/// 恢复都不受影响（head 自身持有 snapshot，不依赖这条边 delta）。
#[test]
fn r2_damaged_delta_is_detected_by_chain_scrub() {
    let fixture = ArtworkFixture::new();
    fixture.commit(96 * 1024, 1, "first");
    let head = fixture.commit(80 * 1024, 2, "second");

    let state = assert_no_stray(&fixture.repository);
    assert_eq!(
        state.deltas.len(),
        1,
        "两节点链恰有一条边 delta：{:?}",
        state.deltas
    );
    let delta_relative = state.deltas[0].clone();
    fs::write(fixture.repository.join(&delta_relative), b"damaged delta").expect("无法改写 delta");

    let scrub = fixture.command("scrub").finish();
    assert!(
        !scrub.ok() && !scrub.timed_out,
        "损坏的 delta 必须让全库校验失败：\n{}",
        scrub.describe()
    );
    assert!(
        scrub.error().contains("delta"),
        "失败原因应指向 delta：\n{}",
        scrub.describe()
    );

    // 数据库级校验不受文件内容影响。
    assert_reopens(&fixture.workspace, &fixture.repository);

    // head 自身持有 snapshot，恢复不需要这条 delta，因此仍成功且逐位一致。
    let output = fixture.workspace.out("head.bin");
    fixture
        .command("restore")
        .arg("history", &head)
        .arg("output", output.to_string_lossy())
        .finish()
        .expect_ok();
    assert_files_identical(&fixture.work, &output);
    let _ = fs::remove_file(&output);

    record(
        "R2 damaged delta detected",
        "R",
        "small",
        &scrub,
        json!({ "delta": delta_relative }),
    );
}
