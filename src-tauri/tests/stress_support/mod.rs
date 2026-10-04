//! 压力测试共享助手：工作区、进程编排、阶段协议与磁盘事实断言。
//!
//! 纪律（见 `docs/guides/validation.md`）：测试只通过命令行参数、stdin 与进程退出
//! 干预被测进程，**不读取其内部状态**。全部断言基于「子命令返回的 JSON」与
//! 「磁盘事实」。
//!
//! 覆盖批次 1 需要的部分；批次 2–5 会在此基础上扩展（强杀、稀疏大文件、磁盘账本、
//! 并行调度），因此这里允许部分助手暂时未被使用。
#![allow(dead_code)]

use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex, OnceLock,
    },
    time::{Duration, Instant},
};

use serde_json::{json, Value};

/// 本次 `cargo test` 构建出的 debug 二进制。`CARGO_BIN_EXE_*` 只在集成测试与
/// benchmark 中注入——测试因此天然落在 crate 之外，也天然是「真实的接口调用者」。
pub const DEFAULT_HEADLESS_BIN: &str = env!("CARGO_BIN_EXE_lilith-artworks");

const POLL_INTERVAL: Duration = Duration::from_millis(5);
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(180);
const STDERR_LIMIT: usize = 4000;

static SPAWN_COUNTER: AtomicUsize = AtomicUsize::new(0);
static PROBE: OnceLock<()> = OnceLock::new();
/// 同一个测试进程内的多个场景会并行追加同一份报告；不加锁时两行会交错成
/// 无法解析的碎片，因此这里把「打开 + 写入一行」整体串行化。
static REPORT_LOCK: Mutex<()> = Mutex::new(());

/// 被测可执行文件。默认是本次构建的 debug 二进制；把 `LILITH_STRESS_BIN` 指向
/// 维护者自己构建的 release 二进制即可让整套断言跑在**发布档**上：
///
/// ```text
/// cargo build --release --features headless
/// LILITH_STRESS_BIN=target/release/lilith-artworks.exe \
///   cargo test --features headless --test stress_cancel
/// ```
///
/// 覆盖路径会先做一次探针：`--headless help` 必须正常返回。指向未启用 `headless`
/// feature 的构建时 `--headless` 会被忽略并启动 GUI，探针会终止它并报错。
/// 需要真实数值的只有「峰值内存 / 存储放大 / 耗时」类场景（C、D、G 组）；
/// 取消、崩溃与一致性断言与构建档无关。
pub fn headless_bin() -> PathBuf {
    let Ok(overridden) = std::env::var("LILITH_STRESS_BIN") else {
        return PathBuf::from(DEFAULT_HEADLESS_BIN);
    };
    let path = PathBuf::from(overridden);
    assert!(
        path.is_file(),
        "LILITH_STRESS_BIN 指向的文件不存在：{}",
        path.display()
    );
    PROBE.get_or_init(|| probe_headless(&path));
    path
}

fn probe_headless(path: &Path) {
    let mut child = Command::new(path)
        .arg("--headless")
        .arg("help")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("无法启动 LILITH_STRESS_BIN");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                assert!(
                    status.success(),
                    "LILITH_STRESS_BIN 的 `--headless help` 未正常返回：{status}"
                );
                return;
            }
            Ok(None) => {}
            Err(error) => panic!("探针等待失败：{error}"),
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "LILITH_STRESS_BIN 没有启用 headless feature（`--headless` 被忽略，已终止）。\
                 请用 `cargo build --release --features headless` 构建。"
            );
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// 单一临时工作区根：仓库、工作文件、恢复输出、灾备目标与临时台账都在它之内。
pub struct Workspace {
    _directory: tempfile::TempDir,
    root: PathBuf,
}

impl Workspace {
    pub fn new() -> Self {
        let directory = tempfile::tempdir().expect("无法创建临时目录");
        let root = directory.path().join("stress-workspace");
        for name in ["work", "out", "backup", "scratch"] {
            fs::create_dir_all(root.join(name)).expect("无法创建工作区子目录");
        }
        Self {
            _directory: directory,
            root,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn repository(&self) -> PathBuf {
        self.root.join("repository")
    }

    pub fn parallel_repository(&self, slot: usize) -> PathBuf {
        self.root.join(format!("repository-p{slot}"))
    }

    pub fn work(&self, name: &str) -> PathBuf {
        self.root.join("work").join(name)
    }

    pub fn out(&self, name: &str) -> PathBuf {
        self.root.join("out").join(name)
    }

    pub fn out_directory(&self) -> PathBuf {
        self.root.join("out")
    }

    pub fn backup_directory(&self) -> PathBuf {
        self.root.join("backup")
    }

    fn scratch_directory(&self) -> PathBuf {
        self.root.join("scratch")
    }
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new()
    }
}

/// 无头子命令的构造器。所有命令都会带上 `--workspace`、`--marker` 与 `--result`。
pub struct Spawn<'a> {
    workspace: &'a Workspace,
    command: String,
    args: Vec<String>,
    gate: bool,
    timeout: Duration,
}

/// 启动一个无头子命令（`--headless <command> [options]`）。
pub fn headless<'a>(workspace: &'a Workspace, command: &str) -> Spawn<'a> {
    Spawn {
        workspace,
        command: command.to_owned(),
        args: Vec::new(),
        gate: false,
        timeout: DEFAULT_TIMEOUT,
    }
}

impl<'a> Spawn<'a> {
    pub fn arg(mut self, key: &str, value: impl AsRef<str>) -> Self {
        self.args.push(format!("--{key}"));
        self.args.push(value.as_ref().to_owned());
        self
    }

    pub fn repository(self, path: &Path) -> Self {
        self.arg("repository", path.to_string_lossy())
    }

    /// 打开逐检查点闸门：每个取消检查点阻塞等待本进程的一行判定。
    pub fn gate(mut self) -> Self {
        self.gate = true;
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn start(self) -> Running {
        let id = SPAWN_COUNTER.fetch_add(1, Ordering::SeqCst);
        let scratch = self.workspace.scratch_directory();
        let marker = scratch.join(format!("{}-{id}.marker", self.command));
        let result = scratch.join(format!("{}-{id}.result.json", self.command));
        let stderr_path = scratch.join(format!("{}-{id}.stderr", self.command));
        let _ = fs::remove_file(&marker);
        let stderr = File::create(&stderr_path).expect("无法创建 stderr 文件");

        let mut command = Command::new(headless_bin());
        command
            .arg("--headless")
            .arg(&self.command)
            .arg("--workspace")
            .arg(self.workspace.root())
            .arg("--marker")
            .arg(&marker)
            .arg("--result")
            .arg(&result);
        if self.gate {
            command.arg("--cancel-on-stdin");
        }
        command
            .args(&self.args)
            .stdin(if self.gate {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr));
        let mut child = command.spawn().expect("无法启动无头进程");
        let stdin = child.stdin.take();
        Running {
            child,
            stdin,
            marker,
            result,
            stderr_path,
            gate: self.gate,
            deadline: Instant::now() + self.timeout,
            consumed: 0,
            checkpoints: Vec::new(),
            cancelled: false,
        }
    }

    /// 启动并驱动到进程结束。`decide(index, stage)` 返回 true 表示在该检查点取消。
    pub fn drive<F: FnMut(usize, &str) -> bool>(self, decide: F) -> Outcome {
        self.start().drive(decide)
    }

    /// 不干预地跑到结束。
    pub fn finish(self) -> Outcome {
        self.drive(|_, _| false)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    pub index: usize,
    pub stage: String,
}

pub struct Running {
    child: Child,
    stdin: Option<ChildStdin>,
    marker: PathBuf,
    result: PathBuf,
    stderr_path: PathBuf,
    gate: bool,
    deadline: Instant,
    consumed: usize,
    checkpoints: Vec<Checkpoint>,
    cancelled: bool,
}

impl Running {
    pub fn drive<F: FnMut(usize, &str) -> bool>(mut self, mut decide: F) -> Outcome {
        let started = Instant::now();
        let mut timed_out = false;
        loop {
            for index in self.consume_new_checkpoints() {
                let stage = self.checkpoints[index].stage.clone();
                let checkpoint = self.checkpoints[index].index;
                let cancel = decide(checkpoint, &stage);
                if cancel {
                    self.cancelled = true;
                }
                if self.gate {
                    self.answer(cancel);
                }
            }
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {}
                Err(error) => panic!("等待无头进程失败：{error}"),
            }
            if Instant::now() >= self.deadline {
                timed_out = true;
                let _ = self.child.kill();
                let _ = self.child.wait();
                break;
            }
            std::thread::sleep(POLL_INTERVAL);
        }
        // 进程已结束：只收集最后几行，不再作答。
        let _ = self.consume_new_checkpoints();
        let status = match self.child.try_wait() {
            Ok(Some(status)) => status,
            _ => self.child.wait().expect("无法回收无头进程"),
        };
        let result = fs::read(&self.result)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .unwrap_or(Value::Null);
        let stderr = fs::read_to_string(&self.stderr_path).unwrap_or_default();
        Outcome {
            exit_code: status.code().unwrap_or(-1),
            timed_out,
            cancelled: self.cancelled,
            result,
            checkpoints: self.checkpoints,
            elapsed: started.elapsed(),
            stderr: truncate(stderr, STDERR_LIMIT),
        }
    }

    /// 返回新出现的检查点在 `self.checkpoints` 中的下标。
    fn consume_new_checkpoints(&mut self) -> Vec<usize> {
        let Ok(content) = fs::read_to_string(&self.marker) else {
            return Vec::new();
        };
        let complete = match content.rfind('\n') {
            Some(position) => &content[..=position],
            None => return Vec::new(),
        };
        let lines = complete.lines().collect::<Vec<_>>();
        if lines.len() <= self.consumed {
            return Vec::new();
        }
        let mut added = Vec::new();
        for line in &lines[self.consumed..] {
            if let Some(checkpoint) = parse_checkpoint(line) {
                self.checkpoints.push(checkpoint);
                added.push(self.checkpoints.len() - 1);
            }
        }
        self.consumed = lines.len();
        added
    }

    fn answer(&mut self, cancel: bool) {
        let Some(stdin) = self.stdin.as_mut() else {
            return;
        };
        let line: &[u8] = if cancel { b"cancel\n" } else { b"continue\n" };
        // 进程可能已经在两次写入之间退出，写失败无需处理。
        let _ = stdin.write_all(line);
        let _ = stdin.flush();
    }
}

fn parse_checkpoint(line: &str) -> Option<Checkpoint> {
    let rest = line.strip_prefix("checkpoint index=")?;
    let (index_text, stage) = match rest.find(" stage=") {
        Some(position) => (
            &rest[..position],
            rest[position + " stage=".len()..].to_owned(),
        ),
        None => (rest, String::new()),
    };
    Some(Checkpoint {
        index: index_text.trim().parse().ok()?,
        stage,
    })
}

fn truncate(mut value: String, limit: usize) -> String {
    if value.len() > limit {
        value.truncate(limit);
        value.push_str("...(truncated)");
    }
    value
}

pub struct Outcome {
    pub exit_code: i32,
    pub timed_out: bool,
    pub cancelled: bool,
    pub result: Value,
    pub checkpoints: Vec<Checkpoint>,
    pub elapsed: Duration,
    pub stderr: String,
}

impl Outcome {
    pub fn ok(&self) -> bool {
        self.result["ok"].as_bool().unwrap_or(false)
    }

    pub fn cancelled(&self) -> bool {
        self.result["cancelled"].as_bool().unwrap_or(false)
    }

    pub fn error(&self) -> String {
        self.result["error"].as_str().unwrap_or_default().to_owned()
    }

    pub fn data(&self, key: &str) -> Value {
        self.result["data"].get(key).cloned().unwrap_or(Value::Null)
    }

    pub fn data_str(&self, key: &str) -> String {
        self.data(key).as_str().unwrap_or_default().to_owned()
    }

    pub fn data_u64(&self, key: &str) -> u64 {
        self.data(key).as_u64().unwrap_or(0)
    }

    pub fn checkpoint_stages(&self) -> Vec<(usize, &str)> {
        self.checkpoints
            .iter()
            .map(|checkpoint| (checkpoint.index, checkpoint.stage.as_str()))
            .collect()
    }

    pub fn describe(&self) -> String {
        format!(
            "exit_code={} timed_out={} cancelled={} elapsed_ms={}\n\
             checkpoints={:?}\nresult={}\nstderr:\n{}",
            self.exit_code,
            self.timed_out,
            self.cancelled,
            self.elapsed.as_millis(),
            self.checkpoint_stages(),
            serde_json::to_string(&self.result).unwrap_or_default(),
            self.stderr
        )
    }

    #[track_caller]
    pub fn expect_ok(&self) -> &Self {
        assert!(
            !self.timed_out && self.ok(),
            "期望成功但未成功：\n{}",
            self.describe()
        );
        self
    }

    #[track_caller]
    pub fn expect_cancelled(&self) -> &Self {
        assert!(
            !self.timed_out,
            "等待取消结果时超时（闸门可能没有被驱动）：\n{}",
            self.describe()
        );
        assert!(
            self.cancelled() && !self.ok() && self.exit_code == 2,
            "期望取消结果：\n{}",
            self.describe()
        );
        self
    }
}

/// 取消策略：按检查点序号，或按首次出现的阶段名决定在哪一个检查点取消。
///
/// 与逐检查点闸门配合使用时完全确定：进程在每个检查点阻塞等待判定，测试因此不必
/// 与进程的速度赛跑。
#[derive(Debug, Clone)]
pub struct Policy {
    index: Option<usize>,
    stage: Option<&'static str>,
    fired: std::cell::Cell<bool>,
}

impl Policy {
    /// 永不取消（用于正向对照）。
    pub fn never() -> Self {
        Self {
            index: None,
            stage: None,
            fired: std::cell::Cell::new(false),
        }
    }

    /// 在第 `index` 个检查点取消（1 起）。
    pub fn at_index(index: usize) -> Self {
        Self {
            index: Some(index),
            stage: None,
            fired: std::cell::Cell::new(false),
        }
    }

    /// 在第一个阶段名包含 `needle` 的检查点取消。
    pub fn at_stage(needle: &'static str) -> Self {
        Self {
            index: None,
            stage: Some(needle),
            fired: std::cell::Cell::new(false),
        }
    }

    pub fn decide(&self, index: usize, stage: &str) -> bool {
        if self.fired.get() {
            return false;
        }
        let hit = self.index.is_some_and(|value| value == index)
            || self.stage.is_some_and(|needle| stage.contains(needle));
        if hit {
            self.fired.set(true);
        }
        hit
    }
}

// ---------------------------------------------------------------------------
// 工作文件与磁盘事实
// ---------------------------------------------------------------------------

/// 确定性伪随机内容：同一 (len, seed) 永远得到同一份字节，因此摘要可复算。
pub fn pattern_bytes(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(0x1234_5678_9ABC_DEF0);
    let mut bytes = Vec::with_capacity(len + 8);
    while bytes.len() < len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        bytes.extend_from_slice(&state.to_le_bytes());
    }
    bytes.truncate(len);
    bytes
}

pub fn write_work_file(path: &Path, len: usize, seed: u64) {
    fs::write(path, pattern_bytes(len, seed)).expect("无法写入工作文件");
}

/// 仓库内受历史管理的实体文件。路径为相对仓库根的 `/` 分隔形式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageState {
    pub snapshots: Vec<String>,
    pub deltas: Vec<String>,
    pub temporaries: Vec<String>,
}

impl StorageState {
    pub fn entities(&self) -> usize {
        self.snapshots.len() + self.deltas.len()
    }
}

pub fn storage_state(repository: &Path) -> StorageState {
    let mut files = Vec::new();
    collect_files(repository, repository, &mut files);
    let mut snapshots = Vec::new();
    let mut deltas = Vec::new();
    let mut temporaries = Vec::new();
    for path in files {
        if path.contains("/temp/") {
            temporaries.push(path);
        } else if path.contains("/snapshots/") {
            snapshots.push(path);
        } else if path.contains("/deltas/") {
            deltas.push(path);
        }
    }
    snapshots.sort();
    deltas.sort();
    temporaries.sort();
    StorageState {
        snapshots,
        deltas,
        temporaries,
    }
}

fn collect_files(root: &Path, directory: &Path, files: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, files);
        } else if path.is_file() {
            if let Ok(relative) = path.strip_prefix(root) {
                files.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
}

/// 断言仓库内没有遗留的临时文件，并返回实体文件清单。
#[track_caller]
pub fn assert_no_stray(repository: &Path) -> StorageState {
    let state = storage_state(repository);
    assert!(
        state.temporaries.is_empty(),
        "仓库内残留临时文件：{:?}",
        state.temporaries
    );
    state
}

/// 重开仓库（完整性与语义校验）并全库校验历史链，返回历史节点数。
///
/// 这是「重开可用」而不是「重开不报错」的断言：`verify` 覆盖迁移、`integrity_check`、
/// 外键与 UUID/路径/SHA-256 全表语义校验，`scrub` 逐块复算摘要链。
#[track_caller]
pub fn assert_healthy(workspace: &Workspace, repository: &Path) -> u64 {
    let verify = headless(workspace, "verify")
        .repository(repository)
        .finish();
    verify.expect_ok();
    let scrub = headless(workspace, "scrub").repository(repository).finish();
    scrub.expect_ok();
    scrub.data_u64("historyNodes")
}

/// 断言待清理队列为空（没有孤儿文件需要回收）。
#[track_caller]
pub fn assert_cleanup_empty(workspace: &Workspace, repository: &Path) {
    let outcome = headless(workspace, "cleanup")
        .repository(repository)
        .finish();
    outcome.expect_ok();
    assert_eq!(
        outcome.data_u64("pendingCount"),
        0,
        "待清理队列非空：\n{}",
        outcome.describe()
    );
}

// ---------------------------------------------------------------------------
// 报告
// ---------------------------------------------------------------------------

/// 追加一行 JSON Lines 到 `target/stress-report.jsonl`，字段可直接摘录进交接文档。
///
/// `target/` 不进版本控制，因此能长期引用的证据是摘录进
/// `docs/planning/current-handoff.md` 的汇总。
pub fn record(scenario: &str, tier: &str, outcome: &Outcome, extra: Value) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("stress-report.jsonl");
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let peak = outcome.result["peakWorkingSetBytes"].as_u64();
    let line = json!({
        "scenario": scenario,
        "tier": tier,
        "group": "A",
        "exitCode": outcome.exit_code,
        "cancelled": outcome.cancelled(),
        "ok": outcome.ok(),
        "timedOut": outcome.timed_out,
        "elapsedMs": outcome.elapsed.as_millis() as u64,
        "peakWorkingSetBytes": peak,
        "checkpointCount": outcome.checkpoints.len(),
        "error": if outcome.ok() { Value::Null } else { Value::String(outcome.error()) },
        "detail": extra,
    });
    let lock = REPORT_LOCK.lock();
    let opened = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| writeln!(file, "{line}"));
    drop(lock);
    if let Err(error) = opened {
        eprintln!("无法写入压力测试报告：{error}");
    }
}
