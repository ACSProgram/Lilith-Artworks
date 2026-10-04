//! C 组：大文件端到端。
//!
//! 分块引擎的单元测试已经覆盖了格式与摘要层面的正确性，端到端补的是**真实链路**上的
//! 行为：真实工作文件 → 提交 → 有界区域改动 → 恢复 → 精简，以及这条链路的内存与磁盘
//! 占用特征。覆盖 `docs/planning/todo.md` 第二节「极端大文件与高像素压力的内存、取消、
//! 退出与错误恢复」中的大文件错误恢复部分。
//!
//! 关键手法（见计划 §4.6）：
//!
//! - **稀疏工作文件**：`set_len` 建逻辑长度，只向有界区域写真实随机字节，其余部分不占簇。
//!   因此 4 GiB 逻辑文件的实际磁盘占用约等于写入的区域总量，而 snapshot 仍约 4 GiB——
//!   这正是「逻辑体积」与「磁盘成本」分离的地方。
//! - **逐场景一个临时工作区**：每个场景持有自己的 `TempDir`，随场景结束（含断言失败展开）
//!   被析构回收，因此「运行结束后工作区自动清理、磁盘占用回落」由语言保证。工作区**不能**
//!   放进 `static`/`OnceLock`——Rust 不为 static 运行析构函数，那样每轮都会在 `%TEMP%`
//!   留下数百 MB 到数 GB 的残留（实测偏差，见交接文档）。
//! - **按预算并发**：场景通过 `scenario_slot` 按「预计峰值磁盘」准入，多个场景可并行，
//!   但保留峰值之和不超过 `LILITH_STRESS_DISK_BUDGET`。无头子进程是单线程的，串行只会
//!   用一个核、磁盘只跑到零头带宽；并发让总耗时随核数下降，磁盘峰值仍有界（详见
//!   `scenario_slot` 的注释，以及关于并发代价的说明）。
//! - **磁盘预算守卫**：预期峰值加当前占用超出 `LILITH_STRESS_DISK_BUDGET` 时**明确跳过
//!   并报告**，而不是中途写坏磁盘。
//! - **断言只用磁盘事实与子命令返回的 JSON**：逐位比较恢复产物，不读内部状态。
//!
//! 覆盖矩阵由**两条正交的轴**决定（见 `ChangeProfile`）：
//!
//! | | 小改动（逻辑 1/128） | 大改动（逻辑 1/4，3 轮） |
//! | --- | --- | --- |
//! | 一般文件（`default` 64 MiB） | 日常编辑 | 日常里的大改 |
//! | 大文件（`large`/`heavy`/`extreme`） | 大文件的日常编辑 | 最坏存储放大（4 GiB 改 1 GiB） |
//!
//! 小改动档的轮数随档位下降（`default`/`large` 20 轮、`heavy` 10 轮、`extreme`/`manual`
//! 5 轮）：每轮都要重写整份快照，轮数必须随文件大小下降才能让耗时可控，而链深行为与
//! 文件大小无关，20 层链已在小档位覆盖。
//!
//! 场景：C1 端到端（小改动链）、C2 精简、C3 峰值内存、C4 链体积与账本、C5 恢复产物即删、
//! C6 大改动端到端。C1–C5 走小改动档，C6 走大改动档。
//!
//! 档位由 `LILITH_STRESS_TIERS` 选择（`default` 64 MiB / `large` 256 MiB / `heavy`
//! 1 GiB / `extreme` 4 GiB / `manual` 8 GiB）。峰值内存、存储放大与耗时只有指向发布档
//! 才有参考价值：
//!
//! ```text
//! cargo build --release --features headless
//! LILITH_STRESS_BIN=target/release/lilith-artworks.exe \
//!   cargo test --features headless --test stress_large
//! ```
//!
//! 运行：`cargo test --features headless --test stress_large`

mod stress_support;

use std::{fs, path::PathBuf, time::Duration};

use serde_json::json;
use stress_support::*;

/// 大文件场景的等待上限：串行执行，4 GiB 档明显更久。
const SCENARIO_TIMEOUT: Duration = Duration::from_secs(3600);
/// 账本允许的偏差：实测工作区增量与预期之差不得超过预期值的 10% 加 1 MiB。
const LEDGER_TOLERANCE_RATIO: f64 = 0.10;
const LEDGER_TOLERANCE_BYTES: u64 = MIB;
/// snapshot 允许高出逻辑大小的比例（每块 36 字节开销）加固定冗余。
const SNAPSHOT_OVERHEAD_RATIO: f64 = 0.02;
const SNAPSHOT_OVERHEAD_BYTES: u64 = MIB;
/// 小改动场景里 delta 总量相对改动量的允许倍数（内容定义分块在改动边界附近重新同步的开销）。
const DELTA_CHANGE_FACTOR: u64 = 2;
/// 大改动场景里 delta 总量相对改动量的允许倍数：增量本应≈改动量，倍数只用来吸收块边界
/// 重新同步与操作表的开销。
const LARGE_DELTA_CHANGE_FACTOR: u64 = 3;

/// **变动量**：与文件大小正交的第二个轴。
///
/// 只测「大文件」不够——真实使用里同样重要的是**每次提交改动多少**。两轴组合覆盖四象限：
/// 一般文件小改动、一般文件大改动、大文件小改动、大文件大改动（4 GiB 文件改 1 GiB 即属
/// 最后一格）。两者的代价特征完全不同：小改动下增量由改动量决定且远小于快照；大改动下
/// 增量本身接近快照量级，存储放大到最坏。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChangeProfile {
    /// 日常编辑：每轮改动逻辑大小的 1/128（夹在 256 KiB–8 MiB），每轮换一批区域。
    Small,
    /// 大改动：每轮改动逻辑大小的 1/4（4 GiB 文件 → 1 GiB），反复编辑同一批区域，
    /// 因此父版本的旧内容同样是真实随机数据，delta 无法靠压缩变小。
    Large,
}

impl ChangeProfile {
    fn name(self) -> &'static str {
        match self {
            ChangeProfile::Small => "small-change",
            ChangeProfile::Large => "large-change",
        }
    }

    fn change_bytes(self, tier: Tier) -> u64 {
        match self {
            // 改动量必须远小于逻辑大小，否则「增量远小于快照」会退化成对写入量的同义反复。
            ChangeProfile::Small => (tier.logical_size() / 128).clamp(256 * 1024, 8 * MIB),
            ChangeProfile::Large => (tier.logical_size() / 4).max(4 * MIB),
        }
    }

    /// 固定轮数。
    ///
    /// 小改动场景每轮都要**重写整份快照**（格式不压缩、不去重），因此轮数必须随文件大小
    /// 下降，否则单场景耗时按「轮数 × 逻辑大小」增长：4 GiB 档跑 20 轮要写约 80 GiB 快照。
    /// 链深行为与文件大小无关，20 层链已在 `default`/`large` 覆盖，高档位因此缩短到 10/5 轮。
    /// 大改动场景每轮增量接近改动量、固定 3 轮，覆盖最坏存储放大。
    fn rounds(self, tier: Tier) -> usize {
        match self {
            ChangeProfile::Small => match tier {
                Tier::Default | Tier::Large => 20,
                Tier::Heavy => 10,
                Tier::Extreme | Tier::Manual => 5,
            },
            ChangeProfile::Large => 3,
        }
    }

    /// 大改动场景每轮使用同一批区域（只换内容）；小改动场景每轮换一批区域。
    fn keeps_layout(self) -> bool {
        self == ChangeProfile::Large
    }
}

/// 一个已经建好仓库与稀疏工作文件的场景。
///
/// 每个场景持有**自己的** `TempDir`：它随场景结束（含断言失败展开）被析构回收，因此
/// 「运行结束后工作区自动清理、磁盘占用回落」由语言保证，而不是靠额外步骤。共享一个
/// 工作区要么需要显式重置（漏一次就跨场景污染），要么放进 `static`（永不回收）。
struct Scenario {
    workspace: Workspace,
    /// 并发准入名额。声明在 `workspace` 之后，因此析构时工作区先被删除、名额后释放——
    /// 下一个被准入的场景不会与尚未回收的旧工作区争磁盘。
    _slot: ScenarioSlot,
    repository: PathBuf,
    work: PathBuf,
    branch_id: String,
    logical: u64,
    profile: ChangeProfile,
    /// 每轮改动的字节数（由档位与变动量档决定）。
    change_bytes: u64,
    /// 大变动场景反复编辑的那批区域（小改动场景每轮另取一批）。
    layout: Vec<RegionWrite>,
    /// 期望内容的写入计划，按应用顺序保存初始构造与每轮改动的区域。
    plan: Vec<RegionWrite>,
    /// 初始构造占用的计划条目数。
    initial_writes: usize,
    /// 每轮占用的计划条目数（只由改动总量决定，因此是常量）。
    writes_per_commit: usize,
    /// 已提交的历史节点标识，按提交顺序。
    nodes: Vec<String>,
}

impl Scenario {
    /// 建立场景。磁盘预算不足时返回 `None`，并已如实记录跳过（不写坏磁盘）。
    fn new(name: &str, profile: ChangeProfile) -> Option<Self> {
        let tier = tier();
        let logical = tier.logical_size();
        let change_bytes = profile.change_bytes(tier);
        let workspace = Workspace::new();
        let repository = workspace.repository();

        // 预期峰值 = 新旧两份快照 + 一份全尺寸恢复输出 + 全部增量（最坏取「增量 ≈ 改动量」）
        // + 工作文件实际占用。
        let expected = tier.peak_disk_floor()
            + change_bytes * profile.rounds(tier) as u64
            + tier.initial_region_bytes()
            + change_bytes;
        let current = tree_usage(&workspace).allocated_estimate(0);
        let budget = disk_budget();
        if current + expected > budget {
            record_skip(
                name,
                "C",
                tier.name(),
                &format!(
                    "档位 {}（逻辑 {}，{}）预计峰值 {} 加当前占用 {} 超过 \
                     LILITH_STRESS_DISK_BUDGET {}",
                    tier.name(),
                    human_bytes(logical),
                    profile.name(),
                    human_bytes(expected),
                    human_bytes(current),
                    human_bytes(budget)
                ),
            );
            return None;
        }
        // 并发准入：保留本场景的预计峰值磁盘，多个场景的保留之和不超过预算即可并行。
        // 这一步会阻塞到有足够磁盘与并发名额，因此大文件档不再串行。
        let slot = scenario_slot(expected);

        // 稀疏构造工作文件。它**必须**在仓库之外：`create_artwork` 拒绝仓库内部路径。
        let work = workspace.work("artwork.bin");
        let layout = region_writes(logical, change_bytes, 0xC0FF_EE00, 0);
        let mut initial = region_writes(
            logical,
            tier.initial_region_bytes(),
            0x5EED_0001,
            0x5EED_0001,
        );
        if profile.keeps_layout() {
            // 大改动场景：初始就把「将被反复编辑的区域」填成真实随机数据，否则第一轮的
            // 父版本内容全是全零区（可压缩），delta 会假性变小。
            initial.extend(rewrite_layout(&layout, 0xBEEF_0001));
        }
        let writes_per_commit = rewrite_layout(&layout, 0).len();
        write_sparse_file(&work, logical, &initial);

        headless(&workspace, "init-repository").finish().expect_ok();
        assert_fresh_repository(&workspace, &repository);
        let created = headless(&workspace, "create-artwork")
            .repository(&repository)
            .arg("title", "Large Artwork")
            .arg("branch-title", "Main")
            .arg("source", work.to_string_lossy())
            .finish();
        created.expect_ok();
        let branch_id = created.data_str("branchId");
        assert!(!branch_id.is_empty(), "{}", created.describe());

        Some(Self {
            workspace,
            _slot: slot,
            repository,
            work,
            branch_id,
            logical,
            profile,
            change_bytes,
            layout,
            initial_writes: initial.len(),
            plan: initial,
            writes_per_commit,
            nodes: Vec::new(),
        })
    }

    fn command(&self, command: &str) -> Spawn<'_> {
        headless(&self.workspace, command)
            .repository(&self.repository)
            .timeout(SCENARIO_TIMEOUT)
    }

    fn rounds(&self) -> usize {
        self.profile.rounds(tier())
    }

    /// 工作文件实际触及的字节数（区间取并集）。反复编辑同一批区域不会让它变大。
    fn region_bytes(&self) -> u64 {
        distinct_written_bytes(&self.plan)
    }

    /// 应用下一轮提交要改动的区域（就地写入工作文件，不改变长度）。
    fn next_change(&mut self, index: usize) {
        let regions = if self.profile.keeps_layout() {
            rewrite_layout(&self.layout, 0xC0FF_EE00_u64.wrapping_add(index as u64))
        } else {
            region_writes(
                self.logical,
                self.change_bytes,
                0xC0FF_EE00_u64.wrapping_add(index as u64),
                0xC0FF_EE00_u64.wrapping_add(index as u64),
            )
        };
        assert_eq!(
            regions.len(),
            self.writes_per_commit,
            "改动区域的条目数必须只由改动总量决定"
        );
        apply_regions(&self.work, &regions);
        self.plan.extend(regions);
    }

    /// 提交一次；返回命令结果，并把节点标识追加到 `nodes`。
    fn commit(&mut self, note: &str) -> Outcome {
        let outcome = self
            .command("commit")
            .arg("branch", &self.branch_id)
            .arg("commit-kind", "manual")
            .arg("note", note)
            .finish();
        outcome.expect_ok();
        assert!(
            outcome.data("created").as_bool().unwrap_or(false),
            "每次改动都必须产生新节点：\n{}",
            outcome.describe()
        );
        let history_id = outcome.data_str("historyId");
        assert!(!history_id.is_empty(), "{}", outcome.describe());
        self.nodes.push(history_id);
        outcome
    }

    /// 把「第 `index` 次提交时」的期望内容写成稀疏文件并返回其路径。
    ///
    /// 期望内容由写入计划复算，因此不需要在内存或磁盘上保留逻辑大小的副本。
    fn expected_state(&self, index: usize) -> PathBuf {
        let count = self.initial_writes + index * self.writes_per_commit;
        assert!(
            count <= self.plan.len(),
            "期望状态 {index} 超出已记录的计划（{count} > {}）",
            self.plan.len()
        );
        let path = self
            .workspace
            .scratch_directory()
            .join(format!("expected-{index}.bin"));
        let _ = fs::remove_file(&path);
        write_sparse_file(&path, self.logical, &self.plan[..count]);
        path
    }

    /// 恢复一个历史节点到 `out/`，返回命令结果与输出路径。
    fn restore(&self, history_id: &str, name: &str) -> (Outcome, PathBuf) {
        let output = self.workspace.out(name);
        assert!(!output.exists(), "恢复输出路径必须事先不存在");
        assert!(
            self.workspace.out_directory().is_dir(),
            "恢复输出目录不存在：{}；工作区顶层={:?}，scratch={:?}",
            self.workspace.out_directory().display(),
            directory_names(self.workspace.root()),
            directory_names(&self.workspace.scratch_directory()),
        );
        let outcome = self
            .command("restore")
            .arg("history", history_id)
            .arg("output", output.to_string_lossy())
            .finish();
        outcome.expect_ok();
        (outcome, output)
    }

    /// 重开 + 全库逐块校验。**必须显式带上场景超时**：全库校验是
    /// O(节点数 × 文件大小)，1 GiB 档远超 helper 的默认 180 s。
    fn healthy(&self) -> HealthReport {
        assert_healthy_within(&self.workspace, &self.repository, SCENARIO_TIMEOUT)
    }

    /// 账本口径的工作区实际占用。
    fn allocated(&self) -> u64 {
        tree_usage(&self.workspace).allocated_estimate(self.region_bytes())
    }
}

/// 一次提交的公共统计：总耗时与最大峰值工作集。
struct CommitRun {
    elapsed_ms: u64,
    peak: u64,
}

impl CommitRun {
    fn new() -> Self {
        Self {
            elapsed_ms: 0,
            peak: 0,
        }
    }

    fn observe(&mut self, outcome: &Outcome) {
        self.elapsed_ms += outcome.elapsed.as_millis() as u64;
        self.peak = self.peak.max(peak_of(outcome));
    }
}

fn peak_of(outcome: &Outcome) -> u64 {
    outcome.result["peakWorkingSetBytes"].as_u64().unwrap_or(0)
}

/// 启动基线：用同一份二进制做一次最轻的完整仓库校验，取其峰值工作集。
///
/// 峰值内存只有相对基线的增量才可解释——它扣掉了运行时、SQLite 与固定缓冲的贡献。
fn baseline_peak(scenario: &Scenario) -> u64 {
    let outcome = scenario.command("verify").finish();
    outcome.expect_ok();
    peak_of(&outcome)
}

/// 把一次场景的磁盘账本写进 `ledger.jsonl`，并断言实测增量不超过预期加容差。返回
/// `(实测增量, 预期增量, 容差)`，供调用方写进可长期引用的报告。
///
/// 预期增量 = 场景新增的实体文件（snapshot + delta）与工作文件新增写入的区域之和；实测
/// 增量是工作区「文件长度之和，稀疏工作文件按已写入区域计」的差值。上界比对是一条
/// 「存储放大是否失控」的回归断言：泄漏的中间 snapshot、未回收的临时文件与游离实体都会
/// 让实测超出预期。
fn record_ledger(
    scenario: &Scenario,
    name: &str,
    before: u64,
    after: u64,
    expected_delta: u64,
    extra: serde_json::Value,
) -> (u64, u64, u64) {
    let measured = after.saturating_sub(before);
    let tolerance =
        (expected_delta as f64 * LEDGER_TOLERANCE_RATIO) as u64 + LEDGER_TOLERANCE_BYTES;
    append_ledger(
        &scenario.workspace,
        json!({
            "scenario": name,
            "tier": tier().name(),
            "logicalBytes": scenario.logical,
            "regionBytes": scenario.region_bytes(),
            "workspaceBeforeBytes": before,
            "workspaceAfterBytes": after,
            "measuredDeltaBytes": measured,
            "expectedDeltaBytes": expected_delta,
            "toleranceBytes": tolerance,
            "detail": extra,
        }),
    );
    assert!(
        measured <= expected_delta.saturating_add(tolerance),
        "账本实测增量超出预期：实测 {measured} 字节，预期 {expected_delta} 字节，\
         容差 {tolerance} 字节（{extra}）"
    );
    (measured, expected_delta, tolerance)
}

/// 断言 head snapshot 的大小符合文档记录的「不压缩、不去重」特征：约等于逻辑大小加
/// 每块开销。
#[track_caller]
fn assert_snapshot_matches_logical(label: &str, snapshot_bytes: u64, logical: u64) {
    let allowance = (logical as f64 * SNAPSHOT_OVERHEAD_RATIO) as u64 + SNAPSHOT_OVERHEAD_BYTES;
    assert!(
        snapshot_bytes >= logical && snapshot_bytes <= logical + allowance,
        "{label}：snapshot 应约等于逻辑大小（{logical} 字节），实际 {snapshot_bytes} 字节"
    );
}

/// C1 大文件端到端：提交 → 20 次有界区域改动 → 恢复 → 逐位一致。
///
/// 恢复两个端点：最早的节点（要穿过 20 层 delta 才能还原）与 head（直接读 snapshot）。
/// 两者都要求恢复产物与「当时的期望内容」逐位一致，而不是「不报错」。
#[test]
fn c1_large_file_round_trip_is_bit_identical() {
    let Some(mut scenario) = Scenario::new("C1 large file round trip", ChangeProfile::Small) else {
        return;
    };
    let tier = tier();
    let before = scenario.allocated();
    let regions_before = scenario.region_bytes();

    let baseline = scenario.commit("baseline");
    let mut run = CommitRun::new();
    run.observe(&baseline);
    for index in 1..=scenario.rounds() {
        scenario.next_change(index);
        let outcome = scenario.commit(&format!("bounded change {index}"));
        run.observe(&outcome);
    }
    assert_eq!(scenario.nodes.len(), scenario.rounds() + 1);

    // 重开可用：迁移 + 完整性 + 外键 + 语义校验，以及逐块摘要链。
    let health = scenario.healthy();
    assert_eq!(health.nodes as usize, scenario.rounds() + 1);
    assert_no_stray(&scenario.repository);
    assert_cleanup_empty(&scenario.workspace, &scenario.repository);

    let usage = tree_usage(&scenario.workspace);
    let changed = scenario.change_bytes * scenario.rounds() as u64;
    // 账本预期 = 新增实体（快照 + 增量）+ 工作文件真实触及字节数的增量（区间取并集：
    // 反复编辑同一批区域不会让稀疏文件变大）。
    let region_growth = scenario.region_bytes() - regions_before;
    let (ledger_measured, ledger_expected, ledger_tolerance) = record_ledger(
        &scenario,
        "C1 large file round trip",
        before,
        scenario.allocated(),
        usage.snapshots + usage.deltas + region_growth,
        json!({
            "snapshotBytes": usage.snapshots,
            "deltaBytes": usage.deltas,
            "changedBytes": changed,
            "regionGrowthBytes": region_growth,
            "commitMs": run.elapsed_ms,
        }),
    );

    // 最早的节点：恢复链最长，必须与第 0 次提交时的期望内容逐位一致。
    let expected_oldest = scenario.expected_state(0);
    let (oldest, oldest_output) = scenario.restore(&scenario.nodes[0], "c1-oldest.bin");
    assert_eq!(file_bytes(&oldest_output), scenario.logical);
    assert_files_identical(&expected_oldest, &oldest_output);

    // head：与工作文件（即最后一次改动后的真实内容）逐位一致。
    let (head, head_output) = scenario.restore(&scenario.nodes[scenario.rounds()], "c1-head.bin");
    assert!(head.ok());
    assert_files_identical(&scenario.work, &head_output);

    record(
        "C1 large file round trip",
        "C",
        tier.name(),
        &oldest,
        json!({
            "logicalBytes": scenario.logical,
            "regionBytes": scenario.region_bytes(),
            "commitCount": scenario.rounds() + 1,
            "commitMs": run.elapsed_ms,
            "commitPeakWorkingSetBytes": run.peak,
            "oldestChainSteps": scenario.rounds(),
            "oldestRestoreMs": oldest.elapsed.as_millis() as u64,
            "oldestRestorePeakWorkingSetBytes": peak_of(&oldest),
            "headRestoreMs": head.elapsed.as_millis() as u64,
            "verifyMs": health.verify_ms,
            "scrubMs": health.scrub_ms,
            "scrubPeakWorkingSetBytes": health.scrub_peak_bytes,
            "verifiedBytes": scenario.logical * 2,
            "snapshotBytes": usage.snapshots,
            "deltaBytes": usage.deltas,
            "ledgerMeasuredDeltaBytes": ledger_measured,
            "ledgerExpectedDeltaBytes": ledger_expected,
            "ledgerToleranceBytes": ledger_tolerance,
        }),
    );

    // 恢复产物是纯瞬时产物，断言后立即拆除。
    fs::remove_file(&oldest_output).expect("无法清理恢复输出");
    fs::remove_file(&head_output).expect("无法清理恢复输出");
    let _ = fs::remove_file(&expected_oldest);
    assert!(directory_names(&scenario.workspace.out_directory()).is_empty());
}

/// C2 大文件精简：精简中间的改动节点后仍可恢复，且替换 delta 的体积符合预期。
#[test]
fn c2_large_file_compaction_keeps_restorable_history() {
    let Some(mut scenario) = Scenario::new("C2 large file compaction", ChangeProfile::Small) else {
        return;
    };
    let tier = tier();

    scenario.commit("n0");
    for index in 1..=3 {
        scenario.next_change(index);
        scenario.commit(&format!("n{index}"));
    }
    let before = tree_usage(&scenario.workspace);
    assert_eq!(scenario.healthy().nodes, 4);

    // 精简第 1 个节点（n0 与 n2 之间）：它的 delta 被替换成 n2 → n0。
    let compact = scenario
        .command("compact")
        .arg("history", &scenario.nodes[1])
        .finish();
    compact.expect_ok();
    assert_eq!(scenario.healthy().nodes, 3, "精简应恰好移除一个节点");
    assert_no_stray(&scenario.repository);
    assert_cleanup_empty(&scenario.workspace, &scenario.repository);

    // 精简后两端都必须能恢复出逐位一致的内容。
    let expected_oldest = scenario.expected_state(0);
    let (oldest, oldest_output) = scenario.restore(&scenario.nodes[0], "c2-oldest.bin");
    assert_files_identical(&expected_oldest, &oldest_output);
    let (head, head_output) = scenario.restore(&scenario.nodes[3], "c2-head.bin");
    assert_files_identical(&scenario.work, &head_output);

    // 替换后的 delta 覆盖两次改动，体积仍应远小于 snapshot。
    let after = tree_usage(&scenario.workspace);
    assert!(
        after.deltas > 0 && after.deltas * 4 <= after.snapshots,
        "精简后的 delta 总量应远小于 snapshot：delta {} 字节，snapshot {} 字节",
        after.deltas,
        after.snapshots
    );
    assert_snapshot_matches_logical("精简后", after.snapshots, scenario.logical);

    record(
        "C2 large file compaction",
        "C",
        tier.name(),
        &head,
        json!({
            "logicalBytes": scenario.logical,
            "regionBytes": scenario.region_bytes(),
            "nodesBefore": 4,
            "nodesAfter": 3,
            "snapshotBytes": after.snapshots,
            "deltaBytesBefore": before.deltas,
            "deltaBytesAfter": after.deltas,
            "oldestRestoreMs": oldest.elapsed.as_millis() as u64,
            "headRestoreMs": head.elapsed.as_millis() as u64,
        }),
    );

    fs::remove_file(&oldest_output).expect("无法清理恢复输出");
    fs::remove_file(&head_output).expect("无法清理恢复输出");
    let _ = fs::remove_file(&expected_oldest);
}

/// C3 大文件内存：提交与恢复的峰值内存增量不随逻辑大小线性增长。
///
/// 允许的增量是「逻辑大小的 1/8 + 24 MiB」（固定项覆盖 debug 档运行时与 zstd 工作区）。
/// 整文件入内存的实现会多出整整一个逻辑大小，因此在任何档位都会超出这个界。
///
/// 峰值只有指向发布档才有参考价值：`LILITH_STRESS_BIN=target/release/lilith-artworks.exe`。
#[test]
fn c3_large_file_memory_does_not_follow_logical_size() {
    let Some(mut scenario) = Scenario::new("C3 large file memory", ChangeProfile::Small) else {
        return;
    };
    let tier = tier();
    let baseline = baseline_peak(&scenario);

    scenario.commit("baseline");
    scenario.next_change(1);
    let commit = scenario.commit("bounded change 1");
    let (restore, output) = scenario.restore(&scenario.nodes[0], "c3.bin");

    let allowance = scenario.logical / 8 + 24 * MIB;
    for (label, outcome) in [("提交", &commit), ("恢复", &restore)] {
        let peak = peak_of(outcome);
        let increment = peak.saturating_sub(baseline);
        assert!(
            increment <= allowance,
            "{label}峰值内存增量超出流式实现的量级：基线 {baseline} 字节，峰值 {peak} 字节，\
             增量 {increment} 字节，允许 {allowance} 字节（逻辑 {} 字节）",
            scenario.logical
        );
        assert!(
            increment < scenario.logical,
            "{label}峰值内存增量必须远小于逻辑大小，否则不是流式实现：增量 {increment} 字节，\
             逻辑 {} 字节",
            scenario.logical
        );
    }

    record(
        "C3 large file memory",
        "C",
        tier.name(),
        &commit,
        json!({
            "logicalBytes": scenario.logical,
            "baselineWorkingSetBytes": baseline,
            "commitPeakWorkingSetBytes": peak_of(&commit),
            "commitIncrementBytes": peak_of(&commit).saturating_sub(baseline),
            "restorePeakWorkingSetBytes": peak_of(&restore),
            "restoreIncrementBytes": peak_of(&restore).saturating_sub(baseline),
            "allowanceBytes": allowance,
            "restoreMs": restore.elapsed.as_millis() as u64,
        }),
    );

    fs::remove_file(&output).expect("无法清理恢复输出");
}

/// C4 大文件链体积：delta 总量由改动量决定，而不是由文件大小决定；账本实测增量与预期
/// 的偏差在阈值内。
#[test]
fn c4_large_file_chain_volume_stays_bounded() {
    let Some(mut scenario) = Scenario::new("C4 large file chain volume", ChangeProfile::Small)
    else {
        return;
    };
    let tier = tier();

    // 基线提交：此后仓库已经持有 head 的 snapshot，账本只需再计增量与新增触及的区域。
    scenario.commit("baseline");
    let before = scenario.allocated();
    let regions_before = scenario.region_bytes();

    let mut run = CommitRun::new();
    let mut last = None;
    for index in 1..=scenario.rounds() {
        scenario.next_change(index);
        let outcome = scenario.commit(&format!("bounded change {index}"));
        run.observe(&outcome);
        last = Some(outcome);
    }
    let last = last.expect("至少要有一次改动");

    let usage = tree_usage(&scenario.workspace);
    let changed = scenario.change_bytes * scenario.rounds() as u64;
    let region_growth = scenario.region_bytes() - regions_before;

    // head 恒持有 snapshot，且 snapshot 不压缩、不去重：约等于逻辑大小。
    assert_snapshot_matches_logical("C4", usage.snapshots, scenario.logical);
    assert!(
        usage.deltas * 4 <= usage.snapshots,
        "delta 总量（{} 字节）应远小于 snapshot（{} 字节）",
        usage.deltas,
        usage.snapshots
    );
    // delta 跟随改动量：允许内容定义分块在改动边界附近重新同步的固定倍数开销。
    let delta_allowance = changed * DELTA_CHANGE_FACTOR + MIB;
    assert!(
        usage.deltas <= delta_allowance,
        "delta 总量应跟随改动量：delta {} 字节，改动 {} 字节，允许 {} 字节",
        usage.deltas,
        changed,
        delta_allowance
    );

    let (ledger_measured, ledger_expected, ledger_tolerance) = record_ledger(
        &scenario,
        "C4 large file chain volume",
        before,
        scenario.allocated(),
        usage.deltas + region_growth,
        json!({
            "snapshotBytes": usage.snapshots,
            "deltaBytes": usage.deltas,
            "changedBytes": changed,
            "regionGrowthBytes": region_growth,
            "commitMs": run.elapsed_ms,
            "commitPeakWorkingSetBytes": run.peak,
        }),
    );

    // 仓库仍可重开；链完整性由 C1 覆盖，这里不再做一次全库逐块校验（那是
    // O(节点数 × 文件大小) 的代价，本场景的重点是链**体积**）。
    assert_reopens(&scenario.workspace, &scenario.repository);
    assert_no_stray(&scenario.repository);
    assert_cleanup_empty(&scenario.workspace, &scenario.repository);
    // 链形状的磁盘事实：head 一枚 snapshot，其余每个节点一枚反向 delta。
    let entities = storage_state(&scenario.repository);
    assert_eq!(entities.snapshots.len(), 1, "{:?}", entities.snapshots);
    assert_eq!(
        entities.deltas.len(),
        scenario.rounds(),
        "每个非 head 节点应恰好持有一枚反向 delta：{:?}",
        entities.deltas
    );

    record(
        "C4 large file chain volume",
        "C",
        tier.name(),
        &last,
        json!({
            "logicalBytes": scenario.logical,
            "regionBytes": scenario.region_bytes(),
            "snapshotBytes": usage.snapshots,
            "deltaBytes": usage.deltas,
            "deltaToSnapshotRatio": usage.deltas as f64 / usage.snapshots.max(1) as f64,
            "changedBytes": changed,
            "deltaToChangedRatio": usage.deltas as f64 / changed.max(1) as f64,
            "commitMs": run.elapsed_ms,
            "ledgerMeasuredDeltaBytes": ledger_measured,
            "ledgerExpectedDeltaBytes": ledger_expected,
            "ledgerToleranceBytes": ledger_tolerance,
        }),
    );
}

/// C5 恢复产物即删：恢复输出是全尺寸的瞬时产物，断言完成后立即清空、工作区占用回落。
#[test]
fn c5_restore_output_is_transient_and_released() {
    let Some(mut scenario) = Scenario::new("C5 restore output teardown", ChangeProfile::Small)
    else {
        return;
    };
    let tier = tier();

    scenario.commit("n0");
    scenario.next_change(1);
    scenario.commit("n1");
    let before = scenario.allocated();

    let (restore, output) = scenario.restore(&scenario.nodes[1], "c5.bin");
    let during = tree_usage(&scenario.workspace);
    assert!(output.exists());
    assert_eq!(
        during.out, scenario.logical,
        "恢复产物必须是全尺寸文件（约等于逻辑大小）"
    );
    assert_eq!(
        directory_names(&scenario.workspace.out_directory()).len(),
        1,
        "恢复期间输出目录应只有一个产物"
    );
    let during_allocated = during.allocated_estimate(scenario.region_bytes());

    fs::remove_file(&output).expect("无法清理恢复输出");
    let after = scenario.allocated();
    assert!(
        directory_names(&scenario.workspace.out_directory()).is_empty(),
        "恢复输出必须在断言后立即清空"
    );
    assert_eq!(tree_usage(&scenario.workspace).out, 0);
    // 仓库只被读过，因此工作区占用必须回落到恢复之前（SQLite 的 WAL 允许极小抖动）。
    assert!(
        after <= before + MIB,
        "工作区占用必须回落：恢复前 {before} 字节，拆除后 {after} 字节"
    );

    record(
        "C5 restore output teardown",
        "C",
        tier.name(),
        &restore,
        json!({
            "logicalBytes": scenario.logical,
            "outputBytes": during.out,
            "workspaceBeforeBytes": before,
            "workspaceDuringBytes": during_allocated,
            "workspaceAfterBytes": after,
            "restoreMs": restore.elapsed.as_millis() as u64,
        }),
    );
}

/// C6 大文件大变动：每次提交改动约四分之一的文件（4 GiB 文件改 1 GiB）。
///
/// 与 C1 互补，覆盖「变动量」这条正交的轴：C1 证明「增量跟随改动量、远小于快照」，
/// C6 证明**最坏存储放大**下依然正确——每轮增量本身接近改动量量级，此时断言换成
/// 「增量与改动量同量级、不失控」，而不是「远小于快照」。
///
/// 夹具刻意让每轮编辑**同一批区域**：这样父版本的旧内容同样是真实随机数据，delta 无法
/// 靠压缩变小，测的就是最坏情形。
#[test]
fn c6_large_file_large_change_round_trip() {
    let Some(mut scenario) = Scenario::new("C6 large change round trip", ChangeProfile::Large)
    else {
        return;
    };
    let tier = tier();
    let before = scenario.allocated();
    let regions_before = scenario.region_bytes();

    let baseline = scenario.commit("baseline");
    let mut run = CommitRun::new();
    run.observe(&baseline);
    let mut last = None;
    for index in 1..=scenario.rounds() {
        scenario.next_change(index);
        let outcome = scenario.commit(&format!("large change {index}"));
        run.observe(&outcome);
        last = Some(outcome);
    }
    let last = last.expect("至少要有一次大改动");
    assert_eq!(scenario.nodes.len(), scenario.rounds() + 1);

    // 仓库可重开（迁移 + 完整性 + 外键 + 全表语义校验）；全部节点的逐块摘要链校验由 C1
    // 覆盖，这里不再重复 O(节点数 × 文件大小) 的全库校验。
    assert_reopens(&scenario.workspace, &scenario.repository);
    assert_no_stray(&scenario.repository);
    assert_cleanup_empty(&scenario.workspace, &scenario.repository);

    // 体积与账本必须在**恢复之前**测量：期望内容文件与恢复产物都是全尺寸临时文件，一旦
    // 生成会把工作区占用抬高约一个逻辑大小，混进账本就会误报「实测超出预期」。C1/C4 同样
    // 在恢复前记账，这里保持一致的顺序。
    let usage = tree_usage(&scenario.workspace);
    let rounds = scenario.rounds() as u64;
    let changed = scenario.change_bytes * rounds;
    assert_snapshot_matches_logical("C6", usage.snapshots, scenario.logical);

    // 大改动下不再断言「增量远小于快照」，改为断言增量与改动量同量级且不失控。
    let delta_allowance = changed * LARGE_DELTA_CHANGE_FACTOR + MIB;
    assert!(
        usage.deltas > 0 && usage.deltas <= delta_allowance,
        "大改动下的 delta 总量应与改动量同量级：delta {} 字节，改动 {} 字节，允许 {} 字节",
        usage.deltas,
        changed,
        delta_allowance
    );
    // 每一轮都必须产生真实的、不可压缩的增量，否则「大变动」并没有真的发生。
    let per_round = usage.deltas / rounds.max(1);
    assert!(
        per_round > scenario.change_bytes / 4,
        "每轮增量应接近改动量量级：每轮 {} 字节，改动 {} 字节",
        per_round,
        scenario.change_bytes
    );

    // 账本：预期 = 新增实体 + 工作文件真实触及字节数的增量（同一批区域被反复编辑时后者为 0）。
    let region_growth = scenario.region_bytes() - regions_before;
    let (ledger_measured, ledger_expected, ledger_tolerance) = record_ledger(
        &scenario,
        "C6 large change round trip",
        before,
        scenario.allocated(),
        usage.snapshots + usage.deltas + region_growth,
        json!({
            "snapshotBytes": usage.snapshots,
            "deltaBytes": usage.deltas,
            "changedBytes": changed,
            "regionGrowthBytes": region_growth,
            "rounds": rounds,
            "commitMs": run.elapsed_ms,
        }),
    );

    // 恢复到两个端点：最早的节点要穿过全部**大**增量才能还原，head 直接读快照；两者都必须
    // 与当时的期望内容逐位一致。此步放在账本之后，避免全尺寸临时产物干扰占用测量。
    let expected_oldest = scenario.expected_state(0);
    let (oldest, oldest_output) = scenario.restore(&scenario.nodes[0], "c6-oldest.bin");
    assert_eq!(file_bytes(&oldest_output), scenario.logical);
    assert_files_identical(&expected_oldest, &oldest_output);
    let (head, head_output) = scenario.restore(&scenario.nodes[scenario.rounds()], "c6-head.bin");
    assert_files_identical(&scenario.work, &head_output);

    record(
        "C6 large change round trip",
        "C",
        tier.name(),
        &last,
        json!({
            "logicalBytes": scenario.logical,
            "changeProfile": scenario.profile.name(),
            "changeBytesPerRound": scenario.change_bytes,
            "rounds": rounds,
            "regionBytes": scenario.region_bytes(),
            "snapshotBytes": usage.snapshots,
            "deltaBytes": usage.deltas,
            "deltaToChangedRatio": usage.deltas as f64 / changed.max(1) as f64,
            "deltaToSnapshotRatio": usage.deltas as f64 / usage.snapshots.max(1) as f64,
            "oldestRestoreMs": oldest.elapsed.as_millis() as u64,
            "headRestoreMs": head.elapsed.as_millis() as u64,
            "commitMs": run.elapsed_ms,
            "commitPeakWorkingSetBytes": run.peak,
            "ledgerMeasuredDeltaBytes": ledger_measured,
            "ledgerExpectedDeltaBytes": ledger_expected,
            "ledgerToleranceBytes": ledger_tolerance,
        }),
    );

    fs::remove_file(&oldest_output).expect("无法清理恢复输出");
    fs::remove_file(&head_output).expect("无法清理恢复输出");
    let _ = fs::remove_file(&expected_oldest);
}
