# 验证策略

## 明确禁止

本项目禁止代理执行 GUI 自动化测试，包括启动桌面窗口进行操作、Playwright/Electron/Tauri 窗口驱动、截图和基于截图的视觉检查。界面视觉与交互由用户在阶段验收时手工检查。

同时禁止输入操纵：不得使用 `SendInput`、鼠标/键盘模拟、`pyautogui` 或注入式 `SendMessage` 等手段驱动真实光标与按键，也不得用自动化方式打开和操作应用窗口。允许的自动化手段只有程序化断言、HTTP 契约测试、进程外只读查询（`EnumWindows` / `GetWindowRect` 等）和纯计算复现。报告结论时只陈述断言结果，涉及视觉与手感的部分明确标注"待人工确认"。

## 分层验证

- 允许 `npm run build`、`npm test`、`cargo fmt --check`、`cargo check` 和 `cargo test`，
  即依赖已经可以正常编译和运行测试，不再要求绕开重依赖。
- 不允许编译 release 产物（`cargo build --release`、`npm run tauri build`），
  也不允许启动应用本身做自动化操作；界面与手感仍由用户手工验收。
- 每次修改运行 `git diff --check`，检查补丁空白和冲突标记。
- 前端纯逻辑、请求竞态或交互状态变更运行 `npm test`；素材板范围可用 `npm run test:pin-board`。Windows CI 固定运行完整前端测试集。
- 数据库变更使用临时目录与临时 SQLite 文件测试，不读取或修改用户真实仓库。如引入 schema 迁移，迁移 SQL 先在仓库数据库副本上用只读脚本核对效果，再进入 `schema.rs`。
- 文件提交/恢复测试使用小型确定性样本，重点验证哈希、原子发布、不覆盖和取消边界。
- `backup/chunk_file.rs` 与 `backup/restore.rs` 只依赖 sha2、zstd、tempfile（restore 另需 history/storage 少量接口），可以在仓库外用只含这几个依赖的轻量 crate `#[path]` 引入编译并运行其测试；这是重依赖边界内验证分块与物化逻辑的标准做法。
- `trustmark` 经 `ort-sys` 的构建脚本从 pyke CDN 下载预编译 ONNX Runtime，因此**全新环境首次构建需要网络**。改动依赖版本后必须重跑 `npm run legal` 并提交 `licenses/THIRD_PARTY_LICENSES.html`。

## 压力测试：只在发布前运行

`src-tauri/tests/` 下的八组压力测试（`stress_cancel` 取消边界、`stress_crash` 跨进程强杀、
`stress_large` 大文件端到端、`stress_scale` 规模与灾备与参数边界、`stress_cleanup` 崩溃孤儿
回收闭环、`stress_pin_board` 画板 DDS 完整性、`stress_authenticity` 认证发布内存与回读、
`stress_damage` 损坏文件的检测与恢复）是**发布前手动运行**的套件，**不在 CI 中**，
也**不属于日常开发的任何阶段**：

- **日常开发只在改动范围内运行轻量检查**：`npm test`、`cargo fmt --check`、`cargo check`、
  `cargo test --lib`、`git diff --check`。**不要顺手运行压力测试**——即使到了收尾、整理、
  提交前的阶段，只要不是准备发布，也不需要跑。
- **它很耗时间与磁盘，这是设计属性而不是卡住**：测试以真实可执行文件、真实落盘路径执行，
  覆盖「逻辑文件大小 × 单次改动量」的四象限；`stress_large` 的 `large` 及以上档位单次运行
  常以十分钟计，峰值磁盘占用可达数 GiB，`extreme` 档（4 GiB 文件、每次改 1 GiB）接近十几 GiB。
- **只在准备发布、需要产出或更新 `docs/user/stress-test-report.md` 的实测数据时运行**，
  并由维护者显式发起。代理不得在普通开发或收尾阶段自行运行；需要跑时先向维护者确认档位与
  磁盘预算。
- **大文件档按磁盘预算并发执行**：`stress_large` 的场景通过 `scenario_slot` 按「预计峰值磁盘」
  准入，可多个并行，但保留峰值之和不超过 `LILITH_STRESS_DISK_BUDGET`（默认 24 GiB），并发数
  不超过 `LILITH_STRESS_JOBS`（默认可用核数）。这样在有磁盘余量的机器上充分利用多核——
  被测子进程是单线程的，串行只会用一个核——同时避免盲目并行把磁盘写满。
- 运行方式、档位、磁盘需求与实测数据见 `docs/user/stress-test-report.md` 第 4、6 节。
- **断电持久性不做自动化验证**：测试用 `Child::kill()` 终止进程，它不触及操作系统与磁盘的
  缓存、也不保证目录项落盘，结构上无法覆盖「断电后已提交数据是否仍在」。该保证由操作系统、
  磁盘与 SQLite `synchronous = FULL` 声明承担，保留为声明/人工项，不列入自动化目标。

## 持续集成

- `windows-ci.yml`：`dependency-audit`（ubuntu，npm audit、`rustsec/audit-check`、SBOM）与
  `validate`（windows，发布元数据、许可证一致性、前端构建与测试、`cargo fmt --check`、
  `cargo test --lib`）。
- `release.yml`：推送 `v*` tag 时运行，含同样的审计与 Rust 测试步骤。
- 两处易踩的坑：
  1. `rustsec/audit-check` 的 `ignore` 输入**只按逗号切分**。用 YAML 块标量每行写一个
     advisory ID 会被当成**单个**非法参数传给 `cargo-audit`，报错后 stdout 为空，action
     最终失败并只显示 `Unexpected end of JSON input`。必须写成单行逗号分隔。
  2. `trustmark` 用 `=` 精确锁定 `ort` / `ort-sys` 版本，无法单独 `cargo update`。升级前先
     确认目标 ort 版本使用的下载地址（rc.10 起为 `cdn.pyke.io`，rc.9 及更早的
     `parcel.pyke.io` 已失效）。

当接口跨越重依赖边界且静态检查无法提供足够保证时，停止阶段并请用户执行一次完整编译和针对性手工测试；收到结果后再继续，避免错误跨阶段累计。
