//! 压力测试共享助手：工作区、进程编排、阶段协议与磁盘事实断言。
//!
//! 纪律（见 `docs/guides/validation.md`）：测试只通过命令行参数、stdin 与进程退出
//! 干预被测进程，**不读取其内部状态**。全部断言基于「子命令返回的 JSON」与
//! 「磁盘事实」。
//!
//! 覆盖批次 1–3 需要的部分；批次 4–5 会在此基础上扩展（规模场景、认证大图），
//! 因此这里允许部分助手暂时未被使用。
#![allow(dead_code)]

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Condvar, Mutex, OnceLock,
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
///
/// 工作区建在项目 `target/stress-workspaces/` 之下，而**不是** `%TEMP%`：`%TEMP%` 与
/// 系统和其他进程共用，可能被系统清理或配额策略回收——并发运行时实测出现过「工作区子目录
/// 在场景中途消失、`restore` 报『恢复输出目录不存在』」的偶发失败。`target/` 随项目、
/// 已被 gitignore、与项目同盘，且不参与系统临时清理。仍然用 `TempDir` 持有，保证正常结束
/// （含断言失败展开）时自动回收。
///
/// **必须由栈上的变量持有**：Rust 不为 `static` 运行析构函数，把 `TempDir` 放进
/// `static`/`OnceLock` 会永不释放（大文件档会在磁盘上留下数 GB 残留）。
pub struct Workspace {
    _directory: tempfile::TempDir,
    root: PathBuf,
}

impl Workspace {
    pub fn new() -> Self {
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("stress-workspaces");
        fs::create_dir_all(&base).expect("无法创建压力测试工作区根");
        let directory = tempfile::Builder::new()
            .prefix(".stress-")
            .tempdir_in(&base)
            .expect("无法创建临时目录");
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

    pub fn work_directory(&self) -> PathBuf {
        self.root.join("work")
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

    pub fn scratch_directory(&self) -> PathBuf {
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
            .arg(&result)
            // 让子进程的工作目录落在工作区内，而不是继承 `cargo test` 的 CWD：
            // 画板创建在领域层按相对路径补建 `artworks/<id>/boards/<id>` 目录，
            // 若继承 CWD 会在 `src-tauri/` 下留下空目录。所有路径参数都是绝对路径，
            // 因此固定 CWD 不改变任何命令的解析结果，只让副作用留在可回收的工作区内。
            .current_dir(self.workspace.scratch_directory());
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

    /// 逐检查点闸门驱动，在目标检查点出现时 `Child::kill()` 强杀进程。
    ///
    /// 与取消不同，强杀不要求进程有机会收尾：`at(index, stage)` 返回 true 的检查点
    /// **不**作答，直接杀进程；其余检查点一律放行（写 `continue`）。
    ///
    /// 必须带闸门（`Spawn::gate`）：闸门让进程在目标检查点**阻塞**，因此强杀落在
    /// 确定的代码位置，不受进程速度影响（非阻塞的「轮询 marker 后 kill」会与进程
    /// 赛跑，毫秒级窗口下抖动）。`kill` 在 Windows 上等价 `TerminateProcess`，不运行
    /// 任何清理——这正是「进程被杀」的真实模拟。
    pub fn kill_at<F: FnMut(usize, &str) -> bool>(mut self, mut at: F) -> CrashOutcome {
        assert!(
            self.gate,
            "强杀注入必须使用逐检查点闸门（Spawn::gate），否则无法确定命中窗口"
        );
        let started = Instant::now();
        let mut target = None;
        let mut timed_out = false;
        let exit_code;
        'outer: loop {
            for index in self.consume_new_checkpoints() {
                let checkpoint = self.checkpoints[index].clone();
                if at(checkpoint.index, &checkpoint.stage) {
                    target = Some(checkpoint);
                    let _ = self.child.kill();
                    exit_code = self.child.wait().ok().and_then(|status| status.code());
                    break 'outer;
                }
                self.answer(false);
            }
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    exit_code = status.code();
                    break;
                }
                Ok(None) => {}
                Err(error) => panic!("等待无头进程失败：{error}"),
            }
            if Instant::now() >= self.deadline {
                timed_out = true;
                let _ = self.child.kill();
                exit_code = self.child.wait().ok().and_then(|status| status.code());
                break;
            }
            std::thread::sleep(POLL_INTERVAL);
        }
        // 进程已死亡：只收集最后几行，不再作答。
        let _ = self.consume_new_checkpoints();
        let stderr = truncate(
            fs::read_to_string(&self.stderr_path).unwrap_or_default(),
            STDERR_LIMIT,
        );
        CrashOutcome {
            killed: target.is_some(),
            target,
            timed_out,
            exit_code,
            checkpoints: self.checkpoints,
            elapsed: started.elapsed(),
            stderr,
        }
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

/// 强杀注入的结果。与 `Outcome` 不同：进程没有机会写 `--result`，因此断言全部基于
/// 「进程在目标检查点被强杀」这一事实，以及随后由**新的**无头进程执行的磁盘与重开断言。
pub struct CrashOutcome {
    /// 强杀发生时所在的检查点；进程在到达目标前自行退出时为 `None`。
    pub target: Option<Checkpoint>,
    pub killed: bool,
    pub timed_out: bool,
    pub exit_code: Option<i32>,
    pub checkpoints: Vec<Checkpoint>,
    pub elapsed: Duration,
    pub stderr: String,
}

impl CrashOutcome {
    pub fn target_stage(&self) -> &str {
        self.target
            .as_ref()
            .map(|value| value.stage.as_str())
            .unwrap_or("")
    }

    pub fn checkpoint_stages(&self) -> Vec<(usize, &str)> {
        self.checkpoints
            .iter()
            .map(|checkpoint| (checkpoint.index, checkpoint.stage.as_str()))
            .collect()
    }

    pub fn describe(&self) -> String {
        format!(
            "killed={} timed_out={} exit_code={:?} target={:?} elapsed_ms={}\n\
             checkpoints={:?}\nstderr:\n{}",
            self.killed,
            self.timed_out,
            self.exit_code,
            self.target,
            self.elapsed.as_millis(),
            self.checkpoint_stages(),
            self.stderr
        )
    }

    #[track_caller]
    pub fn expect_killed(&self) -> &Self {
        assert!(
            !self.timed_out,
            "强杀等待超时（闸门可能没有被驱动）：\n{}",
            self.describe()
        );
        assert!(
            self.killed,
            "进程在到达目标检查点前已自行退出：\n{}",
            self.describe()
        );
        self
    }

    /// 断言强杀恰好落在第 `index` 个检查点。
    #[track_caller]
    pub fn expect_killed_at(&self, index: usize) -> &Self {
        self.expect_killed();
        let target = self
            .target
            .as_ref()
            .expect("killed 为真时必然记录了目标检查点");
        assert_eq!(
            target.index,
            index,
            "强杀检查点与预期不符：\n{}",
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

// ---------------------------------------------------------------------------
// 稀疏工作文件（批次 3：把大文件的磁盘成本压到接近 0）
// ---------------------------------------------------------------------------

/// 单个有界区域一次写入的字节数上限。区域数量和总量都有界，因此生成器只按区域
/// 分配内存，不随逻辑大小增长。
const TARGET_REGION_BYTES: u64 = 256 * 1024;
const MAX_REGIONS: usize = 8;

/// 一次有界写入：从 `offset` 起写入 `length` 字节确定性伪随机内容。
///
/// 只保存描述，不保存内容——内容由 `pattern_bytes(length, seed)` 复算，因此一个
/// 场景的全部写入计划在内存里只有几十个整型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionWrite {
    pub offset: u64,
    pub length: u64,
    pub seed: u64,
}

impl RegionWrite {
    pub fn end(&self) -> u64 {
        self.offset + self.length
    }
}

/// 生成一批确定性、摊开且大多互不重叠的有界区域，总字节数不超过 `total_bytes`。
///
/// **区域布局只由 `layout_seed` 决定，内容只由 `content_seed` 决定**，两者分开是必要的：
/// 小改动场景每轮换布局（模拟散布在文件各处的编辑），大改动场景固定布局、只换内容
/// （模拟反复编辑同一批区域）——后者才会产生**不可压缩**的大 delta，因为父版本的旧内容
/// 同样是真实随机数据，而不是能被 zstd 抹平的全零区域。
///
/// 区域数量只由总量决定（不依赖种子），因此同一 `total_bytes` 在不同种子下得到同样多的
/// 区域，调用方可以据此推算「一次提交改动了几个区域」。
pub fn region_writes(
    logical_size: u64,
    total_bytes: u64,
    layout_seed: u64,
    content_seed: u64,
) -> Vec<RegionWrite> {
    let mut regions = Vec::new();
    if logical_size == 0 || total_bytes == 0 {
        return regions;
    }
    let total = total_bytes.min(logical_size);
    let count = usize::try_from(total / TARGET_REGION_BYTES)
        .unwrap_or(usize::MAX)
        .clamp(1, MAX_REGIONS);
    let mut state = layout_seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut remaining = total;
    for index in 0..count {
        let share = remaining / u64::try_from(count - index).unwrap_or(1);
        let length = if index + 1 == count { remaining } else { share };
        if length == 0 {
            break;
        }
        // 第 i 个区域落在第 i 个等分窗口内，避免多个区域挤在同一处。
        let window = logical_size / u64::try_from(count).unwrap_or(1);
        let window_start = window.saturating_mul(u64::try_from(index).unwrap_or(0));
        let slack = if index + 1 == count {
            logical_size.saturating_sub(window_start + length)
        } else {
            window.saturating_sub(length)
        };
        let jitter = if slack == 0 {
            0
        } else {
            next_u64(&mut state) % (slack + 1)
        };
        let offset = window_start + jitter;
        regions.push(RegionWrite {
            offset,
            length,
            seed: content_seed
                .wrapping_add(u64::try_from(index).unwrap_or(0))
                .wrapping_mul(0xD1B5_4A32_D192_ED03),
        });
        remaining -= length;
    }
    regions
}

/// 用同一组区间、不同的内容种子重写一次写入计划。
///
/// 返回的偏移与长度逐项相同，只有内容种子变化——因此「同一批区域反复编辑」会让每次的
/// 差异都落在真实随机数据上，delta 无法靠压缩变小。
pub fn rewrite_layout(layout: &[RegionWrite], content_seed: u64) -> Vec<RegionWrite> {
    layout
        .iter()
        .enumerate()
        .map(|(index, region)| RegionWrite {
            offset: region.offset,
            length: region.length,
            seed: content_seed
                .wrapping_add(u64::try_from(index).unwrap_or(0))
                .wrapping_mul(0xD1B5_4A32_D192_ED03),
        })
        .collect()
}

/// 一批写入计划实际触及的**不同字节数**（区间取并集）。
///
/// 稀疏工作文件占用的簇只由「被写过哪些区间」决定，而不是由「写过多少次」决定：反复编辑
/// 同一批区域不会让文件变大。账本必须按并集计量，否则大变动场景的实测增量会低于预期，
/// 断言退化成同义反复。
pub fn distinct_written_bytes(regions: &[RegionWrite]) -> u64 {
    if regions.is_empty() {
        return 0;
    }
    let mut spans = regions
        .iter()
        .map(|region| (region.offset, region.end()))
        .collect::<Vec<_>>();
    spans.sort_unstable();
    let mut total = 0_u64;
    let mut current = spans[0];
    for span in spans.iter().skip(1) {
        if span.0 <= current.1 {
            current.1 = current.1.max(span.1);
        } else {
            total += current.1 - current.0;
            current = *span;
        }
    }
    total + (current.1 - current.0)
}

fn next_u64(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// 在已存在的文件上就地写入区域，不改变文件长度。返回实际写入的字节数。
pub fn apply_regions(path: &Path, regions: &[RegionWrite]) -> u64 {
    let mut file = OpenOptions::new()
        .write(true)
        .open(path)
        .expect("无法打开工作文件");
    let mut written = 0_u64;
    for region in regions {
        file.seek(SeekFrom::Start(region.offset))
            .expect("无法定位写入区域");
        let bytes = pattern_bytes(
            usize::try_from(region.length).expect("区域过大"),
            region.seed,
        );
        file.write_all(&bytes).expect("无法写入随机区域");
        written += region.length;
    }
    file.sync_all().expect("无法同步工作文件");
    written
}

/// 稀疏构造一个工作文件：先 `set_len` 建立逻辑长度，再向有界区域写入真实随机字节，
/// 其余部分保持全零。
///
/// Windows 上 `set_len` 等价 `SetEndOfFile`，Linux 上等价 `ftruncate`：未写入的部分
/// **不占簇**（读取返回全零），因此 4 GiB 逻辑文件的实际磁盘占用约等于写入的区域总量。
/// 返回实际写入的字节数，供账本按「有效占用」而不是「逻辑长度」计量。
pub fn write_sparse_file(path: &Path, logical_size: u64, regions: &[RegionWrite]) -> u64 {
    let file = File::create(path).expect("无法创建稀疏工作文件");
    file.set_len(logical_size).expect("无法设置文件长度");
    drop(file);
    for region in regions {
        assert!(
            region.end() <= logical_size,
            "区域 {}..{} 超出逻辑大小 {logical_size}",
            region.offset,
            region.end()
        );
    }
    apply_regions(path, regions)
}

// ---------------------------------------------------------------------------
// 档位与磁盘预算（批次 3）
// ---------------------------------------------------------------------------

pub const MIB: u64 = 1024 * 1024;
pub const GIB: u64 = 1024 * MIB;

/// 大文件档位，由 `LILITH_STRESS_TIERS` 选择（默认 `default`）。
///
/// 档位只影响大文件场景的逻辑大小；小规模档（取消、崩溃、规模）不读档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// 64 MiB：日常使用的典型体积，成本低到可频繁运行。
    Default,
    /// 256 MiB：跨过明显的块级规模，仍属日常范围。
    Large,
    /// 1 GiB：PSD 常见超限体积。
    Heavy,
    /// 4 GiB：PSD 常态上限；认证大图场景从此档起。
    Extreme,
    /// 8 GiB：4 GiB 现实上限的两倍冗余，需 ≥ 24 GiB 空闲，仅手动。
    Manual,
}

impl Tier {
    pub const ALL: [Tier; 5] = [
        Tier::Default,
        Tier::Large,
        Tier::Heavy,
        Tier::Extreme,
        Tier::Manual,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Tier::Default => "default",
            Tier::Large => "large",
            Tier::Heavy => "heavy",
            Tier::Extreme => "extreme",
            Tier::Manual => "manual",
        }
    }

    pub fn names() -> String {
        Self::ALL
            .iter()
            .map(|tier| tier.name())
            .collect::<Vec<_>>()
            .join("/")
    }

    /// 大文件场景的逻辑大小，同时是工作文件与 head snapshot 的名义大小。
    pub fn logical_size(self) -> u64 {
        match self {
            Tier::Default => 64 * MIB,
            Tier::Large => 256 * MIB,
            Tier::Heavy => GIB,
            Tier::Extreme => 4 * GIB,
            Tier::Manual => 8 * GIB,
        }
    }

    /// 该档位在「提交期间新旧两份快照 + 一份全尺寸恢复输出」下的峰值磁盘下界。
    ///
    /// 快照不压缩、不去重，因此每份约等于逻辑大小；提交期间旧快照要等新快照落库后才释放，
    /// 两份会同时存在。场景的实际预期还要加上全部增量与工作文件的实际占用。
    pub fn peak_disk_floor(self) -> u64 {
        self.logical_size() * 3 + 64 * MIB
    }

    /// 首次稀疏构造时写入的真实字节数：`min(128 MiB, 逻辑大小的一半)`。
    pub fn initial_region_bytes(self) -> u64 {
        (self.logical_size() / 2).min(128 * MIB)
    }
}

/// 当前档位。同一进程内只解析一次，保证一轮测试里的所有场景使用同一档位。
pub fn tier() -> Tier {
    static TIER: OnceLock<Tier> = OnceLock::new();
    *TIER.get_or_init(|| {
        let Ok(value) = std::env::var("LILITH_STRESS_TIERS") else {
            return Tier::Default;
        };
        let name = value.trim();
        if name.is_empty() {
            return Tier::Default;
        }
        Tier::ALL
            .iter()
            .copied()
            .find(|tier| tier.name() == name)
            .unwrap_or_else(|| {
                panic!(
                    "LILITH_STRESS_TIERS 只接受 {}，收到 {name:?}",
                    Tier::names()
                )
            })
    })
}

/// 磁盘上限默认值。
///
/// 24 GiB 的依据是「最坏场景的诚实峰值」：提交期间同时存在新旧两份完整快照，之后还要放
/// 一份全尺寸恢复输出，因此峰值约 `3 × 逻辑大小 + 全部增量 + 工作文件实际占用`。
/// 4 GiB 档 + 每次 1 GiB 变动（大变动场景）约 16 GiB，因此 12 GiB 会把它误判为超预算。
/// 8 GiB 档（`manual`）在最坏情况下需要约 25 GiB，需显式调高上限。
pub const DEFAULT_DISK_BUDGET: u64 = 24 * GIB;

/// `LILITH_STRESS_DISK_BUDGET`，接受纯字节数或 `KiB/MiB/GiB/TiB` 后缀。
pub fn disk_budget() -> u64 {
    static BUDGET: OnceLock<u64> = OnceLock::new();
    *BUDGET.get_or_init(|| match std::env::var("LILITH_STRESS_DISK_BUDGET") {
        Ok(value) if !value.trim().is_empty() => parse_byte_size(&value).unwrap_or_else(|| {
            panic!(
                "LILITH_STRESS_DISK_BUDGET 无法解析：{value:?}\
                 （接受字节数或 KiB/MiB/GiB/TiB 后缀）"
            )
        }),
        _ => DEFAULT_DISK_BUDGET,
    })
}

pub fn parse_byte_size(value: &str) -> Option<u64> {
    let text = value.trim();
    let upper = text.to_ascii_uppercase();
    let suffixes: [(&str, u64); 5] = [
        ("TIB", 1024 * GIB),
        ("GIB", GIB),
        ("MIB", MIB),
        ("KIB", 1024),
        ("B", 1),
    ];
    for (suffix, multiplier) in suffixes {
        if let Some(digits) = upper.strip_suffix(suffix) {
            let number: u64 = digits.trim().parse().ok()?;
            return number.checked_mul(multiplier);
        }
    }
    text.parse().ok()
}

pub fn human_bytes(bytes: u64) -> String {
    if bytes >= GIB {
        format!("{:.2} GiB", bytes as f64 / GIB as f64)
    } else {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    }
}

/// 大文件场景的并发闸门：按「预计峰值磁盘」准入，允许 N 个场景同时运行。
///
/// 替代原来的全局串行闸门。串行的唯一理由是「并行会让多个快照与恢复输出叠加到同一块
/// 磁盘上」——真正的约束是**磁盘峰值**，而不是并发本身。每个无头子进程都是单线程的
/// （分块滚哈希 + SHA-256 + zstd 都是单核路径），因此在多核机器上串行意味着**只用一个
/// 核、磁盘只跑到零头带宽**：16 核机器实测 CPU 约 6%、有效吞吐约 150 MB/s（磁盘可达
/// 2 GB/s）。改为按预算准入后，多个互不共享工作区的场景可以并行，总耗时随核数下降，
/// 同时磁盘峰值仍不越界。
///
/// 准入必须同时满足两条：① `已保留峰值 + 本次预计峰值 ≤ LILITH_STRESS_DISK_BUDGET`；
/// ② 活动场景数 < `LILITH_STRESS_JOBS`（默认取可用并行度）。第 ① 条是安全底线——没有它，
/// 并行的大文件场景会把磁盘写满，运行中途失败比慢得多更糟。
pub struct ScenarioSlot {
    reserved: u64,
}

impl Drop for ScenarioSlot {
    fn drop(&mut self) {
        let mut guard = ADMISSION.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(state) = guard.as_mut() {
            state.used = state.used.saturating_sub(self.reserved);
            state.active = state.active.saturating_sub(1);
        }
        drop(guard);
        ADMISSION_CV.notify_all();
    }
}

struct Admission {
    used: u64,
    active: usize,
}

static ADMISSION: Mutex<Option<Admission>> = Mutex::new(None);
static ADMISSION_CV: Condvar = Condvar::new();

/// 申请一个大文件场景名额；磁盘峰值或并发度不足时阻塞，直到有名额被释放。
pub fn scenario_slot(expected_peak: u64) -> ScenarioSlot {
    let budget = disk_budget();
    let jobs = max_parallel_jobs();
    let mut guard = ADMISSION.lock().unwrap_or_else(|error| error.into_inner());
    loop {
        let (used, active) = {
            let state = guard.get_or_insert(Admission { used: 0, active: 0 });
            (state.used, state.active)
        };
        if used + expected_peak <= budget && active < jobs {
            let state = guard.as_mut().expect("准入表已初始化");
            state.used += expected_peak;
            state.active += 1;
            return ScenarioSlot {
                reserved: expected_peak,
            };
        }
        guard = ADMISSION_CV
            .wait(guard)
            .unwrap_or_else(|error| error.into_inner());
    }
}

/// 同时运行的大文件场景上限，可用 `LILITH_STRESS_JOBS` 覆盖；默认取可用并行度。
pub fn max_parallel_jobs() -> usize {
    static JOBS: OnceLock<usize> = OnceLock::new();
    *JOBS.get_or_init(|| {
        if let Ok(value) = std::env::var("LILITH_STRESS_JOBS") {
            let parsed = value
                .trim()
                .parse::<usize>()
                .unwrap_or_else(|_| panic!("LILITH_STRESS_JOBS 必须是正整数，收到 {value:?}"));
            return parsed.max(1);
        }
        std::thread::available_parallelism()
            .map(|value| value.get())
            .unwrap_or(1)
    })
}

/// 工作区的实测占用，按「文件长度之和」逐类统计。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TreeUsage {
    pub total: u64,
    pub repository: u64,
    pub snapshots: u64,
    pub deltas: u64,
    pub work: u64,
    pub out: u64,
    pub backup: u64,
    pub other: u64,
}

impl TreeUsage {
    /// 账本口径的「实际占用估计」。
    ///
    /// 目录内文件长度之和是**上界**：稀疏工作文件的名义长度远大于它占用的簇，因此
    /// 把工作文件替换为 `work_allocated`（该文件实际写入的字节数）。这也是预算守卫
    /// 使用的口径——偏保守，不会因为低估而写坏磁盘。
    pub fn allocated_estimate(&self, work_allocated: u64) -> u64 {
        self.total
            .saturating_sub(self.work)
            .saturating_add(work_allocated)
    }
}

/// 递归统计工作区占用。`repository` 之下的 `snapshots/`、`deltas/` 另计。
pub fn tree_usage(workspace: &Workspace) -> TreeUsage {
    let mut usage = TreeUsage::default();
    walk_usage(workspace.root(), workspace.root(), &mut usage);
    usage
}

fn walk_usage(root: &Path, directory: &Path, usage: &mut TreeUsage) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_usage(root, &path, usage);
            continue;
        }
        if !path.is_file() {
            continue;
        }
        let length = fs::metadata(&path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        usage.total += length;
        let relative = path
            .strip_prefix(root)
            .map(|value| value.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        match relative.split('/').next().unwrap_or_default() {
            "repository" => {
                usage.repository += length;
                if relative.contains("/snapshots/") {
                    usage.snapshots += length;
                } else if relative.contains("/deltas/") {
                    usage.deltas += length;
                }
            }
            "work" => usage.work += length,
            "out" => usage.out += length,
            "backup" => usage.backup += length,
            _ => usage.other += length,
        }
    }
}

/// 场景账本：把一行的实测占用与预期追加到 `<workspace>/ledger.jsonl`。
///
/// 工作区随场景结束被回收（`TempDir` 的析构），因此同时镜像一份到
/// `target/stress-ledger.jsonl`——能被长期引用的是这一份，以及
/// `target/stress-report.jsonl` 里的同一组数值。
pub fn append_ledger(workspace: &Workspace, entry: Value) {
    let line = json!({ "at": now_ms(), "entry": entry });
    let destinations = [
        workspace.root().join("ledger.jsonl"),
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("stress-ledger.jsonl"),
    ];
    let lock = LEDGER_LOCK.lock();
    for path in destinations {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Err(error) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut file| writeln!(file, "{line}"))
        {
            eprintln!("无法写入磁盘账本 {}：{error}", path.display());
        }
    }
    drop(lock);
}

static LEDGER_LOCK: Mutex<()> = Mutex::new(());

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// 清空一个目录的全部条目（不删除目录本身）。用于场景间的即时拆除。
pub fn clear_directory(directory: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let _ = fs::remove_dir_all(&path);
        } else {
            let _ = fs::remove_file(&path);
        }
    }
}

/// 断言一个刚建立（或刚重置）的仓库通过 `verify`：这是「重置本身也是一次断言」的落点。
pub fn assert_fresh_repository(workspace: &Workspace, repository: &Path) {
    headless(workspace, "verify")
        .repository(repository)
        .finish()
        .expect_ok();
}

/// 逐位比较两个文件。失败时给出长度差异或第一处不一致的字节偏移。
pub fn compare_files(expected: &Path, actual: &Path) -> Result<(), String> {
    let expected_len = file_bytes(expected);
    let actual_len = file_bytes(actual);
    if expected_len != actual_len {
        return Err(format!(
            "长度不一致：期望 {expected_len} 字节（{}），实际 {actual_len} 字节（{}）",
            expected.display(),
            actual.display()
        ));
    }
    const BUFFER: usize = 1024 * 1024;
    let mut left =
        File::open(expected).map_err(|error| format!("无法打开 {expected:?}：{error}"))?;
    let mut right = File::open(actual).map_err(|error| format!("无法打开 {actual:?}：{error}"))?;
    let mut left_buffer = vec![0_u8; BUFFER];
    let mut right_buffer = vec![0_u8; BUFFER];
    let mut offset = 0_u64;
    loop {
        let left_read = read_up_to(&mut left, &mut left_buffer)
            .map_err(|error| format!("无法读取 {expected:?}：{error}"))?;
        let right_read = read_up_to(&mut right, &mut right_buffer)
            .map_err(|error| format!("无法读取 {actual:?}：{error}"))?;
        if left_read != right_read {
            return Err(format!("第 {offset} 字节处读取长度不一致"));
        }
        if left_read == 0 {
            return Ok(());
        }
        if left_buffer[..left_read] != right_buffer[..right_read] {
            let position = left_buffer[..left_read]
                .iter()
                .zip(&right_buffer[..right_read])
                .position(|(left, right)| left != right)
                .unwrap_or(0);
            return Err(format!("第 {} 字节处不一致", offset + position as u64));
        }
        offset += u64::try_from(left_read).unwrap_or(0);
    }
}

fn read_up_to(reader: &mut impl Read, buffer: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        let read = reader.read(&mut buffer[filled..])?;
        if read == 0 {
            break;
        }
        filled += read;
    }
    Ok(filled)
}

#[track_caller]
pub fn assert_files_identical(expected: &Path, actual: &Path) {
    if let Err(description) = compare_files(expected, actual) {
        panic!(
            "文件内容必须逐位一致：{}\n  期望：{}\n  实际：{}",
            description,
            expected.display(),
            actual.display()
        );
    }
}

pub fn file_bytes(path: &Path) -> u64 {
    fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0)
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

/// 目录内条目的名字（不递归），已排序。缺失的目录返回空表。
pub fn directory_names(directory: &Path) -> Vec<String> {
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
    assert_healthy_within(workspace, repository, DEFAULT_TIMEOUT).nodes
}

/// `verify` + `scrub` 的实测耗时与结果。
pub struct HealthReport {
    pub nodes: u64,
    pub verify_ms: u64,
    pub scrub_ms: u64,
    pub verify_peak_bytes: u64,
    pub scrub_peak_bytes: u64,
}

/// 与 `assert_healthy` 相同，但由调用方指定超时，并把两段耗时交回给报告。
///
/// **大文件场景必须显式放宽超时**：全库校验的代价是 O(节点数 × 文件大小)——它要为每个
/// 节点从最近的快照重放整条链，再把整份内容流式复算一遍块摘要。1 GiB 档的 21 节点链远超
/// helper 的默认 180 s，用默认超时会让 `scrub` 被掐断，看起来像产品失败。
#[track_caller]
pub fn assert_healthy_within(
    workspace: &Workspace,
    repository: &Path,
    timeout: Duration,
) -> HealthReport {
    let verify = headless(workspace, "verify")
        .repository(repository)
        .timeout(timeout)
        .finish();
    verify.expect_ok();
    let scrub = headless(workspace, "scrub")
        .repository(repository)
        .timeout(timeout)
        .finish();
    scrub.expect_ok();
    HealthReport {
        nodes: scrub.data_u64("historyNodes"),
        verify_ms: verify.elapsed.as_millis() as u64,
        scrub_ms: scrub.elapsed.as_millis() as u64,
        verify_peak_bytes: verify.result["peakWorkingSetBytes"].as_u64().unwrap_or(0),
        scrub_peak_bytes: scrub.result["peakWorkingSetBytes"].as_u64().unwrap_or(0),
    }
}

/// 只做重开断言（`verify`）：迁移 + `integrity_check` + 外键 + 全表语义校验。
///
/// 用于「链完整性已由别的场景覆盖，这里只确认仓库仍可打开」的场合——例如大文件的
/// 链体积场景，它的重点是体积而不是再次全库逐块校验（后者是 O(节点数 × 文件大小)）。
#[track_caller]
pub fn assert_reopens(workspace: &Workspace, repository: &Path) {
    headless(workspace, "verify")
        .repository(repository)
        .finish()
        .expect_ok();
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
pub fn record(scenario: &str, group: &str, tier: &str, outcome: &Outcome, extra: Value) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("stress-report.jsonl");
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let peak = outcome.result["peakWorkingSetBytes"].as_u64();
    let line = json!({
        "scenario": scenario,
        "group": group,
        "tier": tier,
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
    append_report_line(&path, &line);
}

/// 追加一行 B 组（崩溃）报告。强杀使进程来不及写 `--result`，因此没有峰值内存等字段，
/// 只记录强杀位置、耗时与观察到的检查点数。
pub fn record_crash(scenario: &str, group: &str, tier: &str, outcome: &CrashOutcome, extra: Value) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("stress-report.jsonl");
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let line = json!({
        "scenario": scenario,
        "group": group,
        "tier": tier,
        "killed": outcome.killed,
        "targetCheckpoint": outcome.target.as_ref().map(|checkpoint| checkpoint.index),
        "targetStage": outcome.target.as_ref().map(|checkpoint| checkpoint.stage.clone()),
        "exitCode": outcome.exit_code,
        "timedOut": outcome.timed_out,
        "elapsedMs": outcome.elapsed.as_millis() as u64,
        "checkpointCount": outcome.checkpoints.len(),
        "detail": extra,
    });
    append_report_line(&path, &line);
}

/// 追加一行「明确跳过」报告：磁盘预算不足时**跳过并如实报告**，而不是中途写坏磁盘。
pub fn record_skip(scenario: &str, group: &str, tier: &str, reason: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("stress-report.jsonl");
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    eprintln!("跳过压力测试场景 {scenario}：{reason}");
    let line = json!({
        "scenario": scenario,
        "group": group,
        "tier": tier,
        "skipped": true,
        "reason": reason,
    });
    append_report_line(&path, &line);
}

fn append_report_line(path: &Path, line: &Value) {
    let lock = REPORT_LOCK.lock();
    let result = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| writeln!(file, "{line}"));
    drop(lock);
    if let Err(error) = result {
        eprintln!("无法写入压力测试报告：{error}");
    }
}
