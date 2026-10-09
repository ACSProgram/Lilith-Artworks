# 素材板：待办问题与「卡死」调查交接（2026-10-09）

> **临时交接文档**。目的：把「已确认但尚未修复的三个问题」与「卡死调查的全部尝试、已排除的假设、  
> 踩过的坑」一次性交给下一位接手者，避免重复劳动。
>
> 结论先行：**前三个问题已定性（实现缺失或与设计意图不符），修复方案明确、改动很小。**  
> **第四个问题（卡死）已锁定触发条件与阻塞位置，但未定位到具体调用，且已排除最初的主要嫌疑。**
>
> 本文只覆盖上述四项与配套的诊断体系；未提交改动清单见第 6 节。
>
> 注：按 `docs/README.md` 的目录分工，未完成事项的**唯一清单**是 `docs/planning/todo.md`。  
> 本文是**临时调查记录**，用于保留调查过程与已排除的假设，不作为长期执行依据。
>
> **2026-10-09 补充**：其中的临时探针（`post-create` 三级台阶、队列探针、逐帧采样、`span` 步骤
> 记录）已在日志体系整理时全部删除，冻结取证改为「运行标识 + 标签索引 + 取证 Worker」。本文
> 引用的那些日志行不会再出现，保留仅作为当时的调查快照；当前有效事实见
> `docs/guides/logging.md`。

---

## 0. 一句话现状

| # | 问题                         | 状态                   | 性质                         |
| - | -------------------------- | -------------------- | -------------------------- |
| 1 | 拖动导入（Ctrl+V 外部粘贴已决定**不做**） | **已修复**（2026-10-09）  | 前端未接线（后端能力已就绪）             |
| 2 | Ctrl+A 全选（整个画布）            | **已修复**（2026-10-09）  | 完全未实现                      |
| 3 | Shift 旋转应"对齐绝对角度"          | **已修复**（2026-10-09）  | 实现与设计意图相反（增量吸附 ≠ 绝对对齐）     |
| 4 | 切换素材板作品/画板时程序卡死            | 已锁定触发与位置，**未定位具体调用** | WebView2 渲染进程挂起；设备泄漏假设已被排除；触发条件是渲染器 churn 本身，见 [2.7](#27-2026-10-09-1115-复现全量日志分析详细记录) |

> **2026-10-09 更新**：第 1–3 项已修复并由维护者实机验收通过，契约见
> `docs/modules/pin-board.md` 的「领域行为」与 `docs/planning/current-handoff.md`。第 1 节的
> 修复方向描述**不准确**：窗口实际以 `dragDropEnabled: false` 运行（Windows 上使用 HTML5 拖放
> 的前提），webview 不暴露文件系统路径，因此拖入导入走的是
> `import_pin_board_clipboard_image` 的**字节**链路，而不是本节假定的 `import_pin_board_images`（路径）；
> 代价是 DDS/TGA 无法拖入。第 4 项仍未解决。

---

## 1. 已确认但尚未修复的问题

> 本节 1.1–1.3 已于 2026-10-09 修复并验收通过，保留原文仅作为问题记录；当前有效实现见
> `docs/modules/pin-board.md`。

### 1.1 拖动图片导入

**需求**：把图片文件拖进素材板画布即可导入。

**现状**：

- 画布容器 `pin-board-canvas-shell`（`src/modules/pin-board/PinBoardModule.tsx:338`）**没有任何拖放处理**。  
  文件里出现的 `onDragOver` / `onDrop`（同文件 `1218` / `1231` 行）属于**侧栏画板行的排序拖拽**，  
  与图片导入无关。
- 全局 `window` 的 `dragover` / `drop` 被 `preventWebViewFileDrop` 统一 `preventDefault`  
  （`src/app/App.tsx:178-179`，实现见 `src/app/webviewShortcuts.ts`）。  
  因此**把图片拖进素材板当前毫无反应**。
- 后端能力**齐全**：`import_pin_board_images`（`src-tauri/src/pin_board/mod.rs:288`）、  
  `import_pin_board_clipboard_image`（`:324`）、`read_pin_board_clipboard_paths`（`:429`，Windows `CF_HDROP`）；  
  前端已在 `src/modules/pin-board/api.ts:61 / 73 / 93` 封装。
- `importImages()`（`PinBoardModule.tsx:823`）已实现「剪贴板位图 → 原生路径 → 文本路径 → 文件选择器」  
  的完整回退链，但**只挂在工具栏"导入图片"按钮（`:1342`）与右键菜单（`:1432`）**。

**结论**：属**接线缺失**，而非能力缺失。可参考 `src/modules/authenticity/AuthenticityModule.tsx`  
已有的拖放导入实现。

**决策（2026-10-09）**：**不**增加"外部 Ctrl+V 粘贴"，保持内部剪贴板语义，避免语义混乱；  
**只**增加拖入导入。

### 1.2 Ctrl+A 全选

**需求**：在素材板视图按 Ctrl+A 选中**整个画布**（画板内全部未删除图片），而不是仅当前视口。

**现状**：

- 全仓检索无 `selectAll` / Ctrl+A 任何处理。
- 渲染器 `keyDown` 只处理 Delete、Ctrl+S、Ctrl+Z/Y；模块层只处理 Ctrl+C / Ctrl+V。
- `selectedIds` 是渲染器私有 `Set`，**没有"全选"入口**；现有框选（marquee）只覆盖视口内图片。

**结论**：完全未实现。需新增一条"把未删除图片整体加入 `selectedIds` 并刷新选中态"的路径。

### 1.3 Shift 旋转归位（应"对齐角度"而非"固定增量"）

**需求（设计意图）**：按住 Shift 旋转时，应把图片的**绝对角度**吸附到 0°/15°/30°…，  
从而能够精确回到 0°。

**现状**（`src/modules/pin-board/renderer.ts:1427-1437`）：

```ts
} else if (this.dragMode === "rotate") {
  let radians = Math.atan2(
    world[1] - this.rotateCenter[1],
    world[0] - this.rotateCenter[0],
  ) - this.rotateStartAngle;
  if (event.shiftKey) {
    const increment = Math.PI / 12;                 // 15°
    radians = Math.round(radians / increment) * increment;
  }
  ...
}
```

它对**本次拖拽的增量**取 15° 整数倍，而非对**图片的绝对角度**取整。  
因此若图片原本已转了 7°，最终角度恒为 `7° + n×15°`，**永远回不到 0°**，必然带 7° 偏差——  
正是"总是有偏差"的原因。

**另注**：工具栏旋转按钮是固定 ±5°（`PinBoardModule.tsx` → `rotateSelected(±5)`），与角度对齐无关。

**修复方向**：改为对**绝对角度**吸附（吸附后反推回增量），或直接以吸附后的绝对角度重算四角坐标。

---

## 2. 未解决的问题：切换素材板作品/画板时程序卡死

### 2.1 症状

- 切换素材板作品或画板时程序**完全卡死**；随后从右下角托盘点退出，**主窗口关闭但托盘不消失**，  
  约 15 秒后被强制终止（`forcing exit`）。
- 首次报告时间 2026-10-08 20:00 前后，当晚 21:46、22:02 各复现一次。

### 2.2 已锁定的事实

**(a) 触发条件**：创建素材板 WebGPU 渲染器——即切换素材板**作品或画板**。

- 未打开过素材板的会话：GPU 设备 = 0，**从不冻结**（09:53、09:57 两次会话，退出握手瞬时完成）。
- 打开过素材板的会话：**全部冻结**（10:00:59、10:23:01、10:28:11、10:37:28 四次）。

**(b) 触发模式（比预想更窄）**：四次冻结**无一例外**都以「切换到 artwork `74ae4bc3-ec51-48af-a791-86f1d07ecffa`」  
收尾（3 次落在 board 8、1 次 board 7）；其中 3 次在切作品**之前先切过一次画板**（另一作品 `77bd3ddb`）。

> 人工观察一致：**先切画板、再快速切作品**就会触发；只切作品、不切画板则不触发。  
> 机制上说得通——切画板会在**同一块 canvas 上多走一轮渲染器销毁+新建**，把 create/destroy 的节奏加快一倍。

> **⚠️ 本条已于 2026-10-09 11:15 复现后被推翻，勿再据此复现。** 11:15 会话收尾于
> `77bd3ddb`（board 4），且 10:00 与 11:15 两个会话**本会话内 `board select` 次数为 0** 仍冻结。
> 正确表述：触发条件是**渲染器 create/destroy churn 本身**（约 3 个切换周期、6 个设备即足够），
> 与是否切画板、切到哪个作品**无关**。完整证据见 [2.7](#27-2026-10-09-1115-复现全量日志分析详细记录)。

**(c) 冻结点**：渲染器创建后的「post-create 窗口」**末尾**。四次冻结的最后一条前端日志分别是  
`renderer created` / `post-create post-paint timer` / `pin-board committed` / `post-create post-paint timer`，  
即：微任务 → rAF → ResizeObserver 重绘 → post-paint 定时器**全部完成**之后，  
紧接着本该触发的 `load timer fired`（delay=0，同一会话里已成功触发过多次）**没有出现**。

**(d) 性质**：WebView2 **渲染进程挂起，而非崩溃**。

- `Crashpad` 目录中唯一的 `.dmp` 是 **2026-10-03** 的旧文件，当日无任何崩溃转储。
- 原生端全程健康：看门狗告警、托盘退出（`shutting down`）、超时计时（`forcing exit`）都正常执行  
  → **应用主线程活着**（它能处理托盘点击）。

**(e) 不是渐进劣化**：心跳 `gap` 稳定在 ~2000ms、`drift` 仅 ±10ms，事件循环延迟探针告警 **0** 次、  
长任务 **0** 个——直到冻结前一秒都完全健康。这是**突发性硬冻结**。

**(f) 阻塞不在我们的 JS 里**：四次冻结中，前端 span **全部闭合**，包括每一个 GPU 调用  
（`gpu.requestAdapter` / `gpu.requestDevice` / `gpu.configure` / `gpu.pipelines` /  
`draw.encoder` / `draw.getCurrentTexture` / `draw.submit`）。也就是说主线程**不是死在我们的 JS 里**，  
而是死在我们 JS 跑完之后、浏览器为新 canvas 做的**帧提交/GPU 收尾**工作中。

> **重要推论**：这意味着**任何 JS 层探针都抓不到那一次调用**——主线程已死，写不出日志。  
> 想进一步定位，只能换用「独立线程取证」或「隔离实验」两种手段（见 2.6）。

### 2.3 已排除的假设

| 假设                           | 排除依据                                                                                                                                                                                                       |
| ---------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **WebGPU 设备/上下文泄漏**（最初的首要嫌疑） | **判决性实验已否决**：在 `destroy()` 中补上 `context.unconfigure()` + `device.destroy()` 后，5 次销毁**全部** `deviceReleased=true`（0 次 false），**冻结照样发生**。详见 2.4                                                               |
| Rust 侧锁自死锁                   | 所有同步（非 async）命令都是轻量操作（`diagnostics_pong` / `log_frontend_diagnostics` / `open_log_directory` / `read_pin_board_clipboard_paths`），**不持仓库锁、无重 IO**；`cleanup::replay` 只接收 `&Path`，不重入 `with_ready_repository` |
| IPC 派发被阻塞                    | 冻结期间**日志写入本身是正常的**（看门狗、托盘都写成功）；前端心跳走 `log_frontend_diagnostics` 命令，日志能写却不出现心跳 ⟹ 前端根本没再发起 invoke ⟹ 是 JS 主线程停了                                                                                               |
| 备份线程停摆导致锁阻塞                  | 弱信号：备份间隔本身 22–37 秒浮动，窗口内未打点很可能只是"没轮到"，不足以作为证据                                                                                                                                                              |
| IPC 请求洪泛                     | 本次仅成功加载 **4 张**纹理、**3~4 个**在飞，峰值并发 **4**，积压很小                                                                                                                                                              |
| 主线程死循环                       | `evictTextures` 是**有界 `for` 循环**；`drainLoads` 的 `while` 条件单调收敛；延迟探针 0 告警、长任务 0 个                                                                                                                           |

### 2.4 关键日志证据

**判决性实验（设备释放）**——已生效但未消除冻结：

```
# 80  span#38  >> destroy.releaseDevice gpuSeq=1   → deviceReleased=true
#243  span#113 >> destroy.releaseDevice gpuSeq=2   → deviceReleased=true
#322  span#151 >> destroy.releaseDevice gpuSeq=3   → deviceReleased=true
#437  span#203 >> destroy.releaseDevice gpuSeq=4   → deviceReleased=true
#519  span#241 >> destroy.releaseDevice gpuSeq=5   → deviceReleased=true
```

**冻结窗口（10:37:28 会话，5 秒内触发，仅 6 个设备）**：

```
#591 renderer created: boardId=8
#592 post-create microtask      #593 post-create rAF
#594~#613 resize.refreshViewport（ResizeObserver，全部闭合，含 2 次完整 draw）
#614 post-create post-paint timer          ← 最后一条前端日志
[冻结：心跳停止、看门狗无 pong]
```

**冻结时在飞行的纹理读取**（`texture.read <<` 缺失 ⟹ IPC 响应从未投递回 JS）：

```
span#109 texture.load imageId=25  / span#110 texture.read imageId=25
span#195 texture.load imageId=203 / span#196 texture.read imageId=203
span#197 texture.load imageId=97  / span#198 texture.read imageId=97
```

### 2.5 复现方式

1. `npm run tauri dev`（调试构建，详细日志默认已开启）。
2. 打开素材板。
3. 在作品 `77bd3ddb` 里**切换一次画板**。
4. **立刻**切换到作品 `74ae4bc3`（约 5 秒内可触发）。

### 2.6 下一步建议（按推荐顺序）

1. **隔离严格模式**（最便宜、判定明确）：临时注释 `src/main.tsx` 的 `<React.StrictMode>` 重跑同一复现。  
   严格模式会让每次挂载**创建 2 个渲染器**（日志中 `canvasReconfigureCount` 呈 `1→2` 交替即为证据）。  
   冻结消失 ⟹ 双挂载 churn 是机制；仍冻结 ⟹ 与严格模式无关，需继续向下。
2. **结构性修复**（很可能才是真正的修复）：让**切换画板时复用同一个渲染器/设备/canvas**（只换图片集），  
   不再销毁重建。这直接消除触发条件，也是代码本就该走的方向。
3. **独立线程取证**（仅在仍需要定位浏览器内部调用时）：加一个 Web Worker 看门狗（独立线程探测主线程  
   存活，并把"最后一步"写入 IndexedDB，可扛过冻结）；同时补上 `window.onerror` /  
   `unhandledrejection` 捕获（当前完全不可见）。

### 2.7 2026-10-09 11:15 复现：全量日志分析（详细记录）

> 本节存在的目的：**下一次接手不必再从头调查一遍**。下面给出复现条件、全部原始证据、
> 已被推翻的判断、以及可直接复用的分析脚本。凡本节已给出的结论，直接引用，不要重查。

**复现条件**：调试构建（`npm run tauri dev`，`diagnostics=on, log_level=debug`）下打开素材板，
在 `77bd3ddb` 与 `74ae4bc3` 之间快速来回切换作品（约 6 秒内 3 次），即冻结。本次**未切画板**。

#### (1) 全量会话对照：触发条件与冻结 100% 相关

对 `Lilith Artworks.log` 全部 22 次启动逐会话统计（脚本见本节末尾）：

| 会话启动时刻 | GPU 设备创建 | 本会话 `board select` | 结果 |
| ------------ | ------------ | --------------------- | ---- |
| 10:00:59     | 8            | 0                     | 冻结 |
| 10:23:01     | 10           | 2                     | 冻结 |
| 10:28:11     | 24           | 4                     | 冻结 |
| 10:37:28     | 6            | 2                     | 冻结 |
| **11:15:05** | **6**        | **0**                 | **冻结** |
| 其余 17 个会话 | 0          | —                     | 未冻结 |

- **创建过渲染器的会话 5/5 全部冻结；从未创建过的会话 0/17 冻结。** 这是本日志中最干净的分离。
- 设备创建数最低到 **6**（≈3 个切换周期）仍足以触发，说明门槛很低、极易复现。
- 9:53 / 9:57 两次会话 GPU 设备为 0 且退出握手瞬时完成，可作为"未打开素材板"的对照。

#### (2) 两处推翻：收尾作品与画板切换都不是必要条件

| 会话 | 本会话 `board select` | 最后一次 `renderer created` 的作品 |
| ---- | --------------------- | --------------------------------- |
| 10:00:59 | 0 | `74ae4bc3` (board 8) |
| 10:23:01 | 2（board 7） | `74ae4bc3` (board 7) |
| 10:28:11 | 4（board 5、6） | `74ae4bc3` (board 8) |
| 10:37:28 | 2（board 5） | `74ae4bc3` (board 8) |
| **11:15:05** | **0** | **`77bd3ddb` (board 4)** |

- 旧结论"无一例外以 `74ae4bc3` 收尾"仅在前四次成立，是**人工复现序列的巧合**；
- 旧结论"必须先切画板"被 10:00 与 11:15（`board select` 均为 0）直接否证；
- 结论：**只有"渲染器快速 create/destroy"是必要条件**，作品与画板都只是承载 churn 的载体。

#### (3) 11:15 会话完整事件序列（按 JS 侧序号 `#N`）

```text
#1    previous session last activity: 10:37:32 | board select artworkId=77bd3ddb boardId=5   ← 面包屑
#4-6  artwork switch 77bd3ddb (boards=3)
#15   gpu.configure canvasReconfigureCount=1      #19  gpu device created seq=1
#84   renderer destroyed gpuSeq=1
#87   gpu.configure canvasReconfigureCount=2      #91  gpu device created seq=2
#154  renderer created 77bd3ddb boardId=4
#155  post-create microtask   #156 post-create rAF   #177 post-create post-paint timer
#287  renderer destroyed gpuSeq=2 (inFlightTextureLoads=2)
#288  renderer disposing boardId=4
#289-291 artwork switch 74ae4bc3 (boards=3)       #294 renderer disposing boardId=8
#301  gpu.configure canvasReconfigureCount=1      #305 gpu device created seq=3
#370  renderer destroyed gpuSeq=3
#373  gpu.configure canvasReconfigureCount=2      #377 gpu device created seq=4
#440  renderer created 74ae4bc3 boardId=8
#441  post-create microtask   #442 rAF   #463 post-create post-paint timer
#569  destroy.releaseDevice gpuSeq=4   #572 renderer destroyed gpuSeq=4 (inFlight=2)
#573  renderer disposing boardId=8
#574-577 artwork switch 77bd3ddb                 #579 renderer disposing boardId=4
#586  gpu.configure canvasReconfigureCount=1      #590 gpu device created seq=5
#652  destroy.releaseDevice gpuSeq=5   #655 renderer destroyed gpuSeq=5
#658  gpu.configure canvasReconfigureCount=2      #662 gpu device created seq=6
#716  renderer.construct boardId=4 gpuSeq=6
#725  renderer created 77bd3ddb boardId=4
#726  post-create microtask   #727 post-create rAF
#728-747 resize.refreshViewport（ResizeObserver，含 2 次完整 draw）   ← 最后一条前端日志
[冻结]  该渲染器的 post-create post-paint timer 从未出现
```

原生端时间线（与前端对齐）：

```text
11:15:05  starting: diagnostics=on, log_level=debug
11:15:05  repository validated: path=F:\LilithData\Artworks, elapsed_ms=6
11:15:17  WARN repository read took 3056 ms
11:15:18  WARN repository read took 4115 ms
11:15:21  WARN webview unresponsive: no pong for 8000 ms (last pong at 8003 ms, ping #8)
11:15:24  Lilith Artworks shutting down            ← 用户从托盘退出
11:15:39  WARN webview shutdown confirmation timed out; forcing exit
```

**要点**：

- 冻结点是**渲染器 post-create 窗口末尾**。本次最后一条前端日志是 ResizeObserver 的
  `resize.refreshViewport` 闭合（`#747`），而 `#725` 渲染器的 `post-create post-paint timer`
  **没有出现**——即 rAF 与 ResizeObserver（渲染管线内派发）都跑完了，之后的任务循环没有再派发。
  这与 2.2(c) 记录的"最后一条是 post-paint timer"是同一窗口的**第五种收尾形态**，不矛盾。
- **未闭合 span 只有纹理读取**：`span#247/248 texture.load/read imageId=382`、
  `span#257/258 texture.load/read imageId=376`（JS 序号 `#541/542`、`#562/563`）。
  所有 GPU span（`draw.encoder` / `draw.getCurrentTexture` / `draw.submit`）**全部闭合**。
  再次印证 2.2(f)：主线程不是死在我们的 JS 里。
- 看门狗 `last pong at 8003 ms` ⟹ 最后一次 pong 约在 `11:15:13`，与前端最后日志 `11:15:14` 吻合。

#### (4) 新信号：`repository read took` 慢读告警（已评估，不作根因方向）

阈值 `READ_HOLD_WARN_MS = 1500`（`app/settings.rs`）。**只在冻结会话出现**，但**并非必要条件**：

| 会话 | 慢读告警次数 | 时刻（ms） |
| ---- | ------------ | ---------- |
| 10:00:59 | 0 | — |
| 10:23:01 | 4 | 1524 / 1706 / 3812 / 5012 |
| 10:28:11 | 9 | 1501 / 3250 / 4493 / 1619 / 3257 / 4477 / 1579 / 3254 / 3160 |
| 10:37:28 | 0 | — |
| 11:15:05 | 2 | 3056 / 4115 |

**判断为伴随现象，不是根因**，三条依据：

1. **非必要条件**：10:00、10:37 同样冻结却零告警；
2. **不阻塞前端主线程**：`read_pin_board_texture` 走 `spawn_blocking` + 异步 `invoke`，
   前端 `await` 不占主线程；该告警衡量的是 `with_repository_read` 内操作的**执行**耗时
   （不是租约等待——等待会另打 `repository read lease waited`，本次没有）；
3. **与纹理活动量正相关**：10:28 会话有 91 次 `texture.load` / 94 次 `texture.read` 且 9 次慢读，
   11:15 只有 17 / 20 次、2 次慢读。更像 DDS 缓存生成与磁盘 IO 被 churn 拖慢的**结果**。

> 已为此增加归因能力：`with_repository_read_labeled` 让慢读告警带上具体命令
> （如 `[read_pin_board_texture board=4 image=382 dim=256]`），下次复现即可确认是否就是纹理读取。

#### (5) 下一次复现的分析流程（直接照做）

```bash
# 1. 确认模式生效与冻结标记
LOG="$LOCALAPPDATA/com.lilith.artworks/logs/Lilith Artworks.log"
grep -nE "starting:|diagnostics=|log_level=" "$LOG" | tail
grep -nE "webview unresponsive|forcing exit|shutting down" "$LOG"

# 2. 新证据：取证 Worker 的 IndexedDB 回读（唯一能给出"停在哪一步"的行）
grep -n "previous freeze (worker forensics)" "$LOG"
grep -n "worker watchdog" "$LOG"

# 3. 队列探针缺哪一条（区分整个任务循环停摆 vs 仅定时器队列停摆）
grep -nE "post-create (microtask|rAF|post-paint timer|queue probe)" "$LOG" | tail -20

# 4. 慢读归因（现在带命令标签）
grep -n "repository read took" "$LOG"

# 5. GPU 侧新证据
grep -nE "gpu device lost|gpu uncaptured error|releaseDevice|deviceReleased" "$LOG"

# 6. 前端异常（此前完全不可见）
grep -nE "window error|unhandled rejection|resource load error" "$LOG"
```

**分析脚本（按 JS 序号还原真实执行序，避免行序陷阱——见 4.2）**：

```python
import io, re
p = r"C:\Users\ACSProgram\AppData\Local\com.lilith.artworks\logs\Lilith Artworks.log"
lines = io.open(p, encoding="utf-8", errors="replace").read().splitlines()
start = max(i for i, l in enumerate(lines) if "starting:" in l)   # 取最后一次启动
rows = []
for l in lines[start:]:
    if "[webview]" not in l:
        continue
    m = re.search(r"\] \[webview\] #(\d+) (.*)$", l)
    if m:
        rows.append((int(m.group(1)), m.group(2)))
rows.sort(key=lambda r: r[0])          # ← 按 JS 序号，不是行序
op = {}
for seq, msg in rows:
    m = re.match(r"span#(\d+) (>>|<<|!!) (.*)", msg)
    if not m:
        continue
    if m.group(2) == ">>":
        op.setdefault(m.group(1), msg)
    else:
        op.pop(m.group(1), None)
print("真正未闭合的 span:", list(op.values()) or "(无)")
for seq, msg in rows[-30:]:
    print(f"#{seq:5d} {msg}")
```

**逐会话 churn 统计脚本**（用于确认"有 churn 必冻结"）：

```python
import io, re
p = r"C:\Users\ACSProgram\AppData\Local\com.lilith.artworks\logs\Lilith Artworks.log"
lines = io.open(p, encoding="utf-8", errors="replace").read().splitlines()
starts = [(i, re.search(r"\[(\d{2}:\d{2}:\d{2})\]", l).group(1))
          for i, l in enumerate(lines) if "starting:" in l] + [(len(lines), "END")]
for k in range(len(starts) - 1):
    s, t = starts[k]; e = starts[k + 1][0]; seg = lines[s:e]
    dev = sum(1 for l in seg if "gpu device created" in l)
    frz = "FREEZE" if any("unresponsive" in l for l in seg) else ""
    reads = sum(1 for l in seg if "repository read took" in l)
    print(f"{t}  devices={dev:2d} repo_read_warn={reads} {frz}")
```

**观察清单（下次复现后按此顺序读）**：

1. `previous freeze (worker forensics)` —— 冻结时刻、最后操作、是否恢复。**这是本次新增的关键行**；
2. `post-create queue probe` 缺哪一条 —— `message-channel` 缺 = 整个任务循环停摆；
   只有 `timer` 缺 = 定时器队列停摆；`rAF` 缺 = 渲染管线停摆；
3. `window error` / `unhandled rejection` —— 此前完全不可见，若出现即为直接线索；
4. `gpu device lost` / `gpu uncaptured error` —— 若出现，指向驱动/设备层而非我们的 JS；
5. `repository read took ... [read_pin_board_texture ...]` —— 确认慢读是否就是纹理读取。

若以上全部为空且仍冻结，则说明阻塞确实落在浏览器内部（Chromium 帧提交/GPU 收尾），
此时按 2.6 的"结构性修复"（切换画板/作品复用同一渲染器与设备）直接消除触发条件。

### 2.8 修复方案（怎么修）

> 结论先行：**不需要先查清浏览器内部原因**。已确认的必要条件是「渲染器 create/destroy churn」，
> 把它消除即可。修复方向是**换画板/换作品时复用同一个渲染器与 GPU 设备**，而不是销毁重建。

#### (1) 现在为什么会重建（两处，都要改）

1. `PinBoardModule.tsx` 用 `{view ? <GpuCanvas …/> : 占位}` 条件渲染（`:1384`）。切画板时
   `select()` 先 `setView(null)`（`:760`）再 `setView(loaded)`（`:765`）；切换作品时同样先
   `setView(null)`（`:701`）再 `setView(loaded)`（`:724`）。`view` 从 `null` 变成值，等于
   **把 `GpuCanvas` 卸载再挂载**。
2. `GpuCanvas` 的创建 effect 依赖 `[view, artworkId, …]`（`:248`）：`view` 一变就
   `cleanup（renderer.destroy()）` + `PinBoardRenderer.create()`，**每次都新建 WebGPU 设备并重新
   `configure` 同一块 canvas**。

> 开发构建下 StrictMode 会让每次挂载再翻一倍，所以日志里每次切换出现 **2 个**
> `gpu device created`（序号 1/2、3/4…，`canvasReconfigureCount` 恒为 1→2）。
> 这只是放大器，不是修复点——按下面改完后即使重挂载也不会再建设备。

#### (2) 怎么改

**步骤 1：把「GPU 初始化」与「装载画板」拆开**

- `PinBoardRenderer.create(canvas, selection, marquee, callbacks, budgets)` —— 只保留**画布生命周期**
  内一次性的事：`requestAdapter` / `requestDevice` / `context.configure` / pipelines / 采样器 /
  `ResizeObserver` / 主题 `MutationObserver` / 各类事件监听。去掉 `view`、`initialSession`、
  `initiallyActive`、`arrangementGapCssPixels`、`autosaveEnabled` 这几个参数。
- 新增 `renderer.loadBoard(view, session)` —— 承接构造函数里所有**随画板变化**的状态：
  `boardId`、`images` / `imageById`、`revision`、`viewport`、`locked`、`selectedIds`、
  `pendingViewportFit`、`savedStateKey`，随后 `resize()` → `refreshViewport(0)` → `emitState()`
  （即原构造末尾那几步）。
- 新增 `renderer.updateSettings({ arrangementGapCssPixels, autosaveEnabled, cacheBudgets })` ——
  这三个当前**只能靠重建生效**的设置改为可变字段。顺带修掉「开着素材板改设置会重建渲染器」。
- `renderer.destroy()` 只保留给**真正的卸载**：模块关闭、切换仓库、应用退出。

**步骤 2：`loadBoard` 内部先结算旧画板**

- 顺序：`finishActiveInteraction()` → `finalize()`（保存 + `finalize_pin_board`，语义与今天
  `destroy(true)` 完全一致）→ 释放旧画板的纹理与 bindGroup → 重置步骤 1 列出的状态 → 装载新画板。
- **必须幂等**：若传入的 `view.boardId` 与当前 `boardId` 相同则直接返回，避免重复结算。

**步骤 3：`GpuCanvas` 保持挂载**

- 不再用 `view` 做条件渲染；另设一个 `loading` 状态显示占位/加载中，`view` 为 `null` 时也不再卸载
  画布。
- 创建 effect 的依赖收缩到只与画布生命周期有关；`view` 变化改由另一个 effect 调 `loadBoard`。
- `select()`（`:751-756`）与切换作品处（`:695-698`）**不再调用 `renderer.destroy()`**，
  改为 `await renderer.loadBoard(...)`。

#### (3) 改完的收益

- 一次切换**不再产生任何** `gpu device created` / `renderer destroyed`；一次会话里
  `canvasReconfigureCount` 只应出现 1。
- 减轻 StrictMode 的影响，但**不是自动获得**：创建 effect 的 cleanup 目前仍调用
  `destroy()`（`PinBoardModule.tsx:341`），StrictMode 下 create → cleanup(destroy) → 再
  create 依然会创建 2 个设备。要兑现"一次会话只出现 1 个设备"的判据，创建 effect 本身也必须
  幂等化，见 (7) 第 1 条。
- 顺带修掉「改设置重建渲染器」这个同类浪费。

#### (4) 必须保持不变的语义（回归重点）

- **切画板前先结算**：保存 + `finalize_pin_board`；失败则中止切换并提示（`select()` 现有行为）。
- 切作品 / 切仓库 / 退出的结算链（`lifecycle.ts` 的 `preparePinBoardRuntimeChange`）。
- 自动保存开关、`pagehide` / `visibilitychange` 保存。
- 进程内视图会话（`session.ts`）的保存与恢复，以及 `pendingViewportFit`（首次打开按最小包围框适配）。
- revision 冲突检测、选中态与锁定态。

#### (5) 验证

- **复现路径**：打开素材板 → 在 `77bd3ddb` 与 `74ae4bc3` 之间快速来回切作品（约 6 秒 3 次），
  重复 10 次以上。
- **补充场景（评审补充）**：切换时正处于拖拽/旋转交互中（`finishActiveInteraction` 路径）；
  自动保存触发瞬间切走；开发构建（StrictMode 双挂载）下确认挂载稳定后设备计数仍为 1；
  人为触发 `device.lost` 后确认恢复链生效（见 (7) 第 3 条）。
- **通过判据**：日志里不再出现成对的 `gpu device created` / `renderer destroyed`，且不再出现
  `webview unresponsive` 与 `forcing exit`。
- **功能回归**：切换后保存与结算正常（`pin-board save done` / `finalize done`）、纹理正常加载、
  视图与会话恢复正确、自动保存仍生效。
- **自动化**：`npm run test:pin-board`（含 `renderer.test.ts`、`PinBoardModule.test.ts`）、
  `cargo test pin_board --lib`。

#### (6) 若修复后仍能复现

说明 churn 只是加速器、机制更宽。此时再上 2.7 节第 1 档的手段：对挂起渲染进程抓 minidump，
直接拿主线程调用栈。

#### (7) 实现风险清单（2026-10-09 评审补充）

> 本节是对上述修复方案的评审结论：**方向合适，照做即可**，但以下 6 点是按现有代码
> （`renderer.ts` 的 `create` / `destroy` 与 `PinBoardModule.tsx` 的两个 effect）逐条核对后
> 补充的实现约束。其中第 1、3、4 条直接决定 (5) 的验收判据能否达成。

| # | 风险 | 现状依据 | 处理要求 |
| - | ---- | -------- | -------- |
| 1 | StrictMode 仍会制造 churn | `GpuCanvas` 创建 effect 的 cleanup 调用 `destroy()`（`PinBoardModule.tsx:341`） | 创建 effect 幂等化：用 ref 持有渲染器实例，cleanup 只 detach/标记；真正的 `destroy()` 只跟随模块卸载 |
| 2 | 在飞纹理读取与加载队列污染新画板 | `destroy()` 靠 `gpu.released` 作废在飞读取，并随设备销毁清空纹理缓存与 `drainLoads` 队列 | `loadBoard` 引入 boardGeneration 并在每次装载时递增，在飞结果按代号拒绝；清空并重排加载候选；并决定纹理缓存"精确释放"还是"保留复用"——保留则切回更快但预算管理更复杂，全释放则把 `textureReservedBytes` 归零逻辑搬入 `loadBoard` |
| 3 | 设备丢失无恢复路径 | `device.lost` 目前只报错（`renderer.ts:758-760`）；现状每次切换重建设备可自愈 | 复用后一个设备要活整个会话，必须补"丢失 → 重新 `requestAdapter`/`requestDevice`/`configure` + 重载纹理"的恢复链，并与 `create()` 共用同一套初始化代码，避免两条创建路径分叉 |
| 4 | finalize 的 boardId 时序 | `destroy(finalize=true)` 在旧画板状态下保存，语义依赖销毁即切换 | `loadBoard` 内必须**先用旧 boardId 完成 finalize（保存 + `finalize_pin_board`）并发出 `onSessionChange`，再改 `this.boardId`**；建议用断言锁住（finalize 完成前 `this.boardId` 不可变）。顺序颠倒会把新画板的视口/选中态写进旧画板的会话 |
| 5 | 失败路径的 UI 回滚 | 现在 `select()` 中 finalize 失败即 return，渲染器与 UI 都停在旧画板，状态一致 | `loadBoard` 成功后才更新 UI 状态（`setSelected` / `setView`），或失败时显式回滚，避免出现"UI 显示新画板、渲染器持有旧画板"的分裂 |
| 6 | 空 view 状态 | 构造函数强依赖 `view`（`renderer.ts:700`），`GpuCanvas` 常驻后 `view === null` 期间无画板可装 | 两个选项：首次 view 到达时才创建渲染器、之后不再卸载（改动更小，但创建 effect 依赖必须收缩到画布生命周期项）；或让渲染器支持空画板（images 为空、不 emit 保存） |

---

## 3. 本轮新建的诊断/日志体系（已具备的能力）

已提交部分见 `b4e6bd9`（"Add a runtime-switchable diagnostics layer"），未提交的增强见第 6 节。

| 能力                | 位置                                          | 用途                                                                                                             |
| ----------------- | ------------------------------------------- | -------------------------------------------------------------------------------------------------------------- |
| 日志等级策略            | `src-tauri/src/app/diagnostics.rs`、`lib.rs` | 构建时不调 `.level()`，改由 `log::set_max_level` **唯一**控制；debug 构建默认 `debug`、release 默认 `info`；`LILITH_LOG_LEVEL` 仍可覆盖 |
| 进程级「详细日志」开关       | 设置页「调试」栏目                                   | 运行时即时切换、无需重启、**不写入设置文件**（重启回到默认）                                                                               |
| 前端→日志桥接           | `log_frontend_diagnostics` 命令               | WebView 侧没有写日志文件的路径，前端事件经此转发；`info` 仅诊断模式写入，`warn`/`error` 始终写入                                                |
| 主线程心跳             | `src/shared/diagnostics.ts`                 | 2 秒一次，含 `gap`/`drift` 与「最近操作」，冻结前最后一条即冻结时刻                                                                     |
| 事件循环延迟探针          | 同上                                          | 500ms 间隔，捕捉心跳粒度以下的阻塞                                                                                           |
| 长任务观察器            | 同上                                          | >200ms 记录，用于判断是否渐进恶化                                                                                           |
| **进入/退出成对 span**  | 同上                                          | `span#N >> 名称` / `span#N << 名称`；**冻结后只有 `>>` 没有 `<<` 的那条即阻塞点**                                                 |
| 原生端 WebView 看门狗   | `diagnostics.rs`                            | 独立线程 ping/pong，冻结进行中即可写 `webview unresponsive`，且能扛过 15 秒强退                                                     |
| localStorage 面包屑  | `shared/diagnostics.ts`                     | 唯一**不经过 IPC** 的取证通道；下次启动读回并写入日志（`previous session last activity:`）                                             |
| GPU 设备序号/存活数/重配计数 | `renderer.ts`                               | `gpu device created seq=`、`canvasReconfigureCount=`、`deviceReleased=`                                          |
| 渲染器生命周期与纹理链路      | `renderer.ts`、`PinBoardModule.tsx`          | 创建/销毁、`texture.load`/`read`/`upload`（无条件记录）、drain 批次、post-create 三级台阶                                          |
| 退出握手与仓库锁探针        | `App.tsx`、`src-tauri/src/app/settings.rs`   | 握手各阶段；锁**等待**与**持有**时长告警（平时零噪声）                                                                                |
| **取证 Worker + IndexedDB** | `src/shared/diagnosticsWorker.ts`            | 独立线程在主线程冻结期间继续计时，把最后心跳/最后操作/静默时长写入 IndexedDB，下次启动回读成 `previous freeze (worker forensics)`；**唯一能给出「停在哪一步、停了多久、是否恢复」的通道** |
| 全局错误捕获              | `src/shared/diagnostics.ts`                  | 接管 `error` / `unhandledrejection`（此前完全不可见），脚本异常与资源加载失败分开记录                                                     |
| GPU 未捕获错误             | `src/modules/pin-board/renderer.ts`          | `device.onuncapturederror` 捕获错误作用域之外被静默吞掉的 GPU 错误；`device.lost` 补设备序号                                              |
| 队列探针                 | `src/modules/pin-board/PinBoardModule.tsx`   | 三级台阶之后追加 MessageChannel / timer / rAF 三探针，区分「整个任务循环停摆」与「仅定时器队列停摆」                                          |
| 慢读归因                 | `src-tauri/src/app/settings.rs`、`pin_board/mod.rs` | `with_repository_read_labeled`：慢读告警带上具体命令，使 `repository read took` 可归因                                     |

---

## 4. 踩过的坑（方法论，务必先读）

### 4.1 日志等级：加了 `log::debug!` 却没有任何输出

`tauri-plugin-log` 用 fern 的 `Dispatch::level` 做**静态**过滤，**构建期固定**；原实现在  
`log_level_from_env()` 里只读环境变量、未设置时一律 `Info`，**完全忽略构建配置**。  
于是 `npm run tauri dev` 下新增的 `log::debug!` 被静默丢弃——不是代码没跑到，是级别被过滤了。

**已修**：构建时不再调 `.level()`，改由 `log::set_max_level` 唯一控制；debug 构建默认 `debug`。  
新增诊断点请优先使用 **Info 及以上**等级，或确认诊断模式已开启。

### 4.2 **日志行序 ≠ JS 执行序**（本轮最容易踩的坑）

前端每条日志都经 `log_frontend_diagnostics` 转发，**原生端多线程写入会乱序落盘**。  
实例：`span#111` 的 `>>` 是 JS 序号 `#237`、`<<` 是 `#238`，但文件里 `#238` 却排在 `#237` **前面**。

**后果**：若按文件行序解析「进入/退出」，会把**已经闭合的 span 误判为"未闭合"**，  
一度得出"有 5 个 span 未闭合"的错误结论。

**正确做法**：一律按 **JS 侧序号 `#N`** 排序后再分析；判断 span 是否未闭合时**与行序无关**，  
只看该 span id 是否**同时**存在 `>>` 与 `<<`/`!!`。

可直接复用的分析脚本（Python）：

```python
import io, re
lines = io.open("Lilith Artworks.log", encoding="utf-8", errors="replace").read().splitlines()
start = next(i for i, l in enumerate(lines) if "starting:" in l and "diagnostics=" in l)  # 取最后一次启动
rows = []
for l in lines[start:]:
    if "[webview]" not in l:
        continue
    m = re.search(r"\] \[webview\] #(\d+) (.*)$", l)
    if m:
        rows.append((int(m.group(1)), m.group(2)))
rows.sort(key=lambda r: r[0])          # ← 按 JS 序号还原真实执行顺序
op = {}
for seq, msg in rows:
    m = re.match(r"span#(\d+) (>>|<<|!!) (.*)", msg)
    if not m:
        continue
    if m.group(2) == ">>":
        op.setdefault(m.group(1), msg)
    else:
        op.pop(m.group(1), None)
print("真正未闭合的 span:", list(op.values()) or "(无)")
for seq, msg in rows[-30:]:
    print(f"#{seq:5d} {msg}")
```

### 4.3 埋点漏传参数导致整条路径失明

`loadTexture` 的调用点漏传了 `detail` 参数，采样分支恒为假，导致**整条纹理路径**  
（IPC 读取 + GPU 上传）**一条日志都没有**。一度据此得出"零未闭合 span ⟹ 主线程没卡在埋点里"的错误结论。

**已修**：`texture.load` / `texture.read` / `texture.upload` 改为**无条件记录**。  
新增埋点时，务必确认采样开关真的传下去了。

### 4.4 面包屑需要"上一会话"才有数据

`previous session last activity:` 读的是**上一次会话**写入 localStorage 的值。  
首次引入该功能的会话启动时必然读不到——不是坏了。

### 4.5 判决性实验的价值

"WebGPU 设备泄漏"是最像元凶的假设（源码注释自己写明"设备只随进程退出回收"），  
但只有**真的做了实验**（补 `device.destroy()` 后复现）才能把它划掉。  
不要凭"很像"就下结论；**能一次否证的实验优先做**。

---

## 5. 日志位置与常用检索

```
%LOCALAPPDATA%\com.lilith.artworks\logs\Lilith Artworks.log
```

单文件 4 MiB 轮转、保留 5 份。启动行会打印 `diagnostics=on/off, log_level=debug`，  
可先确认模式是否真的生效。

```bash
LOG="$LOCALAPPDATA/com.lilith.artworks/logs/Lilith Artworks.log"
grep -nE "starting:|diagnostics=|log_level=" "$LOG" | tail
grep -nE "webview unresponsive|forcing exit|shutting down|confirm_app_shutdown" "$LOG"
grep -nE "releaseDevice|deviceReleased|gpu device created|canvasReconfigureCount" "$LOG"
grep -nE "artwork switch|board select|renderer created|renderer destroyed" "$LOG"
grep -nE "repository (read|mutation)" "$LOG"
```

---

## 6. 当前工作区状态

- **HEAD = `b4e6bd9`**（已提交"可运行时切换的诊断层"）。按仓库约定**未执行 `git push`**。
- 以下为**未提交**改动（属于"日志/诊断能力增强"，验证通过后可并入上一提交）：

| 文件 | 内容 |
| --- | --- |
| `src-tauri/src/app/diagnostics.rs` | 看门狗 ping/pong、等级策略、`log_frontend_diagnostics` |
| `src-tauri/src/app/settings.rs` | 仓库锁**等待/持有**时长探针；`with_repository_read_labeled`（慢读告警带命令标签） |
| `src-tauri/src/lib.rs` | 等级策略、退出握手埋点、命令注册 |
| `src-tauri/src/pin_board/mod.rs` | 五个读命令改用 `with_repository_read_labeled` |
| `src/shared/diagnostics.ts` | span（同步/异步）、慢步骤、长任务、心跳、事件循环探针、面包屑；**取证 Worker 接入、全局错误捕获** |
| **`src/shared/diagnosticsWorker.ts`**（新增） | 独立线程取证 Worker：静默检测 + IndexedDB 落盘 + 历史冻结回读 |
| `src/app/App.tsx` | 退出握手各阶段埋点、设置页新增「调试」栏目 |
| `src/modules/pin-board/PinBoardModule.tsx` | 创建/切换/画板选择埋点、post-create 三级台阶、**队列探针** |
| `src/modules/pin-board/renderer.ts` | GPU 埋点、纹理链路无条件记录、**`device.destroy()` 判决性实验**、**`onuncapturederror`** |
| `CHANGELOG.md`、`docs/architecture/overview.md`、`docs/modules/pin-board.md`、`docs/planning/current-handoff.md` | 文档同步 |

**验证状态**：`npx tsc --noEmit`、`npm test`（141 通过）、`cargo check --lib`、
`cargo fmt --check`、`git diff --check` 全部通过（2026-10-09 11:25 复跑确认）。

**注意**：`renderer.ts` 中的 `device.destroy()` + `context.unconfigure()` 是**判决性实验的产物**。
它**没有**消除冻结（但确实是正确的资源卫生）。若后续实验需要"回到实验前状态"，去掉
`destroy.releaseDevice` 那一段即可。
