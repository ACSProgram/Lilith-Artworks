//! 多进程并发边界：仓库哨兵锁、高频连续命令与「写 + 读」交错。
//!
//! - **哨兵锁**：会写仓库的无头命令独占仓库；第二个写进程必须拿到**明确拒绝**，
//!   而不是与前者在业务层竞态。锁由 SQLite 的 `BEGIN IMMEDIATE` 持有，因此强杀
//!   不会残留锁文件——这一点由「被强杀的写进程之后仍能取得锁」验证。
//! - **高频命令**：连续快速提交后仓库仍自洽（历史计数正确、无残留、可整体校验）。
//! - **J 组（多操作交错）**：写进程持锁时（提交 / 整仓灾备 / 精简），只读命令必须仍能
//!   返回并看到一致的旧状态；读 + 读互不阻塞；真正并发的「结构写 + 全库读」不得损坏仓库。
//!   这是「多操作交错」的采样——A–I 九组此前都是「单操作 × 单故障」的稀疏采样。
//!
//! 这一组覆盖的正是 issue 里登记的缺口：数据规模早有覆盖（D 组），但「命令并发」
//! 与「同一仓库多进程写入」此前未设计也未测试。
//!
//! 它**不**验证「两个进程成功并发写同一仓库」——那正是产品明确不支持的边界，
//! 由哨兵锁拒绝。运行方式见 `docs/guides/validation.md`。
#![cfg(feature = "headless")]

mod stress_support;

use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use stress_support::*;

/// 第一个写进程持有仓库期间，第二个写进程必须被明确拒绝。
#[test]
fn second_writer_is_rejected_while_the_first_holds_the_repository() {
    let fixture = ArtworkFixture::new();
    fixture.commit(96 * 1024, 2, "first");
    // 改动工作文件，确保被闸门拦住的这次提交是新内容。
    fs::write(&fixture.work, vec![7_u8; 96 * 1024]).expect("无法改写工作文件");

    let mut rejected = None;
    let outcome = fixture.commit_command().gate().drive(|index, _stage| {
        if index == 1 && rejected.is_none() {
            // A 持有锁停在该检查点：此刻 B 必须被明确拒绝。
            rejected = Some(fixture.commit_command().finish());
        }
        false
    });

    outcome.expect_ok();
    let rejected = rejected.expect("第二个写进程应当已被启动");
    assert!(
        !rejected.ok(),
        "第二个写进程不应成功：\n{}",
        rejected.describe()
    );
    assert!(
        rejected.error().contains("多进程写入"),
        "拒绝理由应指明多进程写入边界：\n{}",
        rejected.describe()
    );

    // 仓库未被破坏：历史自洽、无残留。
    fixture.healthy();
    fixture.assert_repository_clean();
}

/// 被强杀的写进程不会把仓库永久锁死：随后另一个写进程仍能取得锁并提交成功。
///
/// 这是选 SQLite 写锁而不是锁文件的理由——锁由内核在句柄关闭时释放，本项目大量
/// 的强杀场景不会制造「残留锁」这种只能靠人工清理的死结。
#[test]
fn a_killed_writer_does_not_leave_the_repository_locked() {
    let fixture = ArtworkFixture::new();
    fixture.commit(96 * 1024, 2, "first");
    fs::write(&fixture.work, vec![9_u8; 96 * 1024]).expect("无法改写工作文件");

    // 让第一个写进程停在检查点（此时它已经持有锁），然后强杀。
    let killed = fixture
        .commit_command()
        .gate()
        .start()
        .kill_at(|index, _stage| index == 1);
    assert!(killed.killed, "写进程应当被强杀：{}", killed.describe());

    // 强杀之后，另一个写进程仍能取得锁并成功提交。
    fs::write(&fixture.work, vec![11_u8; 96 * 1024]).expect("无法改写工作文件");
    let follow_up = fixture.commit_command().finish();
    follow_up.expect_ok();

    fixture.healthy();
    fixture.assert_repository_clean();
}

/// 高频连续命令：快速连续提交后仓库最终一致。
///
/// 每次提交是一个独立的写进程，因此这条用例同时也是「锁在每次结束后都被正确释放、
/// 下一次能重新取得」的连续验证。末尾带一个效率护栏：20 次完整提交（每次一个进程）
/// 不应慢到不可接受。
#[test]
fn rapid_sequential_commands_leave_the_repository_consistent() {
    let fixture = ArtworkFixture::new();
    let started = Instant::now();
    for round in 0..20_u64 {
        fixture.commit(32 * 1024, round + 1, &format!("rapid {round}"));
    }
    let elapsed = started.elapsed();

    assert_eq!(fixture.healthy(), 20);
    fixture.assert_repository_clean();
    assert!(
        elapsed < Duration::from_secs(180),
        "20 次连续提交耗时过长：{elapsed:?}"
    );
}

/// 读命令返回的树值（`data`），用于比较写进程持锁期间读到的状态是否与之前一致。
fn tree_value(fixture: &ArtworkFixture) -> Value {
    let outcome = fixture.command("list-tree").finish();
    outcome.expect_ok();
    outcome.result["data"].clone()
}

/// J1：写进程持锁期间，只读命令必须仍能返回，并看到与写之前**一致**的状态。
///
/// 写进程停在两个有代表性的位置：
/// - **检查点 1（尚未发布任何文件）**：写进程只持有哨兵锁，磁盘与数据库都还是旧状态；
/// - **检查点 5（快照已发布、数据库事务尚未提交）**：磁盘上已有新文件，但历史尚未推进。
///   这是最能暴露撕裂读的窗口——读命令必须看到旧 head，而不是半成品。
///
/// 断言三件事：读命令成功且快速返回（未被写进程的锁挡住）、`list-tree` 与写前逐字段一致、
/// 历史节点数未推进。写完后再做一次正向对照，确认写进程正常收尾。
#[test]
fn j1_read_commands_answer_while_a_write_holds_the_repository_lock() {
    for park in [1_usize, COMMIT_AFTER_PUBLISH_CHECKPOINT] {
        let fixture = ArtworkFixture::new();
        fixture.commit(96 * 1024, 1, "baseline");
        assert_eq!(fixture.healthy(), 1);
        let before_tree = tree_value(&fixture);

        // 改动工作文件，确保被闸门拦住的这次提交是新内容。
        fs::write(&fixture.work, vec![0x5a; 96 * 1024]).expect("无法改写工作文件");

        let mut reads: Vec<(String, stress_support::Outcome)> = Vec::new();
        let outcome = fixture.commit_command().gate().drive(|index, _stage| {
            if index == park {
                // 此刻写进程阻塞在该检查点并持有哨兵锁：读命令必须照常回答。
                reads.push(("list-tree".into(), fixture.command("list-tree").finish()));
                reads.push(("verify".into(), fixture.command("verify").finish()));
                reads.push((
                    "list-history".into(),
                    fixture
                        .command("list-history")
                        .arg("artwork", &fixture.artwork_id)
                        .finish(),
                ));
            }
            false
        });
        outcome.expect_ok();

        for (label, read) in &reads {
            assert!(
                !read.timed_out && read.ok(),
                "写进程持锁（检查点 {park}）期间，只读命令 `{label}` 必须成功返回：\n{}",
                read.describe()
            );
            assert!(
                read.elapsed < Duration::from_secs(30),
                "只读命令 `{label}` 被写进程挡住过久（{:?}），哨兵锁不应影响只读命令",
                read.elapsed
            );
        }
        // 最后一个读到的树必须与写之前完全一致：写入尚未提交，历史不得推进。
        let history = &reads[2].1;
        assert_eq!(
            history.data_u64("nodeCount"),
            1,
            "写进程持锁（检查点 {park}）期间读到的历史不应被推进：\n{}",
            history.describe()
        );
        assert_eq!(
            reads[0].1.result["data"], before_tree,
            "写进程持锁（检查点 {park}）期间读到的树必须与写之前一致"
        );

        // 正向对照：写完收尾后仓库仍健康，树只在历史推进后变化。
        assert_eq!(fixture.healthy(), 2);
    }
}

/// J2：读 + 读 并发——多个只读命令同时运行，互不阻塞且都成功。
///
/// 只读命令走共享路径（不取哨兵锁），因此并发读之间不应有任何串行化。
#[test]
fn j2_concurrent_readers_do_not_block_each_other() {
    let fixture = ArtworkFixture::new();
    fixture.commit(96 * 1024, 1, "first");
    fixture.commit(80 * 1024, 2, "second");
    let before_tree = tree_value(&fixture);

    let outcomes: Vec<stress_support::Outcome> = thread::scope(|scope| {
        let handles = (0..4)
            .map(|slot| {
                let fixture = &fixture;
                scope.spawn(move || match slot % 2 {
                    0 => fixture.command("list-tree").finish(),
                    _ => fixture.command("verify").finish(),
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("读线程不应 panic"))
            .collect()
    });

    assert_eq!(outcomes.len(), 4);
    for (index, outcome) in outcomes.iter().enumerate() {
        assert!(
            !outcome.timed_out && outcome.ok(),
            "并发读 #{index} 必须成功返回：\n{}",
            outcome.describe()
        );
    }
    // 并发读之后状态不变。
    assert_eq!(tree_value(&fixture), before_tree);
    assert_eq!(fixture.healthy(), 2);
    fixture.assert_repository_clean();
}

/// 一次「写进程持锁期间，只读命令必须照常回答」的采样：跑三个只读命令并断言它们
/// 成功、快速返回，且读到的历史节点数仍是 `expected_nodes`（写入尚未提交）。
///
/// J1、J3、J4 共用这一份断言，避免三处各写一遍同样的检查。
fn assert_reads_answer_while_locked(
    fixture: &ArtworkFixture,
    label: &str,
    expected_nodes: u64,
) -> Vec<(String, Outcome)> {
    let reads = vec![
        (
            "list-tree".to_owned(),
            fixture.command("list-tree").finish(),
        ),
        ("verify".to_owned(), fixture.command("verify").finish()),
        (
            "list-history".to_owned(),
            fixture
                .command("list-history")
                .arg("artwork", &fixture.artwork_id)
                .finish(),
        ),
    ];
    for (name, read) in &reads {
        assert!(
            !read.timed_out && read.ok(),
            "{label}：写进程持锁期间只读命令 `{name}` 必须成功返回：\n{}",
            read.describe()
        );
        assert!(
            read.elapsed < Duration::from_secs(30),
            "{label}：只读命令 `{name}` 被写进程挡住过久（{:?}），只读路径不应取写锁",
            read.elapsed
        );
    }
    let history = &reads[2].1;
    assert_eq!(
        history.data_u64("nodeCount"),
        expected_nodes,
        "{label}：持锁期间读到的历史不应被推进：\n{}",
        history.describe()
    );
    reads
}

/// J3：整仓灾备（**长写**任务）持锁期间，只读命令仍必须回答且状态一致。
///
/// `repository-backup` 是典型的「持锁以分钟计」的操作（逐个复制仓库文件），而它并不改动
/// 源仓库，因此读命令不仅不能被挡住，读到的还必须是精确的旧状态。与 J1（短写：提交）互补。
#[test]
fn j3_reads_answer_while_a_repository_backup_holds_the_lock() {
    let fixture = ArtworkFixture::new();
    fixture.commit(96 * 1024, 1, "first");
    fixture.commit(80 * 1024, 2, "second");
    let destination = fixture.workspace.backup_directory();

    let mut reads: Vec<(String, Outcome)> = Vec::new();
    let outcome = fixture
        .command("repository-backup")
        .arg("destination", destination.to_string_lossy())
        .gate()
        .drive(|_index, stage| {
            if reads.is_empty() && stage.contains("正在复制仓库文件") {
                reads = assert_reads_answer_while_locked(&fixture, "J3", 2);
            }
            false
        });
    outcome.expect_ok();
    assert!(
        !reads.is_empty(),
        "应当有检查点落在「正在复制仓库文件」阶段：\n{}",
        outcome.describe()
    );

    // 灾备产物必须可独立打开并通过完整性与链路校验。
    let copy = PathBuf::from(outcome.data_str("repositoryPath"));
    assert!(copy.is_dir(), "{}", outcome.describe());
    headless(&fixture.workspace, "verify")
        .repository(&copy)
        .finish()
        .expect_ok();
    let copy_scrub = headless(&fixture.workspace, "scrub")
        .repository(&copy)
        .finish();
    copy_scrub.expect_ok();
    assert_eq!(copy_scrub.data_u64("historyNodes"), 2);

    // 源仓库不受影响。
    assert_eq!(fixture.healthy(), 2);
    fixture.assert_repository_clean();
    record(
        "J3 reads during repository backup",
        "J",
        "small",
        &outcome,
        json!({ "reads": ["list-tree", "verify", "list-history"] }),
    );
}

/// J4：结构写（精简）持锁期间，只读命令仍必须回答，并看到**改接之前**的一致状态。
///
/// 精简是本项目里最「伤筋动骨」的写：它改接历史链并回收被移除节点的实体文件。A3 覆盖了
/// 它在每个检查点被取消时的收尾；这里补同一批检查点上**读者的视角**——写进程停在任何
/// 一个检查点时读命令都必须能返回，且看到完整的旧链（树逐字段一致、节点数不变）。
#[test]
fn j4_reads_answer_while_a_compaction_holds_the_lock() {
    // 精简的 6 个检查点：父链回溯入口 + 2 跳、子链回溯入口、delta 发布前两处（同 A3）。
    for target in 1..=6_usize {
        let fixture = ArtworkFixture::new();
        let first = fixture.commit(96 * 1024, 1, "first");
        let middle = fixture.commit(80 * 1024, 2, "middle");
        fixture.commit(64 * 1024, 3, "head");
        let before_tree = tree_value(&fixture);

        let mut reads: Vec<(String, Outcome)> = Vec::new();
        let outcome = fixture
            .command("compact")
            .arg("history", &middle)
            .gate()
            .drive(|index, _stage| {
                if index == target && reads.is_empty() {
                    reads = assert_reads_answer_while_locked(&fixture, "J4", 3);
                }
                index == target
            });
        outcome.expect_cancelled();
        assert!(
            !reads.is_empty(),
            "检查点 {target} 未被命中：\n{}",
            outcome.describe()
        );
        assert_eq!(
            reads[0].1.result["data"], before_tree,
            "精简持锁期间读到的树必须与改接之前逐字段一致"
        );

        // 取消之后旧链完好：最早节点仍能逐位恢复。
        fixture.assert_repository_clean();
        assert_eq!(fixture.healthy(), 3);
        let output = fixture.workspace.out(&format!("j4-{target}.bin"));
        fixture
            .command("restore")
            .arg("history", &first)
            .arg("output", output.to_string_lossy())
            .finish()
            .expect_ok();
        assert_eq!(
            fs::read(&output).expect("无法读取恢复输出"),
            pattern_bytes(96 * 1024, 1),
            "取消精简后最早的节点仍必须能完整恢复"
        );
        fs::remove_file(&output).expect("无法清理恢复输出");

        record(
            "J4 reads during compact",
            "J",
            "small",
            &outcome,
            json!({ "checkpoint": target, "reads": ["list-tree", "verify", "list-history"] }),
        );
    }

    // 正面对照：精简真正完成后链仍可解析、节点数减一。
    let fixture = ArtworkFixture::new();
    fixture.commit(96 * 1024, 1, "first");
    let middle = fixture.commit(80 * 1024, 2, "middle");
    fixture.commit(64 * 1024, 3, "head");
    fixture
        .command("compact")
        .arg("history", &middle)
        .finish()
        .expect_ok();
    assert_eq!(fixture.healthy(), 2);
    fixture.assert_repository_clean();
}

/// J5：真正并发的「结构写 + 全库读」（不设闸门）。
///
/// J1/J3/J4 都是把写进程停在检查点上、再让读命令跑——确定性高，但读者与写者的危险窗口是
/// **构造**出来的。这一条不构造窗口：让 `compact` 与 `scrub` 真正同时启动、各自跑完，
/// 重复多轮。断言三条不变量：
/// ① 两者都不得挂起；
/// ② `compact` **要么成功、要么拿到那个明确的「多进程写入」拒绝**——`scrub` 的哨兵
///    **探测**本身也用 `BEGIN IMMEDIATE`，零等待下可与同时启动的写者抢到锁，因此精简可能
///    拿到拒绝；这是产品已知的不对称，断言对齐「第二个写者拿到明确拒绝」这条实际承诺，
///    而不是要求一个产品从未承诺过的结果。`scrub` 成功、以「仓库正在被写入，请稍后重试」
///    **明确拒绝**，或以退出码 1 干净失败（无 panic）；
/// ③ 两者都结束后仓库必须完整：被拒绝的精简随后重试一次并成功（此时已无并发写者，因此
///    是确定性的——不 sleep、不轮询），`verify` + `scrub` 通过，且仍存在的每个节点都能
///    逐位恢复。
///
/// 两类拒绝都计入报告：写者持锁时全库校验给出的是**有意义的**拒绝；精简被拒绝则说明
/// `scrub` 的探测先抢到了锁。若一轮里精简不是那条明确的拒绝（而是别的错误），才是需要
/// 继续关注的信号。
#[test]
fn j5_compaction_and_full_scrub_run_concurrently_without_corruption() {
    let mut compact_refusals = 0_usize;
    let mut scrub_refusals = 0_usize;
    let mut other_failures = 0_usize;
    for round in 0..6_u64 {
        let fixture = ArtworkFixture::new();
        let first = fixture.commit(96 * 1024, 1 + round, "first");
        let middle = fixture.commit(80 * 1024, 2 + round, "middle");
        let head = fixture.commit(64 * 1024, 3 + round, "head");
        // 精简会移除 middle，因此它不会被恢复；两端都必须能逐位恢复。
        let expected = [
            (first.clone(), pattern_bytes(96 * 1024, 1 + round)),
            (head.clone(), pattern_bytes(64 * 1024, 3 + round)),
        ];

        let (compaction, scrub) = thread::scope(|scope| {
            let compaction =
                scope.spawn(|| fixture.command("compact").arg("history", &middle).finish());
            let scrub = scope.spawn(|| fixture.command("scrub").finish());
            (
                compaction.join().expect("精简线程不应 panic"),
                scrub.join().expect("校验线程不应 panic"),
            )
        });

        assert!(
            !compaction.timed_out,
            "round={round}：精简不得挂起：\n{}",
            compaction.describe()
        );
        // 精简的两种合法结果：成功，或拿到那个明确的「多进程写入」拒绝。
        let compact_refused = !compaction.ok();
        if compact_refused {
            assert_eq!(
                compaction.exit_code,
                1,
                "round={round}：精简失败必须是普通错误：\n{}",
                compaction.describe()
            );
            assert!(
                compaction.error().contains("仓库正在被另一个进程写入"),
                "round={round}：精简失败的理由应指明多进程写入边界：\n{}",
                compaction.describe()
            );
            assert!(
                !compaction.stderr.contains("panicked"),
                "round={round}：精简不得以 panic 收场：\n{}",
                compaction.describe()
            );
            compact_refusals += 1;
        }
        if !scrub.ok() {
            if scrub.error().contains("仓库正在被写入，请稍后重试") {
                scrub_refusals += 1;
            } else {
                other_failures += 1;
            }
            assert!(
                !scrub.timed_out,
                "round={round}：全库校验不得挂起：\n{}",
                scrub.describe()
            );
            assert_eq!(
                scrub.exit_code,
                1,
                "round={round}：校验失败必须是普通错误：\n{}",
                scrub.describe()
            );
            assert!(
                !scrub.stderr.contains("panicked"),
                "round={round}：校验不得以 panic 收场：\n{}",
                scrub.describe()
            );
        }

        // 两者都结束后：被拒绝的精简必须重试成功（此时已无并发写者，确定性成立），
        // 仓库必须完整，且仍存在的节点都能逐位恢复。
        if compact_refused {
            fixture
                .command("compact")
                .arg("history", &middle)
                .finish()
                .expect_ok();
        }
        assert_eq!(fixture.healthy(), 2, "round={round}：精简后应剩两个节点");
        for (node, bytes) in &expected {
            let output = fixture.workspace.out(&format!("j5-{round}.bin"));
            fixture
                .command("restore")
                .arg("history", node)
                .arg("output", output.to_string_lossy())
                .finish()
                .expect_ok();
            assert_eq!(
                &fs::read(&output).expect("无法读取恢复输出"),
                bytes,
                "round={round}：{node} 必须能逐位恢复"
            );
            fs::remove_file(&output).expect("无法清理恢复输出");
        }
        fixture.assert_repository_clean();

        record(
            "J5 concurrent compact and scrub",
            "J",
            "small",
            &compaction,
            json!({
                "round": round,
                "compactOk": compaction.ok(),
                "compactRefused": compact_refused,
                "compactRefusals": compact_refusals,
                "compactError": if compaction.ok() { Value::Null } else { Value::String(compaction.error()) },
                "scrubOk": scrub.ok(),
                "scrubRefused": !scrub.ok() && scrub.error().contains("仓库正在被写入"),
                "scrubRefusals": scrub_refusals,
                "scrubError": if scrub.ok() { Value::Null } else { Value::String(scrub.error()) },
            }),
        );
    }

    // 如实记录：真并发下精简被明确拒绝、全库校验被明确拒绝，以及以其他错误失败的轮次。
    // 前两类是既定行为（精简的拒绝来自 `scrub` 的探测抢到锁，见报告「边界」），最后一类
    // 才是需要继续关注的信号。若 6 轮里一次精简拒绝都没出现，说明抢锁窗口本轮未被触发，
    // 重试分支未覆盖——如实注明，不用 sleep 或人为构造窗口。
    eprintln!(
        "J5：6 轮真并发中精简被拒绝 {compact_refusals} 轮、\
         全库校验被拒绝 {scrub_refusals} 轮、其他失败 {other_failures} 轮"
    );
}

/// J6：写哨兵被持有时，内容级全库校验必须**明确拒绝**。
///
/// 全库校验会在扫描中刷新节点级校验状态（`history_nodes.verified_ms`），因此从 2026-10-19
/// 起按**前台写命令**执行、自己取跨进程 owner lock；另一个进程持锁时它拿到的是那条
/// 「不支持同一仓库的多进程写入」的明确拒绝，而不是早先只探测哨兵时的
/// 「仓库正在被写入，请稍后重试」。这里用闸门把写进程停在检查点（此时它确实持有哨兵），
/// 确定性验证这条拒绝；同时断言轻量只读命令（`verify`）保持 J1/J2 的承诺不回退，并验证
/// 写者结束后同一条命令正常通过——拒绝不能变成永久关闭。
#[test]
fn j6_full_scrub_is_refused_while_a_writer_holds_the_sentinel() {
    let fixture = ArtworkFixture::new();
    fixture.commit(96 * 1024, 1, "baseline");
    fs::write(&fixture.work, vec![0x33_u8; 96 * 1024]).expect("无法改写工作文件");

    let mut refusal = None;
    let mut verify = None;
    let outcome = fixture.commit_command().gate().drive(|index, _stage| {
        if index == 1 {
            // 写进程在此持有哨兵锁：全库校验必须立刻拿到有意义的拒绝。
            refusal = Some(fixture.command("scrub").finish());
            verify = Some(fixture.command("verify").finish());
        }
        false
    });
    outcome.expect_ok();

    let refusal = refusal.expect("全库校验应当已被启动");
    assert!(
        !refusal.timed_out,
        "拒绝必须是立刻返回的，不得等待：\n{}",
        refusal.describe()
    );
    assert!(
        !refusal.ok(),
        "写者持锁时全库校验不应成功：\n{}",
        refusal.describe()
    );
    assert_eq!(
        refusal.exit_code,
        1,
        "拒绝必须是普通错误（退出码 1）：\n{}",
        refusal.describe()
    );
    assert!(
        refusal
            .error()
            .contains("仓库正在被另一个进程写入；不支持同一仓库的多进程写入"),
        "拒绝必须给出可行动的理由：\n{}",
        refusal.describe()
    );

    // J1/J2 的承诺不回退：轻量只读命令在写者持锁期间照常作答。
    let verify = verify.expect("verify 应当已被启动");
    assert!(
        !verify.timed_out && verify.ok(),
        "写者持锁期间 verify 仍必须作答：\n{}",
        verify.describe()
    );

    // 写者结束后同一命令恢复正常（拒绝不是一个永久关闭）。
    let after = fixture.command("scrub").finish();
    after.expect_ok();
    assert_eq!(
        after.data_u64("historyNodes"),
        2,
        "写者结束后全库校验必须覆盖全部节点：\n{}",
        after.describe()
    );
    fixture.assert_repository_clean();
}
