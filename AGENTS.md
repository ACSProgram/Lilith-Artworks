# Lilith Artworks 工作约定

## 先选择上下文入口

代码任务先读 `docs/architecture/ai-reading-guide.md` 选择路由；不要默认通读整个仓库。先按任务范围选择一条路径，只有契约跨界时才补读相邻模块。

- 历史图、分支、精简、检查点、恢复或自动备份：先读 `docs/modules/history-and-backup.md` 的“上下文入口”，再进入 `src/modules/history/` 或 `src-tauri/src/history/`、`src-tauri/src/backup/`。
- Artwork 树、搜索、拖放或回收站：先读 `docs/modules/library.md`，再进入 `src/modules/library/` 或 `src-tauri/src/library/`。
- 素材板、画板画布、图片导入导出或画板回收站：先读 `docs/modules/pin-board.md`，再进入 `src/modules/pin-board/` 或 `src-tauri/src/pin_board/`。
- 发布、成品、C2PA、TrustMark 或识别：先读 `docs/modules/authenticity.md`，再进入 `src/modules/authenticity/` 或 `src-tauri/src/authenticity/`。
- 设置、窗口、托盘和应用生命周期：读 `docs/architecture/overview.md` 的“应用生命周期”，再进入 `src/app/` 或 `src-tauri/src/app/`、`src-tauri/src/lib.rs`。
- 仓储空间状态（数据库记录口径或磁盘扫描）：先读 `docs/modules/history-and-backup.md` 的“仓储空间状态”，再进入 `src-tauri/src/storage_overview.rs` 或 `src/app/RepositoryStoragePanel.tsx`。
- 样式问题：业务模块样式优先读对应 `src/styles/<module>.css`；`src/styles/index.css` 只保留基础控件和仍未拆出的共享规则。
- 日志与诊断（分档、新增日志的取舍、格式约定）：读 `docs/guides/logging.md`，再进入 `src/shared/diagnostics.ts` 或 `src-tauri/src/app/diagnostics.rs`。
- 构建与验证：只读 `docs/guides/validation.md`，按用户要求选择轻量检查或完整验证；压力测试只随发布运行，不在日常开发或收尾阶段执行。
- 版本、CHANGELOG 与发布口径：只读 `docs/guides/release-policy.md`。
- 本轮尚未验收的工作：读 `docs/planning/current-handoff.md`；未完成事项查 `docs/planning/todo.md`，不要从旧聊天记录重建范围。

## 边界

- `src/modules/<module>/api.ts` 是前端调用该领域 Tauri 命令的唯一入口。
- `src/modules/<module>/types.ts` 只保存边界 DTO；纯前端图计算放模块自己的 model/helper 文件。
- `src-tauri/src/history/` 管图结构和 SQLite 事务，不读取 ChunkFile。
- `src-tauri/src/backup/` 管 snapshot/delta、恢复、精简、检查点和运行进度，通过 history API 改图。
- `src-tauri/src/app/` 管设置；托盘构建和应用生命周期仍由 `src-tauri/src/lib.rs` 管理。
- 不把认证模块、素材库模块和历史模块互相直接导入；跨模块流程由应用层或原生命令编排。

## 提问与决策

- 面对显著不确定、且不同选择会实质影响程序行为、性能或架构的决策时，先向用户提问，不要替用户拍板。
- 但不要把思考责任推给用户：有明显最优解的决策直接选择并在答复中说明理由，不为走流程而专门提问；拿不准是否值得问时，倾向于先做可逆的小选择并如实标注。
- 提问时交代足够背景，并用直白的话说明各选项的后果，例如“会明显增加某功能的 CPU/内存占用”“会让架构更复杂、模块之间耦合变强”“以后改动 A 时总要连着改 B”这类具体表述，不堆砌术语；一次聚焦一个问题，有推荐项时先给推荐项。

## 文档与验证

- 当前有效事实写入 `docs/architecture/`、`docs/modules/` 和 `docs/guides/`。
- 表层文档只保留当前状况、未完成事项与规划：`docs/planning/current-handoff.md` 只写当前批次状态与当前基线，`docs/planning/todo.md` 只写未完成、未验证与待决策事项。
- 历史增量一律归 `docs/planning/archive/`：已完成批次、已通过的人工验收和被替代的计划搬入归档（按日期归入已有日志，并同步更新 `docs/planning/archive/README.md` 的索引），同时从 `current-handoff.md` 与 `todo.md` 中删除，不在表层留下“已通过”的痕迹。
- 代码入口或契约改变时同步更新模块文档。不要把“计划实现”写成“已经验收”。
- 版本号**滞后**：日常改动不改动五处版本字段，也不新建版本小节；改动一律写入 `CHANGELOG.md` 顶部的 `## Unreleased` 暂存段，绝不写回带日期的版本小节。切版本与打标签由用户单独决定。条目按改动分量写（大改多写、小改一两句），不套固定模板，完整口径见 `docs/guides/release-policy.md` 的「CHANGELOG 写法」。
- 默认做与改动匹配的类型、格式和静态检查；完整编译、GUI 流程与大文件测试由用户明确安排。`src-tauri/tests/` 下的压力测试（取消、崩溃、大文件端到端）只在发布前由用户显式发起，日常开发与任何收尾阶段都不运行，详见 `docs/guides/validation.md` 的「压力测试：只在发布前运行」。

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
