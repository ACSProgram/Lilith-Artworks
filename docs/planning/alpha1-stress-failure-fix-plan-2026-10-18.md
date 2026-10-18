# alpha1 压力测试未通过项修复计划

制定日期：2026-10-18
适用基线：`dev` 上的 `0.3.0-alpha.1`（repository schema v4，ChunkFile 默认 v2）
范围：只处理 2026-10-18 release 档整套复跑中**未通过**的 8 条记录（B1/B2/B4/B5、C1、
C2/C4/C6、C5、G4），外加维护者本次明确要求的一项产品侧改动：小选区水印警告（§3.4，归阶段二）。
不涉及 alpha2 的其他功能改动，不改持久 schema。

本文是**计划**，不是验收记录。逐条失败的原文与断言见
`docs/user/stress-test-report.md` 的「本次运行未通过的项」；待决策点原本记在
`docs/planning/todo.md` 的「alpha1 未通过项与度量口径」。本文在它们的基础上给出**已确认
的根因**与修复口径。

## 一、结论速览

| 场景 | 已确认的根因 | 性质 | 修复归属 |
| --- | --- | --- | --- |
| B1 / B5 | 热循环取消检查（`ChunkTask`）每次都发一枚**交互检查点**，检查点总数随分块数膨胀；测试按固定序号强杀，强杀点已漂移到「发布之前」 | 产品侧（`headless` 门控路径） | §3.1 |
| B4 | 同上：事务标记点是第 **12** 个检查点，测试按第 5 个强杀，从未落到标记点 | 产品侧 | §3.1 |
| B2 | 同上：导出临时文件在检查点 10 之前就已创建，测试按第 6 个强杀，落在「临时文件尚未创建」处 | 产品侧 | §3.1 |
| C1 | 账本「实测占用」把**测试夹具自身**的 `--marker` 文件计入工作区；该文件按分块逐条追加，21 节点链的 `scrub` 可写数十 MB | 测试口径 | §3.1（同源，随 §3.1 一起消失） |
| C5 | 同上：一次 `restore` 的 `--marker` 文件实测 **3.31 MB**，与断言缺口 3.29 MB 一致 | 测试口径 | §3.1 |
| C2 / C4 / C6 | `assert_snapshot_matches_logical` 仍按 v1 口径断言「不压缩、不去重」；v2 对稀疏夹具里的全零区去重并压缩，快照合法地小于逻辑大小 | 测试断言过时 | §3.2 |
| G4 | 128×128 / JPEG q90 下 TrustMark **像素域回读**约 5% 失败（水印 ID 每次发布随机，失败与水印载荷相关）；C2PA soft binding 始终匹配 | 水印引擎在小图上的固有鲁棒性 / 测试夹具口径 | §3.3 |
| 小选区水印警告（新增需求） | 回读率随选区边长急剧下降（96 级 23/30 失败、128 级 2/40、192 级 1/40、256 级 0/40），当前发布对小选区毫无提示 | 产品侧新增（维护者要求） | §3.4 |

关键点：B/C1/C5 的六条失败**同源**——都源于「热循环取消检查 = 交互检查点」这一件事；
一次产品侧修复即可同时消解它们，不需要改动 C 组与 B 组的断言。

**阶段划分（本次按两个阶段实施）**

- **阶段一：让 alpha1 的未通过项清零** = §3.1（检查点分层，产品侧）+ §3.2（快照体积断言与新增
  密集不可压缩用例，测试侧）+ §3.3（G4 夹具，测试侧）。完成后 B1/B2/B4/B5、C1、C5、C2/C4/C6
  全部转绿，alpha1 可以收尾；顺带修复 §3.1 提到的 A1/A3/A6/C7「通过但覆盖位置已漂移」。
- **阶段二：新增小选区水印警告** = §3.4（产品侧新增，维护者要求）。独立增量：不动阶段一的任何
  断言与产品路径，可紧随阶段一之后实现，不阻塞「未通过项清零」。
- 两者互不依赖（改动面分别落在 `headless`/`backup` 与 `authenticity`），共用同一批便宜验证
  （§4.3）。阶段一的退出条件是九组便宜压力测试与 `gate_guard` 全绿、`stress_large` 的
  `default`（必要时 `large`）档存储类断言转绿；阶段二是 CI 的 `stress_authenticity` 与前端
  测试全绿。

## 二、证据（本次实测，可复现）

以下三组实验都用**已有的** `target/release/lilith-artworks.exe`（含 `headless` feature）
直接驱动，不重新编译、不跑整套压力测试；实验工作区建在 `target/` 下并已删除。

### 2.1 检查点序列：commit（第二次提交，80 KiB 工作文件）

```text
checkpoint index=1..11  stage=started command=commit
checkpoint index=12     stage=事务已写入，尚未提交
```

12 个检查点，而测试与文档期望 5 个：`run_backup` 的边界只有 5 处
（`worker.rs` 的 183 / 208 / 303 / 311 与 `history::commit` 的事务标记点
`repository.rs:541`），其余 7 枚来自分块循环内部的 `task.check()`。

### 2.2 检查点序列：restore（2 跳链，96 KiB）

```text
checkpoint index=1..8   stage=started command=restore
checkpoint index=9      stage=正在计算历史节点 1/2
checkpoint index=10     stage=正在导出恢复文件 0/98304     ← 输出临时文件已创建
checkpoint index=11     stage=正在导出恢复文件 0/98304
checkpoint index=12..14 stage=正在导出恢复文件 N/98304
```

B2 断言「第 5 个检查点 = 创建输出临时文件之前、第 6 个 = 导出并 sync 之后」；实测这两枚
都还在链解析之前（阶段名仍是 `started command=restore`），输出临时文件尚未创建，
输出目录因此为空——这正是 `实际 0` 的来源。

### 2.3 工作区占用：测试夹具自身的 marker 文件

| 步骤 | 该步骤的 `marker` 文件大小 |
| --- | --- |
| 一次 commit（64 MiB 工作文件） | ≈ 26 KiB |
| 一次 scrub（3 节点链） | **5.25 MB** |
| 一次 restore（64 MiB 输出） | **3.31 MB** |

C5 的断言缺口是 3,288,905 字节，与该次 restore 的 marker 大小一致；C1（`default` 档）
缺口 36.3 MB，约等于「3 节点 5.25 MB × 21 节点 ≈ 36.75 MB」。`large` 档缺口 145.8 MB 也正好
是 `default` 的 4 倍（节点数相同、单节点体积 4 倍）。

### 2.4 G4 复现率

同一份 release 二进制、128×128 夹具、`regions=0,0,1,1`、`jpeg-quality=90`、默认强度
1.0，重复「发布 → 回读」20 轮：`watermarkPresent=true` 19 次、`false` 1 次（5%），
失败那轮的 `c2paWatermarkId` 仍与本地记录一致。每次发布的 40 位水印 ID 是随机生成的，
失败与水印载荷相关，不是固定输入下的确定性缺陷。

### 2.5 回读率 vs 选区边长（用于 §3.4 的警告阈值）

同一份 release 二进制，`--regions 0,0,1,1`（选区 = 整图，因此选区边长 = 图像边长）、
`--jpeg-quality 90`、默认强度 1.0、`decode-authenticity` 按整图回读：

| 选区边长 | 内容 | 轮数 | 回读成功 | 失败 | 失败率 |
| --- | --- | --- | --- | --- | --- |
| 96（项目允许的最小区域） | 照片（128 夹具放大/缩小） | 30 | 7 | 23 | 77% |
| 128 | 照片 | 40 | 38 | 2 | 5% |
| 192 | 照片 | 40 | 39 | 1 | 2.5% |
| **256** | 照片 | 40 | 40 | 0 | 0 |
| 512 | 照片 | 25 | 25 | 0 | 0 |
| 128 | 平滑渐变（仓库存档里的 16K 夹具同形） | 20 | 7 | 13 | 65% |
| **256** | 平滑渐变 | 20 | 20 | 0 | 0 |
| 512 | 平滑渐变 | 20 | 20 | 0 | 0 |

结论：**256 是实测到的最小稳定边长**（两种内容下都 0 失败），因此 §3.4 的警告值取 256。
样本有限（0/40 的 95% 置信上界约 7%），且失败与内容强相关（同为 128 级，照片 5%、渐变 65%），
所以警告是「降低风险」而不是「保证」——口径见 §3.4 与文档同步项。

## 三、逐条根因与修复

### 3.1 根因一（阶段一·产品侧）：热循环取消检查被当成交互检查点（B1/B2/B4/B5 + C1 + C5）

**机制。** 无头入口把取消来源接成「闸门检查点」：

- `headless.rs:683`：`CancelToken::with_source(move || interlock.checkpoint())`；
- `headless.rs:627-661`：`checkpoint()` 每个检查点都**写一行 marker**，带闸门时再阻塞等
  stdin 一行判定；
- `cancel.rs:47-49`：`is_cancelled()` = 本地标志 `||` 调用 `source()`，也就是**每一次**查询
  都会走一遍 `checkpoint()`；
- `chunk_file/mod.rs:244-254`：`ChunkTask::check()` 就是 `token.is_cancelled()`，而它被放在
  分块主循环里——`snapshot.rs:316`、`snapshot.rs:374` 是**每块一次**（`copy_original` /
  `write_snapshot`），`delta.rs:434` 在载荷落盘处，`snapshot.rs:440`、`snapshot.rs:481`、
  `delta.rs:246`、`delta.rs:314` 按 `CANCEL_CHECK_INTERVAL = 1024` 或每个数据块一次。

于是「每条边界一次检查点」变成了「每块一次检查点」，两项后果：

1. **序号漂移**：检查点总数随分块数变化，测试按固定序号强杀就落错位置（B1/B2/B4/B5）；
2. **marker 文件膨胀**：`checkpoint()` 无条件写行，`scrub`/`restore` 这类逐块流式操作会把
   marker 写到 MB 级，这些文件位于测试夹具的 `scratch/` 下、被 `tree_usage()` 计入工作区，
   直接抬高了账本的「实测占用」（C1/C5）。

**同源但未失败的连带损失（必须一并修）。** A1/A3/A6/C7 逐个序号取消时都断言
`checkpoints.len() == target`，因此现在仍然通过——但它们的注释所声称的覆盖位置已经不对了：

- `stress_cancel.rs:41-43`（A1）：注释说第 4 个取消点走 `run_backup` 的已发布文件回滚分支、
  第 5 个走 `history::commit` 的事务错误分支。实测这两个位置是第 11、12 个检查点，**从未被
  覆盖过**。
- `stress_cancel.rs:158`（A3）：注释说 compact 共 6 个检查点（父链入口 + 2 跳、子链入口、
  delta 发布前两处），实测远多于 6 个。
- `stress_large.rs:950`（C7）：逐个序号取消只覆盖到前 5 个「早期」检查点。

**修复方向（产品侧改动之一，`headless` 门控，发布产物逐位不变；另一处产品侧改动见 §3.4）。**
把取消观察分成两层：

- **静默观察**（热循环用）：`ChunkTask` 只读取「取消是否已被判定」，**不写 marker、不阻塞**。
  为此给 `CancelToken` 增加一个静默查询（例如 `observed()`），无头侧的实现只读
  `Interlock` 的 `cancelled` 原子标志（`headless.rs:574`），不发闸门。
- **交互检查点**（边界用）：`ensure_not_cancelled(cancel)` 等显式边界仍走 `is_cancelled()`，
  仍是「写一行 marker + 带闸门时阻塞」，语义与 `docs/guides/validation.md`
  「逐检查点闸门」的描述完全一致。

这样做的效果：

- 每个操作的检查点回到 5–6 个边界点，A1/A3/A6/C7/B1/B2/B4/B5 的**序号重新对应**它们在
  注释里声称的位置，B 组四个失败断言无需改动即通过；
- 取消判定仍在边界做，已判定的取消仍会被热循环在下一次静默查询时立刻观察到，因此取消延迟
  （`stress_baseline`、`stress_bench` 的度量）不回退；
- GUI 侧不受影响：GUI 的取消来源是 `BackupState` 的取消标志（`cancel.rs:32-37`），本来就
  是廉价且无副作用的读取，静默层与交互层可以用同一个闭包；
- marker 文件回到百字节级，C1 与 C5 的账本缺口随之消失。

**备选（不推荐）**：保留热循环逐块发检查点，改为按**阶段名**锚定强杀点。这需要为
`run_backup` 的「发布之后」与 `restore` 的「导出并 sync 之后」新增两处 `headless` 观察标记点
（会把「为测试触及产品代码的标记点」从 2 处扩到 4 处，与报告里的口径不符），而且不解决
marker 膨胀，C1/C5 仍需另行改动账本口径。只有在 §3.1 的静默层改动被证明有风险时才退回此方案。

**落实清单**

| 文件 | 改动 |
| --- | --- |
| `src-tauri/src/cancel.rs` | 增加静默查询（只读取消状态，不触发无头闸门） |
| `src-tauri/src/headless.rs` | 无头令牌同时接「静默来源」与「交互检查点来源」 |
| `src-tauri/src/backup/chunk_file/mod.rs` | `ChunkTask::check/is_cancelled` 改走静默查询 |
| `src-tauri/tests/stress_support/mod.rs` | 可选加固：账本口径只统计仓库 + 工作文件，不计测试夹具自身的 `scratch/`（见 §4.2） |
| 文档 | `docs/guides/validation.md` 补一句「交互检查点只出现在边界，热循环只做静默观察」；`docs/user/stress-test-report.md` 与 `docs/planning/todo.md` 更新对应条目 |

### 3.2 根因二（阶段一·测试侧）：快照体积断言仍是 v1 口径（C2/C4/C6）

`stress_large.rs:427-433` 的 `assert_snapshot_matches_logical` 断言
`logical <= snapshot <= logical + allowance`，注释明确写「不压缩、不去重」。alpha1 的
`ChunkFile v2` 对重复块只写引用、并用 zstd 压缩，C 组夹具又是**稀疏**文件（逻辑 64 MiB 里
只有约 38 MB 真实随机数据，其余是全零区），全零块被折叠成极少数块——快照合法地远小于逻辑
大小。实测：`default` 35.4 MB / `large` 130.7 MB / `heavy` 167.5 MB / `extreme` 234.3 MB。

**修复方向（测试侧，不改产品）。**

1. 把断言改成「不超过逻辑大小 + 每块开销」，即 `snapshot <= logical + allowance`；
2. 单独断言「v2 的去重/压缩确实生效」，例如同一份夹具下 `snapshot < logical`，
   并新建一条**密集不可压缩**用例（工作文件全部写真实随机字节、无全零区）断言
   `snapshot ≈ logical`——否则「快照变小」既可能是去重生效，也可能是内容在别处丢了；
3. 报告里的存储口径改写：现行「仓库常驻大小 ≈ 一个完整快照 + 所有增量」「每次提交新增的
   占用跟随改动的字节数」是 v1 结论，v2 下应表述为「跟随**新增的不同内容**，重复内容只写
   引用」；
4. 顺带复核 C2（`stress_large.rs:565`）与 C4（`stress_large.rs:684`）的
   `deltas * 4 <= snapshots`：去重把 `snapshots` 压小后这条比例会变紧（实测 `default` 档
   7.54 MB × 4 = 30.2 MB 对 35.4 MB，只剩 15% 余量）。建议改成基于**改动量**的口径
   （C4/C6 已有 `delta <= changed × factor` 的断言），避免它随去重率波动。
5. C1/C4 的账本断言（`stress_large.rs:390-422`）**不需要**按去重改写：它的预期项
   （`snapshot + delta + region 增长`）用的就是磁盘上的真实大小，本来就与 v2 一致。
   C1 本轮失败的是「实测」一侧被 marker 文件污染，见 §3.1。

### 3.3 根因三（阶段一·测试侧）：小图 TrustMark 回读不稳定（G4，改测试夹具）

`authenticity/trustmark.rs:63-110` 编码时按 `pixel_bounds(region, w, h, 96)` 裁剪区域并交给
引擎编码；`trustmark.rs:112-130` 解码时按 `minimum_size = 64` 重新裁剪同一区域再交给引擎
解码。夹具是 128×128 源图 + JPEG q90，回读率约 95%（§2.4）。C2PA soft binding
（`soft_binding_value`，`trustmark.rs:169-171`）始终正确——它走 CBOR 文本串，不依赖像素域，
所以失败只出现在像素域回读上。

**判定：这是外部水印引擎在 96 像素量级（本项目允许的最小区域就是 96）上的固有鲁棒性限制，
不是我们的数据完整性缺陷。** 40 位 ID 每次发布随机生成，失败与水印载荷相关。

**采纳方案（测试侧）**：G4 的夹具改用 **512 级源图**断言 `watermarkPresent = true`（§2.5：
512 与 256 都是 0 失败，取 512 留余量）；128×128 的回读率降级为**观察值**（§2.5 的统计表），
并在保证一览的边界一节写明「选区任一边小于 256 时像素域回读不保证，C2PA 声明不受影响」。
「别让用户默默发出读不回的水印」这件事由 §3.4 的发布期警告承担。

**否决方案（改产品参数）**：提高小图的有效强度（当前默认 1.0、上限 1.5，见
`headless.rs:1208-1211`）或把区域填充到引擎推荐画布。理由是：引擎内部的鲁棒性不归我们控制，
调参数只能缓解而无法给出保证；产品侧要做的是**如实警告**（§3.4），不是猜一个参数。若维护者
今后要**保证**小图也能回读，那是一项独立的产品取舍（届时需定「小图下限」的对外口径），
另行立项，不在本计划内。

### 3.4 新增需求（阶段二·产品侧）：小选区水印警告

维护者要求：发布时若**选区在 rendition 上的任一边长**小于警告值，就发出警告。警告值按 §2.5
的实测取 **256**（实测最小稳定边长）：96 级失败 77%、128 级 5%、192 级 2.5%，256 与 512 在
照片与平滑渐变两种内容下都是 0 失败。

实现口径：

1. **判定点与口径**：`trustmark::encode_regions`（`trustmark.rs:63-110`）已经为每个区域算出
   像素边界（`pixel_bounds`，同文件 `:93`）。把这份边界连区域序号一起回传，在发布流水线里按
   `min(width, height) < 256` 生成警告。判定必须用**送进编码器的那张 rendition** 的像素尺寸，
   不能用界面预览的显示分辨率。
2. **不阻断**：发布照常成功——是提示不是错误（与维护者口径一致）。
3. **承载与传递**：`pipeline::PublishedOutput`（`pipeline.rs:33-42`）与
   `model::PublishResult`（`model.rs:210-219`）各加一个结构化字段（例如
   `watermark_warnings: Vec<WatermarkSizeWarning { region_index, width, height, threshold }>`），
   `artifact::publish_artifact`（`artifact.rs:126-141`）原样透传；同时写一行 log，
   便于用户按日志复核。
4. **前端**：
   - **框选时就地提示**：发布前预览已经返回 rendition 尺寸（`PublicationPreview` 的
     `sourceWidth` / `sourceHeight`，`AuthenticityModule.tsx:675` 已在用），因此前端可以在
     用户拉选区时按 `归一化坐标 × 源尺寸` 算出像素边长并即时提示，不必等发布完再告知；
   - **发布结果显示**：在 `AuthenticityModule.tsx:154` 的 `publish-success` 区显示后端返回的
     警告；
   - `types.ts:137-146` 的 `PublishResult` 增加对应字段。
5. **无头可测**：`publish`（`headless.rs:1169+`）的结果 JSON 带该字段，供压力测试断言，
   避免只靠 GUI 人工看。

边界与文档口径（**必须照此写，不能写成保证**）：警告是降低风险，不是保证。实测样本有限
（256 级 0/40、512 级 0/25 照片 + 0/20 渐变，0/40 的 95% 置信上界约 7%），且失败与内容、JPEG
质量相关（同为 128 级，照片 5% 失败、平滑渐变 65% 失败；本轮未测质量维度）。因此
`docs/user/stress-test-report.md` 的边界一节写成：「**选区任一边小于 256 时像素域回读不保证；
更大的选区或更平滑的内容仍可能失败；C2PA 声明与本地记录匹配始终不受影响**」。

## 四、执行顺序与验证

### 4.1 顺序

**阶段一：让 alpha1 的未通过项清零**

1. **§3.1 静默/交互分层** → 跑便宜的九组（含 B1/B2/B4/B5、C7）与 `gate_guard`，确认 B 组四项
   直接转绿、A1/A3/A6/C7 的覆盖位置与注释重新一致；C1/C5 的 marker 膨胀随之消失，不需要改
   这两条断言。
2. **§3.2 断言改写 + 新增密集不可压缩用例** → 跑 `stress_large` 的 `default` 档确认体积断言
   与账本断言转绿（`stress_large` 整档属昂贵分组，按 `docs/guides/validation.md` 由维护者在
   需要时发起）。
3. **§3.3 G4 夹具改到 512** + 128 级回读率降级为观察值 → 随 CI 的 `stress_authenticity` 回归。
4. 可选项：§4.2 的账本加固可并入本阶段一起做（不是退出条件）。
5. 文档同步（阶段一）：`docs/user/stress-test-report.md` 的「未通过项」与存储类口径、
   `docs/planning/todo.md`、`docs/guides/validation.md`；`CHANGELOG.md` 的 `## Unreleased`
   记录产品侧行为变化（未通过项**不**写进带日期的版本小节）。
6. **退出条件**：九组便宜压力测试 + `gate_guard` 全绿；`stress_large` 的 `default`（必要时
   `large`）档存储与账本断言转绿。

**阶段二：新增小选区水印警告**

1. **§3.4 后端**：`trustmark` / `pipeline` / `artifact` / `model` 的判定与承载，headless
   `publish` 结果带警告字段；
2. **§3.4 前端**：框选时即时提示 + 发布结果区显示 + `types.ts` DTO；
3. 新增断言：`stress_authenticity` 用例（96/128/192 有警告、256/512 无警告、条数与选区数一致）
   + 前端单测；
4. 文档同步（阶段二）：保证一览的边界句子（选区任一边 < 256 时回读不保证）、
   `CHANGELOG.md` 的 `## Unreleased`「新增」小节；
5. **退出条件**：CI 的 `stress_authenticity` 与前端测试全绿；警告阈值与边界文案与 §2.5 一致。

### 4.2 可选的账本加固（阶段一，可选）

即使 §3.1 落地，仍建议把账本口径收紧到「只统计仓库与工作文件的增长」——`tree_usage`
（`tests/stress_support/mod.rs:1424-1466`）目前把夹具自身的 `scratch/`（marker / result /
stderr）也算进工作区；这些文件是测试自己产生的，不属于产品的存储放大。顺带记一条已确认的
细节：`storage_state`（同文件 `:1622`）用 `contains("/temp/")` 判定临时文件，因此**仓库根**
下的 `temp/…`（`resolve_chain` 的溢出目录，`restore.rs:329`）对 `assert_no_stray` 不可见；
本次调查没有发现那里残留文件，但该盲区值得一并修掉。

### 4.3 共用验收口径：**不需要重跑 extreme**

两个阶段共用这一节：阶段一验 B 组与 C 组，阶段二验警告，都在最便宜的档位完成。

这四类问题（B 组、C 的账本与体积、G4 与新增的警告）都与文件大小无关，最便宜的一档就能证伪，
因此修复验证**不**以跑完整套（尤其不跑 `heavy` / `extreme` / `manual`）为前提：

- **B 组（B1/B2/B4/B5）**：夹具只有 96 KiB / 80 KiB，机制是检查点数量与分块数挂钩，与体积
  无关。验证就在 CI 的便宜九组里（`stress_crash` / `stress_cleanup` 已在 CI）。
- **C1 / C5**：marker 膨胀按「处理字节数」线性增长，`default` 档（64 MiB）已经完整复现
  （一次 restore 的 marker 3.31 MB、账面缺口 36.3 MB）；`extreme` 只是把同样的现象放大
  60 倍。修复后在 `stress_large` 的 `default` 档跑一次即可证伪——2026-10-18 的实测里，
  `stress_large` 的 `default` 档整档只用 **32.7 秒**，`large` 档 **125.4 秒**（见
  `target/stress-full-run.log`），两者都远低于「数十分钟到两小时」的 `heavy` / `extreme`。
- **G4 与 §3.4 的警告**：都在 `default` 档，本身就是最便宜的档位，且 `stress_authenticity`
  原本就是 CI 的便宜九组之一，因此这两项随 CI 回归：G4 用 512 级夹具断言回读成功；新增用例
  断言 96 / 128 / 192 级发布产生警告、256 / 512 级不产生，且警告条数与选区数一致。
- `extreme` 只在两种情况才需要：① 需要**采集**该档的存储放大数值写进
  `docs/user/stress-test-report.md`；② 认证大图场景 G1/G2（16K 源图）**只在 extreme 及以上
  档运行**，低档会明确跳过。这两件事属于发布前采集，不属于修复验证。

因此建议的验证组合：**CI 的便宜九组 + `gate_guard` + `stress_large` 的 `default` 档**
（如需一个体积台阶，再加 `large`）。其余口径不变：

- `manual` 档仍不跑；本轮 2026-10-18 的 C 组数值不能作为 alpha1 的存储结论；
- G4 的复现率在修复前后都要有统计，避免「改一次跑一次通过」就算修好；
- 性能口径不变：`stress_bench` 只在 `--test-threads=1` 下采集，跨版本不谈群跑墙钟。

## 五、范围外

- 本轮未通过项里**没有**发现数据损坏类缺陷：所有失败场景中 `verify` + `scrub`
  都通过，恢复产物逐字节一致（C7 通过）。
- `stress_large manual` 档仍未跑（需 ≥ 32 GiB 空闲与数小时），不在本计划内。
- 「压力测试只采墙钟与峰值内存、不采进程 CPU 时间」（`todo.md` 的度量口径项）与本次失败
  无关，仍按原优先级挂起。
