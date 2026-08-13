# 系统架构

## 层级

```text
src/app/                 应用启动、窗口、全局设置与跨模块工作区编排
src/modules/library/     Artwork 树、搜索和选择交互
src/modules/history/     分支历史图、提交、fork、恢复和裁剪
src/modules/authenticity 成品、C2PA/TrustMark 发布与识别
src/shared/              无领域语义的 UI、Tauri 调用和格式化工具

src-tauri/src/app/       配置、窗口和应用生命周期
src-tauri/src/library/   仓库、树、Artwork、分支和历史元数据
src-tauri/src/storage.rs               SQLite 连接、迁移共用能力、路径与 ID 工具
src-tauri/src/library/                 作品仓库树、分组/Artwork、搜索与回收站
src-tauri/src/history/                 分支、历史节点、fork、裁剪和历史图契约
src-tauri/src/backup/                  ChunkFile、增量提交、恢复、调度和取消
src-tauri/src/authenticity/             C2PA、TrustMark、成品锁和认证记录
src-tauri/resources/                    应用图标与随包分发的 TrustMark 模型
```

前端业务模块不能直接互相导入。`ArtworkWorkspace` 在应用层组合历史和认证视图；Library 只负责选中 Artwork 与处理跨作品定位。所有文件系统、数据库和模型操作都在 Rust 中完成。

## 应用生命周期

主窗口关闭默认只隐藏到系统托盘，托盘左键或“打开”命令恢复窗口；只有托盘“退出”或关闭到托盘设置被禁用时才结束进程。窗口隐藏期间备份调度器继续运行。真正退出时应用层必须先保存窗口状态，再停止并等待调度器和其他后台任务。

## 聚合模型

- Library tree：分组和 Artwork 构成任意深度树，Artwork 是叶节点。
- Artwork：作品级聚合根，拥有多个 branch。
- Branch：命名工作线，绑定唯一源文件路径和一个可变 head 指针。
- History node：不可变提交，只有一个父节点，可以被多个后继节点引用；fork 是从选定节点创建新 branch/head。
- Final artifact：分支可选的一份最终成品，固定关联进入发布状态时的 head。存在时分支被冻结，该 head 是强制检查点。
- Certification record：不可变导出快照，固定关联 final artifact、branch 和发布节点，记录 TrustMark ID、输出文件摘要、C2PA manifest 与验证状态。

作品树删除使用数据库软删除。删除一个分组会把其完整子树作为同一个回收站根保存，恢复时优先回到原父分组，原父级不可用时回到根级。只有回收站中的永久删除才清理项目元数据。Artwork 内部的历史节点裁剪属于版本图操作，不进入项目回收站。

历史边的真正所有权属于 history node 的 `parent_id`；branch 只记录当前 head。因此同一祖先可以自然产生多个后继，且无需复制祖先数据。

## 存储布局

```text
<repository>/
  lilith-artworks.sqlite3
  artworks/<artwork-id>/
    history/<history-id>.lbs
    deltas/<child-id>--<parent-id>.lbd
    artifacts/<branch-id>/
    temp/
  temp/
```

数据库持有元数据和仓库内文件的相对路径。snapshot、delta 和最终成品先写临时目录、同步并以不覆盖方式发布，最后在 SQLite 事务中切换引用；认证导出由用户选择外部绝对路径，同时记录 SHA-256。失败时清理本次未引用文件；启动时清理残留临时文件。

## 增量历史策略

继续使用 LilithClient 的内容定义分块与 SHA-256。每个 branch head 保留完整 snapshot；父节点默认可由子 snapshot 加反向 delta 恢复。一个节点被多个分支/子节点引用时，清理逻辑按全图引用决定，不能再采用“旧 head 必删”的线性假设。

历史节点删除分两步：先在事务内计算并删除选定节点的完整后代集合、把受影响分支回退到最近未删除祖先；事务提交后再删除不再引用的文件。中间节点精简只处理单子节点普通节点，会用 ChunkFile 重新生成父子反向 delta 后再原子改接历史边。

## 认证边界

分支最终成品是认证输入，不替代备份历史。进入发布状态时先强制固化当前 head，再把成品复制进仓库并锁定分支。每次导出都强制签入 C2PA，TrustMark 可选；成功回读 C2PA 后才记录认证。TrustMark/C2PA 声明 ID 建普通索引以支持全库快速候选匹配；随后仍需比较记录、文件哈希、C2PA 验证状态和 soft binding，不能把模型解码结果单独视为认证成功。

认证模块只通过公开的 backup/history 能力建立检查点，不读取 ChunkFile；history 只通过 `final_artifacts` 是否存在判断分支锁定，不导入认证实现。前端跨 Artwork 的溯源跳转由应用层完成，认证模块不导入 Library。
