//! B 组：跨进程强制退出。
//!
//! 发布顺序是「先发布文件、后提交数据库」，因此崩溃点落在两者之间必然产生孤儿文件。
//! 这类缺陷只能靠真实进程被杀来暴露，且必须验证「重开可用」而不是「重开不报错」。
//!
//! 干预方式：测试以逐检查点闸门（`--cancel-on-stdin`）启动真实可执行文件，进程在到达
//! 目标检查点时**阻塞**，测试随即 `Child::kill()`（Windows 等价 `TerminateProcess`，
//! 不运行任何清理）。因此强杀落在确定的代码位置，不依赖 sleep、不与进程赛跑。
//!
//! 断言只用「进程被杀」这一事实与「新的无头进程返回的 JSON / 磁盘事实」：
//! `verify`（迁移 + `integrity_check` + 外键 + 语义校验）、`scrub`（逐块摘要链）、
//! `cleanup`（清理队列重放），以及随后重跑同一命令必须成功。
//!
//! 覆盖 `docs/planning/todo.md` 第二节「损坏文件的恢复路径」中的崩溃窗口部分。
//!
//! 运行：`cargo test --features headless --test stress_crash`

mod stress_support;

use std::{fs, path::PathBuf, time::Duration};

use serde_json::json;
use stress_support::*;

/// 各场景的闸门等待上限。小规模档用 MiB 级文件，正常应在秒级完成。
const SCENARIO_TIMEOUT: Duration = Duration::from_secs(180);

struct Fixture {
    workspace: Workspace,
    repository: PathBuf,
    work: PathBuf,
    branch_id: String,
}

impl Fixture {
    fn new() -> Self {
        let workspace = Workspace::new();
        let repository = workspace.repository();
        let work = workspace.work("artwork.bin");
        write_work_file(&work, 96 * 1024, 1);
        headless(&workspace, "init-repository").finish().expect_ok();
        let created = headless(&workspace, "create-artwork")
            .arg("title", "Artwork")
            .arg("branch-title", "Main")
            .arg("source", work.to_string_lossy())
            .finish();
        created.expect_ok();
        let branch_id = created.data_str("branchId");
        assert!(!branch_id.is_empty(), "{}", created.describe());
        Self {
            workspace,
            repository,
            work,
            branch_id,
        }
    }

    fn command(&self, command: &str) -> Spawn<'_> {
        headless(&self.workspace, command)
            .repository(&self.repository)
            .timeout(SCENARIO_TIMEOUT)
    }

    /// 写入新内容并做一次普通提交，返回历史节点标识。
    fn commit(&self, len: usize, seed: u64, note: &str) -> String {
        write_work_file(&self.work, len, seed);
        let outcome = self
            .command("commit")
            .arg("branch", &self.branch_id)
            .arg("note", note)
            .arg("commit-kind", "manual")
            .finish();
        outcome.expect_ok();
        let history_id = outcome.data_str("historyId");
        assert!(!history_id.is_empty(), "{}", outcome.describe());
        history_id
    }

    /// 提交命令的公共参数（分支与提交类型）。
    fn commit_command(&self) -> Spawn<'_> {
        self.command("commit")
            .arg("branch", &self.branch_id)
            .arg("commit-kind", "manual")
    }

    fn healthy(&self) -> u64 {
        assert_healthy(&self.workspace, &self.repository)
    }
}

/// B1 提交中途强杀：snapshot/delta 已发布、`history::commit` 未执行。
///
/// 这是一个**预期会留下孤儿文件**的崩溃点（`cleanup` 队列的设计初衷正是回收这类文件）。
/// 断言分两层：
///
/// 1. 重开可用：`verify` + `scrub` 全过，head 未推进（历史节点数不变）；
/// 2. 孤儿事实：比基线多恰好一个 snapshot 与一个 delta、是完整文件、且在任何历史节点
///    都不引用它的情况下仓库依然自洽。
///
/// 如实记录的**缺口**：崩溃发生在入队之前，已发布的文件不会进入 `pending_file_cleanup`，
/// 而 `cleanup` 只重放队列，因此当前实现**不会自动回收**这枚孤儿（见交接文档）。本批次
/// 不修改产品行为，只把这作为实测结论记录下来。
#[test]
fn b1_commit_killed_after_publish_keeps_a_recoverable_orphan() {
    let fixture = Fixture::new();
    fixture.commit(96 * 1024, 1, "first");
    let before = assert_no_stray(&fixture.repository);
    assert_eq!(before.snapshots.len(), 1, "{:?}", before.snapshots);
    assert_eq!(before.deltas.len(), 0, "{:?}", before.deltas);

    // 第二次提交：检查点 1–3 放行，在第 4 个检查点（「发布后、提交前」）强杀。
    write_work_file(&fixture.work, 80 * 1024, 2);
    let outcome = fixture
        .commit_command()
        .arg("note", "killed before commit")
        .gate()
        .start()
        .kill_at(|index, _stage| index == 4);
    outcome.expect_killed_at(4);

    // 重开可用：不是「不报错」，而是完整性与链路校验全过、head 未推进。
    assert_eq!(fixture.healthy(), 1, "head 不应被推进");

    // 已发布的文件成为磁盘孤儿：恰好比基线多一个 snapshot、一个 delta。
    let after = assert_no_stray(&fixture.repository);
    assert_eq!(
        after.snapshots.len(),
        before.snapshots.len() + 1,
        "应多出一个孤儿 snapshot：{:?}",
        after.snapshots
    );
    assert_eq!(
        after.deltas.len(),
        before.deltas.len() + 1,
        "应多出一个孤儿 delta：{:?}",
        after.deltas
    );
    let orphans = after
        .snapshots
        .iter()
        .filter(|path| !before.snapshots.contains(path))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(orphans.len(), 1, "孤儿 snapshot 不唯一：{orphans:?}");
    // 发布前已 sync + rename，因此孤儿是**完整文件**而非半写文件。
    let orphan_bytes = fs::metadata(fixture.repository.join(&orphans[0]))
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    assert!(orphan_bytes > 0, "孤儿 snapshot 应为完整文件");

    // 清理队列不含该孤儿：崩溃发生在入队之前，当前实现不会自动回收它。
    let cleanup = fixture.command("cleanup").finish();
    cleanup.expect_ok();
    assert_eq!(
        cleanup.data_u64("pendingCount"),
        0,
        "{}",
        cleanup.describe()
    );
    assert_eq!(
        cleanup.data_u64("cleanedCount"),
        0,
        "{}",
        cleanup.describe()
    );

    record_crash(
        "B1 commit killed before commit",
        "small",
        &outcome,
        json!({ "orphanSnapshot": orphans[0], "orphanBytes": orphan_bytes }),
    );

    // 正向对照：同一仓库上重跑提交成功，节点前进一格、链路完好；孤儿仍在（不被回收）。
    fixture.commit(80 * 1024, 2, "retry after crash");
    assert_eq!(fixture.healthy(), 2);
    let after_retry = storage_state(&fixture.repository);
    assert!(
        after_retry.snapshots.contains(&orphans[0]),
        "崩溃孤儿不会自动回收：{:?}",
        after_retry.snapshots
    );
}

/// B2 恢复中途强杀：链解析完成后、发布输出之前。
///
/// 覆盖两个位置：检查点 5（创建输出临时文件之前）与检查点 6（导出并 `sync_all` 之后、
/// `persist` 之前）。两者都断言目标输出**不存在**——「不覆盖 + 原子发布」的语义在崩溃
/// 下同样成立：即使临时文件已完整写出，它也从未出现在目标名字上，因此不存在「半写文件
/// 被当作产物」。检查点 6 留下的未发布临时文件位于工作区 `out/`（仓库之外），
/// 由测试自行清理。
#[test]
fn b2_restore_killed_never_publishes_partial_output() {
    let fixture = Fixture::new();
    let first = fixture.commit(96 * 1024, 11, "first");
    fixture.commit(80 * 1024, 12, "second");
    fixture.commit(64 * 1024, 13, "head");
    let expected = pattern_bytes(96 * 1024, 11);

    // 恢复链为 [head, second, first]：入口、链解析入口、2 跳、导出前、发布前，共 6 个检查点。
    let cases = [
        ("before-export-temp", 5_usize, false),
        ("after-export-sync", 6_usize, true),
    ];
    for (label, index, leaves_temp) in cases {
        let output = fixture.workspace.out(&format!("restore-crash-{index}.bin"));
        let outcome = fixture
            .command("restore")
            .arg("history", &first)
            .arg("output", output.to_string_lossy())
            .gate()
            .start()
            .kill_at(|checkpoint, _stage| checkpoint == index);
        outcome.expect_killed_at(index);

        assert!(
            !output.exists(),
            "强杀后不得出现恢复输出：{}",
            output.display()
        );
        assert_eq!(fixture.healthy(), 3, "重开可用且历史图未变");

        let leftovers = directory_names(&fixture.workspace.out_directory());
        if leaves_temp {
            // 导出已完成并同步，但 persist（重命名）前被杀：留下一个未发布的临时文件。
            assert_eq!(
                leftovers.len(),
                1,
                "应恰好留下一个未发布临时文件：{leftovers:?}"
            );
            let target_name = output
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            assert_ne!(leftovers[0], target_name, "临时文件不得占用目标输出名");
            let temporary = fixture.workspace.out_directory().join(&leftovers[0]);
            assert_eq!(
                fs::metadata(&temporary)
                    .map(|metadata| metadata.len())
                    .unwrap_or(0),
                expected.len() as u64,
                "临时文件已是完整导出，只是尚未发布"
            );
            fs::remove_file(&temporary).expect("无法清理未发布临时文件");
        } else {
            assert!(
                leftovers.is_empty(),
                "创建临时文件之前的强杀不应留下任何输出：{leftovers:?}"
            );
        }

        record_crash(
            "B2 restore killed",
            "small",
            &outcome,
            json!({ "position": label, "leavesUnpublishedTemp": leaves_temp }),
        );
    }

    // 正向对照：同一节点仍能完整恢复，且字节逐位一致。
    let output = fixture.workspace.out("restore-crash-verified.bin");
    fixture
        .command("restore")
        .arg("history", &first)
        .arg("output", output.to_string_lossy())
        .finish()
        .expect_ok();
    assert_eq!(fs::read(&output).expect("无法读取恢复输出"), expected);
}

/// B3 整仓灾备中途强杀：复制阶段。
///
/// 灾备先把副本写进目标目录下的 `.lilith-artworks-<id>.tmp` 暂存目录，全部校验通过后
/// 才 `rename` 成 `Lilith-Artworks-backup-*` 发布目录。强杀落在复制中途时：
///
/// - 源仓库不受影响（只读扫描），并可独立校验；
/// - 目标目录留下**可识别**的未发布暂存目录（以 `.tmp` 结尾），且不存在已发布 bundle。
///
/// 如实记录的**缺口**：未发布的暂存目录不会被自动回收（`StagingDirectory::drop` 在进程
/// 被杀时不会运行），本批次只测量并记录现状。
#[test]
fn b3_repository_backup_killed_during_copy_leaves_identifiable_staging() {
    let fixture = Fixture::new();
    fixture.commit(96 * 1024, 21, "first");
    fixture.commit(80 * 1024, 22, "second");
    let destination = fixture.workspace.backup_directory();

    let outcome = fixture
        .command("repository-backup")
        .arg("destination", destination.to_string_lossy())
        .gate()
        .start()
        .kill_at(|_index, stage| stage.contains("正在复制仓库文件"));
    outcome.expect_killed();
    assert!(
        outcome.target_stage().contains("正在复制仓库文件"),
        "强杀应落在复制阶段：\n{}",
        outcome.describe()
    );

    // 源仓库未受影响，且可独立打开并通过完整性与链路校验。
    assert_eq!(fixture.healthy(), 2, "源仓库不受影响");

    // 未发布的暂存目录可识别，且没有已发布 bundle。
    let leftovers = directory_names(&destination);
    assert_eq!(
        leftovers.len(),
        1,
        "应留下一个未发布的暂存目录：{leftovers:?}"
    );
    assert!(
        leftovers[0].starts_with(".lilith-artworks-") && leftovers[0].ends_with(".tmp"),
        "暂存目录命名应可识别为未发布：{leftovers:?}"
    );
    assert!(
        !leftovers
            .iter()
            .any(|name| name.starts_with("Lilith-Artworks-backup-")),
        "不得出现已发布 bundle：{leftovers:?}"
    );

    record_crash(
        "B3 repository backup killed",
        "small",
        &outcome,
        json!({ "staging": leftovers[0] }),
    );

    // 正向对照：灾备可重新完成，副本可独立打开并通过链路校验。
    let retried = fixture
        .command("repository-backup")
        .arg("destination", destination.to_string_lossy())
        .finish();
    retried.expect_ok();
    let copy = PathBuf::from(retried.data_str("repositoryPath"));
    assert!(copy.is_dir(), "{}", retried.describe());
    headless(&fixture.workspace, "verify")
        .repository(&copy)
        .finish()
        .expect_ok();
    let scrub = headless(&fixture.workspace, "scrub")
        .repository(&copy)
        .finish();
    scrub.expect_ok();
    assert_eq!(scrub.data_u64("historyNodes"), 2);

    // 未发布的暂存目录仍留在目标目录（当前实现不会自动回收）：如实记录。
    let names = directory_names(&destination);
    assert!(
        names
            .iter()
            .any(|name| name.starts_with(".lilith-artworks-")),
        "崩溃暂存目录不会被自动回收：{names:?}"
    );
    assert!(
        names
            .iter()
            .any(|name| name.starts_with("Lilith-Artworks-backup-")),
        "重跑后应存在已发布 bundle：{names:?}"
    );
}
