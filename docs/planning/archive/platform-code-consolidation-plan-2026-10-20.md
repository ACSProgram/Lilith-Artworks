# 平台代码集中与可移植性规范

状态：**已实施并归档**（2026-10-20）。代码与规范文档同批提交；自动验证通过，维护者已做初步
手工确认（验证范围未覆盖全部界面入口）。剩余的 GUI 手工验收项只在 `../todo.md` 追踪，本文件
自此只作为计划与落实口径的历史快照。
目标版本：0.3 / alpha4 的组成部分（本文档只覆盖「平台代码集中」，**不是** alpha4 全量计划）
关联条目：维护者 2026-10-20 提出的「集中平台化内容」
文档性质：独立计划，不写入 `todo.md`；已按 AGENTS 的归档规则归入本目录

---

## 1. 背景与动机

2026-10-20 的跨平台调查显示：本项目的业务代码约八成平台无关，但 Windows 专属逻辑散落在
9 处左右（见 §2 清单），且没有成文规则约束「新的平台分支应该写在哪里」。随着代码继续增长，
平台分支会继续渗漏，将来任何新平台（如 macOS）的移植成本都会随渗漏量线性上涨。

本计划做两件事：

1. **代码收口**：把散落的平台分支集中到唯一入口 `src-tauri/src/platform/`；
2. **成文规范**：新增 `docs/guides/platform.md`，把「平台分支只准出现在 platform/ 内」
   从口头约定变成可引用、可检查的规则。

这是业界成熟做法（SQLite 的 VFS、Chromium 的 `//base` 与可移植性约定、VS Code 的
base/platform 分层），不是临时发明。收益与代价见 §8。

**定位澄清**：本计划完成后，项目只是「平台差异集中、加平台更容易」，**并不等于**已经支持
macOS——打包、签名、标题栏、剪贴板实现、素材板 WebGPU 实测等属于后续独立批次，本文档不覆盖。

## 2. 现状盘点（2026-10-20 全量调查，file:line 已核实）

### 2.1 待收口的代码（本次迁移对象）

| 位置 | 内容 | 现状 | 收口后去处 |
| --- | --- | --- | --- |
| `storage.rs:53-56` | `display_path`：剥离 `\\?\` 设备命名空间前缀 | 与设备命名空间簇分离 | `platform/path.rs` |
| `storage.rs:145-150` | `ensure_not_device_path`（含 `path_namespace`） | 同上 | `platform/path.rs` |
| `storage.rs:183-206` | `comparable_path`：分隔符归一 + `cfg!(windows)` 大小写折叠 | `cfg!` 内联在业务文件 | `platform/path.rs` |
| `storage.rs:254-258` | `normalize_source_path` 的路径 key（同款大小写折叠） | 同上 | `platform/path.rs` |
| `app/settings.rs:765-803` | `reveal_path_in_folder`：Windows/macOS/Linux 三路分支 | 已有分支但写在业务文件 | `platform/shell.rs` |
| `app/settings.rs:822-840` | `open_directory` / `open_existing_directory`：`cfg!` 三路 | 同上 | `platform/shell.rs` |
| `pin_board/dds.rs:629-684` | `clipboard_file_paths`：Windows 原生 CF_HDROP；非 Windows 返回空 | 藏在素材板解析器文件里 | `platform/clipboard.rs` |
| `headless.rs:1310-1328` | `peak_memory_bytes`：`GetProcessMemoryInfo`；非 Windows 返回 `None` | 同上 | `platform/process.rs` |

### 2.2 保留原处的例外（规范文档登记，不迁移）

| 位置 | 内容 | 保留理由 |
| --- | --- | --- |
| `main.rs:1` | `windows_subsystem` 属性 | Tauri 标准入口写法，跨平台编译器属性 |
| `Cargo.toml:71-76` | `windows-sys` 的 `[target.'cfg(windows)'.dependencies]` 段 | 依赖声明必须按 target 分节，Cargo 的既定机制 |
| `repository_lock.rs:1002-1040` | `cfg(windows)` 大小写闸门测试 | 测试语义本身就是「Windows 大小写不敏感」，属平台测试 |
| `tests/stress_support/mod.rs:729-787` | CPU 周期计数（`QueryProcessCycleTime`）与 FILETIME | 发布前压力测试专用，见 §7 决策 1 |
| `src/shared/useRuntimeSubscription.ts:19-27` | `normalizeRepositoryKey`：前端自有的路径等价性归一（剥 `\\?\` 前缀、统一分隔符、去尾分隔符、小写），与后端 `equivalence_key` 同语义、异实现，用于事件按仓库过滤 | 见 §7 决策 3：保留并登记同步要求 |

### 2.3 打包与 CI 层（非目标，规范文档登记「加平台时改哪些文件」）

`tauri.conf.json`（`decorations: false`、`bundle.targets: ["nsis"]`、icon 仅 `.ico`）、
`.github/workflows/windows-ci.yml` 与 `release.yml`（`runs-on: windows-latest`）、
`src-tauri/resources/icon.ico`（无 `.icns`）。这些天然按平台分文件配置，收不进一个 Rust
模块，由规范文档统一说明。

## 3. 目标与非目标

### 3.1 目标

1. 新建 `src-tauri/src/platform/` 模块，按职责内分四个子文件：
   `path.rs`（路径表示与等价性）、`shell.rs`（文件管理器打开）、`clipboard.rs`（剪贴板文件
   路径）、`process.rs`（进程内存信息）；`lib.rs` 增加 `mod platform;`。
2. §2.1 的八处全部迁入，调用点机械改名（如 `storage::display_path` →
   `platform::path::display_path`，或经 `use` 引入）。
3. 新增 `docs/guides/platform.md` 可移植性规范（内容要点见 §5）。
4. 行为零变化：错误文案（「不能是设备命名空间路径」「无法打开文件所在文件夹」等）与返回值
   逐字不变，纯机械搬移。

### 3.2 非目标

- 不实现任何新平台的运行支持（不补 macOS 剪贴板、不改打包目标、不加 `.icns`、不改 CI runner）。
- 不改 `decorations: false` 的自绘标题栏。
- 不改 repository schema（保持 v5）、不改前端 API 契约、不改 `AGENTS.md` 现有条目。
- 默认不迁移测试侧代码（§2.2；不纳入的完整理由见 §7 决策 1）。

## 4. 方案：platform 模块设计

### 4.1 对外接口（平台无关命名）

| 函数 | 签名要点 | 语义 |
| --- | --- | --- |
| `path::display_path` | `(&Path) -> String` | 展示用路径，剥离设备命名空间前缀 |
| `path::ensure_not_device_path` | `(&str, label) -> Result<(), String>` | 拒绝 `\\?\` 设备对象路径 |
| `path::equivalence_key` | `(&str) -> String` | 路径等价性 key：统一分隔符，Windows 上大小写折叠 |
| `path::source_path_key` | `(&str) -> String` | 分支工作文件的等价性 key（现 `normalize_source_path` 内联逻辑） |
| `shell::reveal_in_file_manager` | `(&Path, label) -> Result<(), String>` | 在文件管理器中显示文件（Windows 选中、macOS `-R`、Linux 打开父目录） |
| `shell::open_directory` | `(&Path, label) -> Result<(), String>` | 在文件管理器中打开目录 |
| `clipboard::file_paths` | `() -> Result<Vec<String>, String>` | 剪贴板中的文件路径列表；能力缺失时**显式返回空**并在文档标注（非 Windows 现状） |
| `process::peak_memory_bytes` | `() -> Option<u64>` | 峰值工作集；无能力平台返回 `None`，调用方写 null（沿用现契约） |

### 4.2 建模原则（写进规范文档，也是评审标准）

1. **按「业务需要什么」建模，不按 Windows 建模**：例如等价性 key 的名字叫
   `equivalence_key` 而不是 `lowercase_path`——大小写折叠是 Windows 的实现选择，不是业务
   语义；将来 macOS（默认不敏感、可配置）与 Linux（敏感）的差异由该函数内部消化并在文档
   说明，不焊死在函数名里。
2. **能力缺失必须显式**：返回空列表 / `None` / 明确的「不支持」错误，禁止静默假成功。
3. **`cfg` 门控只允许出现在 `platform/` 内**：业务文件出现 `cfg(target_os)`、`cfg(windows)`、
   `cfg!(windows)` 即违规（例外清单见规范文档 §登记表）。
4. **错误文案跨平台逐字一致**，除真实的能力差异外不允许分支产生不同文案。

## 5. 可移植性规范（`docs/guides/platform.md` 内容要点）

与 `logging.md`、`validation.md` 并列的 guide，含：

- **规则 1**：平台分支唯一入口 `src-tauri/src/platform/`；对外 API 平台无关命名（§4.2 原则）。
- **规则 2**：新增平台分支的流程——先在 platform/ 找接口，没有则新增接口 + 各平台实现 +
  更新例外登记表；禁止在业务代码里直接写 `cfg`。
- **规则 3**：例外登记表——`main.rs`、`Cargo.toml` target 段、`repository_lock.rs` 的
  Windows 测试、`tests/stress_support/`（决策 1：不纳入，跨平台压力测试口径见 §7）、
  前端 `useRuntimeSubscription.ts` 的 `normalizeRepositoryKey`（决策 3：保留，登记与
  `platform::path::equivalence_key` 的同步要求），每条注明保留理由。
- **规则 4**：打包与 CI 的「加平台清单」——`tauri.conf.json`（targets/icon）、
  `.github/workflows/`（runner）、`src-tauri/resources/`（图标格式）、
  `docs/guides/release-policy.md`（签名口径）。声明这些文件的按平台分文件是既定形态，
  不受规则 1 约束。
- **规则 5**：能力差异披露——每个接口注明各平台现状（如剪贴板在非 Windows 为空实现），
  新平台实现时按表核对。

### 5.1 机器把关（CI 检查，已定随本批实施）

在 `windows-ci.yml` 增加一步：用几行脚本在 `src-tauri/src/` 下
`grep -n 'cfg(target_os\|cfg(windows\|cfg!(windows'`，命中 `platform/` 与例外清单之外的文件
即失败。把规则 1 从「靠自觉」变成「机器把关」。例外清单以规范文档 §规则 3 的登记表为准，
脚本内以允许列表实现，后续新增例外时同步更新两处。

## 6. 实施步骤

单一批次、两个阶段，文档与代码同批提交（AGENTS 口径）：

1. **阶段 1（代码收口）**：建 `platform/` 目录与四个子文件；按 §2.1 表迁移八处；全仓库
   调用点改名；`cargo test --lib` 等全套自动验证（§6 验证口径）。此阶段不动任何行为。
2. **阶段 2（规范落地）**：写 `docs/guides/platform.md`；按 §5.1 在 `windows-ci.yml` 加
   grep 检查步骤与允许列表；
   `docs/architecture/overview.md` 若有模块清单则补一行 platform 模块；GUI 手工验收项
   记入 `todo.md`；CHANGELOG `## Unreleased` 记入 Changed 条目。

提交拆分建议：阶段 1 与阶段 2 各一个提交（代码迁移与规范文档边界清晰），或合并为一个——
由实施时按改动实际体量决定。

## 7. 决策记录（2026-10-20 维护者拍板）

1. **`tests/stress_support/` 的 Windows 代码不纳入 platform/**（已确认）。它是**压力测试
   专用辅助代码**（`tests/` 下编译，不进产品安装包），读的是已退出子进程的内核记账
   （`GetProcessTimes` 的用户/内核 CPU 时间 + `QueryProcessCycleTime` 的 CPU 周期数；
   后者存在的原因是 Windows 的 CPU 时间按 15.625 ms 时钟中断记账，短操作无法比较）。
   不纳入的理由：`platform/` 是**产品**的平台层，测试专用代码进产品命名空间只会扩大批次
   半径；且它已自带非 Windows 优雅路径（`stress_support/mod.rs:761-765` 返回
   `measured: false`）。
   **跨平台跑压力测试的口径**：通过/失败判定与墙钟数据在任何平台照常工作；CPU 记账在
   非 Windows 平台显示「未测量」；按既有口径（`todo.md`），基线比较本来就只在同一台机器、
   同一构建档、单线程采集下进行（`perf-baseline.json` 记录 `machine` 字段）。若将来需要
   非 Windows 平台的 CPU 精确数据，在 `stress_support` **内部**补对应平台的记账分支
   （如 Unix 的 `wait4`/`getrusage`，十余行），不触碰产品代码；规范文档 §规则 3 的登记表
   注明此约定。
2. **CI 的 grep 检查随本批实施**（已确认）：见 §5.1。
3. **前端 `normalizeRepositoryKey` 保留现状并登记**（已确认）。它是前端事件 bridge 的
   仓库过滤逻辑：后端事件里的 `repositoryKey` 可能来自 canonicalize（Windows 上带 `\\?\`
   前缀），前端订阅事件前对双方各做一次路径等价性归一（剥前缀、统一分隔符、去尾分隔符、
   小写），否则会漏收当前仓库的事件。保留理由：纯比较用途、已有测试、量极小，且 TS 无法
   复用 Rust 的 platform 模块；下沉后端等于改事件契约，收益不成比例。
   **登记要求**：其语义必须与后端 `platform::path::equivalence_key` 保持同步（两边各自
   的文档互相引用）；后端若将来改为在发布事件前归一 `repositoryKey`，则前端该函数连同
   测试一并删除。

## 8. 收益与代价（如实记录）

**收益**：移植成本从「全仓库找」变为「对着 platform/ 接口清单补实现」；平台分支渗漏被
机器检查拦截；敏感逻辑（路径校验、设备命名空间拒绝）只需审查一份平台无关版本。

**代价**：业务代码读路径要多跳一层（`storage.rs` 里看 `comparable_path` 的实现变成看
`platform::path::equivalence_key`）；抽象有被「Windows 语义」反向污染的风险——§4.2 原则 1
是针对它的约束，实施与后续评审都按它把关。

## 9. 风险

- 机械搬移风险低，主要风险是调用点改名遗漏——由 `cargo check --all-targets` 全量兜底。
- 与 alpha3 收尾批次并发会撞工作区：**仅在 alpha3 相关提交全部落库后动工**（现状已满足）。
- 验证口径按 `docs/guides/validation.md`：本批不触发压力测试。

---

## 实施记录

2026-10-20 完成并提交，代码与文档同批；维护者已做初步手工确认（未覆盖全部界面入口），
剩余验收项在 `../todo.md`。

- **阶段 1（代码收口）**：新建 `src-tauri/src/platform/`（`mod.rs` + `path.rs` /
  `shell.rs` / `clipboard.rs` / `process.rs`），§2.1 八处全部迁入；全仓库调用点
  机械改名；`lib.rs` 增加 `mod platform;`。与计划的偏差两处：
  - `shell::reveal_in_file_manager` 未带 `label` 参数（§4.1 表格写的是
    `(&Path, label)`）：现有错误文案（「无法打开文件所在文件夹」）不含 label，为
    满足 §3.1 的「错误文案逐字不变」，按 `(&Path)` 实现。
  - 剪贴板读取的 256 个导入上限（`MAX_IMPORT_FILES`）随迁移从读取侧移到调用方
    （`pin_board/mod.rs` 按业务上限截断），返回的路径列表逐项一致；上限属业务
    约束，不进平台层（此口径已写进规范文档规则 1）。
- **阶段 2（规范落地）**：新增 `docs/guides/platform.md`（规则 1–5、例外登记表、
  能力披露表）；`windows-ci.yml` 在格式检查后增加「Check platform branches stay
  in platform/」grep 步骤与允许列表；`docs/architecture/overview.md` 补 platform
  模块行；`docs/README.md` 索引补一行；GUI 手工验收项记入 `todo.md`；
  CHANGELOG `## Unreleased` 记入 Changed 条目。
- **验收前整理（同日）**：机械搬移完成后又做了一轮读完再改的整理，均不改变行为：
  - 调用点统一为「`use crate::{…, platform::path::display_path, …}` + 裸调用」，
    不再散落 `crate::platform::path::display_path(…)` 全限定写法（与
    `library/repository.rs` 原有风格一致，也避免 fmt 把长表达式折行）。
  - `platform::path::equivalence_key` 去掉「`MAIN_SEPARATOR == '\\\\'` 判断分隔符 +
    `cfg!(windows)` 判断大小写」的双重平台判断，合并为一个 `cfg!(windows)` 分支：
    两个判断在支持的平台上始终同真同假，合并后行为不变、只留一处平台判断。
  - 补测试 `equivalence_keys_match_their_documented_scope`，把
    `equivalence_key`（归一分隔符 + 折叠大小写）与 `source_path_key`（只折叠大小写）
    的分工钉住；`platform::path` 的测试数由 2 增到 3。
  - CI grep 模式由 `cfg\((target_os|windows)|cfg!\(windows` 扩为
    `cfg(_attr)?!?\((target_os|windows|unix)`：原模式漏掉 `cfg!(target_os = …)`、
    `cfg_attr(…)` 与 `cfg(unix)` 三类写法（已用样本逐条验证），而 `shell.rs` 里
    恰有 `cfg!(target_os = "macos")` 这类写法——规则 1 的机器把关原本是漏的。
  - 文档纠错：`docs/modules/history-and-backup.md` 与 `docs/modules/library.md`
    仍把 `ensure_not_device_path` / `path_namespace` 写成 `storage::`，改为
    `platform::path::`；`platform.md` 的能力表把 `equivalence_key` 与
    `source_path_key` 拆成两行（原写法暗示 Windows 上 `source_path_key` 也归一分隔符，
    实际不会）；`display_path` 的非 Windows 单元格改为「原样返回（路径不带该前缀）」，
    与实现一致；`normalizeRepositoryKey` 的同步要求改成准确的组合关系
    （`display_path` + `equivalence_key` + 去尾分隔符），并在该 TS 函数上补了对应
    注释，使「三处注释互相引用」这句话真正成立。
  - `AGENTS.md` 的上下文入口补一条平台分支路由（原计划把它列为非目标，但规范若
    没有入口就等于没人读，故补一行，不改动既有条目）。
  - 计划文档自身移入 `archive/`，`todo.md` 的指针改为归档路径。
- **验证**（validation.md 日常口径）：`cargo check --all-targets`、
  `cargo check --features headless`、`cargo test --lib`（305 通过，含迁入
  `platform::path` 的 2 个测试；整理后 306 通过）、`cargo fmt --check`、
  `git diff --check`；CI grep 步骤已在本地模拟通过（含「注入一行违规即失败」的
  反向验证）。压力测试未运行（本批不触发）。
