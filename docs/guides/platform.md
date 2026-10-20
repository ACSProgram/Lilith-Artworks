# 可移植性规范（平台分支收口）

与 `validation.md`、`logging.md` 并列的 guide。2026-10-20 的跨平台调查显示本项目的
业务代码约八成平台无关；本规范把「平台分支只准出现在 `src-tauri/src/platform/`
内」从口头约定变成可引用、可检查的规则，使将来任何新平台（如 macOS）的移植成本
不再随业务代码的增长线性上涨。

**定位**：本规范生效后，项目只是「平台差异集中、加平台更容易」，**不等于**已经支持
macOS——打包、签名、自绘标题栏、剪贴板实现、素材板 WebGPU 实测等属于后续独立批次。

## 规则 1：平台分支的唯一入口

`src-tauri/src/platform/` 是产品代码中所有平台分支的唯一入口，按职责分四个子文件：

| 文件 | 职责 | 接口 |
| --- | --- | --- |
| `path.rs` | 路径表示、命名空间校验与等价性 | `display_path`、`path_namespace` / `PathNamespace`、`ensure_not_device_path`、`equivalence_key`、`source_path_key` |
| `shell.rs` | 文件管理器打开/定位 | `reveal_in_file_manager`、`open_directory`、`open_existing_directory` |
| `clipboard.rs` | 剪贴板文件路径 | `file_paths` |
| `process.rs` | 进程内存信息 | `peak_memory_bytes` |

业务文件里出现 `cfg(target_os)`、`cfg(windows)`、`cfg!(windows)`、`cfg(unix)` 或
`cfg_attr(...)` 即违规；由 Windows CI 的 grep 检查把关（见「规则 3」末尾）。

**业务约束不进平台层**：数量、体积、像素之类的上限（例如素材板导入的
`MAX_IMPORT_FILES`）是调用方的业务规则，平台接口返回该平台能拿到的全部结果，
由调用方按自己的上限截断——否则每加一个平台都要重新确认一遍业务上限。

对外接口按**业务需要**命名，不按 Windows 命名：例如等价性 key 叫
`equivalence_key` 而不是 `lowercase_path`——大小写折叠是 Windows 文件系统的实现
选择，不是业务语义；将来 macOS（默认不敏感、可配置）与 Linux（敏感）的差异由函数
内部消化并在注释说明。能力缺失必须显式（返回空列表 / `None` / 明确的「不支持」
错误），禁止静默假成功。除真实的能力差异外，错误文案跨平台逐字一致。

## 规则 2：新增平台分支的流程

1. 先在 `platform/` 找现成接口；找到就直接用，不要在业务代码里复制实现。
2. 没有则在 `platform/` 新增平台无关命名的接口 + 各平台实现（含能力缺失的显式
   空实现），并更新本文档的能力披露表。
3. 只有走「例外登记表」（规则 3）的条目才允许在 `platform/` 之外出现平台分支；
   新增例外必须同步更新登记表与 CI 检查脚本内的允许列表（两处一起改）。

## 规则 3：例外登记表

以下位置允许出现平台分支，每条注明保留理由：

| 位置 | 内容 | 保留理由 |
| --- | --- | --- |
| `src-tauri/src/main.rs:1` | `windows_subsystem` 属性 | Tauri 标准入口写法，跨平台编译器属性（本行首参是 `not(debug_assertions)`，检查模式要求 `cfg_attr(` 之后紧跟平台名，因此不会命中；登记以备模式扩展） |
| `src-tauri/Cargo.toml` 的 `[target.'cfg(windows)'.dependencies]` 段 | `windows-sys` 依赖声明 | 依赖必须按 target 分节，Cargo 的既定机制（grep 检查只扫 `src/`，登记以备扩展） |
| `src-tauri/src/repository_lock.rs` 的 `#[cfg(windows)]` 测试 | 跨进程锁的 Windows 大小写闸门测试 | 测试语义本身就是「Windows 大小写不敏感」，属平台测试 |
| `src-tauri/tests/stress_support/mod.rs` | CPU 周期计数（`QueryProcessCycleTime`）与 FILETIME | 发布前压力测试专用辅助代码（`tests/` 下编译，不进产品安装包）。跨平台口径：通过/失败判定与墙钟数据在任何平台照常工作，CPU 记账在非 Windows 平台显示「未测量」；基线比较本来就只在同一台机器、同一构建档、单线程采集下进行。若将来需要非 Windows 的 CPU 精确数据，在 `stress_support` **内部**补记账分支（如 Unix 的 `wait4`/`getrusage`），不触碰产品代码（grep 检查只扫 `src/`，登记以备扩展） |
| `src/shared/useRuntimeSubscription.ts:19-27` | `normalizeRepositoryKey`：前端事件 bridge 的仓库过滤 | 后端事件里的 `repositoryKey` 可能来自 canonicalize（Windows 上带 `\\?\` 前缀），前端订阅事件前对双方各做一次路径等价性归一（剥前缀、统一分隔符、去尾分隔符、小写），否则会漏收当前仓库的事件。保留理由：纯比较用途、已有测试、量极小，且 TS 无法复用 Rust 的 platform 模块；下沉后端等于改事件契约，收益不成比例。**同步要求**：本函数等于后端 `platform::path::display_path`（剥前缀）+ `platform::path::equivalence_key`（分隔符与大小写）再加「去掉结尾分隔符」，后端这两个函数任一改语义都要一并对齐这里（三处注释互相引用）；后端若将来改为在发布事件前归一 `repositoryKey`，则前端该函数连同测试一并删除 |

**机器把关**：`windows-ci.yml` 的「Check platform branches stay in platform/」步骤
用 grep 在 `src-tauri/src/` 下扫描 `cfg(...)`、`cfg!(...)` 与 `cfg_attr(...)` 三种
写法，平台名认 `windows` / `unix` / `target_os`；命中 `platform/` 与允许列表之外的
文件即失败。允许列表以本表为准，脚本内实现；新增例外时同步更新两处。

注意只扫 `src-tauri/src/`：`tests/stress_support/` 与 `Cargo.toml` 不在范围，登记在
本表是为了让「例外」这件事有唯一清单，而不是说它们被脚本拦着。

## 规则 4：打包与 CI 的「加平台清单」

以下文件天然按平台分文件配置，属既定形态，**不受规则 1 约束**；加新平台时逐一核对：

- `src-tauri/tauri.conf.json`：`bundle.targets`（现为 `["nsis"]`）与 icon（现为仅
  `.ico`）。
- `.github/workflows/windows-ci.yml` 与 `release.yml`：`runs-on: windows-latest`。
- `src-tauri/resources/`：图标格式（如 macOS 需要 `.icns`）。
- `docs/guides/release-policy.md`：签名口径（Authenticode 之外的平台签名另立条目）。

## 规则 5：能力差异披露

每个 platform 接口必须注明各平台现状；新平台实现时按下表核对，不允许出现「文档说
有、实现返回空」的漂移：

| 接口 | Windows | macOS | Linux |
| --- | --- | --- | --- |
| `path::display_path` | 剥离 `\\?\` 前缀 | 原样返回（路径不带该前缀） | 同 macOS |
| `path::path_namespace` / `ensure_not_device_path` | 全量实现（设备命名空间拒绝） | 走同一套词法判断，POSIX 路径不会命中 Windows 前缀，实际总是放行 | 同 macOS |
| `path::equivalence_key` | 统一分隔符 + 大小写折叠 | 统一分隔符；是否折叠大小写留待移植时按 macOS 文件系统语义决定 | 统一分隔符（保留大小写） |
| `path::source_path_key` | 只折叠大小写（不归一分隔符） | 原样返回 | 原样返回 |
| `shell::reveal_in_file_manager` | `explorer.exe /select,`（选中文件） | `open -R`（选中文件） | `xdg-open` 父目录（无选中语义） |
| `shell::open_directory` / `open_existing_directory` | `explorer.exe` | `open` | `xdg-open` |
| `clipboard::file_paths` | `CF_HDROP` 全量读取 | **空列表**（未实现） | **空列表**（未实现） |
| `process::peak_memory_bytes` | `GetProcessMemoryInfo` 峰值工作集 | **`None`**（未实现） | **`None`**（未实现） |

注：macOS/Linux 列写的是「当前实现的行为」与「移植时待补的点」，不是承诺。
