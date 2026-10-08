# 当前任务交接

更新时间：2026-10-08

本文件只记录当前批次状态与人工验收结果。未完成事项见 `todo.md`；已完成或被替代的批次记录
见 `archive/`。

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
