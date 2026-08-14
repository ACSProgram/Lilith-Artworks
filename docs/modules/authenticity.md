# 成品与真实性模块

## 上下文入口

- Artwork 级页面切换与跨作品跳转：`src/app/ArtworkWorkspace.tsx`、`src/modules/library/LibraryModule.tsx`。
- 发布、局部范围和识别 UI：`src/modules/authenticity/AuthenticityModule.tsx`，样式只读 `src/styles/authenticity.css`。
- 前端 DTO 与 Tauri 命令：`src/modules/authenticity/types.ts`、`api.ts`。
- 发布/识别流水线：`src-tauri/src/authenticity/pipeline.rs`。
- C2PA 和 TrustMark 实现：`src-tauri/src/authenticity/c2pa.rs`、`trustmark.rs`。
- 成品、配置、导出记录和全库匹配：`src-tauri/src/authenticity/repository.rs`。
- 强制发布检查点：认证命令调用 `backup::ensure_checkpoint`，不要为认证表查询读取 `chunk_file.rs`。

当前未验收项和人工检查清单只读 `docs/planning/current-handoff.md`。

## 发布状态

分支必须先有一个 head 才能进入发布状态。用户选择最终成品后，命令在统一备份运行锁内完成以下步骤：

1. 把当前 head 物化并标记为强制检查点。
2. 流式复制最终成品到 `artworks/<artwork-id>/artifacts/<branch-id>/`，同步文件并计算 SHA-256。
3. 在事务中确认 head 未变化，再插入唯一 `final_artifacts` 行并关联发布节点。

最终成品存在后分支停止提交和自动调度，历史中的发布节点不能被删除或精简。用户可在二次确认后解除发布状态；命令在统一运行锁内删除该分支的认证记录、仓库最终成品副本、认证 JPG 副本和记录指向的已导出文件，之后分支恢复提交和调度。

## C2PA 与 TrustMark

C2PA 是每次认证导出的强制基础，不能关闭。发布版固定输出 JPEG，并包含：

- `art.lilith.artworks.claim` 自定义声明；
- `stds.schema-org.CreativeWork`；
- 最终成品的 `parentOf` ingredient；
- `c2pa.transcoded` action；
- 启用 TrustMark 时的 `c2pa.soft-binding` 和 `c2pa.watermarked` action。

TrustMark 可关闭。启用时使用 Q/BCH_SUPER 模型和 40 位二进制 ID，只在用户框选的最多 8 个矩形区域内嵌入，强度范围为 0.50 到 1.50；没有框选区域时自动降级为仅 C2PA。模型文件不可用时 UI 也强制降级为仅 C2PA。模型变体和编码/解码模型 SHA-256 由后端读取后显示，不在前端写死。私钥只存在于单次命令内并用 `Zeroizing` 包装，不写入配置、数据库、localStorage 或日志。

签名流程先在目标目录生成临时 JPEG，再写 C2PA 并回读验证。只有回读到 manifest 后才发布输出文件和写入记录；数据库失败会删除本次输出文件。

## 导出记录与识别

每条导出记录固定关联分支、发布节点和最终成品，保存时间、首次输出路径、仓库内认证 JPG 副本、大小/SHA-256、内容声明、TrustMark ID、范围 JSON、C2PA manifest JSON 和验证状态。查看和再次导出优先读取仓库副本，不依赖首次输出路径；schema v5 旧记录没有副本时仍回退读取原输出文件，原文件也不存在时明确报告不可用。记录可按 ID、标题、创作者或首次输出路径搜索。

识别始终读取 C2PA；模型可用时还会对整图或用户框选区域解码 TrustMark。候选匹配使用 C2PA 声明 ID 或 TrustMark ID 查询全库索引，并显示双通道一致、单通道证据或冲突。点击候选会切换到所属 Artwork、发布分支并高亮具体导出记录。模型解码本身不等于真实性认证，必须结合 C2PA 验证状态和本地记录判断。

## 命令

```text
enter_branch_publication
get_branch_publication
cancel_branch_publication
publish_branch_artifact
decode_authenticity
search_certification_records
preview_authenticity_image
preview_certification_record
export_certification_record
estimate_authenticity_output_size
```

## 数据版本

schema v6 为认证记录增加可空 `stored_path`。新记录把签名后的 JPG 原子保存到 `artworks/<artwork-id>/artifacts/<branch-id>/certifications/`；从 v5 迁移的旧记录保持 `NULL`，不自动复制外部文件。schema v5 为 `final_artifacts` 增加非空 `history_id`，为配置增加 `trustmark_enabled`，并把认证记录扩展为不可变的发布快照。
## 本轮实现补充（2026-08-13）

TrustMark 使用 Q / BCH_SUPER，标识长度为 40 位。水印只写入用户在预览中框选的区域；没有框选区域时不会启用 TrustMark，发布仍保留 C2PA。发布页提供强度与 JPEG 质量滑条、质量损失提示、大小预览及模型哈希摘要；识别页展示完整 C2PA manifest，导出记录支持自动搜索和详细字段查看。
## 前端交互约定

- 当前分支由 `ArtworkWorkspace` 统一持有；历史页和发布页共用同一选择，任一页面切换分支都会同步到另一页面。发布、进入发布和取消发布后刷新 Artwork 工作区分支数据，发布状态与当前内容保持同步。
- PEM 私钥仅以密码输入控件接收，并且发布完成后清空，不写入共享配置。
- TrustMark 仅在存在框选区域时启用；首次完成框选自动启用，清空全部区域自动关闭。
- JPG 质量、预估大小和透明背景色位于 TrustMark 之前；TrustMark 区明确提示在左侧图片拖动框选区域，避免把开关误解为唯一入口。
- 点击导出记录进入只读查看模式。查看页复用发布编辑页的左右分栏和图片预览结构，显示锁定字段、框选范围、发布节点、文件摘要和 C2PA 报告，并提供再次导出和明确的退出入口；任何认证参数均不可编辑。
- “取消发布并全部删除”是分支发布内容的总删除入口，独立放在导出记录之后的危险区，并使用应用内二次确认；确认后删除最终成品、全部认证记录、仓库副本及记录指向的导出文件。
- 识别区域支持撤销并恢复整图识别；识别结果继续提供匹配记录与记录搜索。
- 发布预览和识别预览的文件标题栏使用固定内容高度，图片舞台独占剩余空间；超大分辨率图片不能拉高标题栏。
- 只读记录中的输出路径、Manifest 标签和 SHA-256 允许完整换行，不用省略号隐藏校验信息；窄布局下危险区改为纵向排列，长 Artwork 标题在操作按钮前省略。
- 警告文本使用主题变量，在浅色与深色主题分别保持可读对比度。
- 框选坐标只相对 `object-fit: contain` 后的实际图片矩形归一化；舞台留黑不进入坐标。图片显示矩形由 `ResizeObserver` 随窗口尺寸同步更新，标签禁用文本选择。识别页开始新拖动时立即清除唯一旧框，并在有效拖动完成后写入新框。
- PEM 私钥非空检查在点击发布命令时进入统一操作提示，不常驻显示独立警告行，也不因输入为空而禁用发布按钮。
