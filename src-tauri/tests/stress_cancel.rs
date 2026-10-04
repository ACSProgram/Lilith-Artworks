//! A 组：取消边界。
//!
//! 每个长操作都有多条清理分支，但既有单元测试完全没有在中途取消并断言收尾状态。
//! 本文件是**独立进程**调用真实可执行文件，通过 `--marker` + `--cancel-on-stdin`
//! 的逐检查点闸门确定性命中每一个取消检查点，再断言：
//!
//! - 仓库无残留（snapshot/delta 未发布、临时文件已回收、待清理队列为空）；
//! - 历史图与 head 未被推进；
//! - 重开可用（`verify` 完整性与语义校验 + `scrub` 逐块摘要链校验）；
//! - 同一仓库上重跑同一命令必须成功。
//!
//! 覆盖 `docs/planning/todo.md` 第二节「各处理阶段的取消边界」。
//!
//! 运行：`cargo test --features headless --test stress_cancel`

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

    /// 给一个已装配好专有参数的命令套上闸门并按策略驱动。
    fn gated(&self, spawn: Spawn<'_>, policy: &Policy) -> Outcome {
        spawn
            .gate()
            .drive(|index, stage| policy.decide(index, stage))
    }

    /// 提交命令的公共参数（分支与提交类型）。
    fn commit_command(&self) -> Spawn<'_> {
        self.command("commit")
            .arg("branch", &self.branch_id)
            .arg("commit-kind", "manual")
    }

    /// 在给定策略下运行一次带闸门的命令。
    fn gated_command(&self, command: &str, policy: &Policy) -> Outcome {
        self.gated(self.command(command), policy)
    }

    fn healthy(&self) -> u64 {
        assert_healthy(&self.workspace, &self.repository)
    }

    fn assert_repository_clean(&self) {
        assert_no_stray(&self.repository);
        assert_cleanup_empty(&self.workspace, &self.repository);
    }
}

/// A1 提交取消：在提交的每一个取消检查点各取消一次。
///
/// `run_backup` 的检查点依次是：载入分支后、snapshot 建成后、发布前、发布后（回滚分支）。
/// 第四个是唯一会走到「已发布文件回滚」代码的取消点，因此逐个覆盖是必要的。
#[test]
fn a1_commit_cancellation_is_clean_at_every_checkpoint() {
    let fixture = Fixture::new();
    let mut expected_nodes = 0_u64;
    for target in 1..=4 {
        let len = 96 * 1024 + target * 16;
        let seed = 100 + target as u64;
        write_work_file(&fixture.work, len, seed);
        let policy = Policy::at_index(target);
        let outcome = fixture.gated(
            fixture.commit_command().arg("note", "cancel probe"),
            &policy,
        );

        outcome.expect_cancelled();
        assert_eq!(
            outcome.checkpoints.len(),
            target,
            "检查点数量与取消位置不符：\n{}",
            outcome.describe()
        );
        fixture.assert_repository_clean();
        assert_eq!(fixture.healthy(), expected_nodes, "head 不应被推进");

        record(
            "A1 commit cancel",
            "A",
            "small",
            &outcome,
            json!({ "checkpoint": target }),
        );

        // 同一仓库上重跑同一命令必须成功，且只前进一格。
        fixture.commit(len, seed, "retry after cancel");
        expected_nodes += 1;
        assert_eq!(fixture.healthy(), expected_nodes);
    }
}

/// A2 恢复取消：链解析中 / 导出前 / 发布前。
///
/// 恢复的输出必须满足「不覆盖」语义：取消后输出文件不存在，且输出目录不残留
/// 临时文件。正向对照再确认同一节点仍能完整恢复出逐位一致的原始字节。
#[test]
fn a2_restore_cancellation_never_publishes_output() {
    let fixture = Fixture::new();
    let first = fixture.commit(96 * 1024, 11, "first");
    fixture.commit(80 * 1024, 12, "second");
    fixture.commit(64 * 1024, 13, "head");
    let expected = pattern_bytes(96 * 1024, 11);

    // 目标节点是最早的提交，恢复链为 [head, second, first]：入口 1 个、链解析 2 个、
    // 导出前 1 个、发布前 1 个，加上链解析入口共 6 个检查点。
    for target in 1..=6 {
        let output = fixture.workspace.out(&format!("restore-{target}.bin"));
        let policy = Policy::at_index(target);
        let outcome = fixture
            .command("restore")
            .arg("history", &first)
            .arg("output", output.to_string_lossy())
            .gate()
            .drive(|index, stage| policy.decide(index, stage));

        outcome.expect_cancelled();
        assert_eq!(
            outcome.checkpoints.len(),
            target,
            "检查点数量与取消位置不符：\n{}",
            outcome.describe()
        );
        assert!(
            !output.exists(),
            "取消后不应存在恢复输出：{}",
            output.display()
        );
        fixture.assert_repository_clean();
        assert_eq!(fixture.healthy(), 3);
        record(
            "A2 restore cancel",
            "A",
            "small",
            &outcome,
            json!({ "checkpoint": target }),
        );
    }

    let leftovers = read_directory_names(&fixture.workspace.out_directory());
    assert!(
        leftovers.is_empty(),
        "恢复输出目录残留临时文件：{leftovers:?}"
    );

    // 正向对照：同一节点仍能完整恢复，且字节逐位一致。
    let output = fixture.workspace.out("restore-verified.bin");
    fixture
        .command("restore")
        .arg("history", &first)
        .arg("output", output.to_string_lossy())
        .finish()
        .expect_ok();
    assert_eq!(fs::read(&output).expect("无法读取恢复输出"), expected);
}

/// A3 精简取消：父链回溯中 / 子链回溯中 / delta 发布前。
///
/// 精简会改接历史图并替换 delta，任何中途取消都必须让图保持原样：最早的节点仍能
/// 恢复出原始字节，节点数不变，临时文件已回收。
#[test]
fn a3_compact_cancellation_keeps_the_history_graph() {
    let fixture = Fixture::new();
    let first = fixture.commit(96 * 1024, 21, "first");
    let middle = fixture.commit(80 * 1024, 22, "middle");
    fixture.commit(64 * 1024, 23, "head");
    let expected = pattern_bytes(96 * 1024, 21);

    // 检查点：父链回溯入口 + 2 跳、子链回溯入口、delta 发布前两处，共 6 个。
    for target in 1..=6 {
        let policy = Policy::at_index(target);
        let outcome = fixture
            .command("compact")
            .arg("history", &middle)
            .gate()
            .drive(|index, stage| policy.decide(index, stage));

        outcome.expect_cancelled();
        assert_eq!(
            outcome.checkpoints.len(),
            target,
            "检查点数量与取消位置不符：\n{}",
            outcome.describe()
        );
        fixture.assert_repository_clean();
        assert_eq!(fixture.healthy(), 3, "历史图不应被改接");

        let output = fixture.workspace.out(&format!("compact-{target}.bin"));
        fixture
            .command("restore")
            .arg("history", &first)
            .arg("output", output.to_string_lossy())
            .finish()
            .expect_ok();
        assert_eq!(
            fs::read(&output).expect("无法读取恢复输出"),
            expected,
            "取消精简后最早的节点仍必须能完整恢复"
        );
        fs::remove_file(&output).expect("无法清理恢复输出");
        record(
            "A3 compact cancel",
            "A",
            "small",
            &outcome,
            json!({ "checkpoint": target }),
        );
    }

    // 正向对照：取消之后精简仍能完成，节点数减一且链路完好。
    fixture
        .command("compact")
        .arg("history", &middle)
        .finish()
        .expect_ok();
    assert_eq!(fixture.healthy(), 2);
}

/// A4 检查点取消：链解析中 / snapshot 发布前。
///
/// 取消后节点不得被登记为检查点，也不得留下孤儿 snapshot。
#[test]
fn a4_checkpoint_cancellation_publishes_nothing() {
    let fixture = Fixture::new();
    fixture.commit(96 * 1024, 31, "first");
    let middle = fixture.commit(80 * 1024, 32, "middle");
    fixture.commit(64 * 1024, 33, "head");

    let before = assert_no_stray(&fixture.repository);
    assert_eq!(
        before.snapshots.len(),
        1,
        "只有 head 持有 snapshot：{:?}",
        before.snapshots
    );

    // 检查点：入口、链解析入口、1 跳、发布前，共 4 个。
    for target in 1..=4 {
        let policy = Policy::at_index(target);
        let outcome = fixture
            .command("checkpoint")
            .arg("history", &middle)
            .gate()
            .drive(|index, stage| policy.decide(index, stage));

        outcome.expect_cancelled();
        assert_eq!(
            outcome.checkpoints.len(),
            target,
            "检查点数量与取消位置不符：\n{}",
            outcome.describe()
        );
        let after = assert_no_stray(&fixture.repository);
        assert_eq!(after.snapshots, before.snapshots, "不应留下孤儿 snapshot");
        assert_eq!(after.deltas, before.deltas);
        assert_eq!(fixture.healthy(), 3);
        record(
            "A4 checkpoint cancel",
            "A",
            "small",
            &outcome,
            json!({ "checkpoint": target }),
        );
    }

    // 正向对照：正式生成检查点，只新增一个 snapshot，节点数不变。
    fixture
        .command("checkpoint")
        .arg("history", &middle)
        .finish()
        .expect_ok();
    let after = assert_no_stray(&fixture.repository);
    assert_eq!(after.snapshots.len(), 2, "{:?}", after.snapshots);
    assert_eq!(fixture.healthy(), 3);
}

/// A5 整仓灾备取消：扫描 / 复制 / 校验 / 发布各阶段。
///
/// 未发布的临时 bundle 必须被清理，返回错误要附带清理结果；源仓库不受影响。
#[test]
fn a5_repository_backup_cancellation_cleans_the_staging_bundle() {
    let fixture = Fixture::new();
    fixture.commit(96 * 1024, 41, "first");
    fixture.commit(80 * 1024, 42, "second");
    let destination = fixture.workspace.backup_directory();

    // 扫描阶段存在两个取消窗口：建临时目录之前（此时没有 bundle 可清理）与之后。
    // 「临时备份已清理」只在后者出现，因此分开断言，不把「无 bundle」当成清理成功。
    let cases = vec![
        ("scan-before-staging", Policy::at_index(1), false),
        ("copy", Policy::at_stage("正在复制仓库文件"), true),
        ("verify", Policy::at_stage("正在校验备份"), true),
        ("publish", Policy::at_stage("正在生成备份校验清单"), true),
    ];
    for (label, policy, staged) in cases {
        let outcome = fixture
            .command("repository-backup")
            .arg("destination", destination.to_string_lossy())
            .gate()
            .drive(|index, stage| policy.decide(index, stage));

        outcome.expect_cancelled();
        assert!(
            outcome.error().contains("已取消"),
            "取消必须如实回报：\n{}",
            outcome.describe()
        );
        if staged {
            assert!(
                outcome.error().contains("临时备份已清理"),
                "已建 stage 目录时错误必须附带清理结果：\n{}",
                outcome.describe()
            );
        }
        assert_eq!(
            read_directory_names(&destination),
            Vec::<String>::new(),
            "灾备目标目录必须被清空，不能留下未发布的 bundle"
        );
        fixture.assert_repository_clean();
        assert_eq!(fixture.healthy(), 2, "源仓库不受影响");
        record(
            "A5 repository backup cancel",
            "A",
            "small",
            &outcome,
            json!({ "stage": label, "staged": staged }),
        );
    }

    // 正向对照：灾备发布成功，副本可独立打开并通过完整性与链路校验。
    let outcome = fixture
        .command("repository-backup")
        .arg("destination", destination.to_string_lossy())
        .finish();
    outcome.expect_ok();
    let copy = PathBuf::from(outcome.data_str("repositoryPath"));
    assert!(copy.is_dir(), "{}", outcome.describe());
    headless(&fixture.workspace, "verify")
        .repository(&copy)
        .finish()
        .expect_ok();
    let scrub = headless(&fixture.workspace, "scrub")
        .repository(&copy)
        .finish();
    scrub.expect_ok();
    assert_eq!(scrub.data_u64("historyNodes"), 2);
}

/// A6 全库扫描取消：逐节点之间。
///
/// 扫描是纯只读操作，取消后仓库状态必须逐项不变。
#[test]
fn a6_repository_scrub_cancellation_leaves_the_repository_unchanged() {
    let fixture = Fixture::new();
    fixture.commit(96 * 1024, 51, "first");
    fixture.commit(80 * 1024, 52, "second");
    fixture.commit(64 * 1024, 53, "head");
    let before = assert_no_stray(&fixture.repository);

    let policies = vec![
        ("entry", Policy::at_index(1)),
        ("between-nodes", Policy::at_index(5)),
        ("late-node", Policy::at_stage("全库扫描 2/3")),
    ];
    for (label, policy) in policies {
        let outcome = fixture.gated_command("scrub", &policy);

        outcome.expect_cancelled();
        let after = assert_no_stray(&fixture.repository);
        assert_eq!(after, before, "只读扫描不应改动任何实体文件");
        assert_eq!(fixture.healthy(), 3);
        record(
            "A6 repository scrub cancel",
            "A",
            "small",
            &outcome,
            json!({ "position": label }),
        );
    }
}

fn read_directory_names(directory: &std::path::Path) -> Vec<String> {
    let mut names = fs::read_dir(directory)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    names.sort();
    names
}
