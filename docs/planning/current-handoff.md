# 当前任务交接

更新时间：2026-09-13

## 当前发布基线

`v0.1.0` **已发布**，使用 repository schema v1 和应用标识 `com.lilith.artworks`。发布标签与资产一经公开即不可移动、覆盖或复用；任何发布后的代码、schema 或签名声明变化都必须使用新的版本号和标签，并按 `docs/guides/release-policy.md` 走发布流程（发布说明由 `write-release-notes.mjs` 生成，未签名需在发布说明中披露）。

版本基线重置批次（应用版本与 schema 重置为 0.1.0 / v1、历史候选版与迁移链清除、维护者验收与发布确认）已归档到 `docs/planning/archive/version-reset-2026-09-12.md`。

## 素材板设置/快捷键/视图适配与拖放排序批次（待人工验收）

在 0.2.0-alpha.1 基础上，由维护者第二轮反馈的 4 项问题已修复，并补齐原 Client 具备的侧栏画板拖放排序；与 Client 原模块的逐项差异见 `docs/planning/pin-board-client-comparison.md`。完整编译与 GUI 手工验收尚未执行：

1. 设置弹窗改为单列纵向布局：分页后不再沿用 Client 的两列紧凑排版，`.settings-content` 去掉两列网格，外观页两个下拉也改为单列并统一片段间距；
2. 锁定画板快捷键恢复 Client 默认 `Ctrl+R`：新增应用层 `src/app/webviewShortcuts.ts`（只取消默认行为、不停止传播）拦截 F5 / `Ctrl+R` 整页刷新，移除素材板模块内提前吞掉 `Ctrl+R` 的分支，使 `Ctrl+R` 能正常命中锁定；设置文件版本升到 v2，读取 v1 时把仍是旧默认值 `CommandOrControl+Shift+K` 的锁定键位迁移为 `CommandOrControl+R`（其余自定义键位保留）；
3. 修复“图片过小 / 进入不自动适配视图”：素材板工作区面板改用 keep-alive 可见性隐藏（`visibility` + 绝对定位）代替 `display:none`，画布始终保有布局尺寸，渲染器首次创建即按未删除图片的最小包围框完成适配；会话写入增加“画布无布局尺寸则不落盘”守卫，杜绝退化视口被恢复；
4. 设置页纹理缓存等级容量标注由 64/128/256 MiB 改为与 Client 一致的约 256 MB / 512 MB / 1 GB（含 Rust 解码缓存与前端 GPU 常驻缓存的总量）；
5. 侧栏画板拖放排序（对齐 Client）：新增 `reorder_pin_boards(artworkId, boardIds)` 命令与 `repository::reorder_boards`，要求传入 id 集合与当前未删除画板完全一致（缺项/重复/跨作品一律拒绝）；前端按落点的上/下半区决定插入前/后，先乐观重排再落库、失败回滚。排序只改 `sort_order`、不改 `revision`，因此不会打断已打开画板的保存。

已验证：`npx tsc --noEmit`、`npm test`（102 通过，含新增 webviewShortcuts、拖放排序与锁定默认键位用例）、`cargo test --lib`（108 通过，含 1 个忽略项，新增排序集合校验与设置 v1→v2 迁移用例）、`cargo fmt --check`、`git diff --check`。

## 素材板迁移后修复批次（待人工验收）

0.2.0-alpha.1 迁移后由维护者反馈的 8 项问题已修复，完整编译与 GUI 手工验收尚未执行：

1. Artwork 工作区标签顺序调整：素材板位于版本历史之前；
2. 设置弹窗改为左侧导航分页（通用 / 仓库与备份 / 素材板），素材板页排版沿用 Client 的行式布局与分段控件；
3. 素材板全屏修复：capabilities 放行 `core:window:allow-set-fullscreen` 与 `allow-is-fullscreen`；
4. 未选择画板时画布区域显示占位提示，不再一直显示加载动画；
5. 画板移入回收站时以 `destroy(false)` 跳过对已删除画板的 finalize 保存，消除时序失败；
6. 剪贴板/导入 DDS 落盘前自动创建缺失的画板目录（迁移仓库目录缺失导致的 os error 3）；
7. 素材板锁定/全屏快捷键可在设置中自定义（默认 `Ctrl+Shift+K` / `F11`），素材板活跃时屏蔽 `Ctrl+R` 整页刷新；
8. 允许不选择工作文件创建 Artwork：`source_path` 为空时自动备份调度与主动提交不可用（UI 禁用并提示、调度查询排除、worker 兜底报错），素材板功能不受影响。

已验证：`npx tsc --noEmit`、`npm test`（95 通过，含新增 settingsShortcuts 与标签顺序用例）、`cargo test --lib`（104 通过，含空工作文件创建/调度排除/DDS 目录自建/快捷键校验新用例）、`cargo fmt --check`、`git diff --check`。

## 素材板迁移（0.2.0-alpha.1，待人工验收）

素材板（pin-board）模块已按 `docs/planning/pin-board-plan.md` 迁入：schema v2（`pin_boards` / `pin_board_images` / `pin_board_history`，v1 追加式迁移）、`src-tauri/src/pin_board/` 持久化与 DDS 处理、`src/modules/pin-board/` 前端模块与 Artwork 工作区挂载（keep-mounted）、画板回收站与 `pending_file_cleanup` 接入、设置项（缓存等级/阵列间距）、模块文档与自动测试。应用版本已提升为 `0.2.0-alpha.1`，`verify-metadata.mjs` 的 schema 断言同步为 v2。

已验证：`npm test`（90 通过）、`cargo test --lib`（98 通过，含 1 个忽略项与素材板迁移用例）、`node tools/release/verify-metadata.mjs`。**完整编译与 GUI 手工验收尚未执行**，由维护者按模块文档“快速验证”一节进行（导入/导出、大图、缓存、回收站、灾备恢复后画板可用）。

仍待完成（对应规划 P4）：

- 整仓灾备复制 `boards/` 目录并入备份清单校验；仓库完整性扫描（scrub）覆盖画板 DDS 文件；
- 规划中的“导入导出/保存走互斥写锁”目前经 `with_ready_repository` 实现，但纹理读取等长任务与灾备调度并行的降级策略（缩略图兜底提示）未评估，实测卡顿不可接受时再处理；
- Lilith Client 侧旧实现保持原样，待本模块验收后由维护者决定是否从 Client 移除。

## 下一阶段入口

- `0.2.x` 后续工作继续遵循"`SCHEMA_VERSION` 递增 + 追加式迁移 + 新建仓库直接以当前版本落库 + `verify-metadata.mjs` 断言同步"，并在 `CHANGELOG.md` 写 Compatibility 条目。
- 后续版本仍需覆盖的验收项：极端大文件与高像素压力、各处理阶段取消、异常 RFC 3161 服务、损坏文件恢复、普通用户安装和 Authenticode 签名，并记录可复查证据。
- 项目规则继续禁止代理执行 GUI 自动化；桌面交互、真实模型、第三方 C2PA 回读和安装包验收由维护者手工执行。
- 观察项：`repository/temp` 中存在认证预览遗留目录（约 40MB），可考虑在打开仓库时清理超期临时目录。

## 文档约束

当前功能契约以 `docs/architecture/`、`docs/modules/` 和 `docs/guides/` 为准。已完成的批次写归档，不再保留在本文件；本文件只记录尚未完成的工作和人工验收结果。
