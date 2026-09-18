# 素材板模块迁入与后续修复（2026-09-12 ~ 2026-09-14）

本文件归档素材板（pin-board）从 Lilith Client 迁入 Artworks 的完整过程与各批次结果。
当前有效契约见 `docs/architecture/`、`docs/modules/` 和 `docs/guides/`；未完成项见
`../todo.md`，本文件不再作为执行依据。

## 背景

素材板是自由画布图片工作台，原先位于 Lilith Client（`F:\programs\Lilith Client`），与该应用
"个人工具箱"的定位不符。2026-09-12 由维护者确认迁入 Artworks，作为第 5 个领域模块，并按
"交互层原样迁移、数据层重写"的方式重构持久化。原始规划与实施后的逐文件对比分别见
`pin-board-plan-2026-09-12.md` 和 `pin-board-client-comparison-2026-09-13.md`。

## 批次一：模块迁入（0.2.0-alpha.1）

- schema v1 → v2 追加式迁移，新增 `pin_boards` / `pin_board_images` / `pin_board_history`
  三张表，不改动既有表；`verify-metadata.mjs` 的 schema 断言同步为 v2。
- `src-tauri/src/pin_board/` 新持久化层：画板 CRUD、图片记录、step 历史、BC7 DDS 写入与纹理缓存。
- `src/modules/pin-board/` 前端模块整体迁入，挂载为 Artwork 工作区标签页并采用 keep-mounted 生命周期。
- 画板回收站接入 `pending_file_cleanup`，DDS 目录按 `repository_directory` 条目清理并可重试。
- 设置项新增纹理缓存等级与阵列间距；应用版本提升为 `0.2.0-alpha.1`。
- 验证：`npm test` 90 通过、`cargo test --lib` 98 通过（含 1 个忽略项）、`verify-metadata.mjs` 通过。

## 批次二：迁移后修复（0.2.0-alpha.1）

维护者反馈的 8 项问题：

1. Artwork 工作区标签顺序调整，素材板位于版本历史之前。
2. 设置弹窗改为左侧导航分页（通用 / 仓库与备份 / 素材板）。
3. 素材板全屏修复：capabilities 放行 `core:window:allow-set-fullscreen` 与 `allow-is-fullscreen`。
4. 未选择画板时画布区域显示占位提示，不再一直显示加载动画。
5. 画板移入回收站时以 `destroy(false)` 跳过对已删除画板的 finalize 保存，消除时序失败。
6. 剪贴板/导入 DDS 落盘前自动创建缺失的画板目录（修复迁移后目录缺失导致的 `os error 3`）。
7. 素材板锁定/全屏快捷键可在设置中自定义（默认 `Ctrl+Shift+K` / `F11`），素材板活跃时屏蔽
   `Ctrl+R` 整页刷新。
8. 允许不选择工作文件创建 Artwork：`source_path` 为空时自动备份调度与主动提交不可用
   （UI 禁用并提示、调度查询排除、worker 兜底报错），素材板功能不受影响。

验证：`npx tsc --noEmit`、`npm test`（95 通过）、`cargo test --lib`（104 通过）、
`cargo fmt --check`、`git diff --check`。

## 批次三：设置/快捷键/视图适配与拖放排序（0.2.0-alpha.1）

维护者第二轮反馈的 4 项问题，并补齐 Client 原有的侧栏画板拖放排序：

1. 设置弹窗改为单列纵向布局，不再沿用 Client 的两列紧凑排版。
2. 锁定画板快捷键恢复 Client 默认 `Ctrl+R`：新增应用层 `src/app/webviewShortcuts.ts`
   （只取消默认行为、不停止传播），移除模块内提前吞掉 `Ctrl+R` 的分支；设置文件升到 v2，
   读取 v1 时把仍是旧默认值 `CommandOrControl+Shift+K` 的锁定键位迁移为 `CommandOrControl+R`。
3. 修复"图片过小 / 进入不自动适配视图"：工作区面板改用 keep-alive 可见性隐藏
   （`visibility` + 绝对定位）代替 `display:none`；会话写入增加"画布无布局尺寸则不落盘"守卫。
4. 设置页纹理缓存等级容量标注改为总量口径（约 256 MB / 512 MB / 1 GB）。
5. 侧栏画板拖放排序：新增 `reorder_pin_boards(artworkId, boardIds)` 与
   `repository::reorder_boards`，要求传入 id 集合与当前未删除画板完全一致；排序只改
   `sort_order`、不改 `revision`，不打断已打开画板的保存。

验证：`npx tsc --noEmit`、`npm test`（102 通过）、`cargo test --lib`（108 通过，含 1 个忽略项）、
`cargo fmt --check`、`git diff --check`。

## 批次四：素材板功能验收（0.2.0-alpha.2）

维护者完成完整编译与 GUI 验收（含整仓灾备与恢复），确认素材板迁入后全部功能正常，项目定位随之
从"版本管理与发布"扩展为"平面美术个人项目的资源、版本管理与发布"。本轮一并修复：

- 未选择工作文件时自动备份开关置灰并强制关闭，新增"清除工作文件路径"按钮，并把未选择文件时的
  "选择文件"入口改为醒目主按钮；
- 界面与文档中的 fork 表述统一为中文"分支 / 创建分支 / 分支起点"。

## 相关设计决策（仍然有效，已并入模块文档）

| 决策点 | 结论 |
| --- | --- |
| 归属 | 素材板以 Artwork 为单位；一个 Artwork 内多块画板，平铺一层，无文件夹嵌套 |
| 存储 | 存入作品仓库（不设独立素材库路径），schema v2 |
| 版本边界 | 画板不纳入分支历史：不进增量提交、不参与恢复/裁剪 |
| 图片格式 | 保留 BC7 DDS 实体文件；SQLite 只存元数据与相对路径 |
| 撤销/恢复 | 沿用 step 模型并持久化到 SQLite，跨应用重启可用 |
| 画板删除 | 画板回收站：软删除 → 可恢复/永久删除/清空；DDS 经 `pending_file_cleanup` 清理 |
| 旧库迁移 | 不迁移 Lilith Client 的 `index.json` 画板库，新模块从空仓库起步 |
