# 验证策略

## 明确禁止

本项目禁止代理执行 GUI 自动化测试，包括启动桌面窗口进行操作、Playwright/Electron/Tauri 窗口驱动、截图和基于截图的视觉检查。界面视觉与交互由用户在阶段验收时手工检查。

同时禁止输入操纵：不得使用 `SendInput`、鼠标/键盘模拟、`pyautogui` 或注入式 `SendMessage` 等手段驱动真实光标与按键，也不得用自动化方式打开和操作应用窗口。允许的自动化手段只有程序化断言、HTTP 契约测试、进程外只读查询（`EnumWindows` / `GetWindowRect` 等）和纯计算复现。报告结论时只陈述断言结果，涉及视觉与手感的部分明确标注"待人工确认"。

## 分层验证

- 允许 `npm run build`、`npm test`、`cargo fmt --check`、`cargo check` 和 `cargo test`，
  即依赖已经可以正常编译和运行测试，不再要求绕开重依赖。
- 不允许编译 release 产物（`cargo build --release`、`npm run tauri build`），
  也不允许启动应用本身做自动化操作；界面与手感仍由用户手工验收。
- 每次修改运行 `git diff --check`，检查补丁空白和冲突标记。
- 前端纯逻辑、请求竞态或交互状态变更运行 `npm test`；素材板范围可用 `npm run test:pin-board`。Windows CI 固定运行完整前端测试集。
- 数据库变更使用临时目录与临时 SQLite 文件测试，不读取或修改用户真实仓库。如引入 schema 迁移，迁移 SQL 先在仓库数据库副本上用只读脚本核对效果，再进入 `schema.rs`。
- 文件提交/恢复测试使用小型确定性样本，重点验证哈希、原子发布、不覆盖和取消边界。
- `backup` 模块的单元测试含**确定性属性测试**（固定种子随机版本链与随机提交/恢复序列，
  断言「恢复任意历史节点都与提交当时的字节逐字节一致」）与**磁盘写满注入**（注入写入器在
  真实快照写入路径上返回 `ErrorKind::StorageFull`，断言干净失败、仓库不受影响且可重试）。
  两者不引入新依赖、随 `cargo test --lib` 运行、秒级完成，因此进 Windows CI。
- **四个解析器各有一组确定性畸形输入模糊**（`chunk_file` 快照/增量容器、`pin_board::dds`
  DDS 头、`authenticity::c2pa` 清单、`app::settings` 设置文件），生成器集中在 `src/fuzz_support.rs`
  （只在 `cfg(test)` 下编译，亦是 `backup::test_support::Rng` 的实现）。断言的是「不受信任的
  字节不能换来 panic / 越界 / 无界分配 / 无限循环」：被接受的容器必须自洽（块数量不超过文件容量、
  可物化且长度等于声明值），被接受的 DDS 尺寸必须落在声明上限之内，解析失败的设置文件必须被
  **原样保留**。同样零新依赖、秒级完成、随 `cargo test --lib` 进 CI。
- `app/settings.rs` 的单元测试覆盖「**长写任务持仓库操作锁期间，只读命令仍能作答**」这一并发
  承诺：写线程持排他锁并停住，读线程走共享读租约，断言读在超时内返回（若读路径错误地取了排他
  锁，会以超时失败而不是无限挂起）。同处还覆盖「切换仓库会等活跃租约」。
- 仓库内**主库文件本身**被改坏（非数据库内容 / 清空）时的干净失败由 `stress_damage` 的 R3/R4
  覆盖，与该组的 R1/R2（快照/增量内容损坏）互补——两层校验都打不开主库时也应给出可行动的错误。
- `backup/chunk_file/` 与 `backup/restore.rs` 只依赖 sha2、zstd、tempfile（restore 另需 history/storage 少量接口），可以在仓库外用只含这几个依赖的轻量 crate `#[path]` 引入编译并运行其测试；这是重依赖边界内验证分块与物化逻辑的标准做法。
- `trustmark` 经 `ort-sys` 的构建脚本从 pyke CDN 下载预编译 ONNX Runtime，因此**全新环境首次构建需要网络**。改动依赖版本后必须重跑 `npm run legal` 并提交 `licenses/THIRD_PARTY_LICENSES.html`。

## 压力测试：便宜的分组进 CI，昂贵的仍只在发布前运行

`src-tauri/tests/` 下的十三组压力测试（`stress_cancel` 取消边界、`stress_crash` 跨进程强杀、
`stress_large` 大文件端到端与大文件中断、`stress_scale` 规模与灾备与参数边界、`stress_cleanup`
崩溃孤儿回收闭环、`stress_pin_board` 画板 DDS 完整性、`stress_authenticity` 认证发布内存与回读、
`stress_damage` 快照/增量/主库的损坏检测与恢复（含随机化单点损坏与画板 DDS 损坏）、
`stress_format` ChunkFile 格式矩阵——合成 `.clip` 容器感知分块（tile 结构 / 无 tile / 截断回退
CDC）与 v1/v2 混存仓库的提交、恢复、检查点与跨格式物化，断言用磁盘事实（ChunkFile 公共头的
版本 / chunker / adapter 字节，delta 先过 zstd 解压）逐提交核对、
`stress_concurrency` 多进程哨兵锁、高频命令与多操作交错（写 + 读、读 + 读、真并发结构写 + 全库读、
写哨兵持有期间全库校验的明确拒绝）、
`stress_input` 输入域极值与路径边界、`stress_bench` 核心算法性能基准、`stress_baseline` 0.3
阶段 0 基线——锁等待 / 任务耗时 / 取消延迟 / 深链恢复 / 调度扫描 / 清理查询 / 灾备 I/O）都以
**真实可执行文件**驱动，只通过命令行、stdin 与磁盘事实断言。

- **由 `headless` feature 门控**：所有 `stress_*.rs` 目标都以 `#![cfg(feature = "headless")]`
  开头，不带该 feature 时它们编译为空目标。因此 `cargo test`（包括裸跑）既不会构建无头二进制、
  也不会启动任何被测进程。原因：`cargo test` 会自动发现并运行 `tests/` 下的**每一个** `.rs`
  文件，文件放在那里就等于默认参与裸 `cargo test`；而在没有该 feature 的构建上 `--headless`
  会被忽略、应用直接以 GUI 启动（debug 档加载 `devUrl`，没有 `npm run dev` 时就是一个
  「找不到网页」的窗口）。被测二进制在启动子进程前会做一次 `--headless help` 探针，指向未启用
  该 feature 的构建会立即报错终止，而不是把 GUI 拉起来。
  **新增 `tests/stress_*.rs` 时必须同样在文件开头写 `#![cfg(feature = "headless")]`**：
  `tests/gate_guard.rs` 会扫描全部 `stress_*.rs` 并断言这一行存在，缺了会直接失败（该守护测试
  自身不带门控，因此在裸 `cargo test` 下就运行），把「漏写门控 → 裸 `cargo test` 拉起 GUI」
  这类事故挡在提交前。
- **交互检查点只出现在边界，热循环只做静默观察**：取消观察分两层（`CancelToken::is_cancelled`
  与 `CancelToken::observed`）。显式边界（提交、恢复、精简、检查点、灾备的阶段边界）仍发一枚
  交互检查点，写一行 marker、并在 `--cancel-on-stdin` 下阻塞等判定；分块主循环（`ChunkTask`）
  只读「取消是否已被判定」，不发 marker、不阻塞。因此**检查点序号与阶段数量由边界决定、与
  文件分成多少块无关**，marker 文件也不会随处理字节数膨胀——测试因此可以按固定序号强杀/取消，
  且删除或新增一处边界时会同时改变序号（改测试的定位断言时要连注释一起核对）。边界判定的取消
  会被热循环在下一次静默查询时立刻看到，取消延迟不回退。
- **按成本分层**：**九组便宜的分组（`stress_cancel` / `stress_crash` / `stress_cleanup` /
  `stress_damage` / `stress_format` / `stress_pin_board` / `stress_authenticity` /
  `stress_concurrency` / `stress_input`，合计约 30 秒运行）进 Windows CI**，日常改动因此获得自动回归保护；
  **`stress_large`（数十分钟到两小时，视档位）、`stress_scale`（约 1 分钟）、`stress_bench`
  与 `stress_baseline` 仍只在发布前手动运行**
  ——前两组依赖大量磁盘与时间，后两组是性能基准（`stress_baseline` 还需一份临时仓库副本），
  数值需要人工解读（见其文件头与本节的环境变量）。
- **`stress_baseline` 的真实仓库场景是一次性采集，不是长期门槛**：`LILITH_BASELINE_REPO`
  指向的副本由维护者临时准备（外部目录，随 0.3 计划落实后撤除）。该组不生成基线文件、不设
  阈值、不进 CI；长期回归对照用它的合成场景（深链恢复、取消延迟）与 `stress_bench`，不要在
  日常改动或后续批次里把它当默认测试标准。
- **日常开发只在改动范围内运行轻量检查**：`npm test`、`cargo fmt --check`、`cargo check`、
  `cargo test --lib`、`git diff --check`。CI 已覆盖的九组不必在本地反复跑；**其余分组不要顺手
  运行**——即使到了收尾、整理、提交前的阶段也一样。
- **昂贵分组的耗时与磁盘是设计属性而不是卡住**：测试以真实落盘路径执行，覆盖「逻辑文件大小 ×
  单次改动量」的四象限；`stress_large` 的 `large` 及以上档位单次运行常以十分钟计，峰值磁盘占用
  可达数 GiB，`extreme` 档（4 GiB 文件、每次改 1 GiB）接近十几 GiB，`manual` 档（8 GiB 文件、
  每次改 2 GiB）约 25–32 GiB、单次约两小时。十一组（`stress_baseline` 之前的口径）+ C 组五个
  档位整轮实测约 3 小时（一个 16 逻辑核 / 32 GiB 内存的桌面机，空闲磁盘 ≥ 77 GB；2026-10-17
  实测，自该轮起 C7 随五个档位并行运行、与其余场景争用磁盘，是耗时较此前约 95 分钟上升的主因）
  ——新增的两组只加秒级开销；`stress_baseline` 另计约 7 分钟（其中整仓灾备 ≈4 分钟、全库校验
  约 3 分钟，需真实仓库副本）。
- **昂贵的分组只在准备发布、需要产出或更新 `docs/user/stress-test-report.md` 的实测数据时运行**，
  并由维护者显式发起；需要跑时先确认档位与磁盘预算。
- **大文件档按磁盘预算并发执行**：`stress_large` 的场景通过 `scenario_slot` 按「预计峰值磁盘」
  准入，可多个并行，但保留峰值之和不超过 `LILITH_STRESS_DISK_BUDGET`（默认 24 GiB），并发数
  不超过 `LILITH_STRESS_JOBS`（默认可用核数）。这样在有磁盘余量的机器上充分利用多核——
  被测子进程是单线程的，串行只会用一个核——同时避免盲目并行把磁盘写满。
- **墙钟不能作为跨版本性能标准**：正因为上面这条与 `cargo test` 默认并行跑测试函数，同一台机器
  上会同时跑多个单线程被测进程，「整机 CPU 占用」随并发场景数变化、「单次操作墙钟」被调度拉长。
  2026-10-18 的实测差异是 2.3–4.1 倍（同一基准并行时 P1 单次提交 121.6 ms、P3 导入 1450.7 ms；
  `--test-threads=1` 后 53.7 ms / 353.0 ms）。因此：`stress_bench` 的数字**只认
  `--test-threads=1` 采集**，且只在同一机器、同一构建档内比较；跨版本谈快慢不要引用群跑墙钟。
  测试目前只采墙钟与峰值内存，**不采进程 CPU 时间**——要评估 CPU 成本需另加度量，见
  `docs/planning/todo.md`。
- 运行方式（构建无头入口、档位选择、环境变量与输出文件）见下方「运行方式」；各档位的实测数据、
  磁盘需求与墙钟见 `docs/user/stress-test-report.md` 的「运行成本」一节。
- **断电持久性不做自动化验证**：测试用 `Child::kill()` 终止进程，它不触及操作系统与磁盘的
  缓存、也不保证目录项落盘，结构上无法覆盖「断电后已提交数据是否仍在」。该保证由操作系统、
  磁盘与 SQLite `synchronous = FULL` 声明承担，保留为声明/人工项，不列入自动化目标。

### 运行方式

被测对象是**带无头入口的二进制**，必须先用 `--features headless` 构建出来（该入口不进发布
产物）。运行命令由 `headless` feature 统一开闸：它既构建无头入口，也让 `tests/stress_*.rs`
参与 `cargo test`。

```text
# 1. 构建带无界面入口的二进制（该入口不进发布产物）
cd src-tauri
cargo build --release --features headless

# 2. 建议把测试指向发布档：峰值内存、存储放大与耗时才有参考价值
LILITH_STRESS_BIN=target/release/lilith-artworks.exe \
  cargo test --features headless --test stress_large

# 3. 选择档位（默认 default；大文件档需要相应磁盘与时间）
LILITH_STRESS_TIERS=extreme LILITH_STRESS_BIN=... \
  cargo test --features headless --test stress_large

# 4. 便宜的分组（MiB 级文件，秒级）：这些已在 Windows CI 中，本地一般不重复跑
cargo test --features headless --test stress_cancel
cargo test --features headless --test stress_crash
cargo test --features headless --test stress_cleanup
cargo test --features headless --test stress_damage
cargo test --features headless --test stress_format
cargo test --features headless --test stress_pin_board
cargo test --features headless --test stress_authenticity
cargo test --features headless --test stress_concurrency
cargo test --features headless --test stress_input
cargo test --test gate_guard

# 5. 规模（约 1 分钟，数百子进程）与性能基准（数值需人工解读；基线/比对用单线程采集）
cargo test --features headless --test stress_scale
cargo test --features headless --test stress_bench -- --nocapture

# 6. 性能基线的更新与回归门（默认只记录，见「可调环境变量」）
LILITH_STRESS_PERF_UPDATE=1 cargo test --features headless --test stress_bench -- --test-threads=1
LILITH_STRESS_PERF_STRICT=1 cargo test --features headless --test stress_bench -- --test-threads=1

# 7. 认证大图（16K 源图）：只在 extreme 及以上档位运行，否则 G1/G2 明确跳过
LILITH_STRESS_TIERS=extreme cargo test --features headless --test stress_authenticity

# 8. 阶段 0 基线（一次性采集，需临时仓库副本；只做读与「不写源仓库」的整仓灾备，用后还原副本）
$env:LILITH_BASELINE_REPO='D:\temp\Artworks'
cargo test --features headless --test stress_baseline -- --test-threads=1 --nocapture
```

可调环境变量：

- `LILITH_BASELINE_REPO`：`stress_baseline` 的真实仓库副本路径（如 `D:\temp\Artworks`）。
  **未设置时该组的真实仓库场景明确跳过**（不是失败），因此裸 `cargo test` 与 CI 不受影响。
  该副本是**外部临时资料**，只服务阶段 0 的一次性采集，0.3 计划落实后撤除，不要把它当长期
  测试夹具。运行会在仓库同级的 `baseline-out` 下建输出目录并在结束时删除，唯一残留是写命令
  取得跨进程 owner lock 时创建的 `.lilith-write-lock.sqlite3*` 与 `.lilith-write-owner.json`，
  采集后请用采集前的副本还原仓库。
- `LILITH_STRESS_TIERS`：档位（`default`/`large`/`heavy`/`extreme`/`manual`），默认 `default`。
- `LILITH_STRESS_BIN`：被测可执行文件，建议指向发布档。
- `LILITH_STRESS_DISK_BUDGET`：磁盘峰值上限（接受字节数或 `KiB/MiB/GiB/TiB` 后缀，默认 24 GiB）。
  **跑 `manual` 档必须显式调高**：该档的 `peak_disk_floor` 本身（3 × 8 GiB + 64 MiB ≈ 24.06 GiB）
  就已超过默认的 24 GiB，不改会把整个 C 组记为**跳过**（`record_skip`，报告里是 `skipped`）而不是
  失败。用 `LILITH_STRESS_DISK_BUDGET=40GiB`——C6 大改动场景的预计峰值约 32.2 GiB，是五档里的上界。
- `LILITH_STRESS_JOBS`：并发场景数上限（默认可用核数）。
- `LILITH_STRESS_PERF_UPDATE=1`：`stress_bench` 把本次测量写入 `src-tauri/tests/perf-baseline.json`
  对应构建档（`debug` / `release`）的条目。基线与构建档绑定——两者绝对耗时相差一个数量级。
- `LILITH_STRESS_PERF_STRICT=1`：`stress_bench` 与基线比对，超过 `LILITH_STRESS_PERF_FACTOR`
  倍（默认 `2.5`）即失败。默认**只记录不比对**，因为绝对耗时依赖机器，把机器差异变成
  flaky 断言没有意义；跨机器比对时按需放宽 factor。

结果写入 `target/stress-report.jsonl`（每场景一行 JSON）与 `target/stress-ledger.jsonl`
（磁盘账本）；工作区位于 `target/stress-workspaces/`，正常结束后自动回收。被测子进程是单线程的
（内容定义分块、SHA-256 与 zstd 都是单核路径），因此串行执行只会用一个核、磁盘远未跑满；
`stress_large` 按磁盘预算准入并行正是为了在有磁盘余量的机器上吃满多核，同时避免盲目并行把
磁盘写满。

## 持续集成

- `windows-ci.yml`：`dependency-audit`（ubuntu，npm audit、`rustsec/audit-check`、SBOM）与
  `validate`（windows，发布元数据、许可证一致性、前端构建与测试、`cargo fmt --check`、
  `cargo test --lib`、`cargo test --test gate_guard`、以及**九组便宜的无头压力测试**——构建
  headless 二进制后依次运行 `stress_cancel` / `stress_crash` / `stress_cleanup` /
  `stress_damage` / `stress_format` / `stress_pin_board` / `stress_authenticity` /
  `stress_concurrency` / `stress_input`）。
  把便宜分组前置到 CI 是为了让日常改动也有回归保护：它们此前只在发布前手动跑，问题会在
  「准备发布」时集中爆发。`stress_large` / `stress_scale` / `stress_bench` /
  `stress_baseline` 不进 CI。
- `release.yml`：推送 `v*` tag 时运行，含同样的审计，Rust 侧为 `cargo fmt --check` +
  `cargo test --lib`（不含压力测试——发布前的候选版验收另有一套完整重跑，见本文与
  `docs/user/stress-test-report.md`）。
- 两处易踩的坑：
  1. `rustsec/audit-check` 的 `ignore` 输入**只按逗号切分**。用 YAML 块标量每行写一个
     advisory ID 会被当成**单个**非法参数传给 `cargo-audit`，报错后 stdout 为空，action
     最终失败并只显示 `Unexpected end of JSON input`。必须写成单行逗号分隔。
  2. `trustmark` 用 `=` 精确锁定 `ort` / `ort-sys` 版本，无法单独 `cargo update`。升级前先
     确认目标 ort 版本使用的下载地址（rc.10 起为 `cdn.pyke.io`，rc.9 及更早的
     `parcel.pyke.io` 已失效）。

当接口跨越重依赖边界且静态检查无法提供足够保证时，停止阶段并请用户执行一次完整编译和针对性手工测试；收到结果后再继续，避免错误跨阶段累计。
