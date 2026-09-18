# 当前任务交接

更新时间：2026-09-18

本文件只记录**当前批次**的执行状态与人工验收结果。已完成批次进 `archive/`，
未完成事项集中到 `todo.md`。

## 当前基线

- 应用版本 `0.2.0-alpha.2`，repository schema **v2**，应用标识 `com.lilith.artworks`。
- `v0.1.0` 已发布；发布标签与资产一经公开即不可移动、覆盖或复用。任何发布后的代码、schema
  或签名声明变化都必须使用新的版本号和标签，并按 `docs/guides/release-policy.md` 走发布流程
  （发布说明由 `tools/release/write-release-notes.mjs` 生成，未签名时必须在发布说明中披露）。
- 项目定位：平面美术个人项目的**资源、版本管理与发布**工具。领域模块为 Library（作品树）、
  History/Backup（分支与增量历史）、Authenticity（成品与 C2PA/TrustMark）、Pin-board（素材板）。

## 本轮批次：素材板验收、备份可用性与文案（待人工验收）

维护者已完成素材板迁入后的完整编译与 GUI 验收（覆盖导入/导出、大图、缓存、回收站、整仓灾备
与灾备恢复后画板可用），确认功能正常。在此结论上完成本批次：

1. **未选择工作文件时的自动备份**：分支设置中自动备份开关在 `sourcePath` 为空时置灰并强制关闭，
   间隔输入同步禁用；点击保存只写回关闭状态。Rust 侧 `history::update_branch` 同样兜底
   （空路径分支的 `backup_enabled` 一律落库为 0），避免出现"开关是开的但永远不会备份"。
2. **工作文件路径按钮**：新增"清除工作文件路径"按钮，紧邻原有的"修改工作文件"按钮；路径清空后
   `source_path` 与 `source_path_key` 一并写空。未选择文件时，原图标按钮改为醒目的主按钮
   "选择文件"，降低漏发现概率。
3. **清空路径的约束**：`source_path_key` 非空且与 `artwork_id` 唯一，因此同一 Artwork 只能有一个
   分支不设置工作文件；第二个分支清空路径会被拒绝并返回明确错误，事务整体回滚。
4. **文案统一**：界面与文档中残留的 fork 表述统一为中文"分支 / 创建分支 / 分支起点"
   （`从此处创建分支`、`<分支名> 分支起点`、`分支 head、分支起点和分叉点`）；代码标识符、
   Tauri 命令名和 SQLite 约束名保持不变。
5. **文档重整**：素材板规划与对比报告进归档，各批次记录合并为一份归档；架构与模块文档只保留
   当前有效的契约并修正了三处不准确描述（素材板写锁的组成、整仓灾备对 `boards/` 的覆盖范围、
   workspace 命令分层中缺少 `pin_board`）；未完成事项集中到 `docs/planning/todo.md`。

## 验证记录

代理侧已执行：

- `npx tsc --noEmit`：通过。
- `npm test`：**104 通过**（新增"无工作文件时自动备份置灰关闭"与"清除工作文件路径"两个用例）。
- `cargo test --lib`：**110 通过**，1 个忽略项（新增"清空路径同时关闭自动备份"与
  "同一 Artwork 只允许一个分支留空"两个用例）。
- `cargo fmt --check`、`git diff --check`：通过。
- `node tools/release/verify-metadata.mjs`：通过（`v0.2.0-alpha.2`、`com.lilith.artworks`、schema v2）。

**待维护者人工确认**：分支设置中新按钮的位置与视觉权重、"选择文件"主按钮是否足够醒目、
清空路径后的提示是否符合预期。代理不执行 UI 自动化，这些项只做过程序化断言。

## 文档约束

当前功能契约以 `docs/architecture/`、`docs/modules/` 和 `docs/guides/` 为准；
未完成事项只写入 `docs/planning/todo.md`；本文件只记录当前批次状态与人工验收结果；
已完成或被替代的计划进入 `docs/planning/archive/`。
