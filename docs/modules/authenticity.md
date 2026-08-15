# 成品与真实性模块

## 上下文入口

- Artwork 级页面切换与跨作品跳转：`src/app/App.tsx`、`src/app/ArtworkWorkspace.tsx`；Library 只接收应用层转换后的导航目标。
- 发布、局部范围和识别 UI：`src/modules/authenticity/AuthenticityModule.tsx`，样式只读 `src/styles/authenticity.css`。
- 发布/识别状态、命令、预览、搜索和请求代次：`src/modules/authenticity/useAuthenticityController.ts`。
- 前端 DTO 与 Tauri 命令：`src/modules/authenticity/types.ts`、`api.ts`；`api.ts` 只由 controller 消费。
- 发布/识别流水线：`src-tauri/src/authenticity/pipeline.rs`。
- C2PA 和 TrustMark 实现：`src-tauri/src/authenticity/c2pa.rs`、`trustmark.rs`。
- 成品、配置、导出记录和全库匹配：`src-tauri/src/authenticity/repository.rs`。
- 强制发布检查点和共享运行锁：`src-tauri/src/app/workflows.rs` 调用公开 `backup::ensure_checkpoint` 后进入认证服务；认证模块不导入 backup 或读取 `chunk_file.rs`。

当前未验收项和人工检查清单只读 `docs/planning/current-handoff.md`。

## 发布状态

分支必须先有一个 head 才能进入发布状态。用户选择最终成品后，应用工作流在统一备份运行锁内完成以下步骤：

1. 把当前 head 物化并标记为强制检查点。
2. 流式复制最终成品到临时文件，同步并计算 SHA-256；正式发布前先在 `pending_file_cleanup` 登记带哈希的仓库文件意图。
3. 以不覆盖方式发布文件，在事务中确认 head 未变化、插入唯一 `final_artifacts` 行，并原子移除清理意图；中途失败或进程退出时由清理队列回收孤儿文件。

最终成品存在后分支停止提交和自动调度，历史中的发布节点不能被删除或精简。用户可在二次确认后解除发布状态；命令在统一运行锁和 SQLite 事务内登记仓库内最终成品与认证副本的待清理项，删除认证记录和该分支保存的认证配置，再解除分支发布元数据。首次导出的 JPG 属于用户发布产物，始终保留在原路径。仓库文件清理失败项保留并返回 UI 重试，之后分支恢复提交和调度。

## C2PA 与 TrustMark

C2PA 是每次认证导出的强制基础，不能关闭。发布版固定输出 JPEG，并包含：

- `art.lilith.artworks.claim` 自定义声明；
- `stds.schema-org.CreativeWork`；
- 最终成品的 `parentOf` ingredient；
- `c2pa.transcoded` action；
- 启用 TrustMark 时的 `c2pa.soft-binding` 和 `c2pa.watermarked` action。

TrustMark 可关闭。启用时使用 Q/BCH_SUPER 模型和 40 位二进制 ID，只在用户框选的最多 8 个矩形区域内嵌入，强度范围为 0.50 到 1.50；没有框选区域时自动降级为仅 C2PA。模型文件不可用时 UI 也强制降级为仅 C2PA。模型变体和编码/解码模型 SHA-256 由后端读取后显示，不在前端写死。私钥只存在于单次命令内并用 `Zeroizing` 包装，不写入配置、数据库、localStorage 或日志。

签名流程先在目标目录生成临时 JPEG，再写 C2PA 并回读验证。只有回读到 manifest 后才计算哈希并登记外部输出与仓库副本的清理意图；两个文件都以不覆盖方式发布，`certification_records` 事务提交时原子移除意图。数据库失败、文件步骤失败或进程中断留下的文件由队列立即清理或在下次启动时重试。

正式签名前必须先生成质量预览。预览从仓库内最终成品重新执行与发布相同的透明背景合成、TrustMark 区域编码和 JPEG 质量编码，返回全分辨率 JPEG 供适应窗口、1:1 和分级缩放检查；C2PA 只写元数据且不重新编码像素，因此在用户确认并选择输出路径后才执行。自动生成的 TrustMark ID 会随预览回填，后续正式发布复用同一 ID。

## 导出记录与识别

每条导出记录固定关联分支、发布节点和最终成品，保存时间、首次输出路径、仓库内认证 JPG 副本、大小/SHA-256、内容声明、TrustMark ID、范围 JSON、C2PA manifest JSON 和验证状态。记录 ID 在签名前生成，并始终写入 `art.lilith.artworks.claim.recordId`；因此关闭 TrustMark 的仅 C2PA 发布也能稳定回查本地记录。所有记录必须有仓库副本，查看和再次导出只读取该副本，不依赖首次输出路径；副本缺失时明确报告不可用。再次导出在后端使用不覆盖发布，目标已存在时明确拒绝。记录可按 ID、标题、创作者或首次输出路径搜索。

识别始终读取 C2PA；模型可用时还会对整图或用户框选区域解码 TrustMark。候选匹配分别使用 C2PA `recordId` 与解码出的 TrustMark ID 查询全库索引；旧 C2PA 记录没有 `recordId` 时回退使用声明中的 TrustMark ID。两组结果按记录 ID 去重并保留 `c2pa` / `trustmark` 证据来源，冲突时同时显示两个通道的候选，不把任一候选自动判为可信。点击候选会切换到所属 Artwork、发布分支并高亮具体导出记录。模型解码本身不等于真实性认证，必须结合 C2PA 验证状态和本地记录判断。

## 命令

```text
enter_branch_publication
get_branch_publication
cancel_branch_publication
publish_branch_artifact
decode_authenticity
search_certification_records
preview_authenticity_image
preview_branch_artifact
preview_branch_artifact_output
preview_certification_record
export_certification_record
estimate_authenticity_output_size
```

## 数据版本

schema v8 将认证记录的 `stored_path` 设为非空，迁移时移除没有仓库副本的旧认证记录；DTO 和界面不再暴露冗余的 `contentStored` 状态。schema v7 的共享待清理队列继续处理发布创建失败和取消发布的数据库/文件边界；取消发布只清理仓库拥有的文件，不清理首次导出路径。历史版本细节见规划归档。
## 本轮实现补充（2026-08-13）

TrustMark 使用 Q / BCH_SUPER，标识长度为 40 位。水印只写入用户在预览中框选的区域；没有框选区域时不会启用 TrustMark，发布仍保留 C2PA。发布页提供强度与 JPEG 质量滑条、质量损失提示、大小预览及模型哈希摘要；识别页展示完整 C2PA manifest，导出记录支持自动搜索和详细字段查看。
## 前端交互约定

- Authenticity 不直接导入 App、Library 或 History。应用层传入只含 `id`、`title`、`headHistoryId` 的认证分支视图，注入清理重试，并把认证记录转换为 Library 导航目标。
- `useAuthenticityController.ts` 是 `authenticity/api.ts` 的唯一消费者；发布页和识别页只渲染控制器状态。发布状态、成品预览、记录预览、大小估算、识别和记录搜索分别使用请求代次，分支或输入变化后旧响应不会回填当前页面。
- 当前分支由 `ArtworkWorkspace` 统一持有；历史页和发布页共用同一选择，任一页面切换分支都会同步到另一页面。发布、进入发布和取消发布后刷新 Artwork 工作区分支数据，发布状态与当前内容保持同步。识别/搜索结果跳转携带单调递增的导航代次；即使目标仍是同一 Artwork 或同一记录，也会重新切到发布页并选择目标分支，同时保留已经加载的分支列表。
- 发布状态、预览和大小估算使用 latest-request-wins；分支切换后旧请求不得覆盖新分支状态。发布命令发出前再次确认当前选择、配置、最终成品和已加载发布状态属于同一分支。
- PEM 私钥仅以密码输入控件接收，并且发布完成后清空，不写入共享配置。
- 发布命令先生成全分辨率质量预览；预览层提供返回调参和直接签名发布，参数改变后再次生成会替换旧预览。
- TrustMark 仅在存在框选区域时启用；首次完成框选自动启用，清空全部区域自动关闭。
- JPG 质量、预估大小和透明背景色位于 TrustMark 之前；TrustMark 区明确提示在左侧图片拖动框选区域，避免把开关误解为唯一入口。
- 点击导出记录进入只读查看模式。查看页复用发布编辑页的左右分栏和图片预览结构，显示锁定字段、框选范围、发布节点、文件摘要和 C2PA 报告，并提供再次导出和明确的退出入口；任何认证参数均不可编辑。
- “取消发布并删除本地数据”收纳在页头“更多发布操作”二级菜单，并使用应用内二次确认；确认后删除仓库内最终成品、全部认证记录、仓库副本和保存配置，但保留记录指向的首次导出文件。
- 识别区域支持撤销并恢复整图识别；识别结果继续提供匹配记录与记录搜索。
- 图片预览、识别和记录搜索均使用独立请求序号丢弃乱序响应；识别 busy 与记录搜索 busy 独立，后台搜索不会解除或阻塞正在进行的图片识别。
- 发布预览和识别预览的文件标题栏使用固定内容高度，图片舞台独占剩余空间；超大分辨率图片不能拉高标题栏。
- 只读记录中的输出路径、Manifest 标签和 SHA-256 允许完整换行，不用省略号隐藏校验信息；窄布局下危险区改为纵向排列，长 Artwork 标题在操作按钮前省略。
- 警告文本使用主题变量，在浅色与深色主题分别保持可读对比度。
- 框选坐标只相对 `object-fit: contain` 后的实际图片矩形归一化；舞台留黑不进入坐标。图片显示矩形由 `ResizeObserver` 随窗口尺寸同步更新，标签禁用文本选择。识别页开始新拖动时立即清除唯一旧框，并在有效拖动完成后写入新框。
- PEM 私钥非空检查在生成质量预览时进入统一操作提示，不常驻显示独立警告行，也不因输入为空而禁用预览按钮。
- 质量预览生成前先检查标题、创作者、证书链和 PEM 私钥；普通滚轮用于浏览放大后的画布，`Ctrl/Command + 滚轮` 执行缩放。图片超出视口时显示导航小图，可点击或拖动视口框快速定位。
- 仓库最终成品预览和大小估算只接收分支 ID，由后端解析受控路径；发布与再次导出目标拒绝仓库内部路径。文件选择器打开的最终成品、证书、外部预览/识别图片和输出目标会进入临时 filesystem scope，原生命令同时核验 scope 和仓库边界；应用重启后 scope 清空，证书等外部路径必须重新选择。filesystem 插件未向前端授予读写命令权限。
