//! R 组：损坏文件的检测与恢复（收尾补充）。
//!
//! 磁盘位损坏、外部工具误写或中断的复制，都会让仓库内 snapshot/delta 的字节与数据库
//! 记录的摘要不再一致。库级单测已覆盖函数层行为，本组补上**真实可执行文件**这一层，
//! 验证使用者实际会遇到的三件事：
//!
//! - **内容级校验**（`scrub`，逐块复算摘要链）必须报错；
//! - **数据库级校验**（`verify`，UUID / 相对路径 / 摘要格式的全表检查）不打开文件内容，
//!   因此仍通过——两层校验的分工由此可断言，也解释了为什么「能打开仓库」不等于「数据完好」；
//! - 恢复损坏的节点必须**拒绝**且不留半成品输出；提交路径不再复检、也不再修复 head
//!   snapshot，内容未变化的提交会直接返回「内容未变化」，损坏留给内容级校验暴露。
//!
//! R1/R2 改的是 `snapshot` / `delta` 的内容；R3/R4 改的是**主库文件本身**
//! （`lilith-artworks.sqlite3`）——这是 R 组此前的直接空白：库级单测覆盖了函数层，
//! 但没有任何用例验证过「主库被改坏」时使用者看到的是**可行动的错误**而不是崩溃、静默
//! 通过或残留半成品。R3 覆盖头部被外部工具误写成非数据库内容，R4 覆盖文件被清空。
//!
//! R7 补上 R1 没覆盖的另一条路径：head snapshot 已损坏、**内容又发生了变化**的提交。
//! 内容变化时提交路径仍要打开旧快照生成反向增量（整份复检已移除），这条链路此前没有
//! 端到端覆盖——必须干净失败、不推进历史、不留半成品、不把损坏扩散到新节点。
//!
//! R5/R6 把「改坏」从**固定位置**扩展成**随机化、逐个文件、多个种子**：R5 对每一个
//! 受管理文件（snapshot / delta）施加确定性随机改写，断言每一次都被检测到；R6 对画板
//! DDS 做结构性损坏与随机改写，并把「结构性检查 vs 内容语义」这条边界如实钉下来。
//!
//! 只改测试自有工作区里文件的字节，不触碰任何产品行为。
//!
//! 运行：`cargo test --features headless --test stress_damage`

// 只在启用 `headless` feature 时编译。理由见 `tests/stress_support/mod.rs` 顶部说明。
#![cfg(feature = "headless")]

mod stress_support;

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;
use stress_support::*;

/// R1：head snapshot 被改坏 → 内容级校验报错、恢复拒绝，数据库仍可打开；随后一次
/// 内容未变化的提交直接返回「内容未变化」，不再复检或改写这份损坏的 snapshot。
#[test]
fn r1_damaged_head_snapshot_is_detected_and_left_alone() {
    let fixture = ArtworkFixture::new();
    let head = fixture.commit(96 * 1024, 1, "baseline");

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

    // 提交路径不再复检旧 snapshot：内容未变化的提交直接返回「内容未变化」，既不
    // 创建新节点，也不改写或回收这份损坏的文件；损坏仍由内容级校验负责暴露。
    write_work_file(&fixture.work, 96 * 1024, 1);
    let unchanged = fixture
        .commit_command()
        .arg("note", "check after damage")
        .finish();
    unchanged.expect_ok();
    assert!(
        unchanged.data("unchanged").as_bool().unwrap_or(false),
        "内容未变化时应按「内容未变化」结果返回：\n{}",
        unchanged.describe()
    );
    let after = assert_no_stray(&fixture.repository);
    assert_eq!(after.snapshots.len(), 1, "{:?}", after.snapshots);
    assert_eq!(
        after.snapshots[0], damaged_relative,
        "损坏的 snapshot 不应被提交路径替换：{:?}",
        after.snapshots
    );
    assert!(
        !fixture.command("scrub").finish().ok(),
        "损坏仍应被内容级校验发现"
    );
    assert!(
        unchanged.data("historyId").is_null(),
        "内容未变化的提交不应返回新节点：\n{}",
        unchanged.describe()
    );
    fixture.assert_repository_clean();

    let restored = fixture.workspace.out("still-damaged.bin");
    let restore = fixture
        .command("restore")
        .arg("history", &head)
        .arg("output", restored.to_string_lossy())
        .finish();
    assert!(
        !restore.ok() && !restore.timed_out,
        "未修复的损坏节点仍须拒绝恢复：\n{}",
        restore.describe()
    );
    assert!(!restored.exists(), "被拒绝的恢复不得产生输出");
}

/// R7：head snapshot 已损坏、**工作文件内容又发生了变化**的提交必须干净失败。
///
/// R1 只覆盖「内容未变化」：那种提交直接返回「内容未变化」，根本不读旧快照。内容变化时
/// 提交仍要打开 head snapshot 生成反向增量——移除提交路径的整份复检后，这条路径没有
/// 端到端测试。断言：错误可读（退出码 1、无 panic）、历史不推进、没有新增 snapshot/delta、
/// 损坏文件不被改写或回收、仓库可重开、恢复旧节点仍被拒绝——即损坏既没有扩散到新节点，
/// 也没有被静默吞掉。
#[test]
fn r7_commit_with_changed_content_over_a_damaged_head_fails_cleanly() {
    let fixture = ArtworkFixture::new();
    let head = fixture.commit(96 * 1024, 1, "baseline");

    let before = assert_no_stray(&fixture.repository);
    assert_eq!(before.snapshots.len(), 1, "{:?}", before.snapshots);
    let damaged_relative = before.snapshots[0].clone();
    let damaged_path = fixture.repository.join(&damaged_relative);

    // 与 R1 相同的损坏手法：翻转 snapshot 最后一个字节。
    let mut damaged_bytes = fs::read(&damaged_path).expect("无法读取 snapshot");
    *damaged_bytes.last_mut().expect("snapshot 不应为空") ^= 0xff;
    fs::write(&damaged_path, &damaged_bytes).expect("无法改写 snapshot");

    // 内容发生变化：这次提交必须打开旧快照生成反向增量，因而必须失败。
    write_work_file(&fixture.work, 96 * 1024, 2);
    let commit = fixture
        .commit_command()
        .arg("note", "changed content over damage")
        .finish();
    assert!(
        !commit.ok() && !commit.timed_out,
        "head snapshot 损坏时，内容变化的提交必须失败：\n{}",
        commit.describe()
    );
    assert_eq!(
        commit.exit_code,
        1,
        "提交失败必须是普通错误（退出码 1）：\n{}",
        commit.describe()
    );
    assert!(
        !commit.stderr.contains("panicked"),
        "提交不得以 panic 收场：\n{}",
        commit.describe()
    );
    assert!(
        !commit.error().is_empty(),
        "失败必须给出可行动的错误信息：\n{}",
        commit.describe()
    );

    // 历史不推进（由 snapshot/delta 计数断言）；数据库级校验不受文件内容影响：
    // 仓库仍可重开（内容级损坏本身仍由 scrub 负责，见 R1，不在这里重复断言）。
    assert_reopens(&fixture.workspace, &fixture.repository);
    let after = assert_no_stray(&fixture.repository);
    assert_eq!(after.snapshots.len(), 1, "{:?}", after.snapshots);
    assert_eq!(after.deltas.len(), 0, "{:?}", after.deltas);
    assert_eq!(
        after.snapshots[0], damaged_relative,
        "损坏的 snapshot 不应被失败提交替换或回收：{:?}",
        after.snapshots
    );
    assert_eq!(
        fs::read(&damaged_path).expect("无法读取 snapshot"),
        damaged_bytes,
        "损坏的 snapshot 字节不应被失败提交改动"
    );

    // 损坏没有扩散：旧节点恢复仍被拒绝（损坏仍在），且没有留下半成品输出。
    let output = fixture.workspace.out("r7-damaged.bin");
    let restore = fixture
        .command("restore")
        .arg("history", &head)
        .arg("output", output.to_string_lossy())
        .finish();
    assert!(
        !restore.ok() && !restore.timed_out,
        "损坏节点的恢复必须仍被拒绝：\n{}",
        restore.describe()
    );
    assert!(!output.exists(), "被拒绝的恢复不得产生输出");
    fixture.assert_repository_clean();

    record(
        "R7 changed content over damaged head",
        "R",
        "small",
        &commit,
        json!({ "snapshot": damaged_relative }),
    );
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

/// 主库文件的路径：仓库根下、以 `.sqlite3` 结尾且不以点开头的那个。
///
/// 用扫描而不是硬编码文件名，是为了不把测试钉死在产品常量上（仓库根还有哨兵锁
/// `.lilith-write-lock.sqlite3`，点号前缀把它排除在外）。若主库改名，这里会给出清晰
/// 的“未找到”失败，而不是悄悄改错文件。
fn main_database(repository: &std::path::Path) -> std::path::PathBuf {
    let candidates = directory_names(repository)
        .into_iter()
        .filter(|name| name.ends_with(".sqlite3") && !name.starts_with('.'))
        .collect::<Vec<_>>();
    assert_eq!(
        candidates.len(),
        1,
        "仓库根应恰有一个主库文件，实际：{candidates:?}"
    );
    repository.join(&candidates[0])
}

/// 断言一次「主库不可用」的校验是**干净失败**：非超时、退出码 1、给出可行动的错误、
/// 没有 Rust panic，且没有因为一次失败的打开在仓库里留下临时文件。
#[track_caller]
fn assert_clean_database_rejection(label: &str, outcome: &Outcome, repository: &std::path::Path) {
    assert!(
        !outcome.ok() && !outcome.timed_out,
        "{label}：损坏的主库必须让校验失败（而不是成功或挂起）：\n{}",
        outcome.describe()
    );
    assert_eq!(
        outcome.exit_code,
        1,
        "{label}：主库不可用应以普通错误退出（退出码 1），而不是崩溃或取消：\n{}",
        outcome.describe()
    );
    assert!(
        outcome.error().contains("数据库"),
        "{label}：错误信息应指向作品数据库，便于用户行动：\n{}",
        outcome.describe()
    );
    assert!(
        !outcome.stderr.contains("panicked"),
        "{label}：不应以 panic 收场：\n{}",
        outcome.describe()
    );
    assert_no_stray(repository);
}

/// R3：主库文件头部被外部工具误写成非数据库内容 → 数据库级校验与内容级校验都必须
/// **干净失败**（可行动的报错、退出码 1、无 panic、无残留），而不是静默通过或崩溃。
#[test]
fn r3_corrupted_main_database_is_rejected_cleanly() {
    let fixture = ArtworkFixture::new();
    fixture.commit(96 * 1024, 1, "baseline");
    // 改写前先确认仓库本来就是健康的，避免把别处的失败误读成这个场景的结果。
    assert_eq!(fixture.healthy(), 1);

    let database = main_database(&fixture.repository);
    assert!(database.is_file(), "{}", database.display());
    let mut bytes = fs::read(&database).expect("无法读取主库");
    assert!(bytes.len() > 64, "主库文件过小，样本不具代表性");
    // 覆盖 SQLite 文件头（前 16 字节是 "SQLite format 3\0" 魔数区）：等价于一次位损坏或
    // 外部工具误写。文件长度不变，因此 `require_database_file` 不会把它当作“文件缺失”。
    for (index, byte) in bytes.iter_mut().take(16).enumerate() {
        *byte = (index as u8).wrapping_mul(7).wrapping_add(3);
    }
    fs::write(&database, &bytes).expect("无法改写主库头部");

    let verify = fixture.command("verify").finish();
    assert_clean_database_rejection("R3 verify", &verify, &fixture.repository);

    // 内容级校验同样要打开主库，因此也必须失败——两层校验在这里不再分歧。
    let scrub = fixture.command("scrub").finish();
    assert_clean_database_rejection("R3 scrub", &scrub, &fixture.repository);

    record(
        "R3 corrupted main database detected",
        "R",
        "small",
        &verify,
        json!({ "database": main_database(&fixture.repository).file_name().map(|n| n.to_string_lossy().into_owned()) }),
    );
}

/// R4：主库被清空（0 字节）→ 校验必须**干净失败**并明确提示“缺少数据库 / 请从备份恢复”，
/// 而不是被当成空仓库重新初始化（那会静默丢弃用户的历史）。
#[test]
fn r4_emptied_main_database_is_rejected_cleanly() {
    let fixture = ArtworkFixture::new();
    fixture.commit(96 * 1024, 2, "baseline");
    assert_eq!(fixture.healthy(), 1);

    let database = main_database(&fixture.repository);
    fs::write(&database, b"").expect("无法清空主库");
    assert_eq!(file_bytes(&database), 0, "主库应已被清空");

    // `verify` 走 `open_existing`：清空的主库应被拒绝，且错误提示指向缺失/需恢复。
    let verify = fixture.command("verify").finish();
    assert_clean_database_rejection("R4 verify", &verify, &fixture.repository);
    assert!(
        verify.error().contains("恢复") || verify.error().contains("缺少"),
        "空主库的错误应提示用户仓库数据缺失并需恢复：\n{}",
        verify.describe()
    );

    // 关键边界：失败的打开不得把空文件“修好”成一个全新的空仓库——若如此，用户的历史
    // 会被静默丢弃。主库必须仍然是 0 字节，且没有新建出任何数据库。
    assert_eq!(
        file_bytes(&database),
        0,
        "被拒绝的打开不得重建或初始化主库：{}",
        database.display()
    );

    record(
        "R4 emptied main database detected",
        "R",
        "small",
        &verify,
        json!({ "databaseBytesAfter": file_bytes(&database) }),
    );
}

/// R5：随机化单点损坏——对仓库内**每一个**受管理文件（snapshot / delta）做多轮确定性
/// 随机改写，每一次都必须被内容级校验发现。
///
/// R1/R2 改的是固定位置（文件末尾、整文件）；这一条补的是「损坏落在哪里都可能」这个更接近
/// 真实位损坏的形态，并且逐个文件、多个种子地扫，避免只覆盖到一种布局。每轮都断言：
/// 被检测到、以普通错误退出（退出码 1）、无 panic，而**数据库级校验不受影响**——两层校验
/// 的分工在任意损坏位置下都成立。
#[test]
fn r5_randomized_single_point_damage_is_always_detected() {
    let fixture = ArtworkFixture::new();
    let first = fixture.commit(96 * 1024, 1, "first");
    fixture.commit(80 * 1024, 2, "second");
    fixture.commit(64 * 1024, 3, "head");

    let state = assert_no_stray(&fixture.repository);
    let mut managed = state.snapshots.clone();
    managed.extend(state.deltas.iter().cloned());
    assert_eq!(
        managed.len(),
        3,
        "三节点链应为 1 枚 snapshot + 2 条 delta：{managed:?}"
    );

    let pristine = managed
        .iter()
        .map(|relative| {
            let bytes = fs::read(fixture.repository.join(relative))
                .unwrap_or_else(|error| panic!("无法读取 {relative}：{error}"));
            assert!(!bytes.is_empty(), "{relative} 不应为空");
            (relative.clone(), bytes)
        })
        .collect::<Vec<_>>();

    let mut damage_cases = 0_usize;
    for (file_index, (relative, original)) in pristine.iter().enumerate() {
        let path = fixture.repository.join(relative);
        for variant in 0..4_usize {
            let seed = (file_index as u64 + 1) * 1_000 + variant as u64;
            let mut damaged = original.clone();
            if variant % 2 == 0 {
                // 单点改写：偏移由种子决定，覆盖文件的首、中、尾各处。
                let offset = (seed as usize * 7919) % damaged.len();
                damaged[offset] ^= pattern_bytes(8, seed)[0] | 1;
            } else {
                // 只在头部改：更容易命中「声明的元数据」（魔数 / 计数 / 长度）分支。
                let offset = (seed as usize * 104_729) % 64.min(damaged.len());
                damaged[offset] = damaged[offset].wrapping_add(37);
            }
            assert_ne!(damaged, *original, "seed={seed}：改写必须真的改动字节");
            fs::write(&path, &damaged).expect("无法写入损坏文件");

            let scrub = fixture.command("scrub").finish();
            assert!(
                !scrub.ok() && !scrub.timed_out,
                "seed={seed}（{relative}）的单点损坏必须被全库校验发现：\n{}",
                scrub.describe()
            );
            assert_eq!(
                scrub.exit_code,
                1,
                "seed={seed}：损坏应以普通错误退出（退出码 1）：\n{}",
                scrub.describe()
            );
            assert!(
                !scrub.stderr.contains("panicked"),
                "seed={seed}：不应以 panic 收场：\n{}",
                scrub.describe()
            );
            damage_cases += 1;

            // 数据库级校验不读取文件内容：任意位置的损坏都不影响它。
            if variant == 0 {
                assert_reopens(&fixture.workspace, &fixture.repository);
            }
        }
        // 复原：下一个文件从干净状态开始。
        fs::write(&path, original).expect("无法复原受管理文件");
    }
    let all_cases = damage_cases;

    // 复原之后一切如常：全库校验通过，最早版本仍能逐位恢复。
    fixture.command("scrub").finish().expect_ok();
    assert_eq!(fixture.healthy(), 3);
    let output = fixture.workspace.out("r5-oldest.bin");
    fixture
        .command("restore")
        .arg("history", &first)
        .arg("output", output.to_string_lossy())
        .finish()
        .expect_ok();
    assert_eq!(
        fs::read(&output).expect("无法读取恢复输出"),
        pattern_bytes(96 * 1024, 1),
        "复原后必须能恢复出最早的版本"
    );
    fs::remove_file(&output).expect("无法清理恢复输出");
    fixture.assert_repository_clean();

    record(
        "R5 randomized single-point damage",
        "R",
        "small",
        &fixture.command("scrub").finish(),
        json!({ "files": managed.len(), "cases": all_cases }),
    );
}

/// 画板 DDS 的落盘目录：`<repository>/artworks/<artwork-id>/boards/<board-id>`。
fn board_directory(repository: &Path, artwork_id: &str, board_id: i64) -> PathBuf {
    repository
        .join("artworks")
        .join(artwork_id)
        .join("boards")
        .join(board_id.to_string())
}

/// 按类型损坏一份 DDS 字节；偏移取自 DDS 的 DX10 头（148 字节）内的固定字段。
fn damage_dds(kind: &str, original: &[u8]) -> Vec<u8> {
    let mut bytes = original.to_vec();
    match kind {
        "magic" => bytes[0..4].copy_from_slice(b"XXXX"),
        "dx10" => bytes[84..88].copy_from_slice(b"XXXX"),
        "format" => bytes[128..132].copy_from_slice(&7_u32.to_le_bytes()),
        "width-zero" => bytes[16..20].copy_from_slice(&0_u32.to_le_bytes()),
        "height-huge" => bytes[12..16].copy_from_slice(&70_000_u32.to_le_bytes()),
        "truncated" => bytes.truncate(64),
        "payload" => {
            // 只动 148 字节头之后的负载（BC7 数据）里一个字节。
            let last = bytes.len() - 1;
            bytes[last] ^= 0xff;
        }
        other => panic!("未知的 DDS 损坏类型：{other}"),
    }
    bytes
}

/// R6：画板 DDS 的畸形输入（真实文件 + 真实扫描命令）。
///
/// 三类断言分开写，避免把「结构性损坏」与「内容语义」混为一谈：
/// ① **结构性损坏**（魔数 / DX10 fourcc / BC7 格式 / 声明尺寸 / 被截断）必须被
///    `scrub-board-dds` 报为 `corrupt`；
/// ② 只改负载字节**不会**被发现——当前扫描检查的是结构而不是像素内容（没有原始素材时
///    内容语义不可判定）。这一条是对既有语义的**特性记录**，同时是本组的边界；
///    若哪天给 DDS 加上内容摘要，这里会先失败，提示同步更新报告；
/// ③ 任意随机改写都必须让扫描**照常成功返回**一份报告，不崩溃、不超时。
#[test]
fn r6_board_dds_damage_is_reported_without_crashing() {
    let fixture = ArtworkFixture::new();
    let board = fixture
        .command("create-board")
        .arg("artwork", &fixture.artwork_id)
        .arg("title", "Board")
        .finish();
    board.expect_ok();
    let board_id = board
        .data("boardId")
        .as_i64()
        .unwrap_or_else(|| panic!("创建画板未返回 boardId：\n{}", board.describe()));
    let mut revision = board.data_str("revision");
    assert!(!revision.is_empty(), "{}", board.describe());

    let images_dir = fixture.workspace.work("dds-fuzz");
    fs::create_dir_all(&images_dir).expect("无法创建源图目录");
    let mut paths = Vec::new();
    for index in 0..3_u32 {
        let path = images_dir.join(format!("img_{index}.png"));
        let image = image::RgbaImage::from_fn(64, 64, |x, y| {
            let value = (x.wrapping_mul(31) ^ y.wrapping_mul(17) ^ index.wrapping_mul(7)) as u8;
            image::Rgba([value, value.wrapping_mul(3), value.wrapping_add(11), 255])
        });
        image.save(&path).expect("无法写入 PNG 源图");
        paths.push(path);
    }
    let joined = paths
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(",");
    let imported = fixture
        .command("import-board-images")
        .arg("board", board_id.to_string())
        .arg("revision", &revision)
        .arg("paths", joined)
        .finish();
    imported.expect_ok();
    revision = imported.data_str("revision");
    assert!(!revision.is_empty(), "{}", imported.describe());
    let ids = imported
        .data("imageIds")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|value| value.as_i64().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(ids.len(), 3, "{}", imported.describe());

    let board_dir = board_directory(&fixture.repository, &fixture.artwork_id, board_id);
    let files = ids
        .iter()
        .map(|id| board_dir.join(format!("{id}.dds")))
        .collect::<Vec<_>>();
    let pristine = files
        .iter()
        .map(|path| fs::read(path).expect("无法读取 DDS"))
        .collect::<Vec<_>>();
    for bytes in &pristine {
        assert!(bytes.len() > 148, "DDS 应含完整的 148 字节头");
    }

    // 对照：健康时四类计数都是「全正常」。
    let healthy = fixture.command("scrub-board-dds").finish();
    healthy.expect_ok();
    assert_eq!(healthy.data_u64("images"), 3, "{}", healthy.describe());
    assert_eq!(healthy.data_u64("corrupt"), 0);
    assert_eq!(healthy.data_u64("missing"), 0);

    // ① 结构性损坏必须被报为 corrupt。
    let target = files[0].clone();
    for kind in [
        "magic",
        "dx10",
        "format",
        "width-zero",
        "height-huge",
        "truncated",
    ] {
        fs::write(&target, damage_dds(kind, &pristine[0])).expect("无法写入损坏 DDS");
        let report = fixture.command("scrub-board-dds").finish();
        report.expect_ok();
        assert_eq!(
            report.data_u64("corrupt"),
            1,
            "结构性损坏 `{kind}` 必须被报为 corrupt：\n{}",
            report.describe()
        );
        assert_eq!(report.data_u64("images"), 3, "{}", report.describe());
    }

    // ② 只改负载：结构未变，因此不会被报为损坏（当前语义边界）。
    fs::write(&target, damage_dds("payload", &pristine[0])).expect("无法写入损坏 DDS");
    let payload_report = fixture.command("scrub-board-dds").finish();
    payload_report.expect_ok();
    assert_eq!(
        payload_report.data_u64("corrupt"),
        0,
        "扫描检查结构而不是像素内容，只改负载不会被发现（语义边界，见报告第 5 节）：\n{}",
        payload_report.describe()
    );

    // ③ 任意随机改写：扫描必须照常成功返回一份报告。
    let mut random_cases = 0_usize;
    for seed in 0..24_u64 {
        let index = (seed as usize * 7) % files.len();
        let mut bytes = pristine[index].clone();
        let offset = (seed as usize * 6151 + 7) % bytes.len();
        bytes[offset] ^= pattern_bytes(4, seed)[0] | 1;
        fs::write(&files[index], &bytes).expect("无法写入损坏 DDS");

        let report = fixture.command("scrub-board-dds").finish();
        assert!(
            !report.timed_out && report.ok(),
            "seed={seed}：任意随机改写后扫描仍必须成功返回报告：\n{}",
            report.describe()
        );
        assert_eq!(report.data_u64("images"), 3, "{}", report.describe());
        assert!(
            report.data_u64("corrupt") + report.data_u64("missing") <= 3,
            "seed={seed}：计数不应超过图片总数：\n{}",
            report.describe()
        );
        random_cases += 1;
    }

    // 复原后恢复正常（复原本身也是一次断言）。
    for (path, bytes) in files.iter().zip(&pristine) {
        fs::write(path, bytes).expect("无法复原 DDS");
    }
    let restored = fixture.command("scrub-board-dds").finish();
    restored.expect_ok();
    assert_eq!(restored.data_u64("corrupt"), 0, "{}", restored.describe());
    assert_eq!(restored.data_u64("missing"), 0, "{}", restored.describe());
    fixture.assert_repository_clean();

    record(
        "R6 board dds damage",
        "R",
        "small",
        &restored,
        json!({ "images": ids.len(), "structuralCases": 6, "randomCases": random_cases }),
    );
}
