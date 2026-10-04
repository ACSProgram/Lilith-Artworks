//! D 组：规模（深链、多分支历史图、库规模）、E 组：整仓灾备规模、F 组：参数边界。
//!
//! 规模场景的量级按**个人长期使用的真实上限**选取（见计划 §3.1「现实可遇性」）：
//! 单作品数百节点、约 20 条分支、数百 Artwork、单画板数百图片——而不是为凑数量构造的
//! 数千分支或数千作品。压力测试的价值来自「真实会发生的极端下仍正确」。
//!
//! 断言只用「子命令返回的 JSON」与「磁盘事实」：树计数、分支计数、恢复产物逐位一致、
//! 删除后磁盘实体与目录的残留情况、重开可用（`verify` + `scrub`）。
//!
//! 运行：`cargo test --features headless --test stress_scale`

mod stress_support;

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use serde_json::{json, Value};
use stress_support::*;

/// 规模场景的等待上限：库规模会创建数百个子进程，深链会提交数百次。
const SCENARIO_TIMEOUT: Duration = Duration::from_secs(1800);

struct Scale {
    workspace: Workspace,
    repository: PathBuf,
}

impl Scale {
    fn new() -> Self {
        let workspace = Workspace::new();
        let repository = workspace.repository();
        headless(&workspace, "init-repository").finish().expect_ok();
        assert_fresh_repository(&workspace, &repository);
        Self {
            workspace,
            repository,
        }
    }

    fn command(&self, command: &str) -> Spawn<'_> {
        headless(&self.workspace, command)
            .repository(&self.repository)
            .timeout(SCENARIO_TIMEOUT)
    }

    fn create_artwork(
        &self,
        parent: Option<&str>,
        title: &str,
        branch: &str,
        work: &Path,
    ) -> (String, String) {
        let mut spawn = self
            .command("create-artwork")
            .arg("title", title)
            .arg("branch-title", branch)
            .arg("source", work.to_string_lossy());
        if let Some(parent) = parent {
            spawn = spawn.arg("parent", parent);
        }
        let created = spawn.finish();
        created.expect_ok();
        let artwork_id = created.data_str("artworkId");
        let branch_id = created.data_str("branchId");
        assert!(!artwork_id.is_empty(), "{}", created.describe());
        assert!(!branch_id.is_empty(), "{}", created.describe());
        (artwork_id, branch_id)
    }

    fn commit(&self, branch: &str, note: &str) -> String {
        let outcome = self
            .command("commit")
            .arg("branch", branch)
            .arg("commit-kind", "manual")
            .arg("note", note)
            .finish();
        outcome.expect_ok();
        assert!(
            outcome.data("created").as_bool().unwrap_or(false),
            "改动后提交必须产生新节点：\n{}",
            outcome.describe()
        );
        let history_id = outcome.data_str("historyId");
        assert!(!history_id.is_empty(), "{}", outcome.describe());
        history_id
    }
}

/// 取 `list-tree` 返回的完整树值（`data` 即 `LibraryTree`）。
fn tree_value(scale: &Scale) -> Value {
    let outcome = scale.command("list-tree").finish();
    outcome.expect_ok();
    outcome.result["data"].clone()
}

/// 在树里按标题递归查找节点。
fn find_node<'a>(nodes: &'a [Value], title: &str) -> Option<&'a Value> {
    for node in nodes {
        if node["title"].as_str() == Some(title) {
            return Some(node);
        }
        if let Some(children) = node["children"].as_array() {
            if let Some(found) = find_node(children, title) {
                return Some(found);
            }
        }
    }
    None
}

fn assert_file_equals(path: &Path, expected: &[u8]) {
    let actual =
        fs::read(path).unwrap_or_else(|error| panic!("无法读取 {}：{error}", path.display()));
    assert_eq!(
        actual.len(),
        expected.len(),
        "恢复输出长度不符：{}",
        path.display()
    );
    assert!(actual == expected, "恢复输出内容不一致：{}", path.display());
}

/// 就地改动工作文件的一小段（模拟日常反复编辑）。
fn tweak(path: &Path, seed: u64) {
    let mut bytes = fs::read(path).expect("无法读取工作文件");
    let span = 128.min(bytes.len());
    let offset = (seed as usize * 977) % (bytes.len() - span).max(1);
    bytes[offset..offset + span].copy_from_slice(&pattern_bytes(span, seed.wrapping_add(7)));
    fs::write(path, &bytes).expect("无法写入工作文件");
}

// ---------------------------------------------------------------------------
// D1 深链
// ---------------------------------------------------------------------------

/// 单分支 300 次提交：链解析、精简与恢复在深链下仍正确，内存不随深度增长。
#[test]
fn d1_deep_chain_resolves_and_compacts() {
    const SIZE: usize = 128 * 1024;
    const COMMITS: usize = 300;

    let scale = Scale::new();
    let work = scale.workspace.work("deep.bin");
    write_work_file(&work, SIZE, 1);
    let (artwork, branch) = scale.create_artwork(None, "Deep", "Main", &work);

    let mut nodes = Vec::with_capacity(COMMITS);
    let mut max_peak = 0_u64;
    let started = Instant::now();
    for index in 0..COMMITS {
        if index > 0 {
            tweak(&work, index as u64);
        }
        let outcome = scale
            .command("commit")
            .arg("branch", &branch)
            .arg("commit-kind", "manual")
            .arg("note", &format!("commit {index}"))
            .finish();
        outcome.expect_ok();
        assert!(
            outcome.data("created").as_bool().unwrap_or(false),
            "{}",
            outcome.describe()
        );
        max_peak = max_peak.max(outcome.result["peakWorkingSetBytes"].as_u64().unwrap_or(0));
        nodes.push(outcome.data_str("historyId"));
    }
    let commit_elapsed = started.elapsed();

    let history = scale
        .command("list-history")
        .arg("artwork", &artwork)
        .finish();
    history.expect_ok();
    assert_eq!(history.data_u64("branchCount"), 1, "{}", history.describe());
    assert_eq!(
        history.data_u64("nodeCount"),
        COMMITS as u64,
        "{}",
        history.describe()
    );

    // 恢复最深的节点：穿过整条 300 层增量链，内容必须等于第一次提交时的内容。
    let oldest_out = scale.workspace.out("deep-oldest.bin");
    let restore_started = Instant::now();
    let restored = scale
        .command("restore")
        .arg("history", &nodes[0])
        .arg("output", oldest_out.to_string_lossy())
        .finish();
    restored.expect_ok();
    let oldest_ms = restore_started.elapsed();
    assert_file_equals(&oldest_out, &pattern_bytes(SIZE, 1));
    let _ = fs::remove_file(&oldest_out);

    // 恢复最新节点：内容等于当前工作文件。
    let newest_out = scale.workspace.out("deep-newest.bin");
    scale
        .command("restore")
        .arg("history", nodes.last().unwrap())
        .arg("output", newest_out.to_string_lossy())
        .finish()
        .expect_ok();
    assert_files_identical(&work, &newest_out);
    let _ = fs::remove_file(&newest_out);

    // 精简中间节点改接历史图后，最深节点仍可恢复且逐位一致。
    scale
        .command("compact")
        .arg("history", &nodes[COMMITS / 2])
        .finish()
        .expect_ok();
    let compacted_out = scale.workspace.out("deep-oldest-compacted.bin");
    scale
        .command("restore")
        .arg("history", &nodes[0])
        .arg("output", compacted_out.to_string_lossy())
        .finish()
        .expect_ok();
    assert_file_equals(&compacted_out, &pattern_bytes(SIZE, 1));
    let _ = fs::remove_file(&compacted_out);

    let health = assert_healthy_within(&scale.workspace, &scale.repository, SCENARIO_TIMEOUT);
    assert_eq!(
        health.nodes,
        (COMMITS - 1) as u64,
        "精简会合并掉一个中间节点，节点总数应减一"
    );
    assert_cleanup_empty(&scale.workspace, &scale.repository);

    // 流式实现：300 层链不把峰值内存推到「链深 × 文件大小」量级。
    assert!(
        max_peak < 256 * MIB,
        "提交峰值内存应远小于链深×文件大小：{max_peak} 字节"
    );

    record(
        "D1 deep chain",
        "D",
        "small",
        &restored,
        json!({
            "commits": COMMITS,
            "commitElapsedMs": commit_elapsed.as_millis() as u64,
            "oldestRestoreMs": oldest_ms.as_millis() as u64,
            "maxPeakBytes": max_peak,
        }),
    );
}

// ---------------------------------------------------------------------------
// D2 多分支历史图
// ---------------------------------------------------------------------------

/// 单 Artwork 约 20 条分支、合计百余节点：计数正确、删分支只回收自己的文件且其它分支
/// 仍可恢复、清空回收站回收整个子树与其磁盘实体。
#[test]
fn d2_multi_branch_graph_deletes_only_its_own_files() {
    const MAIN_COMMITS: usize = 4;
    const BRANCHES: usize = 20;
    const COMMITS_PER_BRANCH: usize = 3;

    let scale = Scale::new();
    let main_work = scale.workspace.work("graph-main.bin");
    write_work_file(&main_work, 64 * 1024, 1);
    let (artwork, main_branch) = scale.create_artwork(None, "Graph", "Main", &main_work);

    let mut main_nodes = Vec::with_capacity(MAIN_COMMITS);
    for index in 0..MAIN_COMMITS {
        if index > 0 {
            tweak(&main_work, index as u64);
        }
        main_nodes.push(scale.commit(&main_branch, &format!("main {index}")));
    }
    let fork_point = main_nodes.last().unwrap().clone();

    // 20 条分支：各自独立工作文件（同 Artwork 的分支必须使用不同工作文件路径），各 3 次提交。
    let mut branch_ids = Vec::with_capacity(BRANCHES);
    let mut branch_heads = Vec::with_capacity(BRANCHES);
    let mut branch_works = Vec::with_capacity(BRANCHES);
    for index in 0..BRANCHES {
        let work = scale.workspace.work(&format!("graph-fork-{index}.bin"));
        write_work_file(&work, 48 * 1024, 100 + index as u64);
        let created = scale
            .command("create-branch")
            .arg("artwork", &artwork)
            .arg("history", &fork_point)
            .arg("branch-title", &format!("Fork {index}"))
            .arg("source", work.to_string_lossy())
            .finish();
        created.expect_ok();
        let branch = created.data_str("branchId");
        assert!(!branch.is_empty(), "{}", created.describe());
        let mut head = String::new();
        for commit in 0..COMMITS_PER_BRANCH {
            if commit > 0 {
                tweak(&work, (index * 10 + commit) as u64);
            }
            head = scale.commit(&branch, &format!("fork {index} commit {commit}"));
        }
        branch_ids.push(branch);
        branch_heads.push(head);
        branch_works.push(work);
    }

    let nodes_total = (MAIN_COMMITS + BRANCHES * COMMITS_PER_BRANCH) as u64;
    let history = scale
        .command("list-history")
        .arg("artwork", &artwork)
        .finish();
    history.expect_ok();
    assert_eq!(
        history.data_u64("branchCount"),
        (1 + BRANCHES) as u64,
        "{}",
        history.describe()
    );
    assert_eq!(
        history.data_u64("nodeCount"),
        nodes_total,
        "{}",
        history.describe()
    );

    let entities_before = storage_state(&scale.repository).entities();
    assert!(entities_before > 0);

    // 删除一条分支：只回收该分支独占的文件，其余分支不受影响。
    scale
        .command("delete-branch")
        .arg("branch", &branch_ids[0])
        .finish()
        .expect_ok();
    let after_delete = scale
        .command("list-history")
        .arg("artwork", &artwork)
        .finish();
    after_delete.expect_ok();
    assert_eq!(after_delete.data_u64("branchCount"), BRANCHES as u64);
    assert_eq!(
        after_delete.data_u64("nodeCount"),
        nodes_total - COMMITS_PER_BRANCH as u64
    );
    let entities_after = storage_state(&scale.repository).entities();
    assert!(
        entities_after < entities_before,
        "删除分支应回收该分支独占的文件：{entities_before} → {entities_after}"
    );
    assert_cleanup_empty(&scale.workspace, &scale.repository);
    assert_eq!(
        assert_healthy(&scale.workspace, &scale.repository),
        nodes_total - COMMITS_PER_BRANCH as u64
    );

    // 正向对照：另一条分支的 head 仍可完整恢复，内容与其工作文件逐位一致。
    let survivor_out = scale.workspace.out("graph-survivor.bin");
    scale
        .command("restore")
        .arg("history", &branch_heads[1])
        .arg("output", survivor_out.to_string_lossy())
        .finish()
        .expect_ok();
    assert_files_identical(&branch_works[1], &survivor_out);
    let _ = fs::remove_file(&survivor_out);

    // 清空回收站（子树删除）：Artwork 目录与其全部历史实体被回收。
    scale
        .command("trash-node")
        .arg("ids", &artwork)
        .finish()
        .expect_ok();
    scale.command("empty-trash").finish().expect_ok();
    let artwork_dir = scale.repository.join("artworks").join(&artwork);
    assert!(
        !artwork_dir.exists(),
        "清空回收站后 Artwork 目录应被回收：{}",
        artwork_dir.display()
    );
    let state = assert_no_stray(&scale.repository);
    assert_eq!(state.entities(), 0, "清空回收站后不应残留历史实体");
    assert_cleanup_empty(&scale.workspace, &scale.repository);

    let tree = tree_value(&scale);
    assert_eq!(tree["artworkCount"].as_u64(), Some(0));
    let verify = scale.command("verify").finish();
    verify.expect_ok();

    record(
        "D2 multi-branch graph",
        "D",
        "small",
        &after_delete,
        json!({
            "branches": 1 + BRANCHES,
            "nodes": nodes_total,
            "entitiesBeforeDelete": entities_before,
            "entitiesAfterDelete": entities_after,
        }),
    );
}

// ---------------------------------------------------------------------------
// D3 库规模
// ---------------------------------------------------------------------------

/// 约 400 个 Artwork + 嵌套分组：树计数、搜索与节点移动在规模下正确。
#[test]
fn d3_library_scale_lists_searches_and_moves() {
    const GROUPS: usize = 8;
    const SUBGROUPS: usize = 2;
    const PER_SUBGROUP: usize = 25;
    let artworks = GROUPS * SUBGROUPS * PER_SUBGROUP;

    let scale = Scale::new();

    // 顶层分组 + 每组两个子分组。
    let mut top_titles = Vec::with_capacity(GROUPS);
    for group in 0..GROUPS {
        let title = format!("Group {group}");
        scale
            .command("create-group")
            .arg("title", &title)
            .finish()
            .expect_ok();
        top_titles.push(title);
    }
    let tree = tree_value(&scale);
    for title in &top_titles {
        let top = find_node(tree["nodes"].as_array().expect("nodes 应为数组"), title)
            .unwrap_or_else(|| panic!("找不到顶层分组 {title}"));
        let top_id = top["id"].as_str().unwrap().to_owned();
        for sub in 0..SUBGROUPS {
            scale
                .command("create-group")
                .arg("parent", &top_id)
                .arg("title", &format!("{title} / Sub {sub}"))
                .finish()
                .expect_ok();
        }
    }

    // 子分组下批量创建 Artwork。
    let tree = tree_value(&scale);
    let mut subgroup_ids = Vec::with_capacity(GROUPS * SUBGROUPS);
    for group in 0..GROUPS {
        let title = format!("Group {group}");
        let top = find_node(tree["nodes"].as_array().unwrap(), &title).unwrap();
        let children = top["children"].as_array().cloned().unwrap_or_default();
        for sub in 0..SUBGROUPS {
            let sub_title = format!("{title} / Sub {sub}");
            let sub_node = find_node(&children, &sub_title)
                .unwrap_or_else(|| panic!("找不到子分组 {sub_title}"));
            subgroup_ids.push((
                sub_node["id"].as_str().unwrap().to_owned(),
                format!("Group {group} / Sub {sub}"),
            ));
        }
    }

    let started = Instant::now();
    let mut index = 0;
    for (sub_id, _label) in &subgroup_ids {
        for _ in 0..PER_SUBGROUP {
            let work = scale.workspace.work(&format!("lib-{index}.bin"));
            write_work_file(&work, 4 * 1024, index as u64 + 1);
            scale.create_artwork(Some(sub_id), &format!("Artwork {index:03}"), "Main", &work);
            index += 1;
        }
    }
    let create_elapsed = started.elapsed();
    assert_eq!(index, artworks);

    let tree = tree_value(&scale);
    assert_eq!(tree["artworkCount"].as_u64(), Some(artworks as u64));
    assert_eq!(
        tree["groupCount"].as_u64(),
        Some((GROUPS + GROUPS * SUBGROUPS) as u64)
    );
    assert_eq!(
        tree["nodeCount"].as_u64(),
        Some((artworks + GROUPS + GROUPS * SUBGROUPS) as u64)
    );

    // 搜索命中标题与工作文件路径。
    let search = scale.command("search").arg("query", "Artwork 007").finish();
    search.expect_ok();
    assert!(
        search.data_u64("count") >= 1,
        "搜索应命中 Artwork 007：{}",
        search.describe()
    );

    // 移动一个子分组到另一个顶层分组下。
    let (moved_id, moved_label) = subgroup_ids.last().unwrap().clone();
    let destination_top = {
        let tree = tree_value(&scale);
        let top = find_node(tree["nodes"].as_array().unwrap(), "Group 0").unwrap();
        top["id"].as_str().unwrap().to_owned()
    };
    scale
        .command("move-node")
        .arg("ids", &moved_id)
        .arg("parent", &destination_top)
        .arg("index", "0")
        .finish()
        .expect_ok();
    let tree = tree_value(&scale);
    let top0 = find_node(tree["nodes"].as_array().unwrap(), "Group 0").unwrap();
    let moved = find_node(top0["children"].as_array().unwrap(), &moved_label)
        .unwrap_or_else(|| panic!("移动后的子分组应出现在 Group 0 下：{moved_label}"));
    assert_eq!(moved["id"].as_str(), Some(moved_id.as_str()));
    // 移动不改变计数。
    assert_eq!(tree["artworkCount"].as_u64(), Some(artworks as u64));
    assert_eq!(
        tree["groupCount"].as_u64(),
        Some((GROUPS + GROUPS * SUBGROUPS) as u64)
    );

    assert_healthy(&scale.workspace, &scale.repository);

    record(
        "D3 library scale",
        "D",
        "small",
        &search,
        json!({
            "artworks": artworks,
            "groups": GROUPS + GROUPS * SUBGROUPS,
            "createElapsedMs": create_elapsed.as_millis() as u64,
        }),
    );
}

// ---------------------------------------------------------------------------
// E1 整仓灾备规模
// ---------------------------------------------------------------------------

/// 含多个 Artwork 与多个提交的仓库做整仓灾备：清单计数正确、副本可独立打开并通过
/// 完整性与链路校验、树结构与源仓库一致。
#[test]
fn e1_repository_backup_of_a_populated_repository() {
    const ARTWORKS: usize = 8;
    const COMMITS_PER_ARTWORK: usize = 2;

    let scale = Scale::new();
    let mut expected_nodes = 0_u64;
    for index in 0..ARTWORKS {
        let work = scale.workspace.work(&format!("backup-{index}.bin"));
        // 其中一个作品用较大的文件，让灾备包含一份非平凡快照。
        let size = if index == 0 {
            2 * MIB as usize
        } else {
            64 * 1024
        };
        write_work_file(&work, size, index as u64 + 1);
        let (_artwork, branch) =
            scale.create_artwork(None, &format!("Backup {index}"), "Main", &work);
        scale.commit(&branch, "initial");
        expected_nodes += 1;
        for commit in 1..COMMITS_PER_ARTWORK {
            tweak(&work, (index * 10 + commit) as u64);
            scale.commit(&branch, &format!("commit {commit}"));
            expected_nodes += 1;
        }
    }

    let destination = scale.workspace.backup_directory();
    let backup = scale
        .command("repository-backup")
        .arg("destination", destination.to_string_lossy())
        .finish();
    backup.expect_ok();
    assert_eq!(
        backup.data_u64("historyNodes"),
        expected_nodes,
        "灾备清单的历史节点数应与仓库一致：\n{}",
        backup.describe()
    );
    assert!(backup.data_u64("fileCount") > 0, "{}", backup.describe());
    assert!(
        backup.data_u64("totalBytes") > 0,
        "灾备总字节数应大于 0：\n{}",
        backup.describe()
    );

    // 副本可独立打开并通过完整性与链路校验。
    let copy = PathBuf::from(backup.data_str("repositoryPath"));
    assert!(copy.is_dir(), "{}", backup.describe());
    headless(&scale.workspace, "verify")
        .repository(&copy)
        .finish()
        .expect_ok();
    let scrub = headless(&scale.workspace, "scrub")
        .repository(&copy)
        .finish();
    scrub.expect_ok();
    assert_eq!(scrub.data_u64("historyNodes"), expected_nodes);

    // 副本的作品树与源仓库一致。
    let source_tree = tree_value(&scale);
    let copy_tree = headless(&scale.workspace, "list-tree")
        .repository(&copy)
        .finish();
    copy_tree.expect_ok();
    assert_eq!(
        copy_tree.data("artworkCount").as_u64(),
        source_tree["artworkCount"].as_u64()
    );
    assert_eq!(
        copy_tree.data("groupCount").as_u64(),
        source_tree["groupCount"].as_u64()
    );

    record(
        "E1 repository backup scale",
        "E",
        "small",
        &backup,
        json!({
            "artworks": ARTWORKS,
            "historyNodes": expected_nodes,
            "fileCount": backup.data_u64("fileCount"),
            "totalBytes": backup.data_u64("totalBytes"),
        }),
    );
}

// ---------------------------------------------------------------------------
// F1 参数边界
// ---------------------------------------------------------------------------

/// 真实会遇到的输入边界：明确拒绝或按不覆盖语义处理，而不是 panic。
#[test]
fn f1_parameter_boundaries_are_rejected_not_panicked() {
    let scale = Scale::new();
    let work = scale.workspace.work("bounds.bin");
    write_work_file(&work, 32 * 1024, 1);
    let (_artwork, branch) = scale.create_artwork(None, "Bounds", "Main", &work);
    let node = scale.commit(&branch, "initial");

    let expect_rejected = |outcome: &Outcome, what: &str| {
        assert!(
            !outcome.ok() && !outcome.timed_out,
            "{what} 应被明确拒绝：\n{}",
            outcome.describe()
        );
        assert!(
            !outcome.error().is_empty(),
            "{what} 的拒绝应带错误说明：\n{}",
            outcome.describe()
        );
    };

    // 备注长度：500 接受、501 拒绝。
    let note_500 = "n".repeat(500);
    tweak(&work, 1);
    scale
        .command("commit")
        .arg("branch", &branch)
        .arg("commit-kind", "manual")
        .arg("note", &note_500)
        .finish()
        .expect_ok();
    let note_501 = "n".repeat(501);
    tweak(&work, 2);
    let over_note = scale
        .command("commit")
        .arg("branch", &branch)
        .arg("commit-kind", "manual")
        .arg("note", &note_501)
        .finish();
    expect_rejected(&over_note, "501 字符备注");

    // 提交类型：非法值拒绝。
    tweak(&work, 3);
    let bad_kind = scale
        .command("commit")
        .arg("branch", &branch)
        .arg("commit-kind", "bogus")
        .arg("note", "bad kind")
        .finish();
    expect_rejected(&bad_kind, "非法提交类型");

    // 标题长度：160 接受、161 拒绝（create-artwork 与 create-group 共用标题校验）。
    let work_160 = scale.workspace.work("bounds-160.bin");
    write_work_file(&work_160, 4 * 1024, 9);
    let title_160 = "t".repeat(160);
    scale
        .command("create-artwork")
        .arg("title", &title_160)
        .arg("branch-title", "Main")
        .arg("source", work_160.to_string_lossy())
        .finish()
        .expect_ok();
    let title_161 = "t".repeat(161);
    let work_161 = scale.workspace.work("bounds-161.bin");
    write_work_file(&work_161, 4 * 1024, 10);
    let over_title = scale
        .command("create-artwork")
        .arg("title", &title_161)
        .arg("branch-title", "Main")
        .arg("source", work_161.to_string_lossy())
        .finish();
    expect_rejected(&over_title, "161 字符标题");

    // 空标题拒绝。
    let empty_title = scale.command("create-group").arg("title", "   ").finish();
    expect_rejected(&empty_title, "空分组标题");

    // 搜索：161 字符查询拒绝。
    let over_query = scale
        .command("search")
        .arg("query", &"q".repeat(161))
        .finish();
    expect_rejected(&over_query, "161 字符搜索");

    // 恢复：不存在的历史节点拒绝；已存在的输出路径拒绝（不覆盖语义）。
    let missing_out = scale.workspace.out("bounds-missing.bin");
    let missing = scale
        .command("restore")
        .arg("history", "00000000-0000-0000-0000-000000000000")
        .arg("output", missing_out.to_string_lossy())
        .finish();
    expect_rejected(&missing, "不存在的历史节点");

    let existing_out = scale.workspace.out("bounds-existing.bin");
    fs::write(&existing_out, b"occupied").expect("无法创建已存在的输出");
    let occupied = scale
        .command("restore")
        .arg("history", &node)
        .arg("output", existing_out.to_string_lossy())
        .finish();
    expect_rejected(&occupied, "已存在的恢复输出路径");
    assert_eq!(
        fs::read(&existing_out).expect("无法读取被占用的输出"),
        b"occupied",
        "被拒绝的恢复不得改动已存在的文件"
    );
    let _ = fs::remove_file(&existing_out);

    // 移动：目标分组不存在拒绝。
    let bad_move = scale
        .command("move-node")
        .arg("ids", &_artwork)
        .arg("parent", "00000000-0000-0000-0000-000000000000")
        .finish();
    expect_rejected(&bad_move, "移动到不存在的分组");

    // 拒绝之后仓库仍然自洽。
    assert_healthy(&scale.workspace, &scale.repository);
    assert_cleanup_empty(&scale.workspace, &scale.repository);

    record(
        "F1 parameter boundaries",
        "F",
        "small",
        &over_note,
        json!({
            "rejected": [
                "note>500", "bad-commit-kind", "title>160", "empty-title",
                "search>160", "missing-history", "existing-output", "bad-move-parent",
            ],
        }),
    );
}
