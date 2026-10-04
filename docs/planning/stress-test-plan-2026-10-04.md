# 自动化压力测试规划（下一阶段）

- 规划日期：2026-10-04（同日订正并重排，见 §7 与 §11.4；2026-10-04 二次订正，见下方）
- 目标版本：待定（落实时由维护者决定是否随 `0.2.0-alpha.4` 递增；本批次本身不要求版本变化）
- 状态：**部分实施**。批次 1（无头入口与取消边界）、批次 2（跨进程崩溃）、批次 2 补充
  （事务中途崩溃 B4）、批次 3（大文件端到端）、批次 4（规模与灾备、参数边界）与批次 5
  （崩溃孤儿回收闭环）已落实并记录于 `docs/planning/current-handoff.md`；统一清理体系
  （`cleanup-system-plan-2026-10-04.md`）亦已落实，改变了本计划的多处前提。批次 6 起待实施。
- 实施完成后：本文件移入 `archive/`，有效契约并入 `docs/guides/validation.md` 与相关模块文档，
  未完成项沉淀到 `todo.md`。

## 0. 二次订正摘要（2026-10-04，统一清理体系落实后）

统一清理体系落地后，本计划的多处前提已变化。二次订正的内容如下（正文各处已同步）：

1. **批次状态**：批次 3（大文件端到端 C）已落实，不再是待实施项。
2. **崩溃孤儿不再「无法回收」**：清理体系提供了 `cleanup::scan_unreferenced`（发现）+
   `cleanup::cleanup_unreferenced`（确认清理）、灾备暂存目录清扫与画板 DDS 双向检查。
   但回收是**两段式（扫描报告 + 用户确认）**且**只经 GUI 进程内命令暴露**，因此
   B 组断言（孤儿存在 + 不被引用 + 重开可用）仍成立，只是理由从「没有能力」改为
   「有能力但需用户确认、且未暴露给无头进程」。
3. **执行模型**：大文件档从「严格串行」改为**按磁盘预算并发**（`scenario_slot`），
   磁盘上限默认值由 12 GiB 提到 **24 GiB**。
4. **新增批次 5（崩溃孤儿回收闭环）与批次 6（画板 DDS 完整性）**：画板 DDS 的双向检查
   （损坏/缺失/孤儿）与崩溃孤儿回收闭环，前置已由清理体系解除；孤儿回收已随批次 5 落实，
   画板 DDS 需要给无头入口补充画板写入命令（见 §4.1）。
5. **测试内容按「现实可遇」重估**：规模场景的量级收敛到个人长期使用的真实上限
   （数百 Artwork、单作品数百节点/数十分支），见 §3.1 的评估标准与 §5 的矩阵。

## 1. 背景

### 1.1 为什么现在做

`docs/planning/todo.md` 第二节「待验证 / 待人工验收」的第一条与第二条，以及
`docs/guides/release-policy.md` 人工门槛中的「使用正式支持上限附近的图片与工作文件验证内存、
取消、退出和错误恢复」，目前**只有人工验收、零自动化覆盖**。这两项的共同特征是：
结论依赖可程序判定的客观事实（文件是否残留、数据库是否可重开、摘要是否一致），
而非界面视觉或手感——因此属于「应当被自动化替代」的部分，而不是必须留给人的部分。

### 1.2 现状盘点：已覆盖与未覆盖

已覆盖（既有 `#[cfg(test)]`，随 `cargo test --lib` 运行）：

| 位置 | 覆盖内容 |
| --- | --- |
| `backup/chunk_file.rs` | 惰性链解析与急切物化等价、基础摘要不匹配拒绝、压缩 delta 往返与越界拒绝、4 GiB 声明上限、32 MiB 大 delta 往返与截断失败 |
| `backup/runtime.rs` | 取消只作用于后台任务、用户操作不被误取消、退出时排队任务被拒绝、前台等待计数在正常/取消/失败路径归零 |
| `cleanup.rs` | 外部与仓库文件按预期摘要清理与重试、父目录穿越拒绝、被数据库引用时保留、重启后意图恢复、删除后重放幂等 |
| `library/schema.rs` | 建库、v1 → 当前版本全链迁移、完整性检查计数 |
| `authenticity/image_resource.rs` | 16K 图像头被接受、极端头被拒绝 |
| `pin_board/*` | 几何、阵列、会话隔离、快捷键、纹理尺寸/内存策略、前后端上限契约（`npm run test:pin-board`） |

未覆盖（本批次要补的缺口）：

1. **端到端链路**。分块引擎有单元级大文件测试，但**没有**任何一条测试走完
   「真实工作文件 → 提交 → 恢复 → 精简」的完整链路。
2. **取消边界**。取消回调存在于所有长操作上，但没有任何测试在**中途**取消并断言收尾状态。
3. **进程被杀（文件发布窗口）**。「先发布文件、后提交数据库」之间的强杀窗口，由批次 2 的
   B 组覆盖（B1–B3，已落实）。
4. **进程被杀（SQLite 事务中途）**。WAL + `synchronous = FULL`（`storage.rs:26`）是明确的
   崩溃安全承诺，但事务提交窗口内没有取消检查点，`Child::kill()` 只能停在事务**之前**或
   **之后**、无法停在事务中途。已给 `history::commit` 增加一个 headless-only 标记点
   （见 §7「批次 2 补充」，已落实）。
5. **规模**。深链（`materialization_chain` 的递归 CTE + 逐跳 resolve）、宽树、整仓灾备的规模行为无覆盖。

### 1.3 纪律边界与已定接口方案

`docs/guides/validation.md` 禁止 GUI 自动化、窗口驱动、输入模拟，以及「启动应用本身做
自动化操作」。允许的手段只有程序化断言、HTTP 契约测试、进程外只读查询和纯计算复现。

**维护者已确认（2026-10-04）：采用「无头命令行模式」。** 主程序增加一种无界面运行方式，
压力测试作为**独立的进程**启动真实可执行文件并通过命令行调用它。理由与边界：

- 测试因此成为**真实的接口调用者**：跨越进程边界，零内部访问，不依赖任何 `pub(crate)` 项。
- 无头模式**不创建窗口、不建托盘、不加载 webview、不经过 IPC**，与那条纪律要防的
  「GUI 自动化 / 视觉验收」性质不同；它不是界面测试，而是数据层验证。
- **无头入口 feature-gate，发布产物中不存在**。因此产品行为零变化，纪律上也不构成
  「把应用改造成可被自动化驱动」。
- 仍被禁止、本批次不做的：窗口驱动、截图、输入模拟、启动 GUI 主流程。

### 1.4 依赖隔离的前提（已核实，影响设计）

主 crate 的 `[dependencies]` 目前**一条都不需要新增**，这是本批次的设计目标（见 4.7）。
原因是测试落在集成测试目标里，可以使用主 crate 已有的普通依赖（`tempfile`、`serde_json`）
与 std，无需引入任何新库。若未来确实需要新库，`[dev-dependencies]` **不是**本项目里干净的
隔离手段——理由与证据见 4.7。

## 2. 可行性依据（代码事实）

1. **领域层不依赖 Tauri 运行时。** 全部长操作签名为纯路径形式，无一需要 `AppHandle` 或 `State`：
   `library::repository::initialize(root)`、`create_artwork(root, parent_id, title, branch_title, source_path)`；
   `history::repository::{create_branch, commit, delete_subtree, apply_compaction, materialization_chain}`；
   `backup::worker::run_backup(root, branch_id, note, commit_kind, cancelled)`；
   `backup::restore::{restore, compact_node, ensure_checkpoint_with_progress, scrub_history}`；
   `backup::create_repository_backup(root, destination_parent, cancelled, progress)`。
   无头入口只是这些函数的**另一个适配器**，与 Tauri 命令层并列，不复制任何业务逻辑。
2. **临时仓库可程序化构造。** `initialize` 会建库并建目录；工作文件经 `normalize_source_path`
   强制要求绝对路径、是普通文件、且**不在仓库内部**——因此夹具必须把工作文件放在仓库的兄弟目录。
3. **取消与进度契约已存在。** 下列入口都带 `cancelled: impl Fn() -> bool` 与
   `progress: impl Fn(&str, u64, u64)`：`worker::run_backup`、`restore::restore`、
   `restore::compact_node`、`restore::ensure_checkpoint_with_progress`、`restore::scrub_history`、
   `create_repository_backup`。**不需要新造任何机制**，无头入口只需把进度回调桥接到
   「写阶段标记文件」和「读 stdin 取消通道」。
4. **例外（需在计划中记录）**：`history::delete_subtree` 与 `history::apply_compaction`
   **没有** `cancelled` 参数，它们只做 SQLite 事务，取消边界在事务之前
   （删除命令的取消点落在其前置的 `ensure_checkpoint_with_progress` 上）。
5. **可用的健全性断言入口已存在**：`library::repository::open_existing` / `check_existing`
   （迁移 + `integrity_check` + `foreign_key_check` + 语义校验）、`schema::validate_repository_semantics`
   （UUID / 相对路径 / SHA-256 全表校验）、`restore::scrub_history`（全链逐块校验）、
   `cleanup::run`（清理队列重放）。它们各自映射为一条无头子命令，供测试在重开后调用。
6. **`main.rs` 目前只有 5 行**，调用 `pub fn run()`；`lib.rs` 的公开表面只有这一个函数。
   增加无头入口即增加第二个 `pub` 入口，改动面可控。

## 3. 测试内容选取：价值评估

### 3.1 评估标准

- **现实可遇性**（新增，2026-10-04）：该路径必须是**真实使用中会发生**的场景，量级收敛到
  个人长期使用的真实上限（例如数百 Artwork、单作品数百节点/数十分支，而不是为凑数量构造
  的数千分支）。纯粹为「极端」而构造、现实中不会遇到的输入一律排除——压力测试的价值来自
  「在真实会发生的极端下仍正确」，而不是「在不可能的情形下也正确」。
- **风险密度**：该路径的复杂度、状态机分支数与清理分支数。`worker.rs` 生产代码中有 8 处
  `fs::remove_file` 回滚分支（delta 发布失败、取消后回滚、`history::commit` 失败、旧 snapshot
  释放、修复 snapshot 登记失败、被替换 snapshot 清理）；`restore.rs` 有 3 处发布后回滚
  （精简 delta 改接失败、检查点登记失败、精简后旧实体清理）。
- **缺陷可发现性**：自动化断言能否稳定判定成功/失败，而非依赖人眼。
- **替代人工价值**：能否把 `release-policy.md` 的人工门槛项转为自动化。
- **成本**：实现成本 + 单次运行成本（磁盘、时间）。
- **前置依赖**：是否依赖尚未落实的功能。

### 3.2 逐项评估

| 候选 | 风险密度 | 可发现性 | 替代人工 | 成本 | 前置依赖 | 结论 |
| --- | --- | --- | --- | --- | --- | --- |
| A 取消边界（提交/恢复/精简/检查点/灾备/扫描） | 高 | 高 | 直接替代 todo 第二条 | 低（小文件即可） | 无 | **P0 入选** |
| B 跨进程强制退出（文件发布窗口） | 中高 | 高 | 替代 todo 第一条「退出」 | 中 | 无 | **P0 入选（批次 2 已落实）** |
| B′ 事务中途强杀 | 高 | 高 | `synchronous=FULL` 承诺的唯一自动化证伪手段 | 低（需 headless-only 标记点） | 无 | **P1 已落实（批次 2 补充）** |
| B″ 崩溃孤儿回收闭环 | 中 | 高 | 验证统一清理体系的发现 + 确认清理 | 低（复用 B 组编排） | 需无头暴露 `scan-unreferenced` / `cleanup-unreferenced` | **P1 入选（新增批次 5）** |
| C 大工作文件端到端 | 高 | 中 | 替代 todo 第一条「极端大文件/内存」 | 高（磁盘主导，见 4.6） | 无 | **P0 入选（已落实，批次 3）** |
| D 深链 / 宽树规模 | 中 | 中 | 无直接对应 | 低-中 | 需无头暴露树/分支操作 | P1 入选（批次 4） |
| E 整仓灾备规模 + 取消 | 中高 | 高 | 无直接对应 | 中 | 无 | P1 入选（批次 4） |
| F 极端参数边界 | 低 | 高 | 无 | 极低 | 无 | P2 入选（批次 4） |
| G 认证发布与大图内存 | 中高 | 高 | 无直接对应 | 高（模型加载 + 高分辨率解码） | 需扩展无头入口 | **P1 入选（维护者要求）** |
| H 画板 DDS 规模 / 孤儿文件 | 中 | 高 | 替代 todo 第二条「画板 DDS」 | 中 | 无（两条 P1 已由统一清理体系落实）；需无头暴露画板 DDS 校验 | **P1 入选（新增批次 6）** |
| I 前端压力测试 | 低 | 低 | 无 | 低 | 无 | **排除：`api.ts` 被 `vi.mock` 整体替换，测不到真实 IO** |
| J 磁盘满 / 权限失败 | 中 | 低（复现不稳） | 无 | 高 | 无 | 排除（可选后置） |

### 3.3 结论与理由

**A 取消边界——价值最高，成本最低，应作为第一批。**
理由：取消路径是唯一「每个长操作都有多分支清理代码、却完全没有中途测试」的区域；
且它不需要大文件——用小工作文件，用无头进程的逐检查点闸门（每个检查点阻塞等待 stdin 判定）
就能精确命中每一个清理分支。`todo.md` 第二节第二条整条因此可被替代。

**B 跨进程强制退出——唯一的 in-process 测法无法覆盖的类别。**
理由：发布顺序是「先发布文件、后提交数据库」，崩溃点落在两者之间必然产生孤儿文件；
这类缺陷只能靠真实进程被杀来暴露，且必须验证「重开可用」而不是「重开不报错」。
采用无头模式后，被杀的是**真实可执行文件**，因此覆盖的是真实启动与恢复路径。

**B′ 事务中途强杀——`synchronous = FULL` 承诺的唯一自动化证伪手段（2026-10-04 补入）。**
理由：B 组的强杀点（`snapshot 已发布、commit 未执行`）落在事务**之前**，B2/B3 不写数据库，
因此 B 组并未触及「事务进行中被杀」。杀在事务中途会走与掉电同一条**数据库层**恢复路径
（未提交事务必须被丢弃），这是该承诺唯一能被自动验证的地方。代价是需要给
`history::commit` 增加一个 headless-only 标记点——属于产品代码改动，因此单独成批次
（§7「批次 2 补充」，已落实；GUI 路径行为不变）。

**C 大工作文件端到端——成本最高，但它是 `release-policy.md` 人工门槛的原句。**
理由：分块引擎的单元测试已经覆盖了格式与摘要层面的正确性，端到端补的是**真实链路**上
的内存行为与多步 delta 链。档位按真实使用场景定：256 MiB 只作冒烟，**默认 1 GiB**，
heavy 档 **4 GiB**（PSD 常态），**8 GiB 极限档 = 4 GiB 现实上限的两倍冗余**。磁盘成本由
snapshot 主导且**不可规避**（见 4.6），因此该档位按磁盘预算**并发**、与其它场景共享同一套
准入闸门（`scenario_slot`），而不是一律串行。

**G 认证模块——维护者要求纳入（2026-10-04）。**
理由：`authenticity/image_resource.rs` 已有 `limits_accept_16k_images_but_reject_extreme_headers`
覆盖**头解析级**拒绝，但**真实解码与 TrustMark 编码在高分辨率下的内存与耗时**从未被验证；
`authenticity.md` 记录的预算（单边 32768 px、总像素 300 MP、解码器 1.5 GiB）目前只是声明。
范围收窄：拒绝路径已有单测，本批次只测**允许范围内的大图**（16K ≈ 268 MP）的实际解码、
TrustMark 编码与 C2PA 签名的内存与耗时，不重复构造超限输入。

**I 明确排除。**
前端测试通过 `vi.mock("./api")` 替换整个 `api.ts`（见 `useHistoryController.test.tsx:10`），
结构性测不到真实 IO；纯 JS 规模测试属于 `npm test` 范畴，不应混入压力套件。

**H 前置已解除，入选批次 6（2026-10-04 二次订正）。**
原判「延后」的理由是 `todo.md` 的两条 P1（完整性扫描覆盖 DDS、实体与 SQLite 双向清理检查）
未落实、代码没有这些能力。统一清理体系批次 E 已落实这两条：`pin_board::scrub::scrub_board_dds`
做双向检查（记录 → 文件的缺失/损坏，文件 → 记录的孤儿），孤儿 DDS 并入
`cleanup::scan_unreferenced` 的发现 + 确认清理流程。因此 H 组现在可断言；唯一前置是给无头入口
补一个画板 DDS 校验子命令（见 §4.1）。

**B″ 崩溃孤儿回收闭环——统一清理体系的直接验证（2026-10-04 二次订正新增）。**
批次 2 的 B1 只断言「孤儿存在 + 不被引用 + 重开可用」，当时如实记录「当前不会自动回收」。
清理体系落地后，`scan_unreferenced` 能发现这枚孤儿、`cleanup_unreferenced` 能在用户确认后
回收它——但两者**只经 GUI 进程内命令暴露**。B″ 因此给无头入口补上这两个子命令，把闭环补成
可自动化的断言：崩溃 → 扫描发现孤儿 → 确认清理 → 再扫描无候选。这既验证清理体系，也把
B 组从「记录缺口」升级为「验证回收」。

## 4. 架构设计

### 4.1 无头入口（主 crate 的产品改动之一）

新增一种无界面运行方式，与 GUI 入口并列，不复制业务逻辑：

```text
lilith-artworks --headless <command> [options]
```

| 子命令 | 映射到的领域调用 |
| --- | --- |
| `init-repository` | `library::repository::initialize` |
| `create-artwork` | `library::repository::create_artwork` |
| `commit` | `backup::worker::run_backup` |
| `restore` | `backup::restore::restore` |
| `compact` | `backup::restore::compact_node` |
| `checkpoint` | `backup::restore::ensure_checkpoint_with_progress` |
| `scrub` | `backup::restore::scrub_history` |
| `repository-backup` | `backup::create_repository_backup` |
| `verify` | `library::repository::open_existing` + 语义校验 |
| `cleanup` | `cleanup::run` |
| `scan-unreferenced` | `cleanup::scan_unreferenced` |
| `cleanup-unreferenced` | `cleanup::cleanup_unreferenced` |
| `create-group` | `library::repository::create_group` |
| `move-node` | `library::repository::move_nodes` |
| `trash-node` | `library::repository::trash_nodes` |
| `empty-trash` | `library::repository::empty_trash` + `cleanup::run` |
| `list-tree` | `library::repository::list_tree` |
| `search` | `library::repository::search` |
| `create-branch` | `backup::ensure_checkpoint` + `history::create_branch` |
| `delete-branch` | `history::delete_branch` + `cleanup::replay` |
| `list-history` | `history::list` |
| `enter-publication` | `authenticity::{branch_head, store_final_artifact, get_publication}` |
| `publish` | `authenticity::publish_artifact`（C2PA 签名 + TrustMark 编码） |
| `cancel-publication` | `authenticity::remove_artifact` + `cleanup::run` |
| `decode-authenticity` | `authenticity::decode_authenticity`（回读与溯源） |

通用选项：

- `--result <path>`：把结构化 JSON 结果（成功/失败、错误摘要、耗时、计数）写入文件。
  **用文件而非 stdout**，因为 `main.rs` 在 release 下有 `windows_subsystem = "windows"`，
  没有控制台；写文件在两种构建下都可靠。
- `--marker <path>`：进入关键阶段时把阶段名写入该文件，供测试决定何时发取消或何时强杀。
- `--cancel-on-stdin`：读 stdin，收到一行或 EOF 即请求取消。这是**对产品「用户取消」
  路径的忠实模拟**，且由测试从进程外部触发。
- `--peak-memory <path>`（可选）：结束时把自身峰值内存写入文件。需要给已有的 `windows-sys`
  依赖增加 `Win32_System_ProcessStatus` 与 `Win32_System_Threading` feature（`GetCurrentProcess`
  在后者中；只加 feature、不加包）。

**入口改动面：**

| 位置 | 改动 |
| --- | --- |
| `src/lib.rs` | 新增 `#[cfg(feature = "headless")] pub fn run_headless(args) -> i32` |
| `src/main.rs` | 解析参数；headless 时走 `run_headless`，否则维持现有 `run()` |
| `Cargo.toml` | 新增 `[features] headless = []`，不加入 default |

**可见性改动（2026-10-04 落实时订正）**：无头入口在 crate 内部，但实际需要把
`backup::restore` / `backup::worker` 提升为 `pub(crate) mod`、`BackupRunError` 提升为
`pub(crate)`、`library::create_artwork` 由 `#[cfg(test)]` 改为
`#[cfg(any(test, feature = "headless"))]`——都是**纯可见性放宽、无行为变化**；
上一版计划里给 `authenticity/*` 提可见性的动作仍然不需要。

#### 4.1.1 纳入认证模块的两个前提（本批次唯一触及安全边界的改动）

**(1) 文件对话框授权必须改成可注入，不能绕过。**

`authenticity::ensure_dialog_authorized`（`authenticity/commands.rs:287`）的实现是
`window.fs_scope().is_allowed(path)`——它依赖 Tauri 的 webview 文件系统作用域，
而作用域由原生文件选择器授权。四个命令依赖它：`create_repository_backup`（备份目录）、
`enter_branch_publication`（最终成品）、`publish_branch_artifact`（输出路径 + 证书链）。

无头环境没有 webview，**该检查在原理上无法满足**。三条路：

| 做法 | 评价 |
| --- | --- |
| 无头下直接跳过检查 | **不可接受**。这会在代码里开一个「安全控制可关闭」的开关；一旦带该 feature 的构建流出，控制即失效 |
| 无头下把工作区路径自动加入作用域 | 等价于第一条，只是换了个写法 |
| **把授权来源抽象成可注入的路径作用域**（推荐） | Tauri 侧仍用 `window.fs_scope()`；无头侧用一个显式声明的作用域，只允许 `--workspace <dir>` 之下的路径 |

推荐做法保留了「路径必须被显式授权」的语义，只是换了授权来源；而且它给无头进程加了一条
**真实的安全属性**：它无法写到工作区之外。改动面是给该函数换一个作用域参数，
调用点 4 处，Tauri 侧行为逐位不变。

**(2) 模型路径必须显式传入，不能依赖 Tauri 资源解析。**

`lib.rs` 目前通过 `application.path().resource_dir()` 寻找
`resources/models/{encoder_Q.onnx,decoder_Q.onnx}`。无头环境没有 Tauri 应用，
因此无头入口用 `--models <dir>` 显式传入；默认值解析为
`env!("CARGO_MANIFEST_DIR")/resources/models`（两个模型文件确实在仓库中：
`encoder_Q.onnx` 约 17 MB、`decoder_Q.onnx` 约 47 MB）。`AuthenticityState::new(models_dir)`
本身不依赖 Tauri，可直接构造。

**(3) 测试用证书与图片的现状（无需新建）。**

`src-tauri/tests/fixtures/authenticity/` 已经具备完整的自签名测试材料：
ES256 端实体私钥/公钥、fixture CA 公钥、证书扩展配置、128×128 源图、
真实 C2PA + TrustMark 签名图、被篡改签名图，合计约 62 KB。
`tests/fixtures/authenticity/README.md` 记录了签名图的重新生成命令。**本批次直接复用，
不新建证书**——新生成证书需要 openssl（外部工具）或 `rcgen`（新依赖），两者都违背
「零新增依赖」。

**唯一需要新增的测试素材是大图**，且**按需生成、用完即删，不提交进仓库**：
测试进程用主 crate 已有的 `image` 依赖生成 16K 级图像。生成 16384×16384 需约 1 GiB 内存，
发布链路解码同样如此——因此该场景只在 `extreme` 及以上档位运行。

### 4.2 测试落点（零新增依赖）

- 测试是 `src-tauri/tests/` 下的**集成测试目标**，即独立 crate，只通过
  `Command::new(env!("CARGO_BIN_EXE_lilith-artworks"))` 启动二进制。
- `CARGO_BIN_EXE_<name>` 只在集成测试与 benchmark 中注入——这正是测试必须放在
  `tests/` 而不是 `src/` 的单元测试里的原因，也正好把测试挡在 crate 之外。
- 测试可用的依赖：主 crate 已有的 `tempfile`、`serde_json`，以及 std。
  **新增依赖数为 0**，因此依赖污染问题在结构上不存在。

```text
src-tauri/tests/
├── fixtures/                 # 既有，认证夹具
├── stress_support/mod.rs     # 共享助手：工作区、账本、进程编排、断言
├── stress_cancel.rs          # A 组
├── stress_crash.rs           # B 组
├── stress_large.rs           # C 组
├── stress_scale.rs           # D / E / F 组
└── stress_cleanup.rs         # B5 崩溃孤儿回收闭环
```

运行方式：`cargo test --features headless --test stress_cancel` 等，按组分别运行。
`smoke`/`default` 之外的档位不进入任何自动流程。

### 4.3 测试侧的场景驱动（`tests/stress_support/mod.rs`）

- **工作区**：测试进程创建 `TempDir/stress-workspace/` 并拥有其生命周期与清理；
  目录布局与账本见 4.6。测试只把路径作为命令行参数传给无头进程。
- **进程编排**：`run_headless(args) -> Outcome`（启动、等待、读 `--result` JSON、返回退出码）；
  `spawn_headless(args) -> Child`（用于需要中途干预的场景）。
- **工作文件生成**：`write_work_file(len, regions)`——稀疏构造 + 有界随机区域，见 4.6。
- **断言**：`assert_healthy(repo)`（调 `verify` + `scrub` 子命令）、`assert_no_stray(repo)`、
  `assert_counts(repo, expect)`、`assert_ledger_within(expected, actual)`。
  断言全部基于**子命令返回的 JSON** 与**磁盘事实**，不读内部状态。

### 4.4 崩溃与取消的干预方式

**取消**：测试 `spawn` 无头进程（带 `--marker` 与 `--cancel-on-stdin`）→ **逐检查点闸门**：
每个取消检查点先写 marker，进程阻塞等待 stdin 的一行判定（`cancel` / `continue`，EOF 视为
取消）→ 测试按检查点决定取消时机 → 等待退出 → 断言 `--result` 为取消结果、仓库无残留、
随后同一仓库上重跑同一命令必须成功。**订正（2026-10-04 落实时确认）**：该闸门取代了本节
原文的「轮询 marker 后写一行」——非阻塞轮询无法在毫秒级窗口下稳定命中具体清理分支。

**强杀**：同一套编排，但调用 `Child::kill()`（Windows 上等价 `TerminateProcess`，无清理）
→ 等待退出 → 用新的无头进程执行 `verify` / `scrub` / `cleanup` 子命令 → 断言
`integrity_check` 为 `ok`、外键无异常、全链校验通过。

**强杀注入的确定性（2026-10-04 落实时确认，偏离本节原文）**：采用「逐检查点闸门暂停后强杀」，
而非本节原文的「轮询 marker 后 kill」。带 `--cancel-on-stdin` 时进程在目标检查点**阻塞**，
测试随即 `kill`，因此强杀落在确定的代码位置；非阻塞轮询在「发布后、提交前」这类亚毫秒
窗口下会与进程赛跑而抖动。

**孤儿文件不会被「自动」回收，但已有两段式回收能力（2026-10-04 二次订正）**：本节原文与
§5 B1、风险第 10 条曾声称孤儿「可被清理队列回收」，与代码不符——崩溃发生在入队之前，已发布的
snapshot/delta **不会**进入 `pending_file_cleanup`，而 `cleanup::run` 只重放该队列。
统一清理体系落地后补上了「发现」能力：`cleanup::scan_unreferenced` 扫描未引用文件（含崩溃
孤儿与画板孤儿 DDS），`cleanup::cleanup_unreferenced` 在**用户确认后**批量入队并重放；灾备暂存
目录则由下一次灾备启动时清扫。因此准确表述是：**孤儿不会被自动回收，但可被扫描发现并经用户
确认后回收**，且这两个入口只经 GUI 进程内命令暴露。B 类场景断言「孤儿存在、不被引用、重开可用、
后续命令正常」；回收闭环由新增的 B″ 场景在补上无头子命令后验证（§5、§7 批次 5）。

**关键：干预全部通过进程外部手段（stdin / kill / 文件标记）完成，不触碰任何内部状态。**

### 4.5 报告输出

每个场景输出一行 JSON Lines 到 `target/stress-report.jsonl`，字段：
场景名、档位、耗时 ms、峰值内存（可选）、结果、失败断言摘要。

- 原始报告在 `target/` 下，而 `target/` **不进版本控制**。因此能长期引用的证据是
  **摘录进 `docs/planning/current-handoff.md` 的汇总**，报告格式据此设计成可直接摘录的形状。
- 报告可把对应人工验收项从「待人工验收」降级为「已自动化覆盖 + 人工复核手感」。

### 4.6 磁盘预算与执行模型（本批次的核心约束）

**事实前提（已核实，决定预算公式）：**

- **snapshot 不压缩、不去重。** `ChunkFile::create` 对每个块无条件调用 `emit_chunk`，
  后者直接 `snapshot.write_all(data)`（`chunk_file.rs:915`）；`from_records` 里的
  `chunk_index` 只用于按 key 反查父版本的块位置，**不参与写入去重**。因此
  `snapshot 文件大小 ≈ 逻辑大小 + 每块 36 字节开销`（默认 avg 16 KiB 时约 0.22%）。
  **逻辑 4 GiB 的文件必然产生约 4 GiB 的 snapshot，无法靠内容构造规避。**
- **delta 压缩。** `create_reverse_delta` 经 zstd level 6 写出（`chunk_file.rs:555`），
  增量步骤的成本由**变化的字节数**决定，而非文件大小。
- **恢复输出是全尺寸。** `copy_original` 逐块写出全部载荷，产物等于逻辑大小。

结论：大文件档位的磁盘成本由 snapshot 主导，只能靠「同一时刻只有一个大文件」与
「即时拆除」控制，不能靠内容构造消除。

**执行模型：**

| 维度 | 策略 | 理由 |
| --- | --- | --- |
| 大文件档（1 GiB 及以上） | **按磁盘预算并发**，同一时刻磁盘上的大文件峰值之和不超过预算 | 并行会让多个 snapshot 与恢复输出叠加；真正约束是**磁盘峰值**而非并发本身，因此用 `scenario_slot` 按预计峰值准入，而不是一律串行 |
| 小规模档（深链、宽树、库规模、取消变体、参数边界） | **并行**（默认 `min(4, 逻辑核心数)` 线程） | 这些用 MiB 级文件，瓶颈是 CPU 与 SQLite 事务而非磁盘 |
| 场景间拆除 | 大文件场景结束立即删除恢复输出与工作文件，只保留 snapshot 供后续断言 | 恢复输出是纯瞬时产物，约占一个完整逻辑大小 |

**这是对「大规模可并行」与「不要让多个大文件占磁盘」两条要求的调和：并行作用于场景
数量维度，不作用于文件体积维度。**

**工作区与仓库布局（单一临时位置）：**

```text
<TempDir>/stress-workspace/          # 唯一临时位置，进程结束自动清理
├── repository/                      # 唯一主仓库：大文件场景共用
├── repository-p<slot>/              # 并行场景的子仓库（仍在工作区内，计入同一账本）
├── work/                            # 工作文件（必须在仓库外）
├── out/                             # 恢复输出（每场景后即删）
├── backup/                          # 整仓灾备目标
└── ledger.jsonl                     # 磁盘账本：逐场景记录实际占用
```

大文件场景复用主仓库，场景之间**显式重置到基线**（清空 `artworks/`、重建库），
避免跨场景污染；重置本身也作为一次断言——重置后仓库必须通过 `verify` 子命令。
并行场景各自使用 `repository-p<slot>/`，因为并行共享同一 SQLite 库会让
「无孤儿文件」「节点计数」这类断言失去意义；全部子仓库仍位于同一工作区根内，
统一计量、统一清理。

**大文件生成（把工作文件成本压到接近 0）：**

- 用 `File::set_len(logical)` 创建文件：Windows 上等价 `SetEndOfFile`，有效数据长度保持 0，
  读取返回全零而**几乎不分配簇**；Linux 上 `ftruncate` 生成稀疏文件。跨平台可用，无需
  `fsutil` 或管理员权限。
- 再向**有界数量的区域**（默认总量 128 MiB）`seek` + 写入真实随机字节，用于制造有意义的
  块多样性与真实 delta；其余部分保持零。
- 结果：4 GiB 逻辑文件的实际磁盘占用约 128 MiB，而 snapshot 仍约 4 GiB——
  这正是「逻辑体积」与「磁盘成本」分离的地方，也是唯一能省的部分。

**预算与守卫：**

- 场景开始前计算预期峰值 `snapshot(≈logical) + 工作文件实际占用 + 恢复输出(≈logical)`，
  加上工作区当前实际占用；超出 `LILITH_STRESS_DISK_BUDGET`（默认 24 GiB，见 §8）则
  **明确跳过并报告**，而不是中途写坏磁盘。
- 场景结束后把实测增量写入 `ledger.jsonl` 并与预期比对，偏差超阈值即报警——
  这本身就是一条「存储放大是否失控」的回归断言。
- 档位由环境变量选择：`LILITH_STRESS_TIERS=smoke|default|heavy|manual`（默认 `default`）。

### 4.7 依赖隔离（已核实，结论反直觉）

**本批次的新增依赖数为 0**，因此不存在污染问题。测试只用主 crate 已有的
`tempfile`、`serde_json` 与 std；无头入口只用主 crate 已有的依赖实现。

若未来确实需要引入新库，**不要默认用 `[dev-dependencies]`**：

- `tools/release/generate-licenses.mjs:105-146` 跑 `cargo metadata`，从根包出发递归遍历
  **完整 resolve 图**，且**不按 `dep_kinds` 过滤**。cargo 的 resolve 图对根包包含
  dev-dependencies，因此测试依赖会进入发布用的 `licenses/THIRD_PARTY_LICENSES.html`；
  该脚本还会在找不到许可证正文时**直接抛错**（第 134 行），让 `npm run legal` 整体失败。
- 真正的隔离是**独立 crate**（不在主 crate 的 metadata 图里）。但给
  `src-tauri/Cargo.toml` 增加 `[workspace]` 可能改变 `metadata.resolve.root`，
  而该脚本正是以它为遍历起点——**动手前必须跑一次 `npm run legal` 做前后对比**。

## 5. 测试矩阵（入选项细化）

| 场景 | 档位 | 断言重点 |
| --- | --- | --- |
| A1 提交取消 | 在每个取消检查点各一次 | 无残留 snapshot/delta、head 未推进、可立即重试成功 |
| A2 恢复取消 | 链解析中 / 导出前 / 发布前 | 输出文件不存在（不覆盖语义未生效）、无残留临时文件 |
| A3 精简取消 | 父链回溯中 / 子链回溯中 / delta 发布前 | 历史图未被改接、旧 delta 仍在、节点仍可恢复 |
| A4 检查点取消 | 链解析中 / snapshot 发布前 | 节点未被标记为检查点、无孤儿 snapshot |
| A5 灾备取消 | 扫描 / 复制 / 校验 / 发布各阶段 | 未发布的临时 bundle 被清理、返回错误附带清理结果 |
| A6 全库扫描取消 | 逐节点之间 | 返回取消结果、仓库状态不变 |
| B1 提交中途强杀 | snapshot 已发布、commit 未执行 | 重开可用、孤儿 snapshot 不被引用（**不会被自动回收**，可被 `scan-unreferenced` 发现，见 §4.4 与 B5）、后续提交正常 |
| B2 恢复中途强杀 | 导出临时文件中 | 重开可用、输出文件不存在、无半写文件被引用 |
| B3 灾备中途强杀 | 复制中途 | 重开可用、未发布 bundle 残留可识别、源仓库未受影响 |
| B4 事务中途强杀（批次 2 补充，已落实） | 事务已 BEGIN、INSERT 已写、未 COMMIT | 重开可用、未提交事务被丢弃（WAL 恢复）、无半提交状态、head 与节点数不前进、后续提交正常 |
| B5 崩溃孤儿回收闭环（批次 5） | 复用 B1 的强杀点 | `scan-unreferenced` 发现孤儿 → `cleanup-unreferenced` 确认清理删除 → 再扫描无候选；清理后重开仍可用 |
| C1 大文件端到端 | 1 GiB（默认）/ 4 GiB（heavy） | 提交 → 20 次有界区域改动 → 恢复 → 摘要逐位一致 |
| C2 大文件精简 | 同 C1 | 精简后仍可恢复且摘要一致、delta 体积符合预期 |
| C3 大文件内存 | 同 C1 | 峰值内存不随逻辑大小线性增长（验证流式实现，不整文件入内存） |
| C4 大文件链体积 | 1 GiB 逻辑 + 20 次改动 | delta 总量远小于 snapshot；账本实测增量与预期偏差在阈值内 |
| C5 恢复产物即删 | 同 C1 | 断言完成后 `out/` 立即清空，工作区占用回落 |
| D1 深链 | 单分支 **300 次**提交（长期反复修改的同一作品） | 链解析与精简的耗时/内存随深度线性而非平方；精简中间节点后仍可恢复 |
| D2 多分支历史图 | 单 Artwork **约 20 条分支**、合计数百节点 | 分支列表与节点计数正确；删除一条分支只回收该分支独占的文件（队列清空、其它分支不受影响）；回收站清空后子树与其文件被回收 |
| D3 库规模 | **约 400 个 Artwork** + 嵌套分组（个人长期积累） | 树列举的计数正确；按标题/工作文件路径搜索命中正确；节点移动后树结构与归属正确 |
| E1 灾备规模 | 含大文件与多 Artwork 的仓库 | 清单 SHA-256 全对、副本可独立打开并通过语义校验 |
| F1 参数边界 | 备注 500/501 字符、标题 160/161 字符、无效提交类型、超长搜索、恢复输出已存在 | 明确拒绝或按不覆盖语义处理，而非 panic |
| H1 画板 DDS 双向检查（批次 6） | 含正常/缺失/损坏/孤儿 DDS 的仓库 | 报告区分四类计数；缺失与损坏**报告不失败**（命令仍成功返回） |
| H2 画板孤儿 DDS 回收（批次 6） | 复用 H1 的孤儿 DDS | `scan-unreferenced` 报告孤儿 DDS → 确认清理删除 → 再扫描无候选；被引用 DDS 不被误删 |
| H3 画板 DDS 规模（批次 6） | 单画板 **数百张**图片（个人素材板真实上限） | 完整性检查在规模下仍正确、逐条响应取消、内存有界 |
| G1 认证发布内存 | 16K 源图（≈268 MP，`extreme` 起） | 解码 + TrustMark 编码 + C2PA 签名的峰值内存不超出声明的 1.5 GiB 解码预算量级 |
| G2 认证发布耗时 | 同 G1 | 各阶段耗时被记录；无超时/挂起；可取消 |
| G3 认证发布取消 | 预览 / 编码 / 签名各阶段 | 无半成品被登记为成品、`cancel-publication` 后仓库内副本与记录已清除 |
| G4 认证回读 | 由 G1 产出的成品 | `decode-authenticity` 能读回 C2PA 声明与 TrustMark 绑定 |
| G5 认证受控文件校验 | 含成品的仓库 | `scrub` 覆盖最终成品与认证副本的摘要比对 |

> **B4 的标记点（已落实）**：`history::commit` 内、`transaction.commit()` 之前有一个
> headless-only 标记点（feature 门控，发布产物中不存在），领域逻辑与 GUI 路径行为不变；
> 它同时是提交路径的第 5 个取消检查点。批次 4 新增的树/分支子命令与批次 5 的
> `scan-unreferenced` / `cleanup-unreferenced` 也属无头入口扩展，但都保持
> 1:1 薄映射、不进发布产物；认证批次（批次 7）的授权作用域重构属安全控制改动，性质不同。
>
> **不自动化验证的项**：断电持久性。`Child::kill()` 只终止进程，不触及 OS/磁盘缓存与目录项
> 落盘，结构上无法覆盖「断电后已提交数据是否仍在」；该项由 OS/磁盘与 `synchronous = FULL`
> 声明保证，保留为人工/声明项（见 §6 与 §7 批次 8）。

**档位定义**（由 `LILITH_STRESS_TIERS` 选择，默认 `default`）：

| 档位 | 大文件逻辑大小 | 预计峰值磁盘 | 用途 |
| --- | --- | --- | --- |
| `default` | **64 MiB** | ≈ 0.2 GiB | **默认档**：日常使用的典型体积，成本低到可频繁运行 |
| `large` | 256 MiB | ≈ 0.7 GiB | 跨过明显的块级规模，仍属日常范围 |
| `heavy` | 1 GiB | ≈ 2.3 GiB | PSD 常见超限体积 |
| `extreme` | 4 GiB | ≈ 13 GiB | PSD 常态上限；G 组认证大图场景从此档起 |
| `manual` | 8 GiB | ≈ 24 GiB | 极限档：4 GiB 现实上限的**两倍冗余**；需 ≥ 32 GiB 空闲，仅手动 |

峰值按 `snapshot(≈logical) + 工作文件(实际占用，稀疏构造后远小于逻辑大小) + 恢复输出(≈logical)`
估算；大文件档按预计峰值并发准入，同一时刻大文件峰值之和不超过 `LILITH_STRESS_DISK_BUDGET`
（默认 24 GiB）。认证大图场景另计约 1–1.5 GiB 进程内存。

**压力测试不进入 CI**（维护者决定，2026-10-04）：它是发布前手动执行的套件，
与 `cargo test --lib`、`npm test`、`cargo fmt --check` 三条自动门槛完全分离。

## 6. 非目标（明确不做）

- 不做 GUI 自动化、窗口驱动、输入模拟；不启动 GUI 主流程（1.3）。
- 无头入口不进发布产物；不改动任何既有命令的行为。
- **不修改 CI**：不加步骤、不加门槛、不改变 `windows-ci.yml` 与 `release.yml`。
- 不把压力测试加入 `cargo test --lib` 或 `npm test`；它始终是独立入口。
- 不新增前端压力测试（3.3 已说明原因）。
- 认证模块**只覆盖允许范围内的大图**；超限输入的拒绝路径已有单测，不重复。
- **不为「极端」而构造现实不会遇到的输入**：规模与参数场景的量级收敛到个人长期使用的真实
  上限（数百 Artwork、单作品数百节点/数十分支、单画板数百张图片），见 §3.1 的「现实可遇性」。
- **新增依赖数为 0**；不为测试引入任何新库（4.7）。
- **不新建测试证书**：复用 `tests/fixtures/authenticity/` 已有材料（4.1.1 第 3 点）。
- **不把大图测试素材提交进仓库**：按需生成、用完即删。
- **不为省磁盘而改动分块格式**（例如给 snapshot 加压缩或写入去重）。那会改变产品行为与
  磁盘占用特征，属于独立议题；本批次只测量并如实记录现状的存储放大（4.6 的账本断言）。
- **不自动化验证断电持久性**：`Child::kill()` 不触及 OS/磁盘缓存与目录项落盘，无法覆盖
  「断电后已提交数据是否仍在」；该项由 OS/磁盘与 `synchronous = FULL` 声明保证，属人工/声明项。
- **不改变清理语义**：清理体系的两段式（扫描报告 + 用户确认）由 GUI 与无头子命令共用同一批
  领域函数，压力测试只断言其行为，不新增「自动回收」这类产品能力（B5、H2 只验证既有入口）。

## 7. 实施批次

每批次独立提交、独立验证。

### 批次 1：无头入口骨架与取消边界（A）——已落实（2026-10-04）

- `Cargo.toml` 加 `[features] headless`；给已有的 `windows-sys` 增加
  `Win32_System_ProcessStatus` 与 `Win32_System_Threading` feature；`lib.rs` 加 `run_headless`；
  `main.rs` 分派参数。
- 实现 `init-repository` / `create-artwork` / `commit` / `restore` / `compact` /
  `checkpoint` / `scrub` / `verify` / `cleanup` / `repository-backup` 子命令，以及
  `--result`、`--marker`、`--cancel-on-stdin`、`--peak-memory`、`--workspace` 通用选项。
- 无头适配器保持**1:1 薄映射**：只做参数解析、锁与状态的装配、领域函数调用，
  **不含任何业务判断**；锁与状态复用可无 Tauri 构造的 `BackupState::default()` 与
  `AppState::new(...)`。
- `tests/stress_support/mod.rs` 与 `tests/stress_cancel.rs`：工作区、进程编排、A1–A6。
- 验证：`cargo test --features headless --test stress_cancel` 全通过；
  `cargo test --lib` 与 `npm test` 结论不变；`cargo fmt --check`、`git diff --check`；
  **`npm run legal` 前后对比**，确认 `licenses/THIRD_PARTY_LICENSES.html` 无变化（见 4.7）。

### 批次 2：跨进程崩溃（B）——已落实（2026-10-04）

- 复用批次 1 的编排，实现「逐检查点闸门暂停后 `Child::kill()`」（见 §4.4 订正）+ 重开断言；B1–B3。
- 验证：`cargo test --features headless --test stress_crash` 全通过；确认强杀后 `verify` / `scrub`
  通过；`cargo test --lib`、`npm test` 结论不变；`cargo fmt --check`、`git diff --check`、
  `npm run legal` 前后一致。
- 结论：崩溃孤儿与灾备未发布暂存目录**不会**自动回收（见 §4.4 订正），该产品能力转入 `todo.md`。

### 批次 2 补充：事务中途崩溃（B4）——已落实（2026-10-04）

- 给 `history::commit` 增加一个 **headless-only** 标记点（feature 门控，发布产物中不存在），
  使测试能在「事务已 BEGIN、INSERT 已写、尚未 COMMIT」处经闸门停住再强杀；领域逻辑与
  GUI 路径行为不变。标记点由进程级闸门（`headless::commit_marker`）驱动，同时作为第 5 个
  取消检查点，因此 A1 的取消覆盖从 1–4 扩展到 1–5。
- 实现 B4：强杀后 `verify`（迁移 + `integrity_check` + 外键 + 语义校验）与 `scrub` 全通过、
  未提交事务被丢弃（head 与节点数不前进、父节点 snapshot 未被释放、清理队列无新增条目）、
  随后提交正常。
- 前置与依赖：与其它批次无依赖；共用 B 组全部编排，只增加一个标记点。
- 验证：同批次 2；额外确认 GUI 路径（`run_branch_backup`）行为与文案不变。
- 实测（2026-10-04）：`stress_crash` 4/4 通过，B4 强杀落在第 5 个检查点
  （`事务已写入，尚未提交`）、耗时 83 ms；`stress_cancel` 6/6 通过，A1 第 5 个检查点取消时
  错误为「提交已取消」（走 `history::commit` 的错误分支，与第 4 个检查点的
  「备份操作已取消」区分开）。

### 批次 3：大文件端到端（C）——已落实（2026-10-04）

- 工作文件生成器（稀疏构造 + 有界随机区域）。
- 磁盘账本、`LILITH_STRESS_DISK_BUDGET` 守卫、`LILITH_STRESS_TIERS` 档位选择。
- 实现 C1–C6（C6 为「大文件大改动」最坏存储放大）；峰值内存采样落盘。
- 落实偏差（2026-10-04）：执行模型由「大文件档串行」改为**按磁盘预算并发**
  （`scenario_slot`，见 §4.6）；磁盘上限默认值由 12 GiB 提到 24 GiB；工作区从 `%TEMP%`
  移到项目 `target/stress-workspaces/`（`%TEMP%` 与系统共用，实测出现过工作区中途被回收）。
- 验证：`default`/`large`/`heavy`/`extreme` 四档 6/6 通过；账本实测增量与预期偏差 ≤ 0.03%；
  运行结束后工作区自动清理、磁盘占用回落。实测数值见 `docs/guides/stress-test-report.md`。

### 批次 4：规模与灾备（D、E）与参数边界（F）——已落实（2026-10-04）

- **无头入口扩展**（1:1 薄映射、feature 门控）：`create-group` / `move-node` / `trash-node` /
  `empty-trash` / `list-tree` / `search` / `create-branch` / `delete-branch` / `list-history`。
  它们映射到既有的 `library::{create_group, move_nodes, trash_nodes, empty_trash, list_tree,
  search}` 与 `history::{create_branch, delete_branch, list}`，不含业务判断。
- `tests/stress_scale.rs`：D1（深链 300 提交）、D2（约 20 分支的历史图 + 分支删除 + 回收站
  清空）、D3（约 400 Artwork + 嵌套分组 + 搜索 + 移动）、E1（含多 Artwork 的整仓灾备）、
  F1（参数边界）。
- 验证：`cargo test --features headless --test stress_scale` 通过；`cargo check --lib`
  与 `cargo test --lib`、`npm test` 结论不变；`cargo fmt --check`、`git diff --check`。

### 批次 5：崩溃孤儿回收闭环（B5）——已落实（2026-10-04）

- **无头入口扩展**：`scan-unreferenced`（`cleanup::scan_unreferenced`）、
  `cleanup-unreferenced`（`cleanup::cleanup_unreferenced`），保持 1:1 薄映射、feature 门控。
- `tests/stress_cleanup.rs`：B5——复用 B 组强杀编排造出孤儿 snapshot/delta，随后
  `scan-unreferenced` 发现孤儿、`cleanup-unreferenced` 确认清理删除、再扫描无候选；
  被引用的实体文件不被误删；清理后 `verify` / `scrub` 仍通过、后续提交正常。
- 验证：`cargo test --features headless --test stress_cleanup`；`cargo test --features headless
  --test stress_crash` 结论不变；`cargo check --lib`、`cargo test --lib`、`npm test` 不变。

### 批次 6：画板 DDS 完整性（H）——待实施（前置：无头画板命令）

- 需要先给无头入口补上画板命令（创建画板、导入图片），否则「记录 → 文件」的缺失/损坏路径
  无法构造——画板记录只能由导入流程写入，测试进程不直接访问数据库。
- 场景：H1（双向检查：正常/缺失/损坏/孤儿四类计数，缺失与损坏报告不失败）、
  H2（孤儿 DDS 经 `scan-unreferenced` 发现 + 确认清理，被引用 DDS 不误删）、
  H3（单画板数百张图片的规模与取消）。
- 说明：孤儿 DDS 的**发现与清理**已可用（并入批次 5 的扫描），缺的是「记录 → 文件」方向
  所需的画板写入入口；因此 H 拆到本批次单独做。

### 批次 7：认证模块（G）——唯一触及安全边界的批次

- **授权作用域重构**：把 `ensure_dialog_authorized` 的路径授权来源抽象为可注入的作用域；
  Tauri 侧仍用 `window.fs_scope()`，行为逐位不变；无头侧只允许 `--workspace` 之下的路径。
  4 处调用点同步。
- 无头子命令 `enter-publication` / `publish` / `cancel-publication` / `decode-authenticity`；
  `--models <dir>` 选项，默认 `env!("CARGO_MANIFEST_DIR")/resources/models`。
- `tests/stress_authenticity.rs`：复用 `tests/fixtures/authenticity/` 现有材料；
  大图由测试进程用已有的 `image` 依赖按需生成（16K 级，仅 `extreme` 起）。
- 实现 G1–G5。
- 验证：`cargo test --features headless --test stress_authenticity`；**GUI 路径回归**——
  确认授权作用域重构后 `enter_branch_publication` / `publish_branch_artifact` /
  `create_repository_backup` 的行为与文案不变（这批改动触及安全控制，需单独人工确认）。

### 批次 8：文档与收尾

- `docs/guides/validation.md`：新增「压力测试」小节（定位、无头入口、运行方式、档位与
  磁盘上限、覆盖矩阵、与人工门槛的分工、纪律边界、报告解读），并写明**它不在 CI 中**；
- `docs/guides/validation.md` 与 `docs/guides/release-policy.md`：写明**断电持久性不做自动化
  验证**（`Child::kill()` 不触及 OS/磁盘缓存与目录项落盘），由 OS/磁盘与 `synchronous = FULL`
  声明保证；
- `docs/architecture/overview.md`：补一句无头入口的存在与用途（非发布功能）；
- `docs/modules/history-and-backup.md` 与 `docs/modules/authenticity.md`：新增
  「可靠性不变量与覆盖」，逐条陈述命题并标注由哪个场景证明；
- `todo.md`：把已被替代的人工验收项标注为「已自动化覆盖，保留人工复核手感」；确认第一节的
  「崩溃残留文件回收」与第二节的「事务中途崩溃」「断电持久性」三条状态与实际一致；
- `CHANGELOG.md` 追加条目（版本段由维护者决定）；
- 本文件移入 `archive/`，并在 `archive/README.md` 时间线登记。

## 8. 关键决定与待确认事项

**已定（维护者确认，2026-10-04）：**

- **接口方案采用无头命令行模式**：测试是独立进程，通过命令行调用真实可执行文件，
  跨进程边界、零内部访问。
- 无头入口 feature-gate，发布产物中不存在，因此产品行为零变化。
- 测试落在 `src-tauri/tests/` 集成测试目标，只用主 crate 已有依赖，**新增依赖数为 0**。
- 结果写 `--result` 文件而非 stdout（release 构建无控制台）。
- 干预手段只有三种：stdin 取消、`Child::kill()`、marker 文件；不触碰内部状态。
- 全部测试在**单一临时工作区根**内完成（4.6）；大文件档按磁盘预算并发、小规模档并行。
- **档位从 64 MiB 起步**（日常典型体积即默认档）：`default` 64 MiB / `large` 256 MiB /
  `heavy` 1 GiB / `extreme` 4 GiB / `manual` 8 GiB（4 GiB 的两倍冗余）。
- **测量峰值内存**：由无头进程自报（`--peak-memory`），同时记录启动基线与峰值，
  使增量可解释。
- **不修改 CI**：压力测试不进任何 CI 步骤，保持为发布前手动执行的套件。
- **纳入认证模块**（G 组）：复用已有证书夹具，按需生成大图，并接受 4.1.1 的授权作用域重构。

**补充确认（维护者，2026-10-04，订正与重排时定）：**

- B 组强杀采用「逐检查点闸门暂停后 `Child::kill()`」，取代原文的「轮询 marker 后 kill」（§4.4）。
- 崩溃孤儿与灾备暂存目录**不会被自动回收**；本计划断言「存在 + 不被引用 + 重开可用」，并在
  批次 5 用清理体系既有的「扫描 + 确认清理」入口验证回收闭环（§4.4、§7 批次 5）。
- 新增「批次 2 补充：事务中途崩溃（B4）」，以 headless-only 标记点验证 `synchronous = FULL`
  承诺（§7；2026-10-04 已落实）。
- 断电持久性**不做自动化**，由 OS/磁盘与 `synchronous = FULL` 声明保证（§6）。
- 排期按 §11.4 重排（2026-10-04 二次订正）：批次 1–3 与统一清理体系已落实，批次 4（规模与
  灾备、参数边界）、批次 5（崩溃孤儿回收闭环）随后落实；批次 6（画板 DDS）、批次 7（认证）、
  批次 8（文档收尾）待实施。

**待维护者确认：**

1. **磁盘上限默认值**：`LILITH_STRESS_DISK_BUDGET` 默认 **24 GiB** 覆盖到 `extreme`
   （峰值约 13 GiB，含大变动场景）；`manual` 档峰值约 24–25 GiB，需运行时显式调高上限——
   **已按 24 GiB 落实**（2026-10-04 二次订正，原建议 12 GiB 会误判 `extreme` 超预算）。
2. **无头模式的可发现性**：是否写入 `docs/architecture/overview.md`、是否出现在 `--help`
   的显眼位置。倾向：写入文档说明其定位，但不在帮助里作为主要入口宣传，避免被当作产品功能。

## 9. 不变量与风险

**不变量：**

- 无头入口在发布构建中不存在；GUI 路径行为与现状逐位等价，**包括授权作用域重构之后的
  `enter_branch_publication` / `publish_branch_artifact` / `create_repository_backup`**。
- 所有测试在 `TempDir` 内完成，不读取、不修改用户真实仓库（延续 `validation.md` 既有要求）。
- 测试进程只通过命令行参数、stdin 与信号干预被测进程，不读取其内部状态。
- **新增依赖包数为 0**；测试不引入任何新库。
- 不改变 `cargo test --lib`、`npm test`、`cargo fmt --check` 的现有结论。
- **不修改任何 CI 配置**。

**风险与缓解（含「这么做会破坏什么」的逐项回答）：**

1. **授权作用域重构触及安全控制。** `ensure_dialog_authorized` 是防止被攻陷的 webview
   读写任意路径的控制。重构后必须保证：Tauri 侧仍走 `window.fs_scope()`、行为与文案
   逐位不变；无头侧的作用域**只允许 `--workspace` 之下**，且不构成「检查可关闭」的开关。
   → 缓解：该改动单独成批次（批次 7），并列入人工确认项；不接受任何形式的无条件跳过。

2. **无头入口与 GUI 路径行为分叉。** 若两条路径各写一份编排，可能出现「测试通过而 GUI 失败」。
   → 缓解：无头适配器保持 1:1 薄映射、零业务判断；锁与状态复用可无 Tauri 构造的
   `BackupState::default()` 与 `AppState::new(...)`；一旦适配器变厚，改为抽取共享编排层。

3. **`Cargo.toml` 的 feature 变化可能改变依赖包集合。** 新增 `[features] headless` 与给
   `windows-sys` 增加 `Win32_System_ProcessStatus`、`Win32_System_Threading` 都不引入新包，
   但 feature 变化**有可能**连带启用新的传递包。若包集合变化，
   `licenses/THIRD_PARTY_LICENSES.html` 必须重跑 `npm run legal` 并提交，否则 CI 的许可证
   一致性检查会失败。→ 缓解：批次 1 的验证步骤里包含 `npm run legal` 前后对比（4.7）；
   **实测（2026-10-04）**：批次的 feature 变化后 `npm run legal` 输出逐字节不变（529 组件）。

4. **无头入口与测试代码腐烂。** 因维护者决定不修改 CI，feature-gated 的无头入口与
   `tests/` 下的压力测试**不会**被任何自动步骤编译，可能长期失修而无人察觉。
   → 缓解：把「`cargo test --features headless --test stress_*` 至少跑 `default` 档」
   写入 `docs/guides/validation.md` 的发布前手动清单；这是本批次**明确接受**的代价，
   换取 CI 零改动。

5. **认证测试首次构建需网络。** `trustmark` 经 `ort-sys` 的构建脚本从 CDN 下载预编译
   ONNX Runtime，全新环境首次构建必须联网——这不是本批次引入的，但认证测试让它变成
   必经路径。模型文件本身已在仓库中（约 64 MB），无需下载。

6. **大图生成的内存峰值。** 生成 16384×16384 图像需约 1 GiB 内存，发布链路解码同样如此。
   → 缓解：G 组只在 `extreme` 及以上档位运行；峰值由 `--peak-memory` 记录并可判定是否
   超出声明的 1.5 GiB 解码预算量级。

7. **崩溃时机不可复现导致测试抖动。** → 缓解：用 marker 文件而非固定 sleep 确定干预时机；
   **落实时进一步收紧（2026-10-04）**：强杀改用逐检查点闸门让进程在目标检查点阻塞，
   由测试在进程外 `kill`，因此不存在与进程赛跑的问题（§4.4 订正）。

8. **大文件档位占满磁盘。** snapshot 成本不可规避（4.6）。
   → 缓解：大文件档按磁盘预算并发准入（`scenario_slot`）、场景之间即时拆除、以
   `LILITH_STRESS_DISK_BUDGET` 事前守卫；空间不足时**明确跳过并报告**。

9. **并行场景共享 SQLite 导致断言失效。** → 缓解：并行场景一律使用工作区内的独立子仓库
   （`repository-p<slot>/`），不与大文件主仓库共用。

10. **孤儿文件被误判为缺陷。** B 类场景中「文件已发布、数据库未提交」的孤儿是**预期行为**。
    → 缓解：断言检查「不被引用 + 重开可用 + 后续命令正常」，而不是「不存在」。
    **二次订正（2026-10-04）**：清理体系已补上「发现」（`scan_unreferenced`）与「确认清理」
    （`cleanup_unreferenced`）；孤儿**不会被自动回收**，但可被发现并经用户确认后回收。批次 5
    用这两个入口验证回收闭环（B5）。

11. **无头模式被误当作产品功能。** → 缓解：文档明确其定位为开发/验证期入口、不进发布产物；
    `--help` 中不作为主要入口宣传。

## 10. 实施后的文档动作

按 `AGENTS.md` 的文档纪律，形成可追溯的三元组：**① 不变量（模块文档）→ ② 场景
（`validation.md` 覆盖矩阵）→ ③ 结果（`current-handoff.md` 实测）**。只有这条链完整，
文档才构成可靠性的**证据**，而不只是声明。

- `docs/guides/validation.md`：新增压力测试入口与运行方式，明确它与人工门槛的分工
  （自动化覆盖「可程序判定」部分，人工保留界面视觉、手感与桌面交互）；
- `docs/modules/history-and-backup.md`：新增「可靠性不变量与覆盖」，逐条列出原子发布、
  不覆盖、取消无残留、崩溃可恢复、摘要链完整、存储放大有界，并标注各自的证明场景；
- `docs/architecture/overview.md`：说明无头入口的存在、用途与「不进发布产物」的定位；
- `docs/planning/todo.md`：**更新第二节，把本批次覆盖的条目从「待人工验收」改为
  「已自动化覆盖 + 保留人工复核手感」**——具体是「各处理阶段的取消边界」「损坏文件的
  恢复路径（构造损坏文件与摘要不匹配部分）」「极端大文件与高像素压力的错误恢复」三项；
  同时确认第二节「事务中途崩溃」（批次 2 补充覆盖）与「断电持久性」（不自动化）状态一致，
  以及第一节的清理相关 P1（已由统一清理体系落实）已从清单移除；
- `docs/planning/current-handoff.md`：记录本批次各档位的实测数值与结论；
- `docs/planning/archive/`：本文件与 `archive/README.md` 时间线登记。

> **收尾提醒（必须执行）**：本批次落实并通过后，`todo.md` 第二节与本节 11.1 的覆盖结论
> 必须同步更新；否则清单会与实际覆盖情况不一致。这是批次 8 的验收条件之一。

## 11. 本批次在发布路线中的位置

**结论：本计划（压力测试）完成不等于可以进入 rc1。** 它补齐的是 `release-policy.md`
人工门槛里「可程序判定」的那一部分；rc1 还需要另外四类工作。
截至 2026-10-04（批次 2 补充落实后），批次 1–5 与批次 2 补充已落实（含统一清理体系）；
批次 6–8 未落实。

### 11.1 本批次覆盖的部分

`todo.md` 第二节原列 8 项待验收。其中 3 项已由维护者于 2026-10-04 实测通过（见
`current-handoff.md`），不再是待验收项；剩余 5 项中，本计划覆盖 2 项、部分覆盖 1 项
（下表「本批次」指本压力测试计划整体，跨批次 1–8）：

| 待验收项 | 状态 |
| --- | --- |
| 各处理阶段的取消边界（提交、恢复、精简、检查点、整仓灾备、认证签名） | ◐ 部分覆盖（A 组已落实；认证签名 G3 待批次 7） |
| 极端大文件与高像素压力的内存、取消、退出与错误恢复 | ◐ 日常使用已通过（2 GiB 工作文件）；**错误恢复**由 C 组 + G 组覆盖 |
| 损坏文件的恢复路径（snapshot/delta 缺失或摘要不匹配） | ◐ 部分覆盖（崩溃窗口由 B 组、事务中途由 B4、孤儿回收由 B5；缺失/摘要不匹配由既有单测）；异常时间戳服务仍需人工 |
| 画板 DDS 损坏、缺失、孤儿文件 | ◐ 部分覆盖（清理体系已提供双向检查与孤儿回收能力；H 组待批次 6） |
| 异常 RFC 3161 时间戳服务的失败与超时行为 | ❌ 不在范围，保留人工 |
| 普通用户账户安装、Authenticode 签名与时间戳验证 | ✅ 已实测通过（2026-10-04），不再是待验收项 |
| 第三方工具回读 C2PA、随包模型验证 TrustMark 实图 | ✅ 已实测通过（2026-10-04），不再是待验收项 |
| 从上一公开候选版安装升级与卸载 | ✅ 已实测通过（2026-10-04），不再是待验收项 |

### 11.2 rc1 仍需完成的工作

1. **待实现的缺陷修复与批次**（`todo.md` 第一节）：
   - **P1 ×6**：画板结算改为提交后清理；接入素材板运行时变更结算流程；历史文件清理失败
     改为可观测可重试；仓库完整性扫描覆盖画板 DDS；画板实体与 SQLite 双向清理检查；
     崩溃残留文件（历史孤儿与灾备暂存目录）的回收。**其中 5 条已由统一清理体系落实**
     （2026-10-04），仅「接入素材板运行时变更结算流程」仍待办。
   - **P2 ×3**：长任务与灾备并行的降级策略；超期临时目录清理；历史长操作的批量与结果刷新。
     （「任务调度总控与空闲链路校验」已于 2026-10-04 落实，不再是待办；
     「工作文件路径清空约束的人工确认」已于 2026-10-04 实测通过。）
2. **发布链验证**（`release-policy.md` 自动门槛第 6–10 条）：生产与完整依赖漏洞审计、
   RustSec 审计、许可证与 SBOM、从发布标签构建 NSIS 并断言版本/schema/标识/校验和、
   解包检查许可证。
3. **人工桌面验收**：`release-policy.md` 的人工门槛清单已由维护者于 2026-10-04 在本机
   生产使用中走通（见 `current-handoff.md`）；rc1 前仍需在**干净 Windows 用户环境**重跑
   一轮，并补上取消边界、损坏恢复、异常时间戳、画板 DDS 四类未验证项。
4. **版本与标签机制**：`v0.2.0-alpha.3` 标签**尚未创建**（`current-handoff.md` 记录为
   待 Windows CI 跑通后再打）；递增到 `0.2.0-rc.1` 需同步五处版本字段、同步
   `tools/release/verify-metadata.mjs` 的 schema 断言，并确保 Windows CI 通过。
5. **待维护者决策 3 项**（`todo.md` 第三节）：Lilith Client 侧旧实现的去留；是否声明
   旧数据迁移支持；候选版到正式版的标签推进。

### 11.3 与 0.2.0 正式版的差距

按 `release-policy.md`：「未经人工验收的构建只能标记为内部测试包」。因此
**rc1 → 0.2.0 的差距 = rc 期间暴露问题的修复 + 最终一轮完整人工验收 + 正式标签与安装包**。
没有额外的功能门槛，收敛速度取决于 rc 期间的缺陷发现率，而不是新增工作量。

### 11.4 排期建议（2026-10-04，批次 2 补充落实后）

**已落实**：批次 1（无头入口与取消边界）、批次 2（跨进程崩溃）、批次 2 补充（事务中途崩溃
B4）、批次 3（大文件端到端）、批次 4（规模与灾备、参数边界）、批次 5（崩溃孤儿回收闭环）；
统一清理体系与任务调度总控批次亦已完成。

**未落实**：批次 6（画板 DDS）、批次 7（认证 G）、批次 8（文档收尾），以及 §11.2 列出的
剩余 P1（素材板运行时结算接入）与发布链工作。

**建议顺序：**

1. **批次 6（画板 DDS H）**：需先补画板无头命令（创建画板、导入图片），工作量主要在无头入口。
2. **批次 7（认证 G）**：唯一触及安全控制（授权作用域重构），需单独人工确认。
3. **批次 8（文档与收尾）**：最后统一更新 `validation.md`、模块文档、`todo.md`、`CHANGELOG.md`
   并归档本文件。
4. **剩余 P1（素材板运行时变更结算接入）**：与压力测试写路径相关，建议在批次 6 之前完成，
   避免画板相关断言重跑。

**不进入自动化**：断电持久性（见 §6）。**孤儿与暂存目录的回收**已由统一清理体系提供
（扫描报告 + 用户确认），批次 5 验证其闭环；它不再是「待落地的产品能力」。



