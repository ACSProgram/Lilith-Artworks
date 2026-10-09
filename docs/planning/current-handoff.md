# 当前任务交接

更新时间：2026-10-09

本文件只记录当前批次状态与人工验收结果。未完成事项见 `todo.md`；已完成或被替代的批次记录
见 `archive/`。

## 日志与诊断体系（已实现，待人工验收）

把日志升级为可复用的诊断体系，供后续定位「切换作品/切换素材板卡死」与「托盘退出超时」复用。
分档原则与新增日志的取舍见 `docs/guides/logging.md`。系统分三档：常规（默认，事件驱动 + 阈值
触发，常开）、诊断（设置页「调试」栏开关，追加周期性探针与取证通道）、追踪（仅
`LILITH_LOG_LEVEL=trace`，不进设置页）。

- 等级不再由插件在构建时固定。`tauri-plugin-log` 用 fern 的 dispatch 级别做静态过滤，构建后
  无法提级，因此构建时刻意不调用 `.level()`，等级改由 `app::diagnostics::apply_level`
  （`log::set_max_level`）单独控制；
- 基线取 `LILITH_LOG_LEVEL`，未设置时调试构建为 `debug`、发布构建为 `info`；诊断模式开启时至少
  为 `debug`，关闭时取基线，因此环境变量既可抬升也可降低等级；
- 新增进程级「诊断模式」（不写入设置文件）。设置页新增「调试」栏目，内含「详细日志」开关与
  「日志文件夹」，随时切换且无需重启；默认跟随基线，即调试构建默认开启、发布构建默认关闭，
  发布版正常使用不受影响；
- 新增 `log_frontend_diagnostics` 桥接命令：WebView 侧没有写日志文件的路径，前端事件经此转发
  进同一份日志。常规档写入事件驱动的低频记录与阈值告警；诊断档再追加周期性探针；
- 新增前端主线程心跳与事件循环延迟探针（500 ms），只在诊断档启用；两者都只在异常时写日志，
  心跳平时只刷新取证通道；
- 新增原生端 WebView 看门狗：诊断模式下每 2 秒发 ping 并要求立即 pong，前端主线程冻结时由
  原生端线程写出 `[watchdog] webview unresponsive` 告警。这是唯一能在冻结进行中取证、且能扛过
  15 秒兜底强退的位置；与取证记录、`shutting down` / `forcing exit` 对齐即可区分「监听器未被
  调用（主线程冻结）」与「结算未返回（等待命令）」；
- 每行日志前缀为「时间 + 运行标识 + 等级 + target」，探针消息带
  `[hb]`/`[lag]`/`[task]`/`[slow]`/`[forensic]`/`[watchdog]`/`[gpu]`/`[error]` 标签，可按运行
  切片、按标签筛选，规则见 `docs/guides/logging.md`；
- 渲染器创建/销毁、GPU 设备序号与存活渲染器数、GPU 设备丢失、画板保存与结算、纹理批次加载、
  退出握手各阶段与仓库切换结算均写入常规档日志；长任务观察器与慢步骤同样常开，仅超阈值时记录。

### 卡死竞态保护（已实现）

代码检查确认旧渲染器的纹理 IPC 可能在设备销毁后返回，并继续调用已释放的 WebGPU 设备。
`renderer.ts` 现为设备增加 `released` 生命周期标记：读取返回后若设备已释放，直接丢弃结果并
记录 `texture.read completed after gpu release`；销毁时先置 `released` 再解绑上下文并销毁设备，
只允许当前 GPU owner 对 Canvas 执行 `unconfigure()`，销毁日志同时记录在飞纹理加载数。

**已确认的具体竞态**：旧渲染器的纹理 IPC 可能在设备销毁后返回，原实现会继续调用已释放的
WebGPU 设备。现已在读取返回点增加生命周期闸门。已排除 Rust 侧（同步命令均为轻量操作，冻结期间
日志写入正常）。该保护是否消除冻结仍需一次人工复现确认；若仍冻结，再向浏览器呈现/GPU 路径定位。

**验证**：`npx tsc --noEmit`、`npm test`（141 通过）、`cargo check --lib`、`cargo fmt --check`、
`git diff --check` 通过。

### 冻结复现结论与取证能力（已实现，待人工复现）

**复现结论**：多次冻结签名一致——渲染进程**挂起而非崩溃**（Crashpad 无当日新转储），原生端
全程健康，未闭合的步骤记录全部是 `texture.load` / `texture.read`（IPC 响应未回投），所有 GPU
步骤均已闭合。日志中「创建过渲染器的会话」全部冻结，「从未创建的会话」无一冻结。触发条件是
**渲染器 create/destroy churn 本身**（约 3 个切换周期、6 个设备即足够），与是否切画板、切到
哪个作品无关；据此已实现上面的竞态保护。**卡死本身已修复**（复用渲染器与 GPU 设备，见下方专节），并经维护者实机疯狂切换复现不出冻结。

**已交付的取证能力**（均为可复用能力，非一次性探针）：

| 能力 | 位置 | 用途 |
| --- | --- | --- |
| 取证 Worker + IndexedDB | `src/shared/diagnosticsWorker.ts`、`shared/diagnostics.ts` | 独立线程在主线程冻结期间继续计时，把最后心跳 / 最后操作 / 静默时长写入 IndexedDB，下次启动回读成 `[forensic] previous freeze`。这是唯一能给出「停在哪一步、停了多久、是否恢复」的通道，仅诊断档启用 |
| 全局错误捕获 | `shared/diagnostics.ts` | 接管 `error` / `unhandledrejection`（此前完全不可见），脚本异常与资源加载失败分开记录 |
| GPU 健康 | `pin-board/renderer.ts` | `device.lost` 与 `device.onuncapturederror` 捕获设备丢失与错误作用域之外被静默吞掉的 GPU 错误 |
| 阈值告警 | `shared/diagnostics.ts`、`app/settings.rs`、`pin_board/mod.rs` | 长任务（>200 ms）、慢步骤、锁等待/持有与慢读告警；仅在超过阈值时记录 |
| 慢读归因 | `app/settings.rs`、`pin_board/mod.rs` | `with_repository_read_labeled`：慢读告警带上具体命令 |

**验证**：`npx tsc --noEmit`、`npm test`（141 通过）、`cargo check --lib`、`cargo fmt --check`、
`git diff --check` 通过。

**待人工复现**：下一次复现后优先看 `[forensic] previous freeze` 行（给出冻结时刻、最后操作、
是否恢复）。

## 素材板交互补全：拖放导入、全选与旋转吸附（已实现，已人工验收）

补上素材板此前缺失或与设计意图不符的三项交互：

- **拖放导入**：画布区此前没有任何拖放处理，而窗口层又取消了 webview 的文件拖放默认行为，因此把图片拖进素材板毫无反应。现在画布区（`.pin-board-stage`）接收拖入的图片文件，读取字节后走 `import_pin_board_clipboard_image`；放置点对齐鼠标落点，多张一起拖入按前一张显示宽度向右排开。窗口以 `dragDropEnabled: false` 运行（Windows 上使用 HTML5 拖放的前提），webview **不暴露文件系统路径**，因此只接受可被内容嗅探识别的位图（PNG/JPEG/WebP/BMP/GIF），DDS/TGA 仍走「导入图片」选择器；画板锁定时不导入并提示。
- **`Ctrl+A` 全选整个画板**：新增 `renderer.selectAll()`，选中全部未删除图片（不限视口）；选中态属视图状态，不调整图片顺序、不标记 dirty，锁定时不生效。
- **Shift 旋转改为绝对角度吸附**：原实现吸附的是拖拽增量，已带偏角的图片恒停在「偏角 + n×15°」、永远回不到 0°；现由 `geometry.snapRotationDelta` 把绝对角度吸附到 15° 整数倍后反推增量。

**验证**：`npx tsc --noEmit`、`npm test`（145 通过，新增 4 条：`snapRotationDelta` 两条、`selectAll` 两条）、`git diff --check` 通过。

**已知取舍**：拖入导入只能读文件内容、不能获取路径，因此 DDS/TGA 不支持拖入；多张拖入走单图字节命令逐张导入，每张都是一次独立的保存与历史步骤。

## 素材板切换卡死：复用渲染器与设备（已实现，已人工验收）

切换画板/作品会销毁并重建渲染器与 WebGPU 设备，该 churn 已确认会触发 WebView2 渲染进程冻结。现在把渲染器的 GPU 初始化（`create`）与画板装载（`loadBoard`）拆开：GPU 设备在模块生命周期内只创建一次，切换画板与切换作品都只换数据、复用同一设备，因此一次会话只创建一个设备。

- `renderer.ts`：`create` 只做画布生命周期内的一次性初始化；新增 `loadBoard(view, session)`（先以旧 `boardId` 结算旧画板并释放其纹理，再装载新画板，按 `boardId` 幂等，结算失败即中止并保留旧画板）与 `unloadBoard`（作品已无画板时清空）；按选定的纹理策略**切换即全释放**；`destroy()` 只用于模块真正卸载。
- `PinBoardModule.tsx`：`GpuCanvas` 常驻、不再随 `view` 卸载；创建 effect 幂等化（StrictMode 下每次挂载只建 1 个设备）；`select()` 与作品切换改走 `loadBoard`，结算失败即中止并保持旧画板；回收站删除当前画板走跳过 finalize 的切换路径。
- `App.tsx` / `ArtworkWorkspace.tsx`：去掉 `ArtworkWorkspace` 上的 `key={artworkId}`，改为历史/发布/识别三个窗格各自带 `key={artworkId}`（行为与过去整块重挂载一致，其它模块未改），素材板窗格跨作品保留渲染器与设备。
- 设备丢失改为**限流自动重建**（与 `create` 共用初始化路径；每渲染器生命周期最多 2 次、带 600 ms × 次数退避；重建期间暂停绘制，超限或失败即挂起绘制并只上报），取代原来的「只上报」。

**验收证据**：最新一次疯狂切换会话在 8 次切作品 + 27 次切画板 + 33 次画板装载下，**只创建 1 个 GPU 设备、0 次渲染器销毁/重建**，无 `webview unresponsive`、无 `forcing exit`，退出握手瞬时完成（`settle ok` → `shutting down (webview confirmed)`）。对照旧代码同场景会话为 6–24 个设备且必冻结。

**验证**：`npx tsc --noEmit`、`npm test`（145 通过）、`git diff --check` 通过。

**已知取舍**：切换即全释放纹理，因此疯狂切换时会出现 2–4 s 的**异步**纹理装载批次（含 DDS 解码与磁盘 IO 等待，而非主线程阻塞——事件循环健康、看门狗无告警）。如后续要更顺滑的切回，可评估文档 `todo.md` 中的「跨画板保留纹理」方向。

## 本轮批次：素材板结算边界、文档整理与 0.2.0-rc.1 版本递增（已实现，待人工验收）

### 素材板运行时结算（P1）

切换仓库会卸载工作区并释放旧仓库。此前工作区卸载只对渲染器调用 fire-and-forget 的
`destroy()`，最后一次编辑可能落在已经释放的仓库上而丢失。现在仓库切换在 `saveSettings`
之前先 `await preparePinBoardRuntimeChange()`（`src/app/App.tsx`）：结算成功才继续切换，
失败则中止切换、保留旧仓库并提示。退出握手（`app_shutdown_requested`）的结算路径不变；同一
仓库内切换 Artwork 不释放仓库，仍沿用原有卸载保存。

### 文档整理

文档按读者分目录：`docs/user/` 面向使用者（`stress-test-report.md` 迁入），
`architecture/`、`modules/`、`guides/`、`planning/` 面向开发者与代理。重写 `README.md`、
`docs/README.md` 与本文，把 2026-09-12 至 2026-10-08 的历史交接日志归档为
`archive/handoff-log-2026-09-12-to-2026-10-08.md`，并精简各文档的元叙述与重复内容。

### 版本递增

应用版本递增到 `0.2.0-rc.1`（仅版本号，不建 tag、不发布）。

**验证**：`npm test`、`npx tsc --noEmit`、`cargo fmt --check`、`cargo test --lib`、
`git diff --check`、`node tools/release/verify-metadata.mjs` 通过；新增「切换仓库前先结算素材板」
与「结算失败时中止切换」两条前端用例。

**待人工确认**：切换仓库时若素材板结算失败会中止切换并提示，需实机确认提示文案与「保留旧
仓库」的行为符合预期。

## 当前基线

- 版本 `0.2.0-rc.1`，repository schema v4，应用标识 `com.lilith.artworks`。
- 版本与发布口径见 `docs/guides/release-policy.md`。
- 项目定位：平面美术个人项目的资源、版本管理与发布工具。领域模块为 Library（作品树）、
  History/Backup（分支与增量历史）、Authenticity（成品与 C2PA/TrustMark）、Pin-board（素材板）。

## 人工验收结果

### 2026-10-09：素材板切换卡死修复（通过）

维护者实机疯狂切换素材板作品与画板，未再复现此前的程序卡死；日志显示该会话只创建 1 个 GPU 设备、无渲染器销毁/重建，退出握手瞬时完成。设备丢失的限流自动重建尚未在实机触发（正常使用不丢设备），保留观察。改动契约见 `docs/modules/pin-board.md`。

### 2026-10-09：素材板拖放导入、全选与旋转吸附（通过）

维护者实机确认三项功能均可用：把图片拖到画布即可导入；`Ctrl+A` 选中整块画板；把图片转到约 7° 后按住 Shift 可精确转回 0°。拖入提示的呈现方式按维护者反馈调整过一次——不再铺满画布色块，改为只描一圈渐变边并在底部显示小胶囊提示。

改动契约见 `docs/modules/pin-board.md` 的「领域行为」。

### 2026-10-08：认证与识别三项修复（通过）

维护者对认证与识别模块的反馈均已落实并由维护者实机验收通过：质量预览放大后为 1:1 源像素
（无插值放大、无额外压缩痕迹），「显示原图」同样清晰，缩小预览不再出现上下两张图，缩放标签
100% 与源像素 1:1 一致；识别页拖放导入、记录删除菜单与确认弹窗的位置与文案均符合预期。
改动契约见 `docs/modules/authenticity.md`。

### 2026-10-04：生产使用验收（通过）

维护者在本机生产场景完成一轮验收，通过项：

- 常规桌面路径：安装、首次启动、仓库创建/打开、关闭到托盘与显式退出；Artwork 创建、树操作、
  回收站、分支、提交、创建分支、恢复、精简、检查点；进入/取消发布、认证导出与再次导出、
  识别与跨 Artwork 溯源；取消发布保留首次导出 JPG 且仓库内副本、记录与保存配置已清除；
  法律文件可从安装目录或 About/Legal 页面取得。
- `docs/modules/library.md` 声明的真实仓库 lease、设置持久化与 Windows 交互。
- 分支工作文件路径的清空约束（提示文案、分支选择变化、保存失败后的表单回滚）。
- 极端大文件与高像素压力的日常使用（2 GiB 工作文件、大像素图片）；普通用户账户安装、
  Authenticode 签名与时间戳验证；第三方工具回读 C2PA 并用随包模型验证 TrustMark 实图；
  从上一公开候选版安装升级与卸载。
- 设置页三页行样式；快速检查开关、自动保存与关闭时保存开关的位置与文案。

仍未验证的项集中到 `todo.md` 第二节。上述结论来自维护者本机的生产使用，不替代
`docs/guides/release-policy.md` 要求的「干净 Windows 用户环境」桌面验收。

## 已落实批次（按时间倒序）

| 批次 | 内容 | 验证 |
| --- | --- | --- |
| 素材板切换卡死修复 | 复用渲染器与 GPU 设备（`create`/`loadBoard` 拆分、`GpuCanvas` 常驻、工作区 key 下移）与设备丢失限流自动重建 | 维护者实测；`npm test` 145 |
| 统一清理体系 A–F | 画板结算改提交后清理、历史清理入队、未引用文件扫描、灾备暂存目录清扫、完整性扫描覆盖画板 DDS、设置页可观测 UI | `cargo test --lib` 151；`npm test` 121 |
| 压力测试批次 1–8 | 无头命令行入口与 A–R 组压力测试：取消边界、跨进程强杀、事务中途崩溃、大文件端到端、规模与灾备、参数边界、崩溃孤儿回收、画板 DDS、认证发布、损坏恢复 | 全套实测通过，见 `docs/user/stress-test-report.md` |
| 任务调度总控与空闲链路校验 A–D | 任务类型与取消路由、schema v4 校验列、调度器两级选择与空闲链路校验、失败警告面 | `cargo test --lib` 134；`npm test` 116 |
| 素材板退出握手与自动保存设置 | 退出握手 `app_shutdown_requested` / `confirm_app_shutdown`、防抖自动保存、关闭时保存开关 | 维护者实测 |
| 快速自动备份、手动提交优先与打开文件夹 | 快速检查模式、手动提交优先、`reveal_path_in_folder` | 维护者实测 |

各批次的实施细节、落实偏差与验证记录见归档日志
`archive/handoff-log-2026-09-12-to-2026-10-08.md`。

## 文档约束

当前功能契约以 `docs/architecture/`、`docs/modules/` 与 `docs/guides/` 为准；未完成事项只写入
`docs/planning/todo.md`；本文件只记录当前批次状态与人工验收结果；已完成或被替代的计划进入
`docs/planning/archive/`。
