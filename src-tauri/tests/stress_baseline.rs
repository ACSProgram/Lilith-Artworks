//! 阶段 0：0.2 基线采集（锁等待、任务耗时、取消延迟、深链恢复、调度扫描、清理查询、灾备 I/O）。
//!
//! 它**不改变产品行为**：只是给 0.3 各阶段重构前留一组可比数值。测量分两类：
//!
//! - **合成场景**（深链恢复、取消延迟）：真实仓库里没有足够深的链，这部分按确定性夹具在
//!   临时工作区里构造；取消延迟本就取决于检查点粒度与收尾路径，用受控夹具才有意义。
//! - **真实仓库场景**（锁等待、任务耗时、调度扫描、清理查询、灾备 I/O）：在
//!   `LILITH_BASELINE_REPO` 指定的真实仓库副本上运行，测的是仓库规模下的绝对耗时。仓库
//!   副本允许被测试改动，但本组只做**读操作**与**不写源仓库**的整仓灾备；自建的
//!   `baseline-out` 在结束时删除，唯一残留是写命令取得哨兵时创建的跨进程哨兵库
//!   `.lilith-write-lock.sqlite3`，采集后按采集前的副本还原即可。
//!
//! **真实仓库副本是外部临时资料**：`LILITH_BASELINE_REPO` 指向维护者临时准备的目录，随 0.3
//! 计划落实后撤除。本组只把它当阶段 0 的一次性测量对象——不做长期夹具、不设阈值、不生成
//! 基线文件。长期回归对照用本组的合成场景与 `stress_bench`；没有该副本时真实仓库场景明确
//! 跳过，裸 `cargo test` 与 CI 都不受影响。
//!
//! 运行（数值必须用 release 档；`--test-threads=1` 保证真实仓库场景之间不互相争用）：
//!
//! ```text
//! cargo build --release --features headless
//! LILITH_STRESS_BIN=target/release/lilith-artworks.exe \
//! LILITH_BASELINE_REPO=D:\\temp\\Artworks \
//!   cargo test --features headless --test stress_baseline -- --test-threads=1 --nocapture
//! ```
//!
//! 数值同时落进 `target/stress-report.jsonl`（`perf: true` 行），便于摘录进
//! `docs/planning/0.3-baseline-2026-10-18.md`。

// 只在启用 `headless` feature 时编译。理由见 `tests/stress_support/mod.rs` 顶部说明。
#![cfg(feature = "headless")]

mod stress_support;

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use serde_json::{json, Value};
use stress_support::*;

/// 真实仓库副本路径。未设置时本组的真实仓库场景**明确跳过**（不是失败），
/// 因此裸 `cargo test` 与 CI 不受影响。
const BASELINE_REPO_ENV: &str = "LILITH_BASELINE_REPO";

/// 合成深链的节点数与工作文件大小。链深到足以让「每步反向增量」的摊薄值稳定，
/// 同时比 `stress_bench` 的 48 节点更能暴露随链长的退化。
const DEEP_CHAIN_NODES: usize = 256;
const DEEP_NODE_BYTES: usize = 96 * 1024;

/// 取消延迟提交用的载荷大小：4 MiB 让一次提交跨越多个分块，检查点之间不再是「几个块」。
const CANCEL_COMMIT_BYTES: usize = 4 * 1024 * 1024;

/// 真实仓库的重操作（全库校验、整仓灾备）可能以十分钟计，超时放宽。
const REAL_TIMEOUT: Duration = Duration::from_secs(3600);

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

/// 构建档：debug 与 release 的绝对耗时差一个数量级，记录里必须能分辨，否则两份数据混在
/// 同一组标签下就无法判断参考价值。
fn build_profile() -> &'static str {
    if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    }
}

/// 记录一条基线测量：打印（供人工核对）+ 追加到 `target/stress-report.jsonl`。
///
/// 构建档写进 `detail`，与 `stress_bench` 把档位写进标签同一目的：同一组标签下不混档。
fn emit(scenario: &str, label: &str, ms: f64, detail: Value) {
    let detail = match detail {
        Value::Object(mut map) => {
            map.insert("profile".to_owned(), json!(build_profile()));
            Value::Object(map)
        }
        other => json!({ "profile": build_profile(), "detail": other }),
    };
    record_measurement(scenario, "0.2-baseline", label, round1(ms), detail);
    println!(
        "[baseline] {scenario} :: {label} = {ms:.1} ms ({})",
        build_profile()
    );
}

// ---------------------------------------------------------------------------
// 合成场景：深链恢复与取消延迟
// ---------------------------------------------------------------------------

/// B1 + B2：合成深链的提交/恢复单位耗时，以及提交与恢复在各检查点上的取消延迟。
#[test]
fn baseline_synthetic_deep_chain_and_cancel() {
    let fixture = ArtworkFixture::new();

    // B1 建链：每个节点一次提交（每次一个独立进程）。
    let started = Instant::now();
    let mut nodes = Vec::with_capacity(DEEP_CHAIN_NODES);
    for index in 0..DEEP_CHAIN_NODES {
        nodes.push(fixture.commit(
            DEEP_NODE_BYTES + index * 8,
            1_000 + index as u64,
            "deep chain node",
        ));
    }
    let build = started.elapsed();
    let per_commit = millis(build) / DEEP_CHAIN_NODES as f64;

    // 恢复最早节点：要穿越 DEEP_CHAIN_NODES-1 步反向增量才能还原。
    let oldest = fixture.workspace.out("deep-oldest.bin");
    let restore_started = Instant::now();
    fixture
        .command("restore")
        .arg("history", &nodes[0])
        .arg("output", oldest.to_string_lossy())
        .finish()
        .expect_ok();
    let restore_oldest = restore_started.elapsed();
    let per_step = millis(restore_oldest) / (DEEP_CHAIN_NODES - 1) as f64;
    assert_eq!(
        fs::read(&oldest).expect("无法读取恢复输出"),
        pattern_bytes(DEEP_NODE_BYTES, 1_000),
        "恢复最早节点必须得到逐位一致的原始内容"
    );

    // 另一端：head 直接读 snapshot。
    let head = fixture.workspace.out("deep-head.bin");
    let head_started = Instant::now();
    fixture
        .command("restore")
        .arg("history", nodes.last().expect("链上至少一个节点"))
        .arg("output", head.to_string_lossy())
        .finish()
        .expect_ok();
    let restore_head = head_started.elapsed();

    emit(
        "B1 deep chain",
        "commitPerNodeMs",
        per_commit,
        json!({ "nodes": DEEP_CHAIN_NODES, "fileBytes": DEEP_NODE_BYTES }),
    );
    emit(
        "B1 deep chain",
        "restoreOldestMs",
        millis(restore_oldest),
        json!({}),
    );
    emit(
        "B1 deep chain",
        "chainStepMs",
        per_step,
        json!({ "steps": DEEP_CHAIN_NODES - 1 }),
    );
    emit(
        "B1 deep chain",
        "restoreHeadMs",
        millis(restore_head),
        json!({}),
    );
    fs::remove_file(&oldest).expect("无法清理恢复输出");
    fs::remove_file(&head).expect("无法清理恢复输出");

    // B2 提交取消：5 个检查点各取消一次，记录「发出取消 → 进程退出」的延迟。
    for target in 1..=5_usize {
        write_work_file(&fixture.work, CANCEL_COMMIT_BYTES, 4_000 + target as u64);
        let outcome = fixture
            .commit_command()
            .arg("note", "cancel latency")
            .gate()
            .drive(|index, _stage| index == target);
        outcome.expect_cancelled();
        let latency = outcome
            .cancel_latency
            .expect("带闸门的取消必须记录取消延迟");
        let stage = outcome
            .checkpoint_stages()
            .into_iter()
            .find(|(index, _)| *index == target)
            .map(|(_, stage)| stage.to_owned())
            .unwrap_or_default();
        emit(
            "B2 commit cancel",
            &format!("checkpoint{target}Ms"),
            millis(latency),
            json!({ "stage": stage, "payloadBytes": CANCEL_COMMIT_BYTES }),
        );
    }

    // B2 恢复取消：对深链最早节点的恢复，在链解析的早期检查点上取消。
    for target in 1..=6_usize {
        let output = fixture
            .workspace
            .out(&format!("cancel-restore-{target}.bin"));
        let outcome = fixture
            .command("restore")
            .arg("history", &nodes[0])
            .arg("output", output.to_string_lossy())
            .gate()
            .drive(|index, _stage| index == target);
        outcome.expect_cancelled();
        assert!(
            !output.exists(),
            "取消后不应存在恢复输出：{}",
            output.display()
        );
        let latency = outcome
            .cancel_latency
            .expect("带闸门的取消必须记录取消延迟");
        let stage = outcome
            .checkpoint_stages()
            .into_iter()
            .find(|(index, _)| *index == target)
            .map(|(_, stage)| stage.to_owned())
            .unwrap_or_default();
        emit(
            "B2 restore cancel",
            &format!("checkpoint{target}Ms"),
            millis(latency),
            json!({ "stage": stage, "chainNodes": DEEP_CHAIN_NODES }),
        );
    }

    assert!(
        directory_names(&fixture.workspace.out_directory()).is_empty(),
        "恢复输出目录应已清空"
    );
    fixture.assert_repository_clean();
}

// ---------------------------------------------------------------------------
// 真实仓库场景
// ---------------------------------------------------------------------------

struct RealRepository {
    /// 临时工作区：只提供 `scratch`（marker/result）与进程工作目录。
    workspace: Workspace,
    /// 作用域根（仓库目录的父目录）；`--repository` 必须位于它之内。
    scope: PathBuf,
    /// 真实仓库副本。
    repository: PathBuf,
    /// 本组自建的输出目录（作用域根之下），运行结束后删除。
    out: PathBuf,
}

impl RealRepository {
    fn command(&self, command: &str) -> Spawn<'_> {
        headless(&self.workspace, command)
            .repository(&self.repository)
            .scope(&self.scope)
            .timeout(REAL_TIMEOUT)
    }

    /// 计时一条命令并断言成功，返回 (耗时毫秒, 结果)。
    fn timed(&self, command: &str, args: &[(&str, String)]) -> (f64, Outcome) {
        let mut spawn = self.command(command);
        for (key, value) in args {
            spawn = spawn.arg(key, value);
        }
        let started = Instant::now();
        let outcome = spawn.finish();
        let elapsed = millis(started.elapsed());
        outcome.expect_ok();
        (elapsed, outcome)
    }
}

/// 收集作品树里的全部 Artwork 节点 id（`kind == "artwork"`）。
fn artwork_ids(outcome: &Outcome) -> Vec<String> {
    fn walk(nodes: &[Value], ids: &mut Vec<String>) {
        for node in nodes {
            if node["kind"].as_str() == Some("artwork") {
                if let Some(id) = node["id"].as_str() {
                    ids.push(id.to_owned());
                }
            }
            if let Some(children) = node["children"].as_array() {
                walk(children, ids);
            }
        }
    }
    let mut ids = Vec::new();
    if let Some(nodes) = outcome.result["data"]["nodes"].as_array() {
        walk(nodes, &mut ids);
    }
    ids
}

/// 仓库数据字节总数，**排除**跨进程哨兵库 `.lilith-write-lock.sqlite3*`（写命令取得
/// 哨兵时会创建它，与仓库数据无关）。用于「源仓库不受影响」的前后对照。
fn repository_data_bytes(repository: &Path) -> u64 {
    directory_bytes_excluding(repository, |path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(".lilith-write-lock"))
    })
}

/// 停靠窗口内的一个探针：命令、参数与「在写者持锁时是否应被拒绝」。
struct Probe {
    metric: &'static str,
    command: &'static str,
    args: Vec<(&'static str, String)>,
    expect_reject: bool,
}

/// 探针的一次实测：耗时、成功与否与错误文案。取消决策仍以 [`Probe::expect_reject`]
/// 为准，因此这里不复制该字段，避免两处判定漂移。
struct ProbeResult {
    metric: &'static str,
    ms: f64,
    ok: bool,
    error: String,
}

/// B3–B7：真实仓库副本上的锁等待、任务耗时、调度扫描、清理查询与灾备 I/O。
///
/// 只读命令与「不写源仓库的整仓灾备」覆盖全部场景；运行结束后源仓库内容逐字节不变
/// （本测试用前后字节总数与 `verify` 两次断言），仅可能新增跨进程哨兵库。
#[test]
fn baseline_real_repository() {
    let repository = match std::env::var_os(BASELINE_REPO_ENV) {
        Some(value) => PathBuf::from(value),
        None => {
            record_skip(
                "B3-B7 real repository",
                "0.2-baseline",
                "real",
                "未设置 LILITH_BASELINE_REPO，跳过真实仓库基线",
            );
            return;
        }
    };
    assert!(
        repository.is_dir(),
        "LILITH_BASELINE_REPO 指向的目录不存在：{}",
        repository.display()
    );
    let scope = repository
        .parent()
        .expect("仓库目录必须有父目录作为作用域根")
        .to_path_buf();
    let out = scope.join("baseline-out");
    let _ = fs::remove_dir_all(&out);
    fs::create_dir_all(&out).expect("无法创建基线输出目录");

    let fixture = RealRepository {
        workspace: Workspace::new(),
        scope,
        repository,
        out: out.clone(),
    };
    let bytes_before = repository_data_bytes(&fixture.repository);

    // B7 任务耗时：重开校验、全库校验、树与历史读取。
    let (verify_ms, _) = fixture.timed("verify", &[]);
    emit("B7 task duration", "verifyMs", verify_ms, json!({}));
    let (scrub_ms, scrub) = fixture.timed("scrub", &[]);
    emit(
        "B7 task duration",
        "scrubMs",
        scrub_ms,
        json!({ "historyNodes": scrub.data_u64("historyNodes") }),
    );
    let (list_tree_ms, tree) = fixture.timed("list-tree", &[]);
    emit(
        "B7 task duration",
        "listTreeMs",
        list_tree_ms,
        json!({ "nodeCount": tree.data_u64("nodeCount") }),
    );
    let ids = artwork_ids(&tree);
    assert!(!ids.is_empty(), "真实仓库里应当至少有一个 Artwork");
    let mut list_history_total = 0.0;
    let mut node_total = 0_u64;
    for id in &ids {
        let (ms, outcome) = fixture.timed("list-history", &[("artwork", id.clone())]);
        list_history_total += ms;
        node_total += outcome.data_u64("nodeCount");
    }
    emit(
        "B7 task duration",
        "listHistoryPerArtworkMs",
        list_history_total / ids.len() as f64,
        json!({ "artworks": ids.len(), "totalNodes": node_total }),
    );

    // B4 调度扫描：调度器候选生成（`history::list_scheduled`）的直接成本。
    let (scan_ms, scan) = fixture.timed("scheduler-scan", &[]);
    emit(
        "B4 scheduler scan",
        "schedulerScanMs",
        scan_ms,
        json!({
            "branchCount": scan.data_u64("branchCount"),
            "inProcessScanMs": scan.data("scanMs"),
        }),
    );

    // B5 清理查询：未引用文件扫描（只报告）与待清理队列重放。
    let (scan_unref_ms, unref) = fixture.timed("scan-unreferenced", &[]);
    emit(
        "B5 cleanup query",
        "scanUnreferencedMs",
        scan_unref_ms,
        json!({ "candidates": unref.data_u64("count") }),
    );
    let (cleanup_ms, cleanup) = fixture.timed("cleanup", &[]);
    emit(
        "B5 cleanup query",
        "cleanupQueueMs",
        cleanup_ms,
        json!({ "pendingCount": cleanup.data_u64("pendingCount") }),
    );

    // B3 锁等待：把整仓灾备停在「正在复制仓库文件」，此刻它持有跨进程写哨兵，
    // 在这个窗口内测只读命令的响应延迟与第二个写者的拒绝延迟。
    let parked = fixture.out.join("parked-backup");
    fs::create_dir_all(&parked).expect("无法创建停靠用备份目标目录");
    let probes = vec![
        Probe {
            metric: "readListTreeMs",
            command: "list-tree",
            args: Vec::new(),
            expect_reject: false,
        },
        Probe {
            metric: "readListHistoryMs",
            command: "list-history",
            args: vec![("artwork", ids[0].clone())],
            expect_reject: false,
        },
        Probe {
            metric: "readSchedulerScanMs",
            command: "scheduler-scan",
            args: Vec::new(),
            expect_reject: false,
        },
        Probe {
            metric: "readVerifyMs",
            command: "verify",
            args: Vec::new(),
            expect_reject: false,
        },
        Probe {
            metric: "writerCleanupRejectedMs",
            command: "cleanup",
            args: Vec::new(),
            expect_reject: true,
        },
        Probe {
            metric: "contentScrubRejectedMs",
            command: "scrub",
            args: Vec::new(),
            expect_reject: true,
        },
    ];

    let mut results: Vec<ProbeResult> = Vec::new();
    let parked_outcome = fixture
        .command("repository-backup")
        .arg("destination", parked.to_string_lossy())
        .gate()
        .drive(|_index, stage| {
            if !results.is_empty() || !stage.contains("正在复制仓库文件") {
                return false;
            }
            for probe in &probes {
                let mut spawn = fixture.command(probe.command);
                for (key, value) in &probe.args {
                    spawn = spawn.arg(key, value);
                }
                let started = Instant::now();
                let outcome = spawn.finish();
                results.push(ProbeResult {
                    metric: probe.metric,
                    ms: millis(started.elapsed()),
                    ok: outcome.ok(),
                    error: outcome.error(),
                });
            }
            true // 采集完成即取消，中止这次复制。
        });
    parked_outcome.expect_cancelled();
    assert_eq!(
        results.len(),
        probes.len(),
        "停靠窗口应采集到全部探针：{:?}",
        results
            .iter()
            .map(|result| result.metric)
            .collect::<Vec<_>>()
    );
    for (probe, result) in probes.iter().zip(&results) {
        assert_eq!(
            result.ok, !probe.expect_reject,
            "锁等待语义不符：{} ok={} 期望 rejected={}；{}",
            result.metric, result.ok, probe.expect_reject, result.error
        );
        emit(
            "B3 lock wait",
            result.metric,
            result.ms,
            json!({ "rejected": !result.ok, "error": result.error }),
        );
    }
    let latency = parked_outcome
        .cancel_latency
        .expect("带闸门的取消必须记录取消延迟");
    emit(
        "B3 lock wait",
        "parkedBackupCancelMs",
        millis(latency),
        json!({ "note": "复制阶段取消：发出取消 → 中止并清理 stage" }),
    );
    // 取消后灾备目标目录必须被清空。
    assert!(
        directory_names(&parked).is_empty(),
        "取消的灾备不应在目标目录留下内容"
    );

    // B6 灾备 I/O：完整整仓灾备的总耗时与吞吐（含扫描 + 复制 + 校验 + 发布）。
    let destination = fixture.out.join("full-backup");
    fs::create_dir_all(&destination).expect("无法创建灾备目标目录");
    let started = Instant::now();
    let backup = fixture
        .command("repository-backup")
        .arg("destination", destination.to_string_lossy())
        .finish();
    let backup_ms = millis(started.elapsed());
    backup.expect_ok();
    let copy = PathBuf::from(backup.data_str("repositoryPath"));
    assert!(copy.is_dir(), "{}", backup.describe());
    let copied_bytes = directory_bytes(&copy);
    let seconds = backup_ms / 1000.0;
    // 吞吐按「源仓库字节 / 墙钟」算，与报告口径一致；除以源字节而不是副本字节，是为了让
    // 额外写出的 manifest 不计入吞吐。
    let mebibytes_per_second = if seconds > 0.0 {
        bytes_before as f64 / seconds / (1024.0 * 1024.0)
    } else {
        0.0
    };
    emit(
        "B6 repository backup",
        "backupMs",
        backup_ms,
        json!({
            "repositoryBytes": bytes_before,
            "copiedBytes": copied_bytes,
            "mebibytesPerSecond": round1(mebibytes_per_second),
        }),
    );

    // 源仓库不受影响：数据字节总数不变（哨兵库已排除），重开校验仍通过。
    let bytes_after = repository_data_bytes(&fixture.repository);
    assert_eq!(
        bytes_before, bytes_after,
        "真实仓库基线只做读与灾备，源仓库数据字节数不应变化"
    );
    let (verify_after_ms, _) = fixture.timed("verify", &[]);
    emit(
        "B7 task duration",
        "verifyAfterMs",
        verify_after_ms,
        json!({}),
    );

    // 清理本组自建目录；真正的源仓库复原由维护者按哨兵库的存在与否处理。
    fs::remove_dir_all(&destination).expect("无法清理灾备目标");
    fs::remove_dir_all(&out).expect("无法清理基线输出目录");
}
