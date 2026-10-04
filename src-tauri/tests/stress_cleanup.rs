//! B 组补充：崩溃孤儿回收闭环（B5）。
//!
//! 批次 2 的 B1 只断言「孤儿存在 + 不被引用 + 重开可用」，当时如实记录「当前不会自动回收」。
//! 统一清理体系落地后，`cleanup::scan_unreferenced` 能发现未引用文件、`cleanup::cleanup_unreferenced`
//! 能在用户确认后回收它。本用例把这条闭环补成可自动化的断言：
//!
//! 崩溃产生孤儿 → 扫描**只**报告孤儿（被引用文件即便同样「过期」也不报告）→ 确认清理删除
//! 孤儿 → 再扫描无候选 → 重开仍可用、后续提交正常。
//!
//! 扫描有 30 分钟宽限期（避免与进行中的提交/精简赛跑），因此测试把快照/增量文件的修改时间
//! 回拨到宽限期之外——这只改变测试自有工作区里文件的 mtime，不触碰任何产品行为。
//!
//! 运行：`cargo test --features headless --test stress_cleanup`

mod stress_support;

use std::{fs, path::PathBuf, time::Duration};

use serde_json::json;
use stress_support::*;

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

    fn commit_command(&self) -> Spawn<'_> {
        self.command("commit")
            .arg("branch", &self.branch_id)
            .arg("commit-kind", "manual")
    }
}

/// 把文件修改时间回拨 `seconds` 秒（只改测试自有工作区里的文件）。
fn backdate(path: &PathBuf, seconds: u64) {
    let file = fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap_or_else(|error| panic!("无法打开 {} 以修改时间：{error}", path.display()));
    let when = std::time::SystemTime::now() - Duration::from_secs(seconds);
    file.set_modified(when)
        .unwrap_or_else(|error| panic!("无法修改 {} 的时间：{error}", path.display()));
}

/// B5：崩溃孤儿经「扫描发现 + 确认清理」被回收，被引用文件不受影响。
#[test]
fn b5_crash_orphans_are_discovered_and_reclaimed() {
    let fixture = Fixture::new();
    fixture.commit(96 * 1024, 1, "baseline");
    let before = assert_no_stray(&fixture.repository);
    assert_eq!(before.snapshots.len(), 1, "{:?}", before.snapshots);
    assert_eq!(before.deltas.len(), 0, "{:?}", before.deltas);

    // 第二次提交：在检查点 4（snapshot/delta 已发布、commit 未执行）强杀。
    write_work_file(&fixture.work, 80 * 1024, 2);
    let crash = fixture
        .commit_command()
        .arg("note", "killed before commit")
        .gate()
        .start()
        .kill_at(|index, _stage| index == 4);
    crash.expect_killed_at(4);
    assert_eq!(
        assert_healthy(&fixture.workspace, &fixture.repository),
        1,
        "崩溃后 head 不应推进"
    );

    let after = assert_no_stray(&fixture.repository);
    let orphan_snapshots = after
        .snapshots
        .iter()
        .filter(|path| !before.snapshots.contains(path))
        .cloned()
        .collect::<Vec<_>>();
    let orphan_deltas = after
        .deltas
        .iter()
        .filter(|path| !before.deltas.contains(path))
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        orphan_snapshots.len(),
        1,
        "应恰好多出一枚孤儿 snapshot：{after:?}"
    );
    assert_eq!(
        orphan_deltas.len(),
        1,
        "应恰好多出一枚孤儿 delta：{after:?}"
    );

    // 扫描有 30 分钟宽限期；把仓库内全部 snapshot/delta 的时间回拨到宽限期之外，
    // 让「过期」不再是变量——此时扫描**只**应报告孤儿，被引用文件即便同样过期也不报告。
    for relative in after.snapshots.iter().chain(after.deltas.iter()) {
        backdate(&fixture.repository.join(relative), 2 * 60 * 60);
    }

    let scan = fixture.command("scan-unreferenced").finish();
    scan.expect_ok();
    let candidates = scan
        .data("candidates")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|candidate| candidate["path"].as_str().unwrap_or_default().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        scan.data_u64("count"),
        2,
        "扫描只应报告两枚孤儿（被引用文件不报告）：\n{}",
        scan.describe()
    );
    assert!(
        candidates.contains(&orphan_snapshots[0]),
        "孤儿 snapshot 未被报告：{candidates:?}"
    );
    assert!(
        candidates.contains(&orphan_deltas[0]),
        "孤儿 delta 未被报告：{candidates:?}"
    );

    // 确认清理：入队 + 单遍重放删除候选。
    let cleanup = fixture
        .command("cleanup-unreferenced")
        .arg("ids", candidates.join(","))
        .finish();
    cleanup.expect_ok();
    assert_eq!(
        cleanup.data_u64("cleanedCount"),
        2,
        "{}",
        cleanup.describe()
    );

    // 孤儿被回收、被引用文件保留。
    assert!(
        !fixture.repository.join(&orphan_snapshots[0]).exists(),
        "孤儿 snapshot 应被删除"
    );
    assert!(
        !fixture.repository.join(&orphan_deltas[0]).exists(),
        "孤儿 delta 应被删除"
    );
    for relative in &before.snapshots {
        assert!(
            fixture.repository.join(relative).exists(),
            "被引用 snapshot 不得被误删：{relative}"
        );
    }
    assert_cleanup_empty(&fixture.workspace, &fixture.repository);

    // 再扫描无候选（幂等）。
    let rescan = fixture.command("scan-unreferenced").finish();
    rescan.expect_ok();
    assert_eq!(
        rescan.data_u64("count"),
        0,
        "清理后再扫描不应有候选：\n{}",
        rescan.describe()
    );

    // 重开可用 + 正向对照：同一仓库上重跑提交成功，节点前进一格。
    assert_eq!(assert_healthy(&fixture.workspace, &fixture.repository), 1);
    fixture.commit(80 * 1024, 2, "retry after crash");
    assert_eq!(assert_healthy(&fixture.workspace, &fixture.repository), 2);

    record_crash(
        "B5 crash orphan reclaimed",
        "B",
        "small",
        &crash,
        json!({
            "orphanSnapshot": orphan_snapshots[0],
            "orphanDelta": orphan_deltas[0],
            "scannedCandidates": candidates.len(),
        }),
    );
}
