//! 无头命令行入口（`feature = "headless"`）。
//!
//! 定位：它**不是产品功能**。feature 门控、不进发布产物，只供发布前的压力测试以
//! **独立进程**调用真实可执行文件，从而跨越进程边界、零内部访问。
//!
//! 与 Tauri 命令层并列，它是同一批领域函数的**另一个适配器**，只做参数解析、锁与
//! 状态的装配、领域函数调用，**不含任何业务判断**；GUI 路径的行为与现状逐位不变。
//!
//! 定位、运行方式与覆盖矩阵见 `docs/guides/validation.md` 的「压力测试」小节，
//! 面向使用者的结论见 `docs/user/stress-test-report.md`。

use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::Instant,
};

use serde::Serialize;
use serde_json::{json, Value};

use crate::{
    app::{
        command_runtime::{
            CancelToken, CommandContext, CommandError, NoopEventSink, OperationCoordinator,
            OperationDomain, OperationObserver,
        },
        usecases, AppState,
    },
    authenticity::{self, AuthenticityState, PathAuthorization},
    backup::BackupState,
    library,
    platform::process::peak_memory_bytes,
    repository_lock,
};

const EXIT_OK: i32 = 0;
const EXIT_ERROR: i32 = 1;
const EXIT_CANCELLED: i32 = 2;

/// 进程级闸门。两处为测试而设的标记点（[`commit_marker`] 与 [`auth_checkpoint`]）
/// 需要它，而它们位于领域函数内部、拿不到调用栈上的闸门。一个无头进程只执行一条
/// 命令，因此进程级单例足够，不必层层传参。
static INTERLOCK: OnceLock<Interlock> = OnceLock::new();

/// 「事务已 BEGIN、INSERT 已写、尚未 COMMIT」的强杀/取消标记点。
///
/// 这是为测试而触及产品代码的**两处**标记点之一（另一处是 [`auth_checkpoint`]）：
/// `history::commit` 在 `transaction.commit()` 之前调用它，调用点本身由
/// `feature = "headless"` 门控，因此发布产物中不存在、GUI 路径行为逐位不变。
///
/// 返回「此刻是否已请求取消」：调用方据此放弃提交，让未提交事务被整体丢弃。
/// 无头进程没有开闸门时只写一行 marker 并立即返回 `false`。
pub(crate) fn commit_marker() -> bool {
    match INTERLOCK.get() {
        Some(interlock) => {
            interlock.stage("事务已写入，尚未提交");
            interlock.checkpoint()
        }
        None => false,
    }
}

/// 认证流水线的取消检查点（渲染 / 编码 / 签名）。
///
/// 与 [`commit_marker`] 同一范式，是两处为测试而触及产品代码的标记点中的**第二处**：
/// 调用点在领域函数 `authenticity::rendition` 内部、由 `feature = "headless"` 门控，
/// 发布产物中不存在，GUI 路径行为逐位不变。
/// 返回「此刻是否已请求取消」，调用方据此放弃发布。
pub(crate) fn auth_checkpoint(stage: &str) -> bool {
    match INTERLOCK.get() {
        Some(interlock) => {
            interlock.stage(stage);
            interlock.checkpoint()
        }
        None => false,
    }
}

/// 命令行入口。返回进程退出码：0 成功、1 失败、2 取消。
pub fn run(args: &[String]) -> i32 {
    repository_lock::record_process_start();
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
    // 登记为进程级单例：`history::commit` 的标记点（headless-only）据此取得闸门。
    let interlock = INTERLOCK.get_or_init(|| interlock);
    interlock.stage(&format!("started command={}", options.command));
    let runtime = HeadlessRuntime::new(&options, interlock);

    // 跨进程 owner lock 由每条写命令在协调器内按操作取得/释放；无头入口不再按
    // 命令名维护第二张表。只读命令不取锁，全库校验在 use-case 内只做一次探测。
    let outcome = dispatch(&options, &runtime);
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
         \x20 create-group         创建分组\n\
         \x20 move-node            移动作品树节点\n\
         \x20 trash-node           把节点移入回收站\n\
         \x20 empty-trash          清空回收站并重放清理队列\n\
         \x20 list-tree            列出作品树并返回计数\n\
         \x20 search               按标题/工作文件路径搜索\n\
         \x20 create-branch        从历史节点分叉新分支\n\
         \x20 delete-branch        删除分支历史\n\
         \x20 list-history         列出某 Artwork 的分支与节点计数\n\
         \x20 create-board         为 Artwork 创建画板\n\
         \x20 import-board-images  按路径导入画板图片\n\
         \x20 create-board-stroke  创建一笔笔迹（几何来自 --geometry 或 --geometry-hex）\n\
         \x20 create-board-strokes 按 --spec 清单批量创建笔迹\n\
         \x20 read-board-strokes   读取整板笔迹载荷（回报条目数、字节数与摘要）\n\
         \x20 scrub-board-dds      双向检查画板 DDS（缺失/损坏/孤儿）\n\
         \x20 commit               提交分支工作文件\n\
         \x20 restore              恢复历史节点到输出文件\n\
         \x20 compact              精简中间历史节点\n\
         \x20 checkpoint           为历史节点生成检查点\n\
         \x20 scrub                全库校验历史链\n\
         \x20 verify               重开仓库并做完整性与语义校验\n\
         \x20 cleanup              重放待清理文件队列\n\
         \x20 scan-unreferenced    扫描未引用文件（只报告）\n\
         \x20 cleanup-unreferenced 确认清理未引用文件（--ids）\n\
         \x20 scheduler-scan       只读探针：调度器候选扫描成本\n\
         \x20 repository-backup    整仓灾备\n\
         \x20 enter-publication    进入发布状态（固化最终成品）\n\
         \x20 publish              认证签名发布（C2PA + TrustMark）\n\
         \x20 cancel-publication   取消发布并回收仓库内副本\n\
         \x20 decode-authenticity  回读 C2PA 声明与 TrustMark 绑定\n\
         \n\
         common options:\n\
         \x20 --workspace <dir>    作用域根目录；无头进程不读写其之外的路径\n\
         \x20 --repository <dir>   作品仓库目录，默认 <workspace>/repository\n\
         \x20 --models <dir>       TrustMark 模型目录，默认 resources/models\n\
         \x20 --chunk-format <fmt> 新写入 ChunkFile 的格式（v1/v2），缺省 v2\n\
         \x20 --result <file>      结构化 JSON 结果写入该文件\n\
         \x20 --marker <file>      逐阶段追加阶段名，供测试决定何时干预\n\
         \x20 --cancel-on-stdin    每个取消检查点写 marker 后阻塞等待 stdin 的\n\
         \x20                      cancel/continue（EOF 视为取消）\n\
         \x20 --peak-memory <file> 结束时写入自身峰值内存\n",
    );
    // 子命令专用选项只在 --help 里概览，具体取值由调用方与 tests/ 保证。
    text.push_str(
        "\ncommand options:\n\
         \x20 --title/--branch-title/--source/--parent   create-artwork/create-group\n\
         \x20 --artwork/--history/--branch-title/--source create-branch\n\
         \x20 --branch/--note/--commit-kind               commit/delete-branch\n\
         \x20 --artwork                                  list-history\n\
         \x20 --ids/--parent/--index                     move-node\n\
         \x20 --ids                                      trash-node/cleanup/cleanup-unreferenced\n\
         \x20 --query                                    search\n\
         \x20 --history                                  restore/compact/checkpoint\n\
         \x20 --output                                   restore\n\
         \x20 --destination                              repository-backup\n\
         \x20 --artwork/--title                          create-board\n\
         \x20 --board/--revision/--paths                 import-board-images\n\
         \x20 --board/--revision/--tool/--color/--stroke-width\n\
         \x20 --width/--height/--center-x/--center-y     create-board-stroke\n\
         \x20 --geometry/--geometry-hex                  create-board-stroke\n\
         \x20 --board/--revision/--spec/--color          create-board-strokes\n\
         \x20 --board                                    read-board-strokes\n\
         \x20 --branch/--artifact                        enter-publication\n\
         \x20 --branch/--output/--certificate/--key/--title/--creator\n\
         \x20 --rights/--content/--algorithm/--trustmark/--regions\n\
         \x20 --jpeg-quality/--background/--strength/--watermark-id\n\
         \x20 --preview-cache-token/--timestamp-url      publish\n\
         \x20 --branch                                    cancel-publication\n\
         \x20 --input/--region                           decode-authenticity\n",
    );
    text
}

struct Options {
    command: String,
    workspace: PathBuf,
    repository: PathBuf,
    /// TrustMark 模型目录。无头环境没有 Tauri 资源解析，因此显式传入；默认指向仓库内
    /// 的 `resources/models`（两个模型文件确实在仓库中）。
    models: PathBuf,
    result: Option<PathBuf>,
    marker: Option<PathBuf>,
    cancel_on_stdin: bool,
    peak_memory: Option<PathBuf>,
    /// 新写入 ChunkFile 的格式覆盖：`Some(false)` = v1、`Some(true)` = v2、
    /// `None` = 跟随设置默认（无头构造即 v2）。只影响本进程此后写出的
    /// snapshot/delta，既有文件与读取路径不受影响。
    chunk_format: Option<bool>,
    values: HashMap<String, String>,
}

impl Options {
    fn parse(args: &[String]) -> Result<Self, String> {
        const KNOWN: [&str; 51] = [
            "workspace",
            "repository",
            "models",
            "chunk-format",
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
            "artwork",
            "query",
            "index",
            "board",
            "revision",
            "paths",
            "ids",
            "artifact",
            "certificate",
            "key",
            "creator",
            "rights",
            "content",
            "algorithm",
            "trustmark",
            "regions",
            "jpeg-quality",
            "background",
            "strength",
            "watermark-id",
            "preview-cache-token",
            "timestamp-url",
            "input",
            "region",
            "tool",
            "geometry",
            "geometry-hex",
            "stroke-width",
            "width",
            "height",
            "center-x",
            "center-y",
            "color",
            "spec",
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
            if !KNOWN.contains(&key.as_str()) {
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
        // 模型目录是**资源**而不是工作区内的产物，因此不经过 `resolve_within`；默认值
        // 与 `lib.rs` 的候选之一一致，指向仓库内随源码分发的模型。
        let models = match values.get("models") {
            Some(value) => PathBuf::from(value),
            None => PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("resources")
                .join("models"),
        };

        let chunk_format = match values.get("chunk-format").map(String::as_str) {
            Some("v1") => Some(false),
            Some("v2") => Some(true),
            Some(other) => return Err(format!("--chunk-format 只接受 v1/v2，收到 {other:?}")),
            None => None,
        };

        Ok(Self {
            command,
            workspace,
            repository,
            models,
            result: values.get("result").map(PathBuf::from),
            marker: values.get("marker").map(PathBuf::from),
            cancel_on_stdin,
            peak_memory: values.get("peak-memory").map(PathBuf::from),
            chunk_format,
            values,
        })
    }

    fn value(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    fn require(&self, key: &str) -> Result<&str, String> {
        self.value(key).ok_or_else(|| format!("缺少 --{key}"))
    }

    /// 逗号分隔的标识/路径列表（`--ids`）。空值返回空表。
    fn id_list(&self) -> Vec<String> {
        self.value("ids")
            .map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    }

    /// 解析一个必须位于工作区之内的路径参数，并校验其确实在工作区内。
    fn path_within(&self, key: &str, label: &str) -> Result<PathBuf, String> {
        resolve_within(&self.workspace, Path::new(self.require(key)?), label)
    }

    /// 逗号分隔的路径列表（`--paths`）；每一项都必须位于工作区之内，与 `--source`
    /// 同样是无头进程的安全属性。空值返回空表。
    fn path_list(&self, key: &str, label: &str) -> Result<Vec<String>, String> {
        let Some(value) = self.value(key) else {
            return Ok(Vec::new());
        };
        value
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(|item| {
                resolve_within(&self.workspace, Path::new(item), label)
                    .map(|path| path.to_string_lossy().into_owned())
            })
            .collect()
    }

    /// 与 Tauri 侧同样可无 Tauri 构造的状态装配：仓库路径来自命令行，设置与日志
    /// 目录只用于填满字段，无头进程不会调用依赖它们的命令。`--chunk-format`
    /// 覆盖本进程新写入的 ChunkFile 格式，缺省跟随设置默认（v2）。
    fn app_state(&self) -> AppState {
        AppState::for_headless_repository(
            &self.repository,
            &self.workspace.join(".headless"),
            self.chunk_format.unwrap_or(true),
        )
    }

    fn backup_state(&self) -> BackupState {
        BackupState::default()
    }

    /// 认证命令的路径授权作用域：只允许 `--workspace` 之下的路径。
    fn authorization_scope(&self) -> HeadlessScope<'_> {
        HeadlessScope {
            workspace: &self.workspace,
        }
    }

    /// 与 Tauri 侧同样可无 Tauri 构造的认证状态：模型目录来自命令行或默认值，
    /// TrustMark 引擎按需惰性加载，与 GUI 走同一份 `AuthenticityState`。
    fn authenticity_state(&self) -> AuthenticityState {
        AuthenticityState::new(self.models.clone())
    }

    /// 布尔选项（`true`/`false`/`1`/`0`/`yes`/`no`）；缺省为 `false`。
    fn flag(&self, key: &str) -> Result<bool, String> {
        let Some(value) = self.value(key) else {
            return Ok(false);
        };
        match value.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" => Ok(true),
            "false" | "0" | "no" => Ok(false),
            other => Err(format!("--{key} 只接受 true/false，收到 {other:?}")),
        }
    }

    /// 数值选项；缺省返回 `None`（由调用方决定默认值）。
    fn number<T: std::str::FromStr>(&self, key: &str) -> Result<Option<T>, String> {
        match self.value(key) {
            Some(value) => value
                .trim()
                .parse::<T>()
                .map(Some)
                .map_err(|_| format!("--{key} 取值无效：{value:?}")),
            None => Ok(None),
        }
    }

    /// `--regions "x,y,w,h;x,y,w,h"`：归一化坐标的矩形列表，分号分隔。空值返回空表。
    fn regions(&self, key: &str) -> Result<Vec<authenticity::NormalizedRegion>, String> {
        let Some(value) = self.value(key) else {
            return Ok(Vec::new());
        };
        let mut regions = Vec::new();
        for group in value.split(';') {
            let group = group.trim();
            if group.is_empty() {
                continue;
            }
            let parts = group.split(',').map(str::trim).collect::<Vec<_>>();
            if parts.len() != 4 {
                return Err(format!(
                    "--{key} 的每个区域必须形如 x,y,w,h，收到 {group:?}"
                ));
            }
            let mut numbers = [0.0_f32; 4];
            for (slot, text) in numbers.iter_mut().zip(parts) {
                *slot = text
                    .parse::<f32>()
                    .map_err(|_| format!("--{key} 的坐标无效：{text:?}"))?;
            }
            regions.push(authenticity::NormalizedRegion {
                x: numbers[0],
                y: numbers[1],
                width: numbers[2],
                height: numbers[3],
            });
        }
        Ok(regions)
    }
}

/// 无头进程的路径授权作用域：只允许 `--workspace` 之下的路径。
///
/// 它**不是**「跳过检查」的开关——保留「路径必须被显式授权」的语义，只把授权来源
/// 从文件选择器换成命令行显式声明的根目录。因此无头进程无法读写工作区之外的路径，
/// 这条属性对认证命令与其它命令一致。
struct HeadlessScope<'a> {
    workspace: &'a Path,
}

impl PathAuthorization for HeadlessScope<'_> {
    fn is_path_authorized(&self, path: &Path) -> bool {
        resolve_within(self.workspace, path, "授权路径").is_ok()
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
///   由进度回调驱动的阶段（`progress_stage`）只在阶段名切换时写，不会逐块追加。
/// - `--cancel-on-stdin`：每个**交互检查点**先写 marker，再阻塞等待 stdin 的一行判定
///   （`cancel` / `continue`，EOF 视为取消）。因此测试可以**逐检查点**决定取消时机，
///   不存在「读到 marker 时进程已经跑过下一个检查点」的竞态。
///
/// 检查点只出现在显式边界（提交的 5 处、恢复/精简/检查点各自的链与发布边界等），
/// 分块主循环走的是静默观察（[`CancelToken::observed`]），因此检查点总数与分块数
/// 无关、marker 文件也不会随处理字节数膨胀。静默层读的正是这里的 `cancelled` 标志，
/// 所以边界判定的取消仍会被热循环立刻看到。
///
/// 两种模式都只通过文件与 stdin 干预被测进程，不触碰任何内部状态。
struct Interlock {
    marker: Option<Mutex<File>>,
    stdin: Option<Mutex<BufReader<std::io::Stdin>>>,
    index: AtomicUsize,
    cancelled: AtomicBool,
    last_stage: Mutex<String>,
    /// 上次写进标记文件的进度阶段名。进度回调逐块触发，标记行只在阶段切换时写。
    written_stage: Mutex<String>,
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
            written_stage: Mutex::new(String::new()),
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

    /// 由进度回调驱动的阶段：标记行只在阶段名切换时写。
    ///
    /// 进度回调在热路径上逐块触发（一次大文件恢复可达十万次），逐次写标记行会
    /// 让标记文件随处理字节数膨胀，而压测的轮询每次都要整文件重读。`last_stage`
    /// 仍每块刷新，检查点行因此带着最新进度。
    fn progress_stage(&self, label: &str, current: u64, total: u64) {
        if let Ok(mut stage) = self.last_stage.lock() {
            *stage = format!("{label} {current}/{total}");
        }
        let changed = match self.written_stage.lock() {
            Ok(mut written) => {
                if *written == label {
                    false
                } else {
                    *written = label.to_owned();
                    true
                }
            }
            Err(_) => false,
        };
        if changed {
            self.write_line(&format!("stage {label} {current}/{total}"));
        }
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

/// 无头入口的 context 装配：与 GUI 共用 `OperationCoordinator` 与 use-case，
/// 只把 Tauri 事件出口换成 no-op，并把进度写回 marker 闸门。
struct HeadlessRuntime {
    coordinator: OperationCoordinator,
    cancel: CancelToken,
}

impl HeadlessRuntime {
    fn new(options: &Options, interlock: &'static Interlock) -> Self {
        let coordinator = OperationCoordinator::new(
            options.app_state(),
            options.backup_state(),
            Arc::new(NoopEventSink),
        )
        .with_observer(Arc::new(HeadlessObserver { interlock }));
        let cancel = CancelToken::with_layers(
            // 静默层：热循环只读「取消是否已被判定」，不写 marker、不阻塞。
            move || interlock.cancelled(),
            // 交互层：显式边界处的检查点，写一行 marker 并在带闸门时等一行判定。
            move || interlock.checkpoint(),
        );
        Self {
            coordinator,
            cancel,
        }
    }

    fn run<T>(
        &self,
        domain: OperationDomain,
        operation: impl FnOnce(&CommandContext) -> Result<T, CommandError>,
    ) -> Result<T, CommandError> {
        self.coordinator
            .execute_with(domain, self.cancel.clone(), operation)
    }
}

struct HeadlessObserver {
    interlock: &'static Interlock,
}

impl OperationObserver for HeadlessObserver {
    fn progress(&self, label: &str, current: u64, total: u64) {
        self.interlock.progress_stage(label, current, total);
    }
}

fn dispatch(options: &Options, runtime: &HeadlessRuntime) -> Result<Value, String> {
    let outcome = match options.command.as_str() {
        "init-repository" => init_repository(options),
        "create-artwork" => create_artwork(options, runtime),
        "create-group" => create_group(options, runtime),
        "move-node" => move_node(options, runtime),
        "trash-node" => trash_node(options, runtime),
        "empty-trash" => empty_trash(runtime),
        "list-tree" => list_tree(runtime),
        "search" => search(options, runtime),
        "create-branch" => create_branch(options, runtime),
        "delete-branch" => delete_branch(options, runtime),
        "list-history" => list_history(options, runtime),
        "create-board" => create_board(options, runtime),
        "import-board-images" => import_board_images(options, runtime),
        "create-board-stroke" => create_board_stroke(options, runtime),
        "create-board-strokes" => create_board_strokes(options, runtime),
        "read-board-strokes" => read_board_strokes(options, runtime),
        "scrub-board-dds" => scrub_board_dds(runtime),
        "commit" => commit(options, runtime),
        "restore" => restore_node(options, runtime),
        "compact" => compact_node(options, runtime),
        "checkpoint" => checkpoint(options, runtime),
        "scrub" => scrub(runtime),
        "verify" => verify(options),
        "cleanup" => cleanup_queue(options, runtime),
        "scan-unreferenced" => scan_unreferenced(runtime),
        "cleanup-unreferenced" => cleanup_unreferenced(options, runtime),
        "scheduler-scan" => scheduler_scan(runtime),
        "repository-backup" => repository_backup(options, runtime),
        "enter-publication" => enter_publication(options, runtime),
        "publish" => publish(options, runtime),
        "cancel-publication" => cancel_publication(options, runtime),
        "decode-authenticity" => decode_authenticity(options, runtime),
        other => Err(CommandError::invalid_input(format!("未知子命令：{other}"))),
    };
    outcome.map_err(|error: CommandError| error.message)
}

fn init_repository(options: &Options) -> Result<Value, CommandError> {
    // 建库也是写操作：先让目录存在并取得 owner lock，避免两个进程同时初始化。
    if !options.repository.exists() {
        std::fs::create_dir_all(&options.repository)
            .map_err(|error| CommandError::from(format!("无法创建作品仓库目录：{error}")))?;
    }
    let _owner = repository_lock::RepositoryOwnerLock::acquire(
        &options.repository,
        "headless",
        "init-repository",
    )
    .map_err(|error| CommandError::from(error.user_message()))?;
    library::initialize(&options.repository).map_err(CommandError::from)?;
    Ok(Value::Null)
}

fn create_artwork(options: &Options, runtime: &HeadlessRuntime) -> Result<Value, CommandError> {
    let title = options.require("title").map_err(CommandError::from)?;
    let branch_title = options
        .require("branch-title")
        .map_err(CommandError::from)?;
    let source = options
        .path_within("source", "分支工作文件")
        .map_err(CommandError::from)?;
    let created = runtime.run(OperationDomain::Library, |context| {
        usecases::library::create_artwork_created(
            context,
            options.value("parent"),
            title,
            branch_title,
            &source,
        )
    })?;
    Ok(json!({
        "artworkId": created.artwork_id,
        "branchId": created.branch_id,
    }))
}

fn create_group(options: &Options, runtime: &HeadlessRuntime) -> Result<Value, CommandError> {
    let title = options.require("title").map_err(CommandError::from)?;
    let tree = runtime.run(OperationDomain::Library, |context| {
        usecases::library::create_group(
            context,
            options.value("parent").map(str::to_owned),
            title.to_owned(),
        )
    })?;
    Ok(json!({
        "groupCount": tree.group_count,
        "artworkCount": tree.artwork_count,
    }))
}

fn move_node(options: &Options, runtime: &HeadlessRuntime) -> Result<Value, CommandError> {
    let ids = options.id_list();
    if ids.is_empty() {
        return Err(CommandError::invalid_input("move-node 需要 --ids"));
    }
    let index = match options.value("index") {
        Some(value) => value.parse::<u32>().map_err(|_| {
            CommandError::invalid_input(format!("--index 必须是整数，收到 {value:?}"))
        })?,
        None => 0,
    };
    let request = library::MoveLibraryNodesRequest {
        ids,
        parent_id: options.value("parent").map(str::to_owned),
        index,
    };
    let tree = runtime.run(OperationDomain::Library, |context| {
        usecases::library::move_nodes(context, request)
    })?;
    Ok(json!({
        "groupCount": tree.group_count,
        "artworkCount": tree.artwork_count,
    }))
}

fn trash_node(options: &Options, runtime: &HeadlessRuntime) -> Result<Value, CommandError> {
    let ids = options.id_list();
    if ids.is_empty() {
        return Err(CommandError::invalid_input("trash-node 需要 --ids"));
    }
    let tree = runtime.run(OperationDomain::Library, |context| {
        usecases::library::trash_nodes(context, ids)
    })?;
    Ok(json!({
        "groupCount": tree.group_count,
        "artworkCount": tree.artwork_count,
    }))
}

fn empty_trash(runtime: &HeadlessRuntime) -> Result<Value, CommandError> {
    let report = runtime.run(OperationDomain::Library, |context| {
        usecases::library::empty_trash(context)
    })?;
    serde_json::to_value(&report)
        .map_err(|error| CommandError::internal(format!("无法序列化清理结果：{error}")))
}

fn list_tree(runtime: &HeadlessRuntime) -> Result<Value, CommandError> {
    let tree = runtime.run(OperationDomain::Library, |context| {
        usecases::library::list_tree(context)
    })?;
    let mut value = serde_json::to_value(&tree)
        .map_err(|error| CommandError::internal(format!("无法序列化作品树：{error}")))?;
    if let Some(object) = value.as_object_mut() {
        object.insert("nodeCount".into(), json!(count_tree_nodes(&tree.nodes)));
    }
    Ok(value)
}

/// 递归统计作品树节点总数（含嵌套子节点）。
fn count_tree_nodes(nodes: &[library::LibraryNode]) -> usize {
    nodes
        .iter()
        .map(|node| 1 + count_tree_nodes(&node.children))
        .sum()
}

fn search(options: &Options, runtime: &HeadlessRuntime) -> Result<Value, CommandError> {
    let query = options.require("query").map_err(CommandError::from)?;
    let results = runtime.run(OperationDomain::Library, |context| {
        usecases::library::search(context, query.to_owned())
    })?;
    let matches = results
        .iter()
        .map(|result| json!({ "id": result.id, "kind": result.kind, "title": result.title }))
        .collect::<Vec<_>>();
    Ok(json!({ "count": results.len(), "results": matches }))
}

fn create_branch(options: &Options, runtime: &HeadlessRuntime) -> Result<Value, CommandError> {
    let artwork_id = options.require("artwork").map_err(CommandError::from)?;
    let from_history_id = options.require("history").map_err(CommandError::from)?;
    let title = options
        .require("branch-title")
        .map_err(CommandError::from)?;
    let source = options
        .path_within("source", "分支工作文件")
        .map_err(CommandError::from)?;
    let branch_id = runtime.run(OperationDomain::History, |context| {
        usecases::history::create_branch_created(
            context,
            artwork_id,
            from_history_id,
            title,
            &source,
        )
    })?;
    Ok(json!({ "branchId": branch_id }))
}

fn delete_branch(options: &Options, runtime: &HeadlessRuntime) -> Result<Value, CommandError> {
    let branch_id = options.require("branch").map_err(CommandError::from)?;
    let history = runtime.run(OperationDomain::History, |context| {
        usecases::history::delete_branch(context, branch_id)
    })?;
    Ok(json!({ "artworkId": history.artwork_id }))
}

fn list_history(options: &Options, runtime: &HeadlessRuntime) -> Result<Value, CommandError> {
    let artwork_id = options.require("artwork").map_err(CommandError::from)?;
    let history = runtime.run(OperationDomain::History, |context| {
        usecases::history::list(context, artwork_id)
    })?;
    Ok(json!({
        "branchCount": history.branches.len(),
        "nodeCount": history.nodes.len(),
    }))
}

/// 创建画板。与 GUI 的 `create_pin_board` 走同一领域函数（`repository::create_board`），
/// 只多返回一个修订号供后续导入做并发校验——GUI 侧修订号由前端在加载画板时取得。
fn create_board(options: &Options, runtime: &HeadlessRuntime) -> Result<Value, CommandError> {
    let artwork_id = options.require("artwork").map_err(CommandError::from)?;
    let name = options.require("title").map_err(CommandError::from)?;
    let (summary, board) = runtime.run(OperationDomain::PinBoard, |context| {
        usecases::pin_board::create_board_context(context, artwork_id, name)
    })?;
    Ok(json!({
        "boardId": summary.board_id,
        "revision": board.revision,
        "name": board.name,
    }))
}

/// 按路径导入画板图片。与 GUI 的 `import_pin_board_images` 走同一领域函数
/// （`repository::import_images`）。布局参数（中心与间距）取固定默认值——无头进程
/// 不驱动界面，图片摆放位置不影响 DDS 落盘与完整性检查。
fn import_board_images(
    options: &Options,
    runtime: &HeadlessRuntime,
) -> Result<Value, CommandError> {
    let board_id = options
        .require("board")
        .map_err(CommandError::from)?
        .parse::<i64>()
        .map_err(|_| {
            CommandError::invalid_input(format!(
                "--board 必须是整数，收到 {:?}",
                options.value("board")
            ))
        })?;
    let revision = options.require("revision").map_err(CommandError::from)?;
    let paths = options
        .path_list("paths", "画板导入源图")
        .map_err(CommandError::from)?;
    if paths.is_empty() {
        return Err(CommandError::invalid_input(
            "import-board-images 需要 --paths",
        ));
    }
    let (view, image_ids) = runtime.run(OperationDomain::PinBoard, |context| {
        usecases::pin_board::import_images(
            context,
            board_id,
            &paths,
            0.0,
            0.0,
            0.0,
            revision,
            |_current, _total| {},
            // 无头进程没有取消入口：一次调用跑完。
            || false,
        )
    })?;
    let view = serde_json::to_value(&view)
        .map_err(|error| CommandError::internal(format!("无法序列化画板视图：{error}")))?;
    let revision = view
        .get("revision")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let item_count = view
        .get("items")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    Ok(json!({
        "imageIds": image_ids,
        "itemCount": item_count,
        "revision": revision,
    }))
}

/// 双向检查画板 DDS：记录 → 文件（缺失/损坏）与文件 → 记录（孤儿）。缺失与损坏
/// **报告不失败**——命令照常成功返回，计数交给调用方判断。走 GUI 完整性检查第三段
/// 的同一领域函数 `pin_board::scrub::scrub_board_dds`，逐条响应取消。
fn scrub_board_dds(runtime: &HeadlessRuntime) -> Result<Value, CommandError> {
    let report = runtime.run(OperationDomain::Maintenance, |context| {
        usecases::pin_board::scrub_board_dds(context)
    })?;
    Ok(json!({
        // 契约键沿用 `images`；领域报告的 `bitmaps` 只统计有位图载荷的条目。
        "images": report.bitmaps,
        "missing": report.missing,
        "corrupt": report.corrupt,
        "orphans": report.orphans,
    }))
}

/// 从画板视图里取修订号。`PinBoardView` 的字段对适配器不可见，序列化后按键取值，
/// 与 GUI 拿到的 DTO 是同一份口径（`revision`）。
fn revision_of(view: &impl Serialize) -> String {
    serde_json::to_value(view)
        .ok()
        .and_then(|value| {
            value
                .get("revision")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_default()
}

/// 创建一笔笔迹。几何 BLOB 从 `--geometry` 指向的原始文件读入（`--geometry-hex`
/// 供边界用例直接给字节），与 GUI 的 `create_pin_board_stroke` 走同一条领域函数。
fn create_board_stroke(
    options: &Options,
    runtime: &HeadlessRuntime,
) -> Result<Value, CommandError> {
    let board_id = board_id_option(options)?;
    let revision = options.require("revision").map_err(CommandError::from)?;
    let tool = options
        .number::<u8>("tool")
        .map_err(CommandError::from)?
        .unwrap_or(0);
    let geometry = geometry_option(options)?;
    let stroke_width = options
        .number::<f64>("stroke-width")
        .map_err(CommandError::from)?
        .unwrap_or(6.0);
    let width = options
        .number::<f64>("width")
        .map_err(CommandError::from)?
        .unwrap_or(100.0);
    let height = options
        .number::<f64>("height")
        .map_err(CommandError::from)?
        .unwrap_or(100.0);
    let center_x = options
        .number::<f64>("center-x")
        .map_err(CommandError::from)?
        .unwrap_or(0.0);
    let center_y = options
        .number::<f64>("center-y")
        .map_err(CommandError::from)?
        .unwrap_or(0.0);
    let color = options.value("color").unwrap_or("#e5484d");

    let (view, item_ids) = runtime.run(OperationDomain::PinBoard, |context| {
        usecases::pin_board::create_stroke(
            context,
            board_id,
            tool,
            color,
            stroke_width,
            width,
            height,
            &geometry,
            center_x,
            center_y,
            revision,
        )
    })?;
    Ok(json!({
        "itemId": item_ids.first().copied().unwrap_or_default(),
        "revision": revision_of(&view),
    }))
}

/// 批量创建笔迹：`--spec` 指向的二进制清单一次读入、一次事务写完。
///
/// 逐条调用 `create-board-stroke` 要起 N 个进程，量的就不再是写入成本而是进程启动
/// 成本；而一次手势只有一笔，GUI 路径也不会有这种批量。清单布局（小端）：
///
/// ```text
/// u32 count
/// 每条：u8 tool, f32 strokeWidth, f32 width, f32 height, f32 centerX, f32 centerY,
///       u32 geometryLen, bytes
/// ```
fn create_board_strokes(
    options: &Options,
    runtime: &HeadlessRuntime,
) -> Result<Value, CommandError> {
    let board_id = board_id_option(options)?;
    let revision = options.require("revision").map_err(CommandError::from)?;
    let color = options.value("color").unwrap_or("#e5484d");
    let spec_path = options
        .path_within("spec", "笔迹清单")
        .map_err(CommandError::from)?;
    let spec = std::fs::read(&spec_path)
        .map_err(|error| CommandError::invalid_input(format!("无法读取 --spec：{error}")))?;
    let records = parse_stroke_spec(&spec)?;
    let strokes = records
        .iter()
        .map(|record| crate::pin_board::repository::StrokeInsert {
            tool: record.tool,
            color,
            stroke_width: record.stroke_width,
            width: record.width,
            height: record.height,
            geometry: &record.geometry,
            center_x: record.center_x,
            center_y: record.center_y,
        })
        .collect::<Vec<_>>();

    let started = Instant::now();
    let (view, item_ids) = runtime.run(OperationDomain::PinBoard, |context| {
        usecases::pin_board::create_strokes(context, board_id, &strokes, revision)
    })?;
    Ok(json!({
        "itemIds": item_ids,
        "revision": revision_of(&view),
        "count": item_ids.len(),
        "elapsedMs": started.elapsed().as_secs_f64() * 1000.0,
    }))
}

/// 读取整板笔迹的二进制载荷，回报条目数、字节数与 SHA-256。
///
/// 不把整份载荷塞进 JSON（5000 笔约 7 MB，转成十六进制再翻一倍）：调用方拿摘要与
/// 自己按同一布局拼出的期望载荷比对，既验证了字节布局，也不用搬运数据。
fn read_board_strokes(options: &Options, runtime: &HeadlessRuntime) -> Result<Value, CommandError> {
    let board_id = board_id_option(options)?;
    let started = Instant::now();
    let payload = runtime.run(OperationDomain::PinBoard, |context| {
        usecases::pin_board::read_strokes(context, board_id)
    })?;
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    let count = payload
        .get(0..4)
        .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap_or_default()))
        .unwrap_or_default();
    Ok(json!({
        "count": count,
        "bytes": payload.len(),
        "digest": hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&payload)),
        "elapsedMs": elapsed_ms,
    }))
}

/// 解析 `--board`：所有画板子命令共用同一条错误文案。
fn board_id_option(options: &Options) -> Result<i64, CommandError> {
    options
        .require("board")
        .map_err(CommandError::from)?
        .parse::<i64>()
        .map_err(|_| {
            CommandError::invalid_input(format!(
                "--board 必须是整数，收到 {:?}",
                options.value("board")
            ))
        })
}

/// 几何 BLOB 的两种给法：`--geometry` 读原始文件（压力测试的常规用法），
/// `--geometry-hex` 直接给十六进制（边界用例不必落盘）。
fn geometry_option(options: &Options) -> Result<Vec<u8>, CommandError> {
    match options.value("geometry") {
        Some(_) => {
            let path = options
                .path_within("geometry", "笔迹几何")
                .map_err(CommandError::from)?;
            std::fs::read(&path).map_err(|error| {
                CommandError::invalid_input(format!("无法读取 --geometry：{error}"))
            })
        }
        None => {
            let hex = options
                .require("geometry-hex")
                .map_err(CommandError::from)?;
            hex::decode(hex.trim()).map_err(|error| {
                CommandError::invalid_input(format!("--geometry-hex 无效：{error}"))
            })
        }
    }
}

/// 批量清单里的一条记录。
struct StrokeSpecRecord {
    tool: u8,
    stroke_width: f64,
    width: f64,
    height: f64,
    center_x: f64,
    center_y: f64,
    geometry: Vec<u8>,
}

/// 解析批量清单。长度不符、条目数越界一律明确拒绝，不做宽容截断。
fn parse_stroke_spec(spec: &[u8]) -> Result<Vec<StrokeSpecRecord>, CommandError> {
    let invalid = |message: &str| CommandError::invalid_input(message.to_owned());
    if spec.len() < 4 {
        return Err(invalid("--spec 不完整"));
    }
    let count = u32::from_le_bytes(spec[0..4].try_into().unwrap_or_default()) as usize;
    if count == 0 || count > crate::pin_board::contracts::MAX_BOARD_ITEMS {
        return Err(invalid("--spec 的条目数超出范围"));
    }
    let mut offset = 4;
    let mut records = Vec::with_capacity(count);
    for _ in 0..count {
        // u8 tool + 5 × f32 + u32 geometryLen
        const HEADER: usize = 1 + 4 * 5 + 4;
        if offset + HEADER > spec.len() {
            return Err(invalid("--spec 不完整"));
        }
        let tool = spec[offset];
        let float_at =
            |at: usize| f32::from_le_bytes(spec[at..at + 4].try_into().unwrap_or_default()) as f64;
        let stroke_width = float_at(offset + 1);
        let width = float_at(offset + 5);
        let height = float_at(offset + 9);
        let center_x = float_at(offset + 13);
        let center_y = float_at(offset + 17);
        let geometry_len = u32::from_le_bytes(
            spec[offset + 21..offset + 25]
                .try_into()
                .unwrap_or_default(),
        ) as usize;
        offset += HEADER;
        if geometry_len > spec.len() - offset {
            return Err(invalid("--spec 的几何长度越界"));
        }
        records.push(StrokeSpecRecord {
            tool,
            stroke_width,
            width,
            height,
            center_x,
            center_y,
            geometry: spec[offset..offset + geometry_len].to_vec(),
        });
        offset += geometry_len;
    }
    if offset != spec.len() {
        return Err(invalid("--spec 有多余字节"));
    }
    Ok(records)
}

fn commit(options: &Options, runtime: &HeadlessRuntime) -> Result<Value, CommandError> {
    let branch_id = options.require("branch").map_err(CommandError::from)?;
    let note = options.value("note").unwrap_or_default();
    let commit_kind = options.value("commit-kind").unwrap_or("manual");
    let result = runtime.run(OperationDomain::Backup, |context| {
        usecases::backup::commit(context, branch_id, note, commit_kind, false)
    })?;
    Ok(json!({
        "created": result.created,
        "unchanged": result.unchanged,
        "historyId": result.history_id,
    }))
}

fn restore_node(options: &Options, runtime: &HeadlessRuntime) -> Result<Value, CommandError> {
    let history_id = options.require("history").map_err(CommandError::from)?;
    let output = options
        .path_within("output", "恢复输出")
        .map_err(CommandError::from)?;
    let output = output.to_string_lossy().into_owned();
    let path = runtime.run(OperationDomain::Backup, |context| {
        usecases::backup::restore(context, history_id, &output)
    })?;
    Ok(json!({ "outputPath": path }))
}

fn compact_node(options: &Options, runtime: &HeadlessRuntime) -> Result<Value, CommandError> {
    let history_id = options.require("history").map_err(CommandError::from)?;
    runtime.run(OperationDomain::Backup, |context| {
        usecases::backup::compact(context, history_id)
    })?;
    Ok(Value::Null)
}

fn checkpoint(options: &Options, runtime: &HeadlessRuntime) -> Result<Value, CommandError> {
    let history_id = options.require("history").map_err(CommandError::from)?;
    runtime.run(OperationDomain::Backup, |context| {
        usecases::backup::set_checkpoint(context, history_id, true)
    })?;
    Ok(Value::Null)
}

fn scrub(runtime: &HeadlessRuntime) -> Result<Value, CommandError> {
    // 与 `scrub_repository_integrity` 的两段一致：先历史链逐块校验，再认证受控文件
    // （最终成品与认证仓库副本的摘要、C2PA 声明核对）。无认证记录的仓库第二段为 0/0。
    let (nodes, final_artifacts, certification_records) = runtime
        .run(OperationDomain::Maintenance, |context| {
            usecases::backup::scrub_history_and_controlled(context)
        })?;
    Ok(json!({
        "historyNodes": nodes,
        "finalArtifacts": final_artifacts,
        "certificationRecords": certification_records,
    }))
}

fn verify(options: &Options) -> Result<Value, CommandError> {
    // `open_existing` = 迁移到当前版本 + `integrity_check` + 外键检查 + 语义校验
    // （UUID / 仓库相对路径 / SHA-256 全表）。重开可用即以此为准。
    library::open_existing(&options.repository).map_err(CommandError::from)?;
    Ok(Value::Null)
}

/// 只读探针：调度器候选扫描的成本（`history::list_scheduled`，按 `b.id` 全量列举）。
///
/// 它与任何取消/进度检查点无关，也不写 `last_attempt_ms`，与 `list_pending_file_cleanup`
/// 同属「共享读租约、不取运行锁」的只读查询。用途是给 0.3 阶段 5「deadline 优先的候选
/// 生成」留一个重构前的可比基线。
fn scheduler_scan(runtime: &HeadlessRuntime) -> Result<Value, CommandError> {
    let started = Instant::now();
    let branches = runtime.run(OperationDomain::Backup, |context| {
        usecases::backup::list_scheduled(context)
    })?;
    Ok(json!({
        "branchCount": branches.len(),
        "scanMs": started.elapsed().as_millis(),
    }))
}

fn cleanup_queue(options: &Options, runtime: &HeadlessRuntime) -> Result<Value, CommandError> {
    let ids = options.id_list();
    let report = runtime.run(OperationDomain::Cleanup, |context| {
        usecases::cleanup::retry_pending(context, &ids)
    })?;
    serde_json::to_value(&report)
        .map_err(|error| CommandError::internal(format!("无法序列化清理结果：{error}")))
}

/// 扫描未引用文件（崩溃孤儿、手工复制或历史迁移遗留）。只报告、不删除——与 GUI 的
/// `scan_repository_unreferenced` 走同一领域函数；确认清理由 `cleanup-unreferenced` 完成。
fn scan_unreferenced(runtime: &HeadlessRuntime) -> Result<Value, CommandError> {
    let candidates = runtime.run(OperationDomain::Cleanup, |context| {
        usecases::cleanup::scan_unreferenced(context)
    })?;
    let values = candidates
        .iter()
        .map(|candidate| {
            json!({
                "path": candidate.path,
                "byteSize": candidate.byte_size,
                "reason": candidate.reason,
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({ "count": candidates.len(), "candidates": values }))
}

/// 确认清理扫描候选（`--ids` 为仓库相对路径）。入队后单遍重放，幂等。
fn cleanup_unreferenced(
    options: &Options,
    runtime: &HeadlessRuntime,
) -> Result<Value, CommandError> {
    let paths = options.id_list();
    let report = runtime.run(OperationDomain::Cleanup, |context| {
        usecases::cleanup::cleanup_unreferenced(context, &paths)
    })?;
    serde_json::to_value(&report)
        .map_err(|error| CommandError::internal(format!("无法序列化清理结果：{error}")))
}

fn repository_backup(options: &Options, runtime: &HeadlessRuntime) -> Result<Value, CommandError> {
    let destination = options
        .path_within("destination", "备份保存目录")
        .map_err(CommandError::from)?;
    authenticity::ensure_dialog_authorized(
        &options.authorization_scope(),
        &destination,
        "备份保存目录",
    )
    .map_err(|error| CommandError::from_legacy(error.to_string()))?;
    let report = runtime.run(OperationDomain::Maintenance, |context| {
        usecases::backup::create_repository_backup(context, &destination)
    })?;
    serde_json::to_value(&report)
        .map_err(|error| CommandError::internal(format!("无法序列化备份结果：{error}")))
}

/// 进入发布状态。与 GUI 的 `enter_branch_publication` 走同一批领域调用：
/// `branch_head` → `backup::ensure_checkpoint` → `store_final_artifact` →
/// `get_publication`，在同一把运行锁下执行。最终成品路径经无头作用域授权，与其它
/// 命令一致地只允许工作区之内的路径。
fn enter_publication(options: &Options, runtime: &HeadlessRuntime) -> Result<Value, CommandError> {
    let branch_id = options.require("branch").map_err(CommandError::from)?;
    let artifact = options
        .path_within("artifact", "最终成品")
        .map_err(CommandError::from)?;
    authenticity::ensure_dialog_authorized(&options.authorization_scope(), &artifact, "最终成品")
        .map_err(|error| CommandError::from_legacy(error.to_string()))?;
    let request = authenticity::EnterPublicationRequest {
        branch_id: branch_id.to_owned(),
        artifact_path: artifact.to_string_lossy().into_owned(),
    };
    let state = options.authenticity_state();
    let publication = runtime.run(OperationDomain::Authenticity, |context| {
        usecases::authenticity::enter_publication(context, &state, request)
    })?;
    serde_json::to_value(&publication)
        .map_err(|error| CommandError::internal(format!("无法序列化发布状态：{error}")))
}

/// 认证签名发布。与 GUI 的 `publish_branch_artifact` 走同一领域函数
/// `authenticity::publish`（渲染 → TrustMark 编码 → JPEG 编码 → C2PA 签名），
/// 在同一把运行锁与认证操作锁下执行。取消检查点在流水线内部（headless-only 门控），
/// 因此 `--cancel-on-stdin` 的闸门能精确命中渲染 / 编码 / 签名三个阶段。
fn publish(options: &Options, runtime: &HeadlessRuntime) -> Result<Value, CommandError> {
    let branch_id = options.require("branch").map_err(CommandError::from)?;
    let output = options
        .path_within("output", "发布输出路径")
        .map_err(CommandError::from)?;
    let certificate = options
        .path_within("certificate", "证书链")
        .map_err(CommandError::from)?;
    let key_path = options
        .path_within("key", "私钥")
        .map_err(CommandError::from)?;
    let scope = options.authorization_scope();
    authenticity::ensure_dialog_authorized(&scope, &output, "发布输出路径")
        .map_err(|error| CommandError::from_legacy(error.to_string()))?;
    authenticity::ensure_dialog_authorized(&scope, &certificate, "证书链")
        .map_err(|error| CommandError::from_legacy(error.to_string()))?;
    let private_key = std::fs::read_to_string(&key_path)
        .map_err(|error| CommandError::from_legacy(format!("无法读取私钥：{error}")))?;
    let config = authenticity::CertificationConfig {
        branch_id: branch_id.to_owned(),
        title: options
            .require("title")
            .map_err(CommandError::from)?
            .to_owned(),
        creator: options
            .require("creator")
            .map_err(CommandError::from)?
            .to_owned(),
        rights_statement: options.value("rights").unwrap_or_default().to_owned(),
        authentication_content: options.value("content").unwrap_or_default().to_owned(),
        trustmark_enabled: options.flag("trustmark").map_err(CommandError::from)?,
        certificate_path: certificate.to_string_lossy().into_owned(),
        signing_algorithm: options.value("algorithm").unwrap_or("es256").to_owned(),
        timestamp_url: options.value("timestamp-url").map(str::to_owned),
        jpeg_quality: options
            .number::<u8>("jpeg-quality")
            .map_err(CommandError::from)?
            .unwrap_or(90),
        background_color: options.value("background").unwrap_or("#FFFFFF").to_owned(),
        watermark_strength: options
            .number::<f32>("strength")
            .map_err(CommandError::from)?
            .unwrap_or(1.0),
        additional_regions: options.regions("regions").map_err(CommandError::from)?,
        updated_ms: 0,
    };
    let request = authenticity::PublishBranchRequest {
        branch_id: branch_id.to_owned(),
        output_path: output.to_string_lossy().into_owned(),
        private_key_pem: private_key,
        config,
        watermark_id: options.value("watermark-id").map(str::to_owned),
        preview_cache_token: options.value("preview-cache-token").map(str::to_owned),
    };
    let state = options.authenticity_state();
    let result = runtime.run(OperationDomain::Authenticity, |context| {
        usecases::authenticity::publish(context, &state, request)
    })?;
    serde_json::to_value(&result)
        .map_err(|error| CommandError::internal(format!("无法序列化发布结果：{error}")))
}

/// 取消发布并回收仓库内副本。与 GUI 的 `cancel_branch_publication` 走同一批调用：
/// `remove_artifact` 先按记录中的 SHA-256 核对内容，只把未变的文件登记进待清理项，
/// 提交后单遍 `cleanup::run`；被替换的文件留在仓库里交给垃圾扫描。首次导出的
/// JPG 是用户产物，不在回收范围（与 GUI 语义一致）。
fn cancel_publication(options: &Options, runtime: &HeadlessRuntime) -> Result<Value, CommandError> {
    let branch_id = options.require("branch").map_err(CommandError::from)?;
    let report = runtime.run(OperationDomain::Authenticity, |context| {
        usecases::authenticity::cancel_publication(context, branch_id)
    })?;
    serde_json::to_value(&report)
        .map_err(|error| CommandError::internal(format!("无法序列化清理结果：{error}")))
}

/// 回读认证。与 GUI 的 `decode_authenticity` 走同一领域函数 `authenticity::decode`：
/// C2PA 声明 + TrustMark 绑定 + 全库候选匹配。只读，走共享读租约。
fn decode_authenticity(
    options: &Options,
    runtime: &HeadlessRuntime,
) -> Result<Value, CommandError> {
    let input = options
        .path_within("input", "待识别图片")
        .map_err(CommandError::from)?;
    authenticity::ensure_dialog_authorized(&options.authorization_scope(), &input, "待识别图片")
        .map_err(|error| CommandError::from_legacy(error.to_string()))?;
    let request = authenticity::DecodeRequest {
        input_path: input.to_string_lossy().into_owned(),
        region: options
            .regions("region")
            .map_err(CommandError::from)?
            .into_iter()
            .next(),
    };
    let state = options.authenticity_state();
    let result = runtime.run(OperationDomain::Authenticity, |context| {
        usecases::authenticity::decode(context, &state, request)
    })?;
    serde_json::to_value(&result)
        .map_err(|error| CommandError::internal(format!("无法序列化识别结果：{error}")))
}
