# 2026-11-03 一次性无头探针验收（0.3 人工清单中可程序判定的部分）

维护者授权把「能程序判定」的人工验收项改用无头入口与探针处理，界面观感仍走人工。本轮不改产品
行为：为覆盖两个没有无头命令的功能，临时加了 `reset-artwork-history`、`drop-stage-sweep` 两个
子命令，并给 `checkpoint` 加了 `--release` 开关；探针脚本、临时子命令与构建产物在本轮结束后全部
删除/回退，仓库不留任何一次性代码。

## 1. 方法与边界

- 被测对象是 `cargo build --features headless` 产出的调试档二进制（构建 2m02s），以**独立进程**
  调用，只通过命令行、SQLite 事实与磁盘事实断言，不启动 GUI、不模拟输入。
- 数据库侧用 Node 内置 `node:sqlite` 只读打开 `lilith-artworks.sqlite3`，核对 `branches` /
  `history_nodes` / `final_artifacts` / `pin_board_items` / `library_nodes` 的行与字段。
- 探针脚本与工作区放在 `src-tauri/target/probe/`（已 gitignore），验收后连同工作区一起删除。
- 前端侧不新增测试：先核对既有测试的覆盖，重复的不再写（见第 4 节）。

## 2. 场景与结果

十个场景共 51 条断言，50 条通过、1 条失败（失败项见第 3 节）。

| 场景 | 断言 | 结果 |
| --- | --- | --- |
| S1 树与历史基础链路 | 建库、建作品与主分支、三次提交、建分组、从节点分叉、搜索、检查点落 snapshot、精简中间节点、恢复首节点字节一致、`scrub`、`verify`、回收站移入与清空 | 12/12 通过 |
| S2 取消检查点 | 取消后 `snapshot_path` 置空且 `is_checkpoint` 归 0、增量已重建、快照文件不再被引用、被取消节点与后续节点仍可恢复且字节一致、`verify` 通过 | 5/5 通过 |
| S3 取消发布 | 进入发布写出仓库内副本、未变化时取消回收副本（`cleanedCount=1`）、回收后垃圾扫描零候选、副本被替换时保留文件（`pendingCount=0`）、**被保留的副本出现在垃圾扫描结果里**、确认清理后文件消失、`verify` 通过 | 6/7 通过，1 条失败 |
| S4 历史重置 | 重置后剩一条空分支零节点、新分支不设工作文件、作品名/描述/树位置不变、素材板条目不受影响、旧 snapshot/delta 从磁盘回收、`verify` 通过、素材板导入前提成立、重复重置幂等、重置后历史查询可用 | 10/10 通过 |
| S4b 已发布拒绝重置 | 进入发布后重置被拒绝（错误文案指向「先取消发布」），历史节点数不变 | 1/1 通过 |
| S5 拖放暂存目录 | 启动清扫回收超期目录（`reclaimed=1`）、不动宽限期内与非法命名的目录、退出删除本次会话目录 | 3/3 通过 |
| S6 schema 上限 | 正常仓库 `verify` 通过；`schema_version` 改为 99 后拒绝打开并给出可读错误 | 2/2 通过 |
| S7 识别输入 | PNG 可读入并返回「无认证声明」、非图片给出可读错误、截断 PNG 给出可读错误、识别失败不影响仓库完整性 | 4/4 通过 |
| S8 素材板数据侧 | 建画板、导入图片、创建笔迹、读取整板笔迹、`scrub-board-dds`、`verify` | 6/6 通过 |
| S9 整仓灾备 | 灾备产出备份目录 | 1/1 通过 |

补充事实（探针顺带取得，供文档核对）：

- 垃圾文件扫描有 30 分钟宽限期（`cleanup::SCAN_GRACE_MS`）：刚被替换的副本不会立刻出现在候选里，
  探针把 mtime 调到 40 分钟前才复现「可被发现」的前提。
- `repository-backup` 的目标目录必须已存在且是绝对路径，否则以「备份保存目录不存在或不是目录」
  失败；命令本身不创建目录。

## 3. 唯一失败项：被保留的成品副本不会被垃圾扫描发现

S3.5 连续两次失败，不是探针写错：

- `cleanup::scan_unreferenced` 遍历 `artworks/<id>/` 后交给各领域 `ReferenceInventory::scan_candidates`；
  历史清单扫 `snapshots/`、`deltas/`、`temp/`，素材板清单扫画板 DDS，而
  `ArtifactReferenceInventory`（`src/authenticity/contracts.rs`）**只实现了 `collect_targets`，没有实现
  `scan_candidates`**，因此走默认空实现。
- 结果：取消发布时因内容被替换而保留在 `artworks/<id>/artifacts/<branch>/` 下的成品副本，以及
  `certifications/` 下的认证副本，都不会出现在设置页「空间」页的垃圾扫描候选里；用户只能看到磁盘
  被占用，找不到可确认清理的入口（给 `cleanup-unreferenced` 显式传路径仍能删掉，但没有入口拿到路径）。
- 与 `docs/modules/authenticity.md` 的描述冲突：该文写「由设置页『空间』页的垃圾文件扫描负责发现」。
- 因此 `todo.md` 原「第七批」验收项里的这一句无法通过，已改为缺陷项挂起。

同一项里另两句的判定：`pendingCount=0` 且命令成功，前端不会走失败横幅，数据侧成立；日志行
`[cleanup] retained` 无法断言——无头入口不装日志器（`handoff-log-2026-10-25.md` §10.4 已记同一限制），
仍留人工。

## 4. 既有自动化已覆盖的部分（本轮未新增测试）

核对后确认以下验收项的逻辑侧已有测试，不再重复编写：

| 验收项 | 既有覆盖 |
| --- | --- |
| 精简模式选择与确认、批级取消 | `HistoryControls.test.tsx`：`keeps the compact selection on a click confirmation`、`puts irreversible history deletions behind the slide control`、`routes the running-state cancel to the operation instead of closing` |
| 取消提示两句文案 | `useAuthenticityOperation.test.ts`、`usePublicationController.test.ts`、`PublicationView.test.tsx` 均断言「质量预览已取消。」与「签名发布已取消。」 |
| 只读记录（删除记录、损坏预览、框选范围提示） | `PublicationView.test.tsx` 的四个用例 |
| 长任务 busy 拒绝 | `useRepositoryMaintenance.test.tsx`：`refuses to start a scan while another backup operation is running` |
| 错误边界（捕获、写日志、重试恢复、resetKeys） | `ErrorBoundary.test.tsx` 四个用例 |
| 历史重置的二次确认与滑条 | `HistoryControls.test.tsx`：`puts the whole-artwork reset behind a click and then the slide control` |
| 缩略图 `.sai2` / `.tga` / 扩展名决定是否支持 | `preview/sai2.rs` 的 intg 用例与 `preview/mod.rs` 的 `tga_is_decoded_through_its_extension`、`preview_support_follows_the_source_extension` |

本轮跑过的完整前端检查：`npx tsc --noEmit` 通过；`npm test` 56 个文件 / 502 项全部通过。

## 5. 覆盖结论

- 数据侧已可判定：取消检查点、历史重置（含已发布拒绝）、拖放暂存两条路径、schema 上限拒绝、
  识别输入的错误面、素材板数据侧、整仓灾备、树与历史全链路。
- 仍需人工：界面观感与手感（顶条拖动、窗口按钮、兜底卡片位置、热更新、放置模式的光标与提示、
  画板交接的提示条、缩略图在「节点信息」栏的显示）、自动备份 10 秒静默的界面时机、预览缓存
  512 MiB 上限（无头入口没有预览命令）、拖放导入的真实拖放手势、发布页与识别页的实际操作。
- 未能断言：日志类结论（无头入口不装日志器）、签名与安装包相关门槛。

## 6. 清理

探针脚本与全部工作区、`target/debug` 下本次构建、两个临时子命令与 `checkpoint` 的 `--release`
开关全部删除/回退；仓库中不保留任何一次性代码或测试（`git status` 只剩文档改动）。
