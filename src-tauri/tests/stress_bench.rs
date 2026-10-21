//! P 组：核心算法性能基准（节点链、认证、画板 DDS、库树与搜索、整仓灾备、清理扫描）。
//!
//! 与其他压力组的区别：这里测的不是「极端下是否还正确」，而是**常规规模下的单位耗时**，
//! 目的是给重构一个可比的基线——此前的性能断言只有「上界」（例如峰值内存 ≤ 逻辑大小
//! 的 1/8），没有基线，因此「某次重构让吞吐缓慢退化」不会被任何断言发现。
//!
//! 各项都走**真实可执行文件**（与其余压力组同一纪律），因此耗时含进程启动与打开仓库的
//! 固定开销；这也是产品在被调用时实际付出的成本。小文件让固定开销可见，所以同时记录总
//! 耗时与摊薄值，回归看的是**同一档位下的相对变化**，不是绝对值。
//!
//! **每条测量都同时记墙钟与 CPU 时间**（用户 + 内核）。墙钟是用户感知的成本，含等磁盘与
//! 被调度剥夺；CPU 时间是算力成本，不含被剥夺的时间，因此 `cargo test` 并行跑多个场景时
//! 几乎不变——2026-10-18 的实测里同一基准的并行墙钟是单线程的 2.3–4.1 倍，而 CPU 时间不受
//! 这一项影响。两者分工与取舍见 [`Measure`] 与 [`CPU_GATE_MIN_MS`]：CPU 时间看不见磁盘
//! 等待，短操作又会被 Windows 15.625 ms 的记账粒度量化，所以它只回答「是不是更费 CPU」，
//! 不替代墙钟。
//!
//! - **P1 节点链**：96 KiB 工作文件上建 [`CHAIN_NODES`] 个节点的链，测每次提交的摊薄耗时
//!   与「恢复最早节点」（穿越整条反向增量链）的每步耗时；顺带逐位校验恢复内容。
//!   P1B 在同一组里用 4 MiB 工作文件建 [`SEGMENT_NODES`] 个节点，让每条 snapshot/delta
//!   产生大量 chunk 段，反复恢复最早/最新节点并取 p50/p95，作为段索引与读取游标的证据。
//! - **P2 认证**：512 级源图的发布流水线（渲染 / 编码 / 签名各阶段耗时来自产品自报）
//!   与 `decode-authenticity` 回读耗时。
//! - **P3 画板 DDS**：导入 16 张 512 级 PNG（BC7 编码）与全库 DDS 完整性扫描的每张耗时。
//! - **P4 库树与搜索**：[`LIBRARY_GROUPS`] 个分组 × [`LIBRARY_PER_GROUP`] 个作品下，
//!   整树装配（`list-tree`）与两种查询（命中全部 / 命中一个）的耗时。
//! - **P5 整仓灾备**：多作品多版本的仓库复制（扫描 + 复制 + 校验 + 发布）的总耗时与吞吐；
//!   复制阶段同时计算完整/抽样摘要，发布前采用首尾各 64 KiB 的抽样复核。
//! - **P6 清理扫描**：全库未引用文件扫描与待清理队列重放的耗时（此前只有规模数字，
//!   没有基线，因此 D/E 组的耗时退化不会被任何断言发现）。
//! - **P7 多分支调度**：夹具作品之外再造 [`SCHEDULER_ARTWORKS`] 个作品、每作品
//!   1 个主分支 + [`SCHEDULER_FORKS_PER_ARTWORK`] 个分叉下的候选扫描成本。
//! - **P8 大规模孤儿清理**：[`ORPHAN_FILES`] 个无记录画板 DDS 的扫描与确认清理。
//! - **P9 笔迹读写**：500 / 2000 / 5000 笔的批量写入（一次事务）与整板载荷读取，
//!   每档单独一块画板，因此读数就是该档的规模。写入含几何校验与 2N 行历史，读取含
//!   逐条几何校验与二进制打包——两侧都是产品真实路径，不含编解码以外的额外工作。
//!
//! P1B、P2 与 P4–P8 各测 [`REPEATS`] 轮：墙钟取最小值（削弱调度抖动），CPU 时间取均值
//! （单轮只有几十毫秒，逐轮值会被 15.6 ms 的记账粒度量化）。这是**相对基线**能用的前提。
//! P9 只跑单轮：每档要新建一块画板才读得出该档规模，重复写入会把画板越堆越大。
//!
//! **断言只设灾难性上界**（数量级失控），避免把环境噪声变成 flaky；真正的回归门是
//! `LILITH_STRESS_PERF_STRICT=1` 与 `tests/perf-baseline.json` 的比对（见 [`emit`]）。
//! GPU 侧的纹理上传/绘制不在无头入口的覆盖范围内，本组不涉及。
//!
//! 运行：`cargo test --features headless --test stress_bench -- --nocapture`
//! 更新基线：`LILITH_STRESS_PERF_UPDATE=1 cargo test --features headless --test stress_bench -- --test-threads=1`
//! 回归门：`LILITH_STRESS_PERF_STRICT=1 cargo test --features headless --test stress_bench -- --test-threads=1`
//!
//! 用 `LILITH_STRESS_BIN` 指向 release 二进制时，必须同时设 `LILITH_STRESS_PROFILE=release`，
//! 否则数字会被记进（并比对到）debug 档；基线文件顶层的 `machine` 字段记录采集它的机器。
//!
//! 基线与严格比对都在 `--test-threads=1`（单线程）下采集与比对，避免三个基准互相争抢 CPU
//! 带来的抖动；只记录数值时是否单线程无所谓。

// 只在启用 `headless` feature 时编译。理由见 `tests/stress_support/mod.rs` 顶部说明。
#![cfg(feature = "headless")]

mod stress_support;

use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant, SystemTime},
};

use serde_json::{json, Map, Value};
use stress_support::*;

/// P1 的链深：足以让「每步反向增量」的摊薄值稳定，又不至于让场景变慢。
const CHAIN_NODES: usize = 48;
/// P1 的工作文件大小：小到让固定开销可见，大到足以产生多块内容。
const NODE_FILE_BYTES: usize = 96 * 1024;
/// P1B 的段规模：4 MiB 随机内容在 2/16/64 KiB 的 CDC 参数下产生数十个 chunk 段，
/// 足以让「每次线性找段」与「二分 + 游标」的差异在恢复耗时上可见。
const SEGMENT_FILE_BYTES: usize = 4 * 1024 * 1024;
const SEGMENT_NODES: usize = 6;
const SEGMENT_RESTORES: usize = 12;
/// P2 的认证源图边长（512 级：真实链路，成本可控）。
const AUTH_EDGE: u32 = 512;
/// P3 的导入图片数量与边长。
const BOARD_IMAGES: usize = 16;
const BOARD_IMAGE_EDGE: u32 = 512;
/// P4 的库规模：分组数 × 每组作品数（含分组本身共 `GROUPS × (PER_GROUP + 1)` 个节点）。
const LIBRARY_GROUPS: usize = 5;
const LIBRARY_PER_GROUP: usize = 20;
/// P5 的仓库规模：作品数 × 每作品提交数，单版本体积。
const IO_ARTWORKS: usize = 4;
const IO_COMMITS: usize = 3;
const IO_FILE_BYTES: usize = 256 * 1024;
/// P6 的仓库规模：只为让扫描有足够多的快照与增量可遍历。
const SCAN_ARTWORKS: usize = 6;
const SCAN_COMMITS: usize = 4;
const SCAN_FILE_BYTES: usize = 96 * 1024;
/// P7 的多分支规模：10 个作品 ×（主分支 + 11 个分叉）+ 夹具自带的主分支
/// = 121 条调度分支。
const SCHEDULER_ARTWORKS: usize = 10;
const SCHEDULER_FORKS_PER_ARTWORK: usize = 11;
/// P8 的大规模孤儿：120 个画板 DDS 候选，一次扫描并确认清理。
const ORPHAN_FILES: usize = 120;
const ORPHAN_FILE_BYTES: usize = 16 * 1024;
/// P9 的三档笔迹规模（计划里的日常 / 极端 / 极限）与每笔点数。
const STROKE_TIERS: [usize; 3] = [500, 2_000, 5_000];
const STROKE_POINTS: usize = 200;
/// P4–P8 的重测轮数：取最小值以削弱调度抖动（回归看的是同一档位下的相对变化）。
const REPEATS: usize = 3;

/// 灾难性回归护栏（**只拦数量级失控**，正常波动远在其下）。
const CATASTROPHIC_COMMIT_MS: f64 = 5_000.0;
const CATASTROPHIC_CHAIN_STEP_MS: f64 = 1_000.0;
const CATASTROPHIC_SEGMENT_RESTORE_MS: f64 = 60_000.0;
const CATASTROPHIC_AUTH_PUBLISH_MS: f64 = 300_000.0;
const CATASTROPHIC_AUTH_DECODE_MS: f64 = 120_000.0;
const CATASTROPHIC_DDS_PER_IMAGE_MS: f64 = 20_000.0;
const CATASTROPHIC_LIBRARY_LIST_MS: f64 = 60_000.0;
const CATASTROPHIC_LIBRARY_SEARCH_MS: f64 = 60_000.0;
const CATASTROPHIC_BACKUP_MS: f64 = 600_000.0;
const CATASTROPHIC_CLEANUP_SCAN_MS: f64 = 300_000.0;
const CATASTROPHIC_SCHEDULER_SCAN_MS: f64 = 60_000.0;
const CATASTROPHIC_ORPHAN_CLEANUP_MS: f64 = 300_000.0;
/// P9 的护栏按「每笔」与「整板读取」两档给：5000 笔的写入目标 < 500 ms、读取目标
/// < 200 ms（计划里的指标），这里只拦数量级失控（debug 档与冷缓存都远快于此）。
const CATASTROPHIC_STROKE_WRITE_PER_STROKE_MS: f64 = 20.0;
const CATASTROPHIC_STROKE_READ_MS: f64 = 60_000.0;

// ---------------------------------------------------------------------------
// 性能基线的记录、更新与比对
// ---------------------------------------------------------------------------

static BASELINE_LOCK: Mutex<()> = Mutex::new(());

fn baseline_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("perf-baseline.json")
}

/// 基准按**被测二进制**的构建档分开记录：debug 与 release 的绝对耗时差一个数量级，
/// 混在一起没有意义。
///
/// 测试脚手架自身的构建档与被测二进制可以不同（文档里的标准做法就是「debug 脚手架 +
/// `LILITH_STRESS_BIN` 指向 release 二进制」），因此 `cfg!(debug_assertions)` 只在未显式
/// 指定时兜底；指向别的构建档时必须设 `LILITH_STRESS_PROFILE`，否则数字会记到错误的档位上。
fn profile_name() -> &'static str {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| match std::env::var("LILITH_STRESS_PROFILE") {
        Ok(value) if !value.trim().is_empty() => value.trim().to_owned(),
        _ => if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
        .to_owned(),
    })
    .as_str()
}

fn env_flag(name: &str) -> bool {
    matches!(
        std::env::var(name).as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

fn read_baseline() -> Value {
    fs::read(baseline_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_else(|| Value::Object(Map::new()))
}

/// 墙钟测量的严格门比较方向：指标不是同一个方向，门槛也不能是同一个方向。
///
/// - [`WallGate::Latency`]：耗时类，越低越好，`ms <= base * factor`（默认）；
/// - [`WallGate::Throughput`]：吞吐量类，越大越好，`value >= base / factor`——
///   否则恢复速度提升反而会被判成回归；
/// - [`WallGate::RecordOnly`]：只记录不比对。用于分辨率不足的派生指标
///   （如每笔耗时：0.045 ms 在 0.1 ms 写入分辨率下取整成 0，任何非零实测都「超阈值」），
///   严格门改看同场景的整批总量。
#[derive(Clone, Copy, PartialEq, Eq)]
enum WallGate {
    Latency,
    Throughput,
    RecordOnly,
}

/// 一条性能测量：墙钟与 CPU 时间**都记**，两者分工不同。
///
/// - **墙钟**是用户感知的成本（含等磁盘、含被调度剥夺），跨机器、跨负载都不可比；
/// - **CPU 时间**（用户 + 内核）是算力成本，不含进程被剥夺的时间，因此 `cargo test` 并行
///   跑多个场景时几乎不变——这正是墙钟做不到的（同一基准并行采集被拉长 2.3–4.1 倍）。
///
/// 但 CPU 时间**看不见磁盘等待**，而本项目提交/恢复/灾备都是落盘路径，所以它只能回答
/// 「是不是更费 CPU」，不能回答「用户等了多久」。基线里两者各占一个键：
/// `scenario::label` 是墙钟，`scenario::label#cpu` 是 CPU 时间。
struct Measure {
    label: &'static str,
    wall_ms: f64,
    cpu_ms: Option<f64>,
    gate: WallGate,
}

/// 一次测量的样本：墙钟 +（可选的）CPU 时间。派生值（每张图、每步、每节点）用
/// [`Self::scale`] 同时摊薄两者。
#[derive(Clone, Copy)]
struct Sample {
    wall_ms: f64,
    cpu_ms: Option<f64>,
}

impl Sample {
    /// 只有墙钟的样本：产品自报的分阶段耗时（渲染 / 编码 / 签名）与派生的吞吐量。
    fn wall(wall_ms: f64) -> Self {
        Self {
            wall_ms,
            cpu_ms: None,
        }
    }

    /// 一次子进程调用的样本：墙钟由调用方计时，CPU 记账来自 [`Outcome::cpu`]。
    fn of(wall: Duration, outcome: &Outcome) -> Self {
        Self::from_cpu(wall, outcome.cpu)
    }

    /// 同 [`Self::of`]，但 CPU 记账是多个子进程累加的结果（多进程场景）。
    fn from_cpu(wall: Duration, cpu: CpuTime) -> Self {
        Self {
            wall_ms: millis(wall),
            cpu_ms: cpu.measured.then(|| cpu.total_ms()),
        }
    }

    /// 按分母摊薄（每张图片、每个节点）。
    fn scale(self, divisor: f64) -> Self {
        Self {
            wall_ms: self.wall_ms / divisor,
            cpu_ms: self.cpu_ms.map(|cpu| cpu / divisor),
        }
    }

    fn measure(self, label: &'static str) -> Measure {
        Measure {
            label,
            wall_ms: self.wall_ms,
            cpu_ms: self.cpu_ms,
            gate: WallGate::Latency,
        }
    }

    fn wall_measure(self, label: &'static str) -> Measure {
        Measure {
            label,
            wall_ms: self.wall_ms,
            cpu_ms: None,
            gate: WallGate::Latency,
        }
    }
}

impl Measure {
    /// 该指标只记录：照常进基线与报告，但不参与严格比对。
    fn record_only(mut self) -> Self {
        self.gate = WallGate::RecordOnly;
        self
    }

    /// 该指标是吞吐量（越大越好）：严格门按 `value >= base / factor` 比较。
    fn throughput(mut self) -> Self {
        self.gate = WallGate::Throughput;
        self
    }
}

/// CPU 时间进基线比对的最小长度。
///
/// Windows 的 CPU 时间按时钟中断记账，实测粒度 15.625 ms：比它短的操作会被量化成
/// `0 / 15.6 / 31.3` 这样的跳变，单是量化误差就能吃掉 2.5× 的阈值。因此短操作只记
/// CPU 时间（供人工看趋势）、不参与严格比对，墙钟仍照常比对。
const CPU_GATE_MIN_MS: f64 = 50.0;

/// 记录一组测量，并按需更新基线或在严格模式下比对。
///
/// 默认只记录（写进 `<target>/stress/report.jsonl` 并打印）。基线的比对**必须显式开启**
/// （`LILITH_STRESS_PERF_STRICT=1`），因为绝对耗时依赖机器；`LILITH_STRESS_PERF_FACTOR`
/// 控制允许的倍数（默认 2.5×）。CPU 时间同样按该倍数比对，但只在基线值
/// ≥ [`CPU_GATE_MIN_MS`] 时参与——见该常量的说明。
fn emit(scenario: &str, measurements: &[Measure], extra: Value) {
    let profile = profile_name();
    for measure in measurements {
        record_measurement(
            scenario,
            "P",
            &format!("{profile} {}", measure.label),
            measure.wall_ms,
            extra.clone(),
        );
        // 吞吐量的「墙钟」说法没有意义（它本身就是墙钟算出来的派生值），单位直接标 MiB/s。
        if measure.gate == WallGate::Throughput {
            println!(
                "[perf] {scenario} :: {} = {:.1} MiB/s ({profile})",
                measure.label, measure.wall_ms
            );
        } else {
            println!(
                "[perf] {scenario} :: {} = {:.1} ms wall ({profile})",
                measure.label, measure.wall_ms
            );
        }
        let Some(cpu_ms) = measure.cpu_ms else {
            continue;
        };
        record_measurement(
            scenario,
            "P",
            &format!("{profile} {} (CPU)", measure.label),
            cpu_ms,
            extra.clone(),
        );
        println!(
            "[perf] {scenario} :: {} = {:.1} ms cpu ({profile})",
            measure.label, cpu_ms
        );
    }

    if env_flag("LILITH_STRESS_PERF_UPDATE") {
        update_baseline(scenario, profile, measurements);
        return;
    }
    if !env_flag("LILITH_STRESS_PERF_STRICT") {
        return;
    }

    let factor = std::env::var("LILITH_STRESS_PERF_FACTOR")
        .ok()
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(2.5);
    let baseline = read_baseline();
    let table = baseline.get(profile).and_then(Value::as_object);
    for measure in measurements {
        if measure.gate == WallGate::RecordOnly {
            eprintln!(
                "[perf] {} 只记录，不参与严格比对（分辨率不足或派生指标）",
                baseline_key(scenario, measure.label)
            );
        } else {
            let (unit, gate) = match measure.gate {
                WallGate::Throughput => ("吞吐量", WallGate::Throughput),
                _ => ("墙钟", WallGate::Latency),
            };
            compare(
                table
                    .and_then(|table| table.get(&baseline_key(scenario, measure.label)))
                    .and_then(Value::as_f64),
                factor,
                scenario,
                measure.label,
                measure.wall_ms,
                unit,
                gate,
            );
        }
        if let Some(cpu_ms) = measure.cpu_ms {
            let key = cpu_key(scenario, measure.label);
            let base = table
                .and_then(|table| table.get(&key))
                .and_then(Value::as_f64);
            match base {
                Some(base) if base >= CPU_GATE_MIN_MS => compare(
                    Some(base),
                    factor,
                    scenario,
                    measure.label,
                    cpu_ms,
                    "CPU 时间",
                    WallGate::Latency,
                ),
                Some(_) => eprintln!(
                    "[perf] {key} 基线低于 {CPU_GATE_MIN_MS} ms，量化误差过大，只记录不比对"
                ),
                None => {
                    eprintln!(
                        "[perf] {key} 无基线，跳过比对（用 LILITH_STRESS_PERF_UPDATE=1 生成）"
                    )
                }
            }
        }
    }
}

fn baseline_key(scenario: &str, label: &str) -> String {
    format!("{scenario}::{label}")
}

fn cpu_key(scenario: &str, label: &str) -> String {
    format!("{scenario}::{label}#cpu")
}

fn compare(
    base: Option<f64>,
    factor: f64,
    scenario: &str,
    label: &str,
    value: f64,
    unit: &str,
    gate: WallGate,
) {
    let Some(base) = base else {
        eprintln!(
            "[perf] {} 无基线，跳过比对（用 LILITH_STRESS_PERF_UPDATE=1 生成）",
            baseline_key(scenario, label)
        );
        return;
    };
    match gate {
        WallGate::Throughput => assert!(
            value >= base / factor,
            "吞吐量回归：{scenario} :: {label} 的{unit}实测 {value:.1} MiB/s，基线 {base:.1}，\
             下限 {:.1}（基线 / {factor}）。吞吐量越大越好；若确为环境差异，\
             请核对后更新基线。",
            base / factor
        ),
        // `RecordOnly` 在 `emit` 里已被拦下，走不到这里；列出它只是让新增变体时编译报错。
        WallGate::Latency | WallGate::RecordOnly => assert!(
            value <= base * factor,
            "性能回归：{scenario} :: {label} 的{unit}实测 {value:.1} ms，基线 {base:.1} ms，\
             阈值 {:.1} ms（{factor}×）。若确为环境差异，请核对后更新基线。",
            base * factor
        ),
    }
}

/// 把本次测量写进基线的对应构建档。
///
/// 只写 `profile` 指向的那张表，顶层其它键（`note` 与 `machine`）原样保留：`machine` 记录
/// 采集基线的机器型号，由维护者维护——换机器重采时手动改它，而不是让脚本每天改写。
fn update_baseline(scenario: &str, profile: &str, measurements: &[Measure]) {
    let _guard = BASELINE_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut root = read_baseline();
    let object = root
        .as_object_mut()
        .unwrap_or_else(|| panic!("{} 顶层必须是对象", baseline_path().display()));
    if !object.contains_key("note") {
        object.insert(
            "note".to_owned(),
            json!(
                "由 stress_bench 在 LILITH_STRESS_PERF_UPDATE=1 下生成；\
                 供 LILITH_STRESS_PERF_STRICT=1 比对。键 `场景::标签` 是墙钟、\
                 `场景::标签#cpu` 是子进程 CPU 时间（用户 + 内核，多线程阶段会大于墙钟）。\
                 绝对耗时依赖机器与构建档，跨机器比对时请放宽 LILITH_STRESS_PERF_FACTOR；\
                 CPU 时间只在基线 ≥ 50 ms 时参与比对（Windows 的 CPU 记账粒度 15.625 ms）。\
                 吞吐量键（MiBPerSecond）越大越好，按 `value >= base / factor` 比较；\
                 每笔耗时键（PerStrokeMs）分辨率不足，只记录不比对，严格门看整批总量。\
                 `machine` 记录采集基线的机器与日期，由维护者维护，更新基线不会改写它。"
            ),
        );
    }
    if !object.contains_key(profile) {
        object.insert(profile.to_owned(), Value::Object(Map::new()));
    }
    let table = object
        .get_mut(profile)
        .and_then(Value::as_object_mut)
        .expect("基线 profile 必须是对象");
    for measure in measurements {
        table.insert(
            baseline_key(scenario, measure.label),
            json!(round1(measure.wall_ms)),
        );
        if let Some(cpu_ms) = measure.cpu_ms {
            table.insert(cpu_key(scenario, measure.label), json!(round1(cpu_ms)));
        }
    }
    let text = serde_json::to_string_pretty(&root).expect("无法序列化性能基线");
    fs::write(baseline_path(), format!("{text}\n")).expect("无法写入性能基线");
}

fn round1(ms: f64) -> f64 {
    (ms * 10.0).round() / 10.0
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

// ---------------------------------------------------------------------------
// P1 节点链
// ---------------------------------------------------------------------------

/// P1：节点链的提交与恢复单位耗时。
#[test]
fn p1_node_chain_commit_and_restore_baseline() {
    let fixture = ArtworkFixture::new();

    // 建链：每个节点一次提交（每次一个独立进程）。CPU 记账按全部子进程累加后摊薄，
    // 这样量级是秒级，不会被 15.6 ms 的记账粒度吃掉。
    let started = Instant::now();
    let mut nodes = Vec::with_capacity(CHAIN_NODES);
    let mut build_cpu = CpuTime::default();
    for index in 0..CHAIN_NODES {
        let (id, cpu) = fixture.commit_timed(
            NODE_FILE_BYTES + index * 8,
            1_000 + index as u64,
            "chain node",
        );
        build_cpu.merge(&cpu);
        nodes.push(id);
    }
    let build = started.elapsed();
    let per_commit = Sample::from_cpu(build, build_cpu).scale(CHAIN_NODES as f64);

    // 恢复最早节点：要穿越 CHAIN_NODES-1 步反向增量才能还原。
    let oldest = fixture.workspace.out("p1-oldest.bin");
    let restore_started = Instant::now();
    let restored_oldest = fixture
        .command("restore")
        .arg("history", &nodes[0])
        .arg("output", oldest.to_string_lossy())
        .finish();
    let restore_oldest = restore_started.elapsed();
    restored_oldest.expect_ok();
    let oldest_sample = Sample::of(restore_oldest, &restored_oldest);
    let per_step = oldest_sample.scale((CHAIN_NODES - 1) as f64);

    // 顺带校验正确性：恢复产物必须与最早提交时的期望内容逐位一致。
    assert_eq!(
        fs::read(&oldest).expect("无法读取恢复输出"),
        pattern_bytes(NODE_FILE_BYTES, 1_000),
        "恢复最早节点必须得到逐位一致的原始内容"
    );

    // head 恢复：直接读 snapshot，是相反的一端。
    let head = fixture.workspace.out("p1-head.bin");
    let head_started = Instant::now();
    let restored_head = fixture
        .command("restore")
        .arg("history", nodes.last().expect("链上至少一个节点"))
        .arg("output", head.to_string_lossy())
        .finish();
    let restore_head = head_started.elapsed();
    restored_head.expect_ok();
    let head_sample = Sample::of(restore_head, &restored_head);

    assert!(
        per_commit.wall_ms < CATASTROPHIC_COMMIT_MS,
        "单次提交摊薄耗时失控：{:.1} ms",
        per_commit.wall_ms
    );
    assert!(
        per_step.wall_ms < CATASTROPHIC_CHAIN_STEP_MS,
        "恢复每步耗时失控：{:.1} ms",
        per_step.wall_ms
    );

    emit(
        "P1 node chain",
        &[
            per_commit.measure("commitMs"),
            per_step.measure("chainStepMs"),
            oldest_sample.measure("restoreOldestMs"),
            head_sample.measure("restoreHeadMs"),
        ],
        json!({
            "nodes": CHAIN_NODES,
            "fileBytes": NODE_FILE_BYTES,
            "buildMs": millis(build),
            "buildCpuMs": build_cpu.total_ms(),
        }),
    );

    fs::remove_file(&oldest).expect("无法清理恢复输出");
    fs::remove_file(&head).expect("无法清理恢复输出");
    assert!(directory_names(&fixture.workspace.out_directory()).is_empty());
}

/// 百分位（`p` 为 0–1）：样本先排序，取最接近的次序统计量。
fn percentile(samples: &mut [f64], p: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    samples.sort_by(|left, right| left.total_cmp(right));
    let index = ((samples.len() - 1) as f64 * p).round() as usize;
    samples[index]
}

/// 计时一次恢复并逐位校验产物，随后删除输出，避免重复轮次相互争抢磁盘。
#[track_caller]
fn timed_restore(
    fixture: &ArtworkFixture,
    history_id: &str,
    name: &str,
    expected: &[u8],
) -> Sample {
    let output = fixture.workspace.out(name);
    let started = Instant::now();
    let outcome = fixture
        .command("restore")
        .arg("history", history_id)
        .arg("output", output.to_string_lossy())
        .finish();
    let elapsed = started.elapsed();
    outcome.expect_ok();
    assert_eq!(
        fs::read(&output).expect("无法读取恢复输出"),
        expected,
        "段索引场景的恢复产物必须逐位一致"
    );
    fs::remove_file(&output).expect("无法清理恢复输出");
    Sample::of(elapsed, &outcome)
}

/// 多轮样本的 CPU 均值：单轮只有几十毫秒，逐轮值受 15.6 ms 记账粒度影响，取均值才有意义。
fn mean_cpu(samples: &[Sample]) -> Option<f64> {
    let values: Vec<f64> = samples.iter().filter_map(|sample| sample.cpu_ms).collect();
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

/// P1B：大文件多段链上的恢复耗时分布。
///
/// 每个提交都会写出完整 snapshot 与到父节点的反向 delta，4 MiB 随机内容在默认
/// `2/16/64 KiB` 参数下产生数十个 chunk 段。恢复最早节点要穿过整条 delta 链，
/// 每次读块都会命中段查找；这里对同一目标重复恢复并报告 p50/p95，作为「深链恢复
/// 不随 segment 数线性退化」的同机同档证据。耗时含进程启动与打开仓库的固定开销。
#[test]
fn p1b_segment_reads_baseline() {
    let fixture = ArtworkFixture::new();
    let source = fixture.workspace.work("p1b-source.bin");
    write_work_file(&source, SEGMENT_FILE_BYTES, 0xD00);
    let created = fixture
        .command("create-artwork")
        .arg("title", "Segment artwork")
        .arg("branch-title", "Main")
        .arg("source", source.to_string_lossy())
        .finish();
    created.expect_ok();
    let branch_id = created.data_str("branchId");
    assert!(!branch_id.is_empty(), "{}", created.describe());

    let build_started = Instant::now();
    let mut nodes = Vec::with_capacity(SEGMENT_NODES);
    let mut build_cpu = CpuTime::default();
    for index in 0..SEGMENT_NODES {
        // 每个版本内容全变：delta 无法靠重复块缩小，段数才是真实的。
        write_work_file(&source, SEGMENT_FILE_BYTES, 0xD00 + index as u64 + 1);
        let committed = fixture
            .command("commit")
            .arg("branch", &branch_id)
            .arg("commit-kind", "manual")
            .arg("note", &format!("segment node {index}"))
            .finish();
        committed.expect_ok();
        build_cpu.merge(&committed.cpu);
        nodes.push(committed.data_str("historyId"));
    }
    let build = build_started.elapsed();
    let oldest = nodes.first().expect("链上至少一个节点").clone();
    let head = nodes.last().expect("链上至少一个节点").clone();
    let oldest_expected = pattern_bytes(SEGMENT_FILE_BYTES, 0xD01);
    let head_expected = pattern_bytes(SEGMENT_FILE_BYTES, 0xD00 + SEGMENT_NODES as u64);

    let mut oldest_samples = Vec::with_capacity(SEGMENT_RESTORES);
    let mut head_samples = Vec::with_capacity(SEGMENT_RESTORES);
    for repeat in 0..SEGMENT_RESTORES {
        oldest_samples.push(timed_restore(
            &fixture,
            &oldest,
            &format!("p1b-oldest-{repeat}.bin"),
            &oldest_expected,
        ));
        head_samples.push(timed_restore(
            &fixture,
            &head,
            &format!("p1b-head-{repeat}.bin"),
            &head_expected,
        ));
    }

    let mut oldest_walls: Vec<f64> = oldest_samples.iter().map(|sample| sample.wall_ms).collect();
    let mut head_walls: Vec<f64> = head_samples.iter().map(|sample| sample.wall_ms).collect();
    let oldest_p50 = percentile(&mut oldest_walls, 0.50);
    let oldest_p95 = percentile(&mut oldest_walls, 0.95);
    let head_p50 = percentile(&mut head_walls, 0.50);
    let head_p95 = percentile(&mut head_walls, 0.95);
    // CPU 侧取多轮均值：单轮只有几十毫秒，逐轮值会被 15.6 ms 的记账粒度量化。
    let oldest_cpu = mean_cpu(&oldest_samples);
    let head_cpu = mean_cpu(&head_samples);
    assert!(
        oldest_p95 < CATASTROPHIC_SEGMENT_RESTORE_MS,
        "多段深链恢复耗时失控：p95={oldest_p95:.1} ms"
    );
    assert!(
        head_p95 < CATASTROPHIC_SEGMENT_RESTORE_MS,
        "多段快照恢复耗时失控：p95={head_p95:.1} ms"
    );

    let mib = SEGMENT_FILE_BYTES as f64 / (1024.0 * 1024.0);
    emit(
        "P1 segment reads",
        &[
            Sample {
                wall_ms: oldest_p50,
                cpu_ms: oldest_cpu,
            }
            .measure("restoreOldestP50Ms"),
            Sample {
                wall_ms: oldest_p95,
                cpu_ms: oldest_cpu,
            }
            .wall_measure("restoreOldestP95Ms"),
            Sample {
                wall_ms: head_p50,
                cpu_ms: head_cpu,
            }
            .measure("restoreHeadP50Ms"),
            Sample {
                wall_ms: head_p95,
                cpu_ms: head_cpu,
            }
            .wall_measure("restoreHeadP95Ms"),
            Sample::wall(mib / (oldest_p50 / 1000.0).max(1e-9))
                .wall_measure("restoreOldestMiBPerSecond")
                .throughput(),
            Sample::wall(mib / (head_p50 / 1000.0).max(1e-9))
                .wall_measure("restoreHeadMiBPerSecond")
                .throughput(),
        ],
        json!({
            "nodes": SEGMENT_NODES,
            "fileBytes": SEGMENT_FILE_BYTES,
            "restoresPerTarget": SEGMENT_RESTORES,
            "buildMs": millis(build),
            "buildCpuMs": build_cpu.total_ms(),
        }),
    );

    assert!(directory_names(&fixture.workspace.out_directory()).is_empty());
}

// ---------------------------------------------------------------------------
// P2 认证
// ---------------------------------------------------------------------------

fn authenticity_fixture_directory() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("authenticity")
}

struct AuthFixture {
    workspace: Workspace,
    repository: PathBuf,
    branch_id: String,
    certificate: PathBuf,
    private_key: PathBuf,
    artifact: PathBuf,
}

impl AuthFixture {
    /// 建立认证夹具：仓库 + 分支的 head + 工作区内的证书/私钥 + `edge` 级 JPEG 源图。
    fn new(edge: u32) -> Self {
        let workspace = Workspace::new();
        let repository = workspace.repository();
        let work = workspace.work("artwork.bin");
        write_work_file(&work, NODE_FILE_BYTES, 7);
        headless(&workspace, "init-repository").finish().expect_ok();
        let created = headless(&workspace, "create-artwork")
            .arg("title", "Artwork")
            .arg("branch-title", "Main")
            .arg("source", work.to_string_lossy())
            .finish();
        created.expect_ok();
        let branch_id = created.data_str("branchId");
        assert!(!branch_id.is_empty(), "{}", created.describe());
        // 进入发布状态要求分支已有 head。
        headless(&workspace, "commit")
            .arg("branch", &branch_id)
            .arg("note", "initial")
            .arg("commit-kind", "manual")
            .finish()
            .expect_ok();

        let certificate = workspace.work("es256-test.pub");
        let private_key = workspace.work("es256-test.priv");
        for (name, destination) in [
            ("es256-test.pub", &certificate),
            ("es256-test.priv", &private_key),
        ] {
            fs::copy(authenticity_fixture_directory().join(name), destination)
                .expect("无法复制认证凭据夹具");
        }

        // 用平滑渐变而不是噪声：编码快、体积小，解码后的像素仍足以走真实的 TrustMark 路径。
        let artifact = workspace.work("source.jpg");
        let image = image::RgbImage::from_fn(edge, edge, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, ((x ^ y) % 256) as u8])
        });
        image.save(&artifact).expect("无法写入认证源图");

        Self {
            workspace,
            repository,
            branch_id,
            certificate,
            private_key,
            artifact,
        }
    }

    fn command(&self, command: &str) -> Spawn<'_> {
        headless(&self.workspace, command).repository(&self.repository)
    }
}

/// P2：认证发布流水线（渲染 / 编码 / 签名）与回读的单位耗时。
///
/// 发布与回读都重复 [`REPEATS`] 轮取最快值：签名只有 10 ms 级，实测单轮会在 8–50 ms 之间
/// 跳（系统抖动），单轮采样会让严格门把噪声判成回归。
#[test]
fn p2_authenticity_publish_and_decode_baseline() {
    let fixture = AuthFixture::new(AUTH_EDGE);

    fixture
        .command("enter-publication")
        .arg("branch", &fixture.branch_id)
        .arg("artifact", fixture.artifact.to_string_lossy())
        .finish()
        .expect_ok();

    let mut publish_samples = Vec::with_capacity(REPEATS);
    let mut published_runs = Vec::with_capacity(REPEATS);
    for index in 0..REPEATS {
        // 每轮写不同文件，避免依赖「覆盖已存在输出」的行为。
        let target = fixture.workspace.out(&format!("published-{index}.jpg"));
        let started = Instant::now();
        let published = fixture
            .command("publish")
            .arg("branch", &fixture.branch_id)
            .arg("output", target.to_string_lossy())
            .arg("certificate", fixture.certificate.to_string_lossy())
            .arg("key", fixture.private_key.to_string_lossy())
            .arg("title", "Artwork")
            .arg("creator", "Lilith Artworks stress tests")
            .arg("rights", "Stress test")
            .arg("content", "Stress test publish")
            .arg("algorithm", "es256")
            .arg("trustmark", "true")
            .arg("regions", "0,0,1,1")
            .arg("jpeg-quality", "90")
            .finish();
        let elapsed = started.elapsed();
        published.expect_ok();
        assert!(
            published.data_u64("renderMs") > 0
                && published.data_u64("encodeMs") > 0
                && published.data_u64("signingMs") > 0,
            "发布应自报渲染 / 编码 / 签名各阶段耗时：\n{}",
            published.describe()
        );
        publish_samples.push(Sample::of(elapsed, &published));
        published_runs.push(published);
    }
    let publish_sample = Sample {
        wall_ms: publish_samples
            .iter()
            .map(|sample| sample.wall_ms)
            .fold(f64::MAX, f64::min),
        cpu_ms: mean_cpu(&publish_samples),
    };
    // 分阶段耗时由产品按单次运行自报，取各轮最小值——与墙钟取最小的口径一致。
    let stage_min = |key: &str| {
        published_runs
            .iter()
            .map(|outcome| outcome.data_u64(key) as f64)
            .fold(f64::MAX, f64::min)
    };
    let render_ms = stage_min("renderMs");
    let encode_ms = stage_min("encodeMs");
    let signing_ms = stage_min("signingMs");

    // 回读只读同一个成品，重复不影响断言，取最快值。
    let output = fixture
        .workspace
        .out(&format!("published-{}.jpg", REPEATS - 1));
    let decode_sample = repeat_min(|| {
        let decoded = fixture
            .command("decode-authenticity")
            .arg("input", output.to_string_lossy())
            .finish();
        assert_eq!(
            decoded.result["data"]["c2paPresent"].as_bool(),
            Some(true),
            "回读应能读到 C2PA 声明：\n{}",
            decoded.describe()
        );
        decoded
    });

    assert!(
        publish_sample.wall_ms < CATASTROPHIC_AUTH_PUBLISH_MS,
        "认证发布耗时失控：{:.1} ms",
        publish_sample.wall_ms
    );
    assert!(
        decode_sample.wall_ms < CATASTROPHIC_AUTH_DECODE_MS,
        "认证回读耗时失控：{:.1} ms",
        decode_sample.wall_ms
    );

    emit(
        "P2 authenticity",
        &[
            publish_sample.measure("publishMs"),
            // 渲染 / 编码 / 签名是产品自报的**分阶段**耗时，测试侧只能拿到整进程 CPU 时间，
            // 因此这三项只有墙钟。
            Sample::wall(render_ms).wall_measure("renderMs"),
            Sample::wall(encode_ms).wall_measure("encodeMs"),
            Sample::wall(signing_ms).wall_measure("signingMs"),
            decode_sample.measure("decodeMs"),
        ],
        json!({
            "edge": AUTH_EDGE,
            "publishCpuMs": published_runs
                .last()
                .map(|outcome| outcome.cpu.total_ms())
                .unwrap_or_default(),
            "publishes": REPEATS,
        }),
    );
}

// ---------------------------------------------------------------------------
// P3 画板 DDS
// ---------------------------------------------------------------------------

/// P3：画板导入（BC7 编码）与 DDS 完整性扫描的单位耗时。
#[test]
fn p3_board_dds_import_and_scrub_baseline() {
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

    // 生成确定性 PNG 源图：尺寸取 4 的倍数，BC7 编码无边界余量问题。
    let images_dir = fixture.workspace.work("board");
    fs::create_dir_all(&images_dir).expect("无法创建源图目录");
    let mut paths = Vec::with_capacity(BOARD_IMAGES);
    for index in 0..BOARD_IMAGES {
        let path = images_dir.join(format!("img_{index:04}.png"));
        let seed = index as u32;
        let image = image::RgbaImage::from_fn(BOARD_IMAGE_EDGE, BOARD_IMAGE_EDGE, |x, y| {
            let value = (x.wrapping_mul(31) ^ y.wrapping_mul(17) ^ seed.wrapping_mul(7)) as u8;
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
    let import_started = Instant::now();
    let imported = fixture
        .command("import-board-images")
        .arg("board", board_id.to_string())
        .arg("revision", &revision)
        .arg("paths", joined)
        .finish();
    let import_total = import_started.elapsed();
    imported.expect_ok();
    let import_sample = Sample::of(import_total, &imported);
    revision = imported.data_str("revision");
    assert!(!revision.is_empty(), "{}", imported.describe());
    assert_eq!(
        imported.data_u64("imageCount"),
        BOARD_IMAGES as u64,
        "导入数量与预期不符：\n{}",
        imported.describe()
    );

    let scrub_started = Instant::now();
    let scrub = fixture.command("scrub-board-dds").finish();
    let scrub_total = scrub_started.elapsed();
    scrub.expect_ok();
    let scrub_sample = Sample::of(scrub_total, &scrub);
    assert_eq!(
        scrub.data_u64("images"),
        BOARD_IMAGES as u64,
        "完整性扫描应覆盖全部导入图片：\n{}",
        scrub.describe()
    );

    let images = BOARD_IMAGES as f64;
    assert!(
        import_sample.wall_ms / images < CATASTROPHIC_DDS_PER_IMAGE_MS,
        "单张图片的 DDS 导入耗时失控：{:.1} ms",
        import_sample.wall_ms / images
    );

    emit(
        "P3 board dds",
        &[
            import_sample.scale(images).measure("importPerImageMs"),
            import_sample.measure("importTotalMs"),
            scrub_sample.measure("scrubMs"),
            scrub_sample.scale(images).measure("scrubPerImageMs"),
        ],
        json!({
            "images": BOARD_IMAGES,
            "edge": BOARD_IMAGE_EDGE,
            "importedBytes": tree_usage(&fixture.workspace).repository,
        }),
    );
}

// ---------------------------------------------------------------------------
// P4 库树与搜索
// ---------------------------------------------------------------------------

/// 重复 `REPEATS` 轮：**墙钟取最小值**（同一档位下调度抖动只会抬高耗时，最小值最稳），
/// **CPU 时间取均值**（单轮只有几十毫秒，逐轮值会被 15.6 ms 的记账粒度量化，平均后才可用）。
fn repeat_min<F: FnMut() -> Outcome>(mut run: F) -> Sample {
    let mut best = f64::MAX;
    let mut cpu_total = 0.0;
    let mut cpu_runs = 0_u32;
    for _ in 0..REPEATS {
        let started = Instant::now();
        let outcome = run();
        let wall = millis(started.elapsed());
        outcome.expect_ok();
        if outcome.cpu.measured {
            cpu_total += outcome.cpu.total_ms();
            cpu_runs += 1;
        }
        best = best.min(wall);
    }
    Sample {
        wall_ms: best,
        cpu_ms: (cpu_runs > 0).then(|| cpu_total / f64::from(cpu_runs)),
    }
}

/// P4：库树装配与两种查询的耗时。
///
/// 库形状为 [`LIBRARY_GROUPS`] 个顶层分组、每组 [`LIBRARY_PER_GROUP`] 个 Artwork；
/// 查询覆盖「命中全部」与「命中一个」两端——前者是全表匹配，后者是接近最坏情况的
/// 无关扫描，两者一起才能反映索引是否真的起作用。
#[test]
fn p4_library_tree_and_search_baseline() {
    let fixture = ArtworkFixture::new();
    // 夹具自带一个顶层 Artwork，因此节点基线要按实际增量算而不是硬编码。
    let fixture_nodes = {
        let tree = fixture.command("list-tree").finish();
        tree.expect_ok();
        tree.data_u64("nodeCount")
    };
    let source = fixture.workspace.work("library-source.bin");
    for group in 0..LIBRARY_GROUPS {
        fixture
            .command("create-group")
            .arg("title", &format!("Group {group}"))
            .finish()
            .expect_ok();
    }

    let tree = fixture.command("list-tree").finish();
    tree.expect_ok();
    let nodes = tree.result["data"]["nodes"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let group_ids = (0..LIBRARY_GROUPS)
        .map(|group| {
            let title = format!("Group {group}");
            find_tree_node(&nodes, &title)
                .and_then(|node| node["id"].as_str().map(str::to_owned))
                .unwrap_or_else(|| panic!("找不到分组 {title}"))
        })
        .collect::<Vec<_>>();

    for (group, parent) in group_ids.iter().enumerate() {
        for index in 0..LIBRARY_PER_GROUP {
            let ordinal = group * LIBRARY_PER_GROUP + index;
            write_work_file(&source, 4 * 1024, ordinal as u64 + 1);
            fixture
                .command("create-artwork")
                .arg("parent", parent)
                .arg("title", &format!("Artwork {ordinal:03}"))
                .arg("branch-title", "Main")
                .arg("source", source.to_string_lossy())
                .finish()
                .expect_ok();
        }
    }
    let nodes_expected = fixture_nodes + (LIBRARY_GROUPS * (LIBRARY_PER_GROUP + 1)) as u64;

    // 每次重复都断言结果正确：基准里混进错误结果就失去意义。
    let list_tree = repeat_min(|| {
        let outcome = fixture.command("list-tree").finish();
        assert_eq!(outcome.data_u64("nodeCount"), nodes_expected);
        outcome
    });
    let search_all = repeat_min(|| {
        let outcome = fixture.command("search").arg("query", "Artwork").finish();
        assert!(
            outcome.data_u64("count") >= (LIBRARY_GROUPS * LIBRARY_PER_GROUP) as u64,
            "命中全部的查询应返回全部作品：\n{}",
            outcome.describe()
        );
        outcome
    });
    let search_narrow = repeat_min(|| {
        let outcome = fixture
            .command("search")
            .arg("query", "Artwork 007")
            .finish();
        assert!(
            outcome.data_u64("count") >= 1,
            "只命中一个的查询应至少返回该作品：\n{}",
            outcome.describe()
        );
        outcome
    });

    assert!(
        list_tree.wall_ms < CATASTROPHIC_LIBRARY_LIST_MS,
        "整树装配耗时失控：{:.1} ms",
        list_tree.wall_ms
    );
    assert!(
        search_all.wall_ms < CATASTROPHIC_LIBRARY_SEARCH_MS,
        "全量搜索耗时失控：{:.1} ms",
        search_all.wall_ms
    );
    assert!(
        search_narrow.wall_ms < CATASTROPHIC_LIBRARY_SEARCH_MS,
        "窄搜索耗时失控：{:.1} ms",
        search_narrow.wall_ms
    );

    emit(
        "P4 library tree and search",
        &[
            list_tree.measure("listTreeMs"),
            search_all.measure("searchAllMs"),
            search_narrow.measure("searchNarrowMs"),
        ],
        json!({
            "groups": LIBRARY_GROUPS,
            "artworks": LIBRARY_GROUPS * LIBRARY_PER_GROUP,
            "nodes": nodes_expected,
            "repeatMinOf": REPEATS,
        }),
    );
}

// ---------------------------------------------------------------------------
// P5 整仓灾备
// ---------------------------------------------------------------------------

/// 在一个仓库里建出 `artworks × commits` 的版本图，返回作品的各分支与最新节点。
fn build_repository(
    fixture: &ArtworkFixture,
    artworks: usize,
    commits: usize,
    bytes: usize,
) -> Vec<String> {
    let source = fixture.workspace.work("io-source.bin");
    let mut branches = Vec::with_capacity(artworks);
    for artwork in 0..artworks {
        let seed = 0xA00 + artwork as u64;
        write_work_file(&source, bytes, seed);
        let created = fixture
            .command("create-artwork")
            .arg("title", &format!("Library {artwork:03}"))
            .arg("branch-title", "Main")
            .arg("source", source.to_string_lossy())
            .finish();
        created.expect_ok();
        let branch = created.data_str("branchId");
        assert!(!branch.is_empty(), "{}", created.describe());
        for commit in 0..commits {
            write_work_file(
                &source,
                bytes + commit * 4_096,
                0xB00 + (artwork as u64) * 16 + commit as u64,
            );
            fixture
                .command("commit")
                .arg("branch", &branch)
                .arg("commit-kind", "manual")
                .arg("note", &format!("io {commit}"))
                .finish()
                .expect_ok();
        }
        branches.push(branch);
    }
    branches
}

/// P5：整仓灾备（扫描 + 复制 + 校验 + 发布）的总耗时与吞吐。
///
/// 灾备是本项目里 I/O 最密集的操作，此前只有 D/E 组「做完了」的正确性断言，没有耗时基线；
/// 一次让复制退化成逐字节同步的实现改动不会被任何断言发现。
#[test]
fn p5_repository_backup_io_baseline() {
    let fixture = ArtworkFixture::new();
    build_repository(&fixture, IO_ARTWORKS, IO_COMMITS, IO_FILE_BYTES);
    let repository_bytes = tree_usage(&fixture.workspace).repository;
    assert!(repository_bytes > 0, "仓库不应为空");

    let destination = fixture.workspace.backup_directory();
    let started = Instant::now();
    let outcome = fixture
        .command("repository-backup")
        .arg("destination", destination.to_string_lossy())
        .finish();
    let backup_total = started.elapsed();
    outcome.expect_ok();
    let backup_sample = Sample::of(backup_total, &outcome);

    // 顺带做一次正确性对照：副本必须能独立打开并通过完整性与链路校验。
    let copy = PathBuf::from(outcome.data_str("repositoryPath"));
    assert!(copy.is_dir(), "{}", outcome.describe());
    headless(&fixture.workspace, "verify")
        .repository(&copy)
        .finish()
        .expect_ok();

    let bytes_per_second = if backup_total.as_secs_f64() > 0.0 {
        repository_bytes as f64 / backup_total.as_secs_f64()
    } else {
        0.0
    };
    assert!(
        backup_sample.wall_ms < CATASTROPHIC_BACKUP_MS,
        "整仓灾备耗时失控：{:.1} ms",
        backup_sample.wall_ms
    );

    emit(
        "P5 repository backup",
        &[backup_sample.measure("backupMs")],
        json!({
            "artworks": IO_ARTWORKS,
            "commitsPerArtwork": IO_COMMITS,
            "fileBytes": IO_FILE_BYTES,
            "repositoryBytes": repository_bytes,
            "fileCount": outcome.data_u64("fileCount"),
            "totalBytes": outcome.data_u64("totalBytes"),
            "mebibytesPerSecond": bytes_per_second / (1024.0 * 1024.0),
            // 发布前复核采用首尾各 64 KiB 的抽样模式；完整摘要仍在 manifest 里。
            "verification": "sample",
        }),
    );

    fs::remove_dir_all(&destination).expect("无法清理灾备目标");
}

// ---------------------------------------------------------------------------
// P6 清理扫描
// ---------------------------------------------------------------------------

/// P6：全库未引用文件扫描与待清理队列重放的耗时。
///
/// 两者都是「随仓库规模线性增长」的遍历：前者要枚举仓库内每个实体并比对引用表，后者要
/// 重放待清理队列。建立在一个多作品多版本的仓库上，因此遍历是真实的；两者都必须返回
/// **干净结果**（无未引用文件、队列为空），否则基准会掩盖一个真实缺陷。
#[test]
fn p6_cleanup_scan_baseline() {
    let fixture = ArtworkFixture::new();
    build_repository(&fixture, SCAN_ARTWORKS, SCAN_COMMITS, SCAN_FILE_BYTES);

    let scan = repeat_min(|| {
        let outcome = fixture.command("scan-unreferenced").finish();
        assert_eq!(
            outcome.data_u64("count"),
            0,
            "健康仓库不应有未引用文件：\n{}",
            outcome.describe()
        );
        outcome
    });
    let cleanup = repeat_min(|| {
        let outcome = fixture.command("cleanup").finish();
        assert_eq!(
            outcome.data_u64("pendingCount"),
            0,
            "待清理队列应为空：\n{}",
            outcome.describe()
        );
        outcome
    });

    assert!(
        scan.wall_ms < CATASTROPHIC_CLEANUP_SCAN_MS,
        "未引用文件扫描耗时失控：{:.1} ms",
        scan.wall_ms
    );
    assert!(
        cleanup.wall_ms < CATASTROPHIC_CLEANUP_SCAN_MS,
        "待清理队列重放耗时失控：{:.1} ms",
        cleanup.wall_ms
    );

    emit(
        "P6 cleanup scan",
        &[
            scan.measure("scanUnreferencedMs"),
            cleanup.measure("cleanupQueueMs"),
        ],
        json!({
            "artworks": SCAN_ARTWORKS,
            "commitsPerArtwork": SCAN_COMMITS,
            "repositoryBytes": tree_usage(&fixture.workspace).repository,
            "repeatMinOf": REPEATS,
        }),
    );
}

// ---------------------------------------------------------------------------
// P7 多分支调度候选
// ---------------------------------------------------------------------------

/// P7：多分支仓库上的调度候选扫描。
///
/// `scheduler-scan` 走与调度器相同的 `list_scheduled` 只读查询；这里在夹具自带的
/// 一个作品之外再造 [`SCHEDULER_ARTWORKS`] 个作品，每个作品主分支 +
/// [`SCHEDULER_FORKS_PER_ARTWORK`] 个分叉，覆盖 deadline 队列在百余条分支上的
/// 候选生成成本。
#[test]
fn p7_scheduler_candidate_scan_baseline() {
    let fixture = ArtworkFixture::new();
    // 夹具自带一条主分支。
    let mut branches = 1_u64;
    let source = fixture.workspace.work("scheduler-source.bin");
    for artwork in 0..SCHEDULER_ARTWORKS {
        write_work_file(&source, 4 * 1024, 0xC00 + artwork as u64);
        let created = fixture
            .command("create-artwork")
            .arg("title", &format!("Scheduler {artwork:03}"))
            .arg("branch-title", "Main")
            .arg("source", source.to_string_lossy())
            .finish();
        created.expect_ok();
        let artwork_id = created.data_str("artworkId");
        let main_branch = created.data_str("branchId");
        write_work_file(&source, 4 * 1024, 0xC80 + artwork as u64);
        let committed = fixture
            .command("commit")
            .arg("branch", &main_branch)
            .arg("commit-kind", "manual")
            .arg("note", "scheduler base")
            .finish();
        committed.expect_ok();
        let history_id = committed.data_str("historyId");
        branches += 1;

        for fork in 0..SCHEDULER_FORKS_PER_ARTWORK {
            let branch_source = fixture
                .workspace
                .work(&format!("scheduler-{artwork:02}-{fork:02}.bin"));
            write_work_file(
                &branch_source,
                4 * 1024,
                0xD00 + (artwork * 16 + fork) as u64,
            );
            fixture
                .command("create-branch")
                .arg("artwork", &artwork_id)
                .arg("history", &history_id)
                .arg("branch-title", format!("Fork {fork:02}"))
                .arg("source", branch_source.to_string_lossy())
                .finish()
                .expect_ok();
            branches += 1;
        }
    }

    let scan = repeat_min(|| {
        let outcome = fixture.command("scheduler-scan").finish();
        assert_eq!(
            outcome.data_u64("branchCount"),
            branches,
            "调度候选数量与预期不符：\n{}",
            outcome.describe()
        );
        outcome
    });

    assert!(
        scan.wall_ms < CATASTROPHIC_SCHEDULER_SCAN_MS,
        "多分支调度候选扫描耗时失控：{:.1} ms",
        scan.wall_ms
    );
    emit(
        "P7 scheduler scan",
        &[scan.measure("schedulerScanMs")],
        json!({
            "artworks": SCHEDULER_ARTWORKS,
            "branches": branches,
            "forksPerArtwork": SCHEDULER_FORKS_PER_ARTWORK,
            "repeatMinOf": REPEATS,
        }),
    );
}

// ---------------------------------------------------------------------------
// P8 大规模孤儿清理
// ---------------------------------------------------------------------------

/// 把文件的修改时间退回宽限期之前，让未引用扫描把新造的孤儿当作遗留文件。
fn age_file(path: &Path) {
    File::options()
        .write(true)
        .open(path)
        .expect("无法打开待老化文件")
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(3_600))
        .expect("无法设置文件修改时间");
}

/// P8：大规模孤儿的扫描与确认清理。
///
/// [`ORPHAN_FILES`] 个无记录的画板 DDS 分布在同一画板目录：先确认扫描能全部发现，
/// 再用一次 `cleanup-unreferenced` 批量入队并按批删除（`CLEANUP_BATCH_SIZE` 之内），
/// 最后复查目录干净。它覆盖 §3.E「临时引用表 + 分批幂等提交」在真实规模下的成本。
#[test]
fn p8_large_orphan_scan_and_cleanup_baseline() {
    let fixture = ArtworkFixture::new();
    let board = fixture
        .command("create-board")
        .arg("artwork", &fixture.artwork_id)
        .arg("title", "Orphan board")
        .finish();
    board.expect_ok();
    let board_id = board
        .data("boardId")
        .as_i64()
        .unwrap_or_else(|| panic!("创建画板未返回 boardId：\n{}", board.describe()));
    let board_directory = fixture
        .repository
        .join("artworks")
        .join(&fixture.artwork_id)
        .join("boards")
        .join(board_id.to_string());
    fs::create_dir_all(&board_directory).expect("无法创建画板目录");

    let mut relative_paths = Vec::with_capacity(ORPHAN_FILES);
    for index in 0..ORPHAN_FILES {
        let name = format!("{}.dds", index + 1);
        let path = board_directory.join(&name);
        fs::write(&path, vec![(index % 251) as u8; ORPHAN_FILE_BYTES]).expect("无法写入孤儿文件");
        age_file(&path);
        relative_paths.push(format!(
            "artworks/{}/boards/{board_id}/{name}",
            fixture.artwork_id
        ));
    }

    // 扫描会遍历整棵仓库树，单轮实测在 51–203 ms 之间跳（文件系统缓存与后台干扰），
    // 因此与其他场景一样取 REPEATS 轮最快值；确认清理只能做一次（第二次就没有孤儿了）。
    let scan_sample = repeat_min(|| {
        let outcome = fixture.command("scan-unreferenced").finish();
        assert_eq!(
            outcome.data_u64("count"),
            ORPHAN_FILES as u64,
            "扫描应发现全部孤儿：\n{}",
            outcome.describe()
        );
        outcome
    });

    let ids = relative_paths.join(",");
    let cleanup_started = Instant::now();
    let cleaned = fixture
        .command("cleanup-unreferenced")
        .arg("ids", &ids)
        .finish();
    let cleanup_sample = Sample::of(cleanup_started.elapsed(), &cleaned);
    cleaned.expect_ok();
    assert_eq!(
        cleaned.data_u64("cleanedCount"),
        ORPHAN_FILES as u64,
        "确认清理应删除全部孤儿：\n{}",
        cleaned.describe()
    );
    assert_eq!(cleaned.data_u64("pendingCount"), 0);
    for relative in &relative_paths {
        assert!(
            !fixture.repository.join(relative).exists(),
            "孤儿文件仍存在：{relative}"
        );
    }

    let rescan = fixture.command("scan-unreferenced").finish();
    rescan.expect_ok();
    assert_eq!(
        rescan.data_u64("count"),
        0,
        "清理后不应再有未引用文件：\n{}",
        rescan.describe()
    );

    assert!(
        scan_sample.wall_ms < CATASTROPHIC_ORPHAN_CLEANUP_MS,
        "大规模孤儿扫描耗时失控：{:.1} ms",
        scan_sample.wall_ms
    );
    assert!(
        cleanup_sample.wall_ms < CATASTROPHIC_ORPHAN_CLEANUP_MS,
        "大规模孤儿确认清理耗时失控：{:.1} ms",
        cleanup_sample.wall_ms
    );
    emit(
        "P8 orphan cleanup",
        &[
            scan_sample.measure("scanMs"),
            cleanup_sample.measure("confirmedCleanupMs"),
        ],
        json!({
            "orphans": ORPHAN_FILES,
            "orphanBytes": ORPHAN_FILES * ORPHAN_FILE_BYTES,
        }),
    );
}

// ---------------------------------------------------------------------------
// P9 笔迹读写
// ---------------------------------------------------------------------------

/// 生成 `count` 条笔迹的批量清单：每条 [`STROKE_POINTS`] 个点，走正弦路径 + 起伏压感，
/// 几何长度一致，因此「每笔耗时」可以直接横向比较。
fn stroke_spec(count: usize, points: usize) -> Vec<u8> {
    // 与 `pin_board::stroke` 同一份契约：version 1、tool 0、带压感标志。
    let mut geometry = Vec::with_capacity(8 + points * 5);
    geometry.extend_from_slice(&[1, 0, 0b0000_0100, 0]);
    geometry.extend_from_slice(&(points as u32).to_le_bytes());
    for index in 0..points {
        let t = index as f64 / (points.max(2) - 1) as f64;
        let x = ((t * 400.0).sin().abs() * 0xffff as f64) as u16;
        let y = (t * 0xffff as f64) as u16;
        let pressure = ((t * std::f64::consts::PI * 3.0).sin().abs() * 255.0) as u8;
        geometry.extend_from_slice(&x.to_le_bytes());
        geometry.extend_from_slice(&y.to_le_bytes());
        geometry.push(pressure);
    }

    let mut spec = (count as u32).to_le_bytes().to_vec();
    for index in 0..count {
        spec.push(0); // tool = pen
        for value in [
            6.0_f32,             // strokeWidth
            400.0_f32,           // width
            200.0_f32,           // height
            index as f32 * 10.0, // centerX
            0.0_f32,             // centerY
        ] {
            spec.extend_from_slice(&value.to_le_bytes());
        }
        spec.extend_from_slice(&(geometry.len() as u32).to_le_bytes());
        spec.extend_from_slice(&geometry);
    }
    spec
}

/// P9：500 / 2000 / 5000 笔的批量写入（一次事务）与整板载荷读取。
///
/// 每档单独一块画板：`read-board-strokes` 返回整块画板的笔迹，混在一起就读不出单档的
/// 数字。写入走 `create-board-strokes`（与 GUI 的单笔创建共用同一份校验与落库逻辑，
/// 只是不逐笔重读视图），读取走 `read-board-strokes`（逐条校验几何后打包成二进制）。
///
/// 计划里的指标是 5000 笔读取 < 200 ms、写入 < 500 ms；本组只拦数量级失控，指标本身
/// 需要在 release 档上人工对照（数值进 `tests/perf-baseline.json` 的 `release` 档）。
#[test]
fn p9_stroke_write_and_read_baseline() {
    let fixture = ArtworkFixture::new();
    let mut measurements = Vec::new();
    let mut extra = Map::new();

    for strokes in STROKE_TIERS {
        let board = fixture
            .command("create-board")
            .arg("artwork", &fixture.artwork_id)
            .arg("title", format!("Strokes {strokes}"))
            .finish();
        board.expect_ok();
        let board_id = board
            .data("boardId")
            .as_i64()
            .unwrap_or_else(|| panic!("创建画板未返回 boardId：\n{}", board.describe()));
        let revision = board.data_str("revision");
        assert!(!revision.is_empty(), "{}", board.describe());

        let spec_path = fixture.workspace.work(&format!("strokes-{strokes}.bin"));
        fs::write(&spec_path, stroke_spec(strokes, STROKE_POINTS)).expect("无法写入笔迹清单");

        let write_started = Instant::now();
        let created = fixture
            .command("create-board-strokes")
            .arg("board", board_id.to_string())
            .arg("revision", &revision)
            .arg("spec", spec_path.to_string_lossy())
            .finish();
        let write_total = write_started.elapsed();
        created.expect_ok();
        let write_sample = Sample::of(write_total, &created);
        assert_eq!(
            created.data_u64("count"),
            strokes as u64,
            "{}",
            created.describe()
        );

        let read_started = Instant::now();
        let read = fixture
            .command("read-board-strokes")
            .arg("board", board_id.to_string())
            .finish();
        let read_total = read_started.elapsed();
        read.expect_ok();
        let read_sample = Sample::of(read_total, &read);
        assert_eq!(
            read.data_u64("count"),
            strokes as u64,
            "{}",
            read.describe()
        );
        assert!(
            read.data_u64("bytes") as usize > strokes * STROKE_POINTS * 5,
            "载荷应包含每笔的几何：\n{}",
            read.describe()
        );

        assert!(
            write_sample.wall_ms / (strokes as f64) < CATASTROPHIC_STROKE_WRITE_PER_STROKE_MS,
            "{strokes} 笔的写入耗时失控：{:.1} ms（每笔 {:.2} ms）",
            write_sample.wall_ms,
            write_sample.wall_ms / (strokes as f64)
        );
        assert!(
            read_sample.wall_ms < CATASTROPHIC_STROKE_READ_MS,
            "{strokes} 笔的读取耗时失控：{:.1} ms",
            read_sample.wall_ms
        );

        // 标签必须是编译期常量（基线键按标签记），因此按档位取固定名字。
        let (write_total_label, write_per_label, read_label) = match strokes {
            500 => ("writeTotalMs500", "writePerStrokeMs500", "readTotalMs500"),
            2_000 => (
                "writeTotalMs2000",
                "writePerStrokeMs2000",
                "readTotalMs2000",
            ),
            _ => (
                "writeTotalMs5000",
                "writePerStrokeMs5000",
                "readTotalMs5000",
            ),
        };
        measurements.push(write_sample.measure(write_total_label));
        // 每笔耗时分辨率不足（0.045 ms 取整为 0），只记录；严格门看上面的整批总量断言。
        measurements.push(
            write_sample
                .scale(strokes as f64)
                .measure(write_per_label)
                .record_only(),
        );
        measurements.push(read_sample.measure(read_label));
        extra.insert(
            format!("payloadBytes{strokes}"),
            json!(read.data_u64("bytes")),
        );
    }
    extra.insert("pointsPerStroke".into(), json!(STROKE_POINTS));

    emit("P9 stroke io", &measurements, Value::Object(extra));
}
