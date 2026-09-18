# 验证策略

## 明确禁止

本项目禁止代理执行 GUI 自动化测试，包括启动桌面窗口进行操作、Playwright/Electron/Tauri 窗口驱动、截图和基于截图的视觉检查。界面视觉与交互由用户在阶段验收时手工检查。

同时禁止输入操纵：不得使用 `SendInput`、鼠标/键盘模拟、`pyautogui` 或注入式 `SendMessage` 等手段驱动真实光标与按键，也不得用自动化方式打开和操作应用窗口。允许的自动化手段只有程序化断言、HTTP 契约测试、进程外只读查询（`EnumWindows` / `GetWindowRect` 等）和纯计算复现。报告结论时只陈述断言结果，涉及视觉与手感的部分明确标注"待人工确认"。

## 分层验证

- ONNX/C2PA 引入前：允许 `npm run build`、`cargo fmt --check`、`cargo test` 和 `cargo check`。
- ONNX/C2PA 引入后：不执行全量 `cargo build`、`cargo check` 或全量 `cargo test`，避免重依赖反复编译；保留 TypeScript 构建、格式检查、数据库/纯逻辑测试和可独立编译的轻量 crate 测试。
- 每次修改运行 `git diff --check`，检查补丁空白和冲突标记。
- 前端纯逻辑、请求竞态或交互状态变更运行 `npm test`；素材板范围可用 `npm run test:pin-board`。Windows CI 固定运行完整前端测试集。
- 数据库变更使用临时目录与临时 SQLite 文件测试，不读取或修改用户真实仓库。如引入 schema 迁移，迁移 SQL 先在仓库数据库副本上用只读脚本核对效果，再进入 `schema.rs`。
- 文件提交/恢复测试使用小型确定性样本，重点验证哈希、原子发布、不覆盖和取消边界。
- `backup/chunk_file.rs` 与 `backup/restore.rs` 只依赖 sha2、zstd、tempfile（restore 另需 history/storage 少量接口），可以在仓库外用只含这几个依赖的轻量 crate `#[path]` 引入编译并运行其测试；这是重依赖边界内验证分块与物化逻辑的标准做法。

当接口跨越重依赖边界且静态检查无法提供足够保证时，停止阶段并请用户执行一次完整编译和针对性手工测试；收到结果后再继续，避免错误跨阶段累计。
