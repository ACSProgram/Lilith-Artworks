//! 无头命令行入口（`feature = "headless"`）。
//!
//! 定位：它**不是产品功能**。feature 门控、不进发布产物，只供发布前的压力测试以
//! **独立进程**调用真实可执行文件，从而跨越进程边界、零内部访问。
//!
//! 与 Tauri 命令层并列，它是同一批领域函数的**另一个适配器**，只做参数解析、锁与
//! 状态的装配、领域函数调用，**不含任何业务判断**；GUI 路径的行为与现状逐位不变。
//!
//! 规格见 `docs/planning/stress-test-plan-2026-10-04.md` 的 4.1 与 4.4。

use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Mutex,
    },
    time::Instant,
};

use serde::Serialize;
use serde_json::{json, Value};

use crate::{
    app::AppState,
    backup::{self, restore, worker, BackupState, BackupTaskKind},
    cleanup, history, library,
};

const EXIT_OK: i32 = 0;
const EXIT_ERROR: i32 = 1;
const EXIT_CANCELLED: i32 = 2;

/// 命令行入口。返回进程退出码：0 成功、1 失败、2 取消。
pub fn run(args: &[String]) -> i32 {
    let started = Instant::now();
    if args.is_empty() || matches!(args[0].as_str(), "--help" | "-h" | "help") {
        eprint!("{}", usage());
        return EXIT_OK;
    }
    let options = match Options::parse(args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("lilith-artworks --headless: {error}");
            eprint!("{}", usage());
            return EXIT_ERROR;
        }
    };
    let interlock = match Interlock::new(&options) {
        Ok(interlock) => interlock,
        Err(error) => {
            eprintln!("lilith-artworks --headless: {error}");
            return EXIT_ERROR;
        }
    };
    interlock.stage(&format!("started command={}", options.command));

    let outcome = dispatch(&options, &interlock);
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let cancelled = interlock.cancelled();
    let (ok, error, data) = match outcome {
        Ok(data) => (true, None, data),
        Err(error) => (false, Some(error), Value::Null),
    };
    let peak = peak_memory_bytes();
    let result = HeadlessResult {
        command: options.command.clone(),
        ok,
        cancelled,
        error,
        elapsed_ms,
        peak_working_set_bytes: peak,
        data,
    };
    if let Some(path) = options.result.as_deref() {
        if let Err(write_error) = write_json(path, &result) {
            eprintln!("lilith-artworks --headless: {write_error}");
        }
    }
    if let Some(path) = options.peak_memory.as_deref() {
        let payload = json!({
            "peakWorkingSetBytes": peak,
            "elapsedMs": elapsed_ms,
        });
        if let Err(write_error) = write_json(path, &payload) {
            eprintln!("lilith-artworks --headless: {write_error}");
        }
    }
    if result.cancelled {
        EXIT_CANCELLED
    } else if result.ok {
        EXIT_OK
    } else {
        EXIT_ERROR
    }
}

/// 取消结果在部分领域函数里仍以普通错误串返回（`restore`/`cleanup`/灾备各有一份
/// 文案），因此取消判定以闸门自身为准，而不是比对错误文案。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HeadlessResult {
    command: String,
    ok: bool,
    cancelled: bool,
    error: Option<String>,
    elapsed_ms: u64,
    peak_working_set_bytes: Option<u64>,
    data: Value,
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let bytes =
        serde_json::to_vec_pretty(value).map_err(|error| format!("无法序列化结果：{error}"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("无法创建结果目录：{error}"))?;
    }
    std::fs::write(path, bytes).map_err(|error| format!("无法写入结果文件：{error}"))
}

fn usage() -> String {
    let mut text = String::from(
        "lilith-artworks --headless <command> [options]\n\
         \n\
         开发/验证期入口，不进发布产物。完整规格见 docs/guides/validation.md 的压力测试小节。\n\
         \n\
         commands:\n\
         \x20 init-repository      建立或迁移作品仓库\n\
         \x20 create-artwork       创建 Artwork 与其主分支\n\
         \x20 commit               提交分支工作文件\n\
         \x20 restore              恢复历史节点到输出文件\n\
         \x20 compact              精简中间历史节点\n\
         \x20 checkpoint           为历史节点生成检查点\n\
         \x20 scrub                全库校验历史链\n\
         \x20 verify               重开仓库并做完整性与语义校验\n\
         \x20 cleanup              重放待清理文件队列\n\
         \x20 repository-backup    整仓灾备\n\
         \n\
         common options:\n\
         \x20 --workspace <dir>    作用域根目录；无头进程不读写其之外的路径\n\
         \x20 --repository <dir>   作品仓库目录，默认 <workspace>/repository\n\
         \x20 --result <file>      结构化 JSON 结果写入该文件\n\
         \x20 --marker <file>      逐阶段追加阶段名，供测试决定何时干预\n\
         \x20 --cancel-on-stdin    每个取消检查点写 marker 后阻塞等待 stdin 的\n\
         \x20                      cancel/continue（EOF 视为取消）\n\
         \x20 --peak-memory <file> 结束时写入自身峰值内存\n",
    );
    // 子命令专用选项只在 --help 里概览，具体取值由调用方与 tests/ 保证。
    text.push_str(
        "\ncommand options:\n\
         \x20 --title/--branch-title/--source/--parent   create-artwork\n\
         \x20 --branch/--note/--commit-kind               commit\n\
         \x20 --history                                  restore/compact/checkpoint\n\
         \x20 --output                                   restore\n\
         \x20 --destination                              repository-backup\n\
         \x20 --ids                                      cleanup\n",
    );
    text
}

struct Options {
    command: String,
    workspace: PathBuf,
    repository: PathBuf,
    result: Option<PathBuf>,
    marker: Option<PathBuf>,
    cancel_on_stdin: bool,
    peak_memory: Option<PathBuf>,
    values: HashMap<String, String>,
}

impl Options {
    fn parse(args: &[String]) -> Result<Self, String> {
        const KNOWN: [&str; 15] = [
            "workspace",
            "repository",
            "result",
            "marker",
            "peak-memory",
            "branch",
            "history",
            "title",
            "branch-title",
            "source",
            "note",
            "commit-kind",
            "output",
            "destination",
            "parent",
        ];
        let command = args
            .first()
            .filter(|value| !value.starts_with('-'))
            .cloned()
            .ok_or("缺少子命令")?;
        let mut values = HashMap::new();
        let mut cancel_on_stdin = false;
        let mut index = 1;
        while index < args.len() {
            let argument = args[index].clone();
            if argument == "--cancel-on-stdin" {
                cancel_on_stdin = true;
                index += 1;
                continue;
            }
            let key = argument
                .strip_prefix("--")
                .ok_or_else(|| format!("无法识别的参数：{argument}"))?
                .to_owned();
            if key != "ids" && !KNOWN.contains(&key.as_str()) {
                return Err(format!("未知选项：{argument}"));
            }
            let value = args
                .get(index + 1)
                .filter(|value| !value.starts_with("--"))
                .ok_or_else(|| format!("选项 {argument} 缺少取值"))?
                .clone();
            values.insert(key, value);
            index += 2;
        }

        let workspace = values.get("workspace").ok_or("缺少 --workspace")?.clone();
        let workspace = PathBuf::from(workspace);
        if !workspace.is_dir() {
            return Err("--workspace 必须是已存在的目录".into());
        }
        let workspace = workspace
            .canonicalize()
            .map_err(|error| format!("无法访问 --workspace：{error}"))?;
        let repository = match values.get("repository") {
            Some(value) => PathBuf::from(value),
            None => workspace.join("repository"),
        };
        let repository = resolve_within(&workspace, &repository, "作品仓库")?;

        Ok(Self {
            command,
            workspace,
            repository,
            result: values.get("result").map(PathBuf::from),
            marker: values.get("marker").map(PathBuf::from),
            cancel_on_stdin,
            peak_memory: values.get("peak-memory").map(PathBuf::from),
            values,
        })
    }

    fn value(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    fn require(&self, key: &str) -> Result<&str, String> {
        self.value(key).ok_or_else(|| format!("缺少 --{key}"))
    }

    /// 解析一个必须位于工作区之内的路径参数，并校验其确实在工作区内。
    fn path_within(&self, key: &str, label: &str) -> Result<PathBuf, String> {
        resolve_within(&self.workspace, Path::new(self.require(key)?), label)
    }

    /// 与 Tauri 侧同样可无 Tauri 构造的状态装配：仓库路径来自命令行，设置与日志
    /// 目录只用于填满字段，无头进程不会调用依赖它们的命令。
    fn app_state(&self) -> AppState {
        AppState::for_headless_repository(&self.repository, &self.workspace.join(".headless"))
    }

    fn backup_state(&self) -> BackupState {
        BackupState::default()
    }
}

/// 把路径解析到 `workspace` 之内：不存在的路径按「最近的已存在祖先 + 剩余后缀」
/// 解析，因此新建仓库、新建输出目录也适用。这是无头进程的真实安全属性——
/// 它无法读写工作区之外的路径。
fn resolve_within(workspace: &Path, path: &Path, label: &str) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err(format!("{label}必须使用绝对路径"));
    }
    let existing = path
        .ancestors()
        .find(|candidate| candidate.exists())
        .ok_or_else(|| format!("无法解析{label}"))?;
    let canonical = existing
        .canonicalize()
        .map_err(|error| format!("无法访问{label}：{error}"))?;
    let suffix = path
        .strip_prefix(existing)
        .map_err(|_| format!("无法解析{label}"))?;
    // `join("")` 会追加一个尾部分隔符，把「已存在的那个祖先就是目标本身」的情况
    // 变成 `...\file\`，因此空后缀必须原样返回。
    let resolved = if suffix.as_os_str().is_empty() {
        canonical
    } else {
        canonical.join(suffix)
    };
    if !resolved.starts_with(workspace) {
        return Err(format!("{label}必须位于 --workspace 之内"));
    }
    Ok(resolved)
}

/// 阶段标记与取消闸门。
///
/// - `--marker`：每个可观察阶段追加一行，只写不阻塞，供测试观察与后续强杀场景使用。
/// - `--cancel-on-stdin`：每个取消检查点先写 marker，再阻塞等待 stdin 的一行判定
///   （`cancel` / `continue`，EOF 视为取消）。因此测试可以**逐检查点**决定取消时机，
///   不存在「读到 marker 时进程已经跑过下一个检查点」的竞态。
///
/// 两种模式都只通过文件与 stdin 干预被测进程，不触碰任何内部状态。
struct Interlock {
    marker: Option<Mutex<File>>,
    stdin: Option<Mutex<BufReader<std::io::Stdin>>>,
    index: AtomicUsize,
    cancelled: AtomicBool,
    last_stage: Mutex<String>,
}

impl Interlock {
    fn new(options: &Options) -> Result<Self, String> {
        let marker = match options.marker.as_deref() {
            Some(path) => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|error| format!("无法创建阶段标记目录：{error}"))?;
                }
                Some(Mutex::new(
                    OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                        .map_err(|error| format!("无法打开阶段标记文件：{error}"))?,
                ))
            }
            None => None,
        };
        let stdin = options
            .cancel_on_stdin
            .then(|| Mutex::new(BufReader::new(std::io::stdin())));
        Ok(Self {
            marker,
            stdin,
            index: AtomicUsize::new(0),
            cancelled: AtomicBool::new(false),
            last_stage: Mutex::new(String::new()),
        })
    }

    fn write_line(&self, line: &str) {
        let Some(marker) = &self.marker else {
            return;
        };
        if let Ok(mut file) = marker.lock() {
            let _ = writeln!(file, "{line}");
            let _ = file.flush();
        }
    }

    /// 记录一个可观察阶段（来自领域函数的进度回调）。
    fn stage(&self, label: &str) {
        if let Ok(mut stage) = self.last_stage.lock() {
            *stage = label.to_owned();
        }
        self.write_line(&format!("stage {label}"));
    }

    /// 一个取消检查点，返回值即「此刻是否已请求取消」。
    fn checkpoint(&self) -> bool {
        if self.cancelled.load(Ordering::SeqCst) {
            return true;
        }
        let index = self.index.fetch_add(1, Ordering::SeqCst) + 1;
        let stage = self
            .last_stage
            .lock()
            .map(|value| value.clone())
            .unwrap_or_default();
        self.write_line(&format!("checkpoint index={index} stage={stage}"));
        let Some(stdin) = &self.stdin else {
            return false;
        };
        let cancel = match stdin.lock() {
            Ok(mut reader) => {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    // EOF：测试提前关闭 stdin，按取消处理。
                    Ok(0) => true,
                    Ok(_) => line.trim() != "continue",
                    Err(_) => true,
                }
            }
            Err(_) => true,
        };
        if cancel {
            self.cancelled.store(true, Ordering::SeqCst);
        }
        self.write_line(&format!(
            "decision index={index} {}",
            if cancel { "cancel" } else { "continue" }
        ));
        cancel
    }

    fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

fn dispatch(options: &Options, interlock: &Interlock) -> Result<Value, String> {
    match options.command.as_str() {
        "init-repository" => init_repository(options),
        "create-artwork" => create_artwork(options),
        "commit" => commit(options, interlock),
        "restore" => restore_node(options, interlock),
        "compact" => compact_node(options, interlock),
        "checkpoint" => checkpoint(options, interlock),
        "scrub" => scrub(options, interlock),
        "verify" => verify(options),
        "cleanup" => cleanup_queue(options),
        "repository-backup" => repository_backup(options, interlock),
        other => Err(format!("未知子命令：{other}")),
    }
}

fn init_repository(options: &Options) -> Result<Value, String> {
    library::initialize(&options.repository)?;
    Ok(Value::Null)
}

fn create_artwork(options: &Options) -> Result<Value, String> {
    let title = options.require("title")?;
    let branch_title = options.require("branch-title")?;
    let source = options.path_within("source", "分支工作文件")?;
    let app_state = options.app_state();
    let created = app_state.with_ready_repository(|root| {
        library::create_artwork(root, options.value("parent"), title, branch_title, &source)
    })?;
    Ok(json!({
        "artworkId": created.artwork_id,
        "branchId": created.branch_id,
    }))
}

fn commit(options: &Options, interlock: &Interlock) -> Result<Value, String> {
    let branch_id = options.require("branch")?;
    let note = options.value("note").unwrap_or_default();
    let commit_kind = options.value("commit-kind").unwrap_or("manual");
    let app_state = options.app_state();
    let state = options.backup_state();
    // 与 `run_branch_backup` 一致：手动提交走 `run_logged` + `UserOperation`，失败时
    // 记录分支错误摘要。无头进程不启动调度器，因此没有手动优先与唤醒步骤。
    let result = state.run_logged(
        "manual backup",
        &format!("branch_id={branch_id}, note_chars={}", note.chars().count()),
        Some(branch_id),
        BackupTaskKind::UserOperation,
        || {
            app_state.with_ready_repository(|root| {
                let result = worker::run_backup(root, branch_id, note, commit_kind, || {
                    interlock.checkpoint()
                });
                if let Err(error) = result.as_ref() {
                    history::mark_error(root, branch_id, &error.to_string());
                }
                result.map_err(|error| error.to_string())
            })
        },
    )?;
    Ok(json!({
        "created": result.created,
        "unchanged": result.unchanged,
        "historyId": result.history_id,
    }))
}

fn restore_node(options: &Options, interlock: &Interlock) -> Result<Value, String> {
    let history_id = options.require("history")?;
    let output = options.path_within("output", "恢复输出")?;
    let output = output.to_string_lossy().into_owned();
    let app_state = options.app_state();
    let state = options.backup_state();
    let path = state.run_logged_foreground(
        "restore",
        &format!("history_id={history_id}, output={output}"),
        None,
        || {
            app_state.with_ready_repository(|root| {
                restore::restore(
                    root,
                    history_id,
                    &output,
                    || interlock.checkpoint(),
                    |label, _current, _total| interlock.stage(label),
                )
            })
        },
    )?;
    Ok(json!({ "outputPath": path }))
}

fn compact_node(options: &Options, interlock: &Interlock) -> Result<Value, String> {
    let history_id = options.require("history")?;
    let app_state = options.app_state();
    let state = options.backup_state();
    state.run_logged_foreground("compact", &format!("history_id={history_id}"), None, || {
        app_state.with_ready_repository(|root| {
            restore::compact_node(
                root,
                history_id,
                || interlock.checkpoint(),
                |label, _current, _total| interlock.stage(label),
            )
        })
    })?;
    Ok(Value::Null)
}

fn checkpoint(options: &Options, interlock: &Interlock) -> Result<Value, String> {
    let history_id = options.require("history")?;
    let app_state = options.app_state();
    let state = options.backup_state();
    state.run_logged_foreground(
        "checkpoint",
        &format!("history_id={history_id}"),
        None,
        || {
            app_state.with_ready_repository(|root| {
                restore::ensure_checkpoint_with_progress(
                    root,
                    history_id,
                    || interlock.checkpoint(),
                    |label, _current, _total| interlock.stage(label),
                )
            })
        },
    )?;
    Ok(Value::Null)
}

fn scrub(options: &Options, interlock: &Interlock) -> Result<Value, String> {
    let app_state = options.app_state();
    let state = options.backup_state();
    // 与 `scrub_repository_integrity` 的历史链部分一致；无头批次1 不覆盖认证受控文件。
    let nodes = state.run_foreground(None, || {
        app_state.with_ready_repository(|root| {
            restore::scrub_history(
                root,
                || interlock.checkpoint(),
                |current, total| interlock.stage(&format!("全库扫描 {current}/{total}")),
            )
        })
    })?;
    Ok(json!({ "historyNodes": nodes }))
}

fn verify(options: &Options) -> Result<Value, String> {
    // `open_existing` = 迁移到当前版本 + `integrity_check` + 外键检查 + 语义校验
    // （UUID / 仓库相对路径 / SHA-256 全表）。重开可用即以此为准。
    library::open_existing(&options.repository)?;
    Ok(Value::Null)
}

fn cleanup_queue(options: &Options) -> Result<Value, String> {
    let ids = options
        .value("ids")
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let app_state = options.app_state();
    let state = options.backup_state();
    let report = state.run_exclusive(None, BackupTaskKind::UserOperation, || {
        app_state.with_ready_repository(|root| cleanup::run(root, &ids))
    })?;
    serde_json::to_value(&report).map_err(|error| format!("无法序列化清理结果：{error}"))
}

fn repository_backup(options: &Options, interlock: &Interlock) -> Result<Value, String> {
    let destination = options.path_within("destination", "备份保存目录")?;
    let app_state = options.app_state();
    let state = options.backup_state();
    let report = state.run_foreground(None, || {
        app_state.with_ready_repository(|root| {
            backup::create_repository_backup(
                root,
                &destination,
                || interlock.checkpoint(),
                |label, _current, _total| interlock.stage(label),
            )
        })
    })?;
    serde_json::to_value(&report).map_err(|error| format!("无法序列化备份结果：{error}"))
}

/// 进程自身的峰值工作集。Windows 用 `GetProcessMemoryInfo`；其它平台返回 `None`，
/// 调用方据此写 null 而不是伪造数值。
#[cfg(target_os = "windows")]
fn peak_memory_bytes() -> Option<u64> {
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    // SAFETY: 结构体按 C 布局分配并清零，长度与 API 期望的一致。
    let mut counters: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    let size = u32::try_from(std::mem::size_of::<PROCESS_MEMORY_COUNTERS>()).ok()?;
    let succeeded = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, size) };
    (succeeded != 0).then(|| u64::try_from(counters.PeakWorkingSetSize).unwrap_or(u64::MAX))
}

#[cfg(not(target_os = "windows"))]
fn peak_memory_bytes() -> Option<u64> {
    None
}
