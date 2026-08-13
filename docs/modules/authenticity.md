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

最终成品存在后分支停止提交和自动调度，历史中的发布节点不能被删除或精简。当前版本不提供解除发布状态，避免已有导出记录失去稳定来源。

## C2PA 与 TrustMark

C2PA 是每次认证导出的强制基础，不能关闭。发布版固定输出 JPEG，并包含：

- `art.lilith.artworks.claim` 自定义声明；
- `stds.schema-org.CreativeWork`；
- 最终成品的 `parentOf` ingredient；
- `c2pa.transcoded` action；
- 启用 TrustMark 时的 `c2pa.soft-binding` 和 `c2pa.watermarked` action。

TrustMark 可关闭。启用时复用 Proven 的 Q/BCH-5 模型、61 位二进制 ID、整图基础水印和最多 8 个额外矩形区域，强度范围为 0.50 到 1.50。模型文件不可用时 UI 强制降级为仅 C2PA。私钥只存在于单次命令内并用 `Zeroizing` 包装，不写入配置、数据库或日志。

签名流程先在目标目录生成临时 JPEG，再写 C2PA 并回读验证。只有回读到 manifest 后才发布输出文件和写入记录；数据库失败会删除本次输出文件。

## 导出记录与识别

每条导出记录固定关联分支、发布节点和最终成品，保存时间、输出路径/大小/SHA-256、内容声明、TrustMark ID、范围 JSON、C2PA manifest JSON 和验证状态。记录可按 ID、标题、创作者或输出路径搜索。

识别始终读取 C2PA；模型可用时还会对整图或用户框选区域解码 TrustMark。候选匹配使用 C2PA 声明 ID 或 TrustMark ID 查询全库索引，并显示双通道一致、单通道证据或冲突。点击候选会切换到所属 Artwork、发布分支并高亮具体导出记录。模型解码本身不等于真实性认证，必须结合 C2PA 验证状态和本地记录判断。

## 命令

```text
enter_branch_publication
get_branch_publication
publish_branch_artifact
decode_authenticity
search_certification_records
preview_authenticity_image
```

## 数据版本

schema v5 为 `final_artifacts` 增加非空 `history_id`，为配置增加 `trustmark_enabled`，并把认证记录扩展为不可变的发布快照。v4 的认证表只是未投入使用的预留结构，迁移时重建该表；已有最终成品会保留并绑定其分支当前 head。
