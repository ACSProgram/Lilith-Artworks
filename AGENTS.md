# Lilith Artworks 工作约定

## 先选择上下文入口

代码任务先读 `docs/architecture/ai-reading-guide.md` 选择路由；不要默认通读整个仓库。先按任务范围选择一条路径，只有契约跨界时才补读相邻模块。

- 历史图、分支、精简、检查点、恢复或自动备份：先读 `docs/modules/history-and-backup.md` 的“上下文入口”，再进入 `src/modules/history/` 或 `src-tauri/src/history/`、`src-tauri/src/backup/`。
- Artwork 树、搜索、拖放或回收站：先读 `docs/modules/library.md`，再进入 `src/modules/library/` 或 `src-tauri/src/library/`。
- 素材板、画板画布、图片导入导出或画板回收站：先读 `docs/modules/pin-board.md`，再进入 `src/modules/pin-board/` 或 `src-tauri/src/pin_board/`。
- 发布、成品、C2PA、TrustMark 或识别：先读 `docs/modules/authenticity.md`，再进入 `src/modules/authenticity/` 或 `src-tauri/src/authenticity/`。
- 设置、窗口、托盘和应用生命周期：读 `docs/architecture/overview.md` 的“应用生命周期”，再进入 `src/app/` 或 `src-tauri/src/app/`、`src-tauri/src/lib.rs`。
- 样式问题：业务模块样式优先读对应 `src/styles/<module>.css`；`src/styles/index.css` 只保留基础控件和仍未拆出的共享规则。
- 构建与验证：只读 `docs/guides/validation.md`，按用户要求选择轻量检查或完整验证。
- 本轮尚未验收的工作：读 `docs/planning/current-handoff.md`；未完成事项查 `docs/planning/todo.md`，不要从旧聊天记录重建范围。

## 边界

- `src/modules/<module>/api.ts` 是前端调用该领域 Tauri 命令的唯一入口。
- `src/modules/<module>/types.ts` 只保存边界 DTO；纯前端图计算放模块自己的 model/helper 文件。
- `src-tauri/src/history/` 管图结构和 SQLite 事务，不读取 ChunkFile。
- `src-tauri/src/backup/` 管 snapshot/delta、恢复、精简、检查点和运行进度，通过 history API 改图。
- `src-tauri/src/app/` 管设置；托盘构建和应用生命周期仍由 `src-tauri/src/lib.rs` 管理。
- 不把认证模块、素材库模块和历史模块互相直接导入；跨模块流程由应用层或原生命令编排。

## 文档与验证

- 当前有效事实写入 `docs/architecture/`、`docs/modules/` 和 `docs/guides/`。
- 当前批次状态与人工验收结果写入 `docs/planning/current-handoff.md`；未完成事项写入 `docs/planning/todo.md`；已完成或被替代的计划移入 `docs/planning/archive/`。
- 代码入口或契约改变时同步更新模块文档。不要把“计划实现”写成“已经验收”。
- 默认做与改动匹配的类型、格式和静态检查；完整编译、GUI 流程与大文件测试由用户明确安排。

## Git 管理

- 代理直接暂存并创建 Git 提交。每个边界清晰、已完成对应验证的阶段结束时，用 `git add` 精确加入该阶段文件后提交：提交信息包含清晰具体的英文祈使句主题和说明行为变化与验证结果的提交正文；不能只给提交范围或主题。
- 代理不自动执行 `git push`，是否推送、推送到哪个远端由用户决定。
- 建议提交范围与当前阶段一致；保留并谨慎处理已有用户改动，不夹带无关文件，也不为制造整洁提交而回退用户内容。
- 只做一次必要的差异与验证结果核对，不反复运行 `git status`、`git log` 等命令确认同一事实。
- 建议提交主题使用清晰、具体的英文祈使句；文档与其对应代码归入同一阶段提交。

### 提交信息格式

- 主题行：英文祈使句、首字母大写、句末不加句点、长度不超过 72 字符；只用一句话概括该阶段最主要的改动，其余内容放进正文，不用 `and` 串联多组改动。
- 主题与正文之间空一行；正文硬换行宽度统一为 72 字符；多条目用 `- ` 列表，每条以句点结尾。
- 不使用 `type(scope):` 这类 Conventional Commits 前缀；改动分类统一体现在 `CHANGELOG.md` 的 Added / Fixed / Changed 小节。
- 正文先写行为变化与原因，再写验证结果；只描述事实，不写“计划实现”的内容。
- 重建仓库、初始导入类提交允许沿用过去式或名词短语（如 `Initial commit`），不为了格式统一而改写它们。

### 改写历史

- 仅存在于本地、尚未推送的提交可以自由改写；改写前先建保险分支（如 `backup/before-rewrite`），并用 `--committer-date-is-author-date` 保留原始提交日期。
- 已推送到远端或带有 tag 的提交默认不改写：改写会改变 commit id，需要 `git push --force-with-lease` 并重新指向 tag。确需改写时先向用户说明影响范围并取得确认。
