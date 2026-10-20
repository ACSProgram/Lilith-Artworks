# 缩略图格式扩展计划（2026-10-20）

状态：**已实施并归档**（2026-10-20，第一批）。2026-10-20 构思完成；同日经维护者确认缩小
范围，只落第一批（`.sai2`、`.procreate`）与无魔数的 `.tga`，其余格式（kra/krz、ora、cbz、
ooxml、odf、blend、svg、exr/hdr/qoi/ff/pnm）**暂不做**，`supports_preview` 不登记它们。
实施范围与验证见 §9；有效契约并入 `docs/modules/history-and-backup.md`，行为变化见
`CHANGELOG.md` 的 `## Unreleased`；剩余的 GUI 手工验收只在 `../todo.md` 追踪。本文件自此
只作为计划、格式依据与落实口径的历史快照。

文档性质：独立计划，不写入 `todo.md`；已按 AGENTS 的归档规则归入本目录。
本计划不引入 Windows Shell 缩略图接口（已决定先不考虑）。

## 1. 一句话结论

在既有 preview 管线上新增 11 项格式（条目见 §2.1）：位图 6 种（纯 feature flag）、ZIP 家族 6 类
（kra/krz、ora、procreate、ooxml、odf、cbz，共用一个 zip extractor）、`.sai2`、
`.blend`、`.svg`。所有格式的内嵌预览结构均有公开依据，其中 `.blend` 已用本机双样本
实证、`.sai2` 已从 MIT 开源的 libsai 实现提取到完整算法——**不需要额外样本即可开工**，
仅端到端实测环节建议补真实样本（见 §8）。

## 2. 范围

### 2.1 纳入

| # | 格式 / 扩展名 | 预览来源 | 格式依据 | 实现量 |
| --- | --- | --- | --- | --- |
| 1 | tga、pnm（.pnm/.ppm/.pgm/.pbm）、hdr、qoi、farbfeld（.ff） | 文件即图 | image crate feature flag（纯 Rust，已有依赖） | 极小 |
| 2 | exr | 文件即图 | image crate "exr" feature（exr crate，纯 Rust） | 小 |
| 3 | .kra / .krz | ZIP 内 `preview.png`（krz 无 mergedimage.png） | Krita 官方文档 | 小（zip 家族） |
| 4 | .ora | ZIP 内 `Thumbnails/thumbnail.png` | OpenRaster 开放规范 | 小（zip 家族） |
| 5 | .procreate | ZIP 内 `QuickLook/Thumbnail.png` | 社区逆向（silica-viewer / silicate / mica 一致） | 小（zip 家族） |
| 6 | .docx / .xlsx / .pptx | ZIP 内 `docProps/thumbnail.*`（Office 保存时勾选才有） | ECMA-376 / OOXML | 小（zip 家族） |
| 7 | .odt / .ods / .odp | ZIP 内 `Thumbnails/thumbnail.png` | ODF 规范 | 小（zip 家族） |
| 8 | .cbz | ZIP 内第一张图片 | 事实标准 | 小（zip 家族） |
| 9 | .sai2 | `thum` chunk（JSSF→JPEG）或 `intg` chunk（dpcm→BGRA） | Photopea 非官方规范 + libsai（MIT）实现 | 中大 |
| 10 | .blend | TEST block = 宽 + 高 + 原始 RGBA | 无官方规范；本机双样本实证（§5） | 中 |
| 11 | .svg | 渲染矢量图 | W3C 标准 + resvg | 中（新依赖 resvg） |

### 2.2 不纳入（及原因）

| 格式 | 原因 |
| --- | --- |
| .pdf | 可靠渲染需 pdfium-render（捆绑原生二进制，安装包显著变大） |
| mp4/mov/mkv 等视频 | 抽帧需 ffmpeg 系，依赖重 |
| 音频封面（mp3/flac） | 价值待定 |
| avif | image crate 的 avif feature 仅编码；解码需 avif-native（dav1d，C 构建） |
| heic/heif、相机 RAW | C 依赖或价值窄 |
| .dds | 素材板已有 image_dds 链路；历史缩略图是否需求待定 |
| .sai（v1） | 加密容器，无 Rust 方案，工作量大 |
| .xcf/.pdn/.afphoto/.alp/.mdp 等 | 无内嵌预览或专有格式 |
| .ico | 顺手可加（image "ico" feature），未列入范围 |

## 3. 接入设计（不动主链）

### 3.1 分发

`preview/mod.rs::extract()` 现有 16 字节头探测增加分支：

- `SAI-CANVAS-TYPE0`（16 字节魔法，正好放得进现有探测）→ `sai2::extract`
- `PK\x03\x04` → `zip::extract(path, ...)`（按扩展名分发布局；zip 家族是唯一需要扩展名的地方，`path` 已在手上，签名不变）
- `BLENDER` → `blend::extract`（内部分辨老/新两种文件头）
- 其余不变（clip / psd / bitmap）

`format::container` 不动：它与 chunk writer 共享，thumbnail-only 的容器知识不进
chunking 路径（PSD 的 detect-only 先例说明真到需要 chunk 对齐时再提升）。

### 3.2 输出与预算（全部复用）

512px 最长边、无损 WebP、≤2MB；64MP / 32768px 解码预算；取消令牌；「失败返回
None、不阻塞提交/恢复」的 degrade 契约不变。新增 extractor 一律走 `decode_bytes` /
`checked_pixels` / `decode_limits`。

### 3.3 白名单

`supports_preview()` 追加：sai2、kra、krz、ora、procreate、cbz、svg、blend、docx、
xlsx、pptx、odt、ods、odp、tga、pnm、ppm、pgm、pbm、hdr、qoi、ff、exr。前端占位与
刷新入口自动跟随。

**「支持 ≠ 一定有」**：docx 等没有内嵌 thumbnail 时仍显示占位——此行为差异写进
`docs/modules/history-and-backup.md`。

## 4. 各格式实现要点

### 4.1 位图

`Cargo.toml`：image features += pnm、hdr、qoi、ff、exr（tga 已开）。
`bitmap.rs` allowlist += tga、pnm、hdr、qoi、farbfeld、exr。tga 无可靠 magic，靠
扩展名白名单兜底；其余四种有 magic 可依。

### 4.2 ZIP 家族（`preview/zip.rs`，新）

新增依赖：`zip = { version = "2", default-features = false, features = ["deflate"] }`
（纯 Rust；deflate 复用已有 flate2；上述格式均为 deflate 或 stored）。

候选条目表（根路径、大小写不敏感匹配）：

| 扩展名 | 候选条目（按序尝试） |
| --- | --- |
| kra, krz | `preview.png` → `mergedimage.png` |
| ora | `Thumbnails/thumbnail.png` → `mergedimage.png` |
| procreate | `QuickLook/Thumbnail.png` |
| docx, xlsx, pptx | `docProps/thumbnail.jpeg` → `.jpg` → `.png` → `.bmp` |
| odt, ods, odp | `Thumbnails/thumbnail.png` |
| cbz | 中央目录中按条目名字典序的第一张图片（png/jpg/jpeg/webp/gif/bmp/tif/tiff） |

预算：单条目未压缩大小上限 64 MiB（参照 `clip.rs` 的 `MAX_SQLI_BYTES`）；流式读取
目标条目 + `take(limit+1)` 闸门防 zip 炸弹；分块拷贝循环内查取消令牌；条目数上限。
解出的字节走 `decode_bytes`（自带尺寸/像素预算）。

### 4.3 `.sai2`（`preview/sai2.rs`，新）

结构（Photopea 规范 + libsai MIT 实现，已完全掌握）：

- 64 字节文件头：`Identifier[16] = "SAI-CANVAS-TYPE0"`、Flags0 / CanvasBackgroundFlags /
  Flags2 / Flags3（各 u8）、Width / Height（u32）、PrintingResolution / TableCount /
  SelectedLayer（u32）、UnknownA / UnknownB（u64）、CanvasBackgroundColor /
  LayerEffectColor（u32）
- chunk 表：TableCount × 16 字节 { Type（u32 LE 四CC）、LayerID（u32）、BlobsOffset
  （u64 绝对偏移）}；blob 大小 = 下一项 offset − 当前 offset（末项到 EOF）
- 缩略图两种形态：
  - `thum`（JSSF，≥512² 画布用）：blob = u32 w + u32 h（未用）+ 四CC `jssf` +
    u16 jw + u16 jh + u16 channels + JSSF 体。JSSF 体 = 64 字节 luma 量化表
    （channels>1 时再 64 字节 chroma）+ MCU 行序列（每行 u16 长度 + 熵编码数据）。
    重包装成标准基线 JPEG（SOI / DQT / DHT 四张标准 Huffman 表 / SOF0 / DRI / SOS /
    RSTm / EOI；DRI 重启间隔 = (jw+7)/8，MCU 行数 = (jh+7)/8），交 `image` 解码。
  - `intg`（dpcm，小画布用）：blob = 四CC `dpcm` + TileSizes[TilesX*TilesY]（u32）+
    各 tile 数据。TileSize = 256；通道数 = 3 + ((CanvasBackgroundFlags & 7) == 0 ? 1 : 0)；
    每行先 UnpackDeltaRLE16（控制掩码操作码的位级 RLE）得 i16 delta，再
    DeltaUnpackRow16Bpc（类 PNG Up 的 16bpc 预测器 + 饱和运算）得 8bpc BGRA；3 通道时
    Alpha 填 0xFF。每个 tile 行首/行尾有 u16 校验（高字节 = tile X 索引）。
- 顺序：先 `thum`，失败或不存在再 `intg`；两者都遵守 degrade 契约。
  **实施时改为 `intg` 优先**：`thum` 恒定只有约 256 px 宽（本机四个样本都是
  256×448），而输出目标最长边 512 px 会把它放大成糊图；`intg` 就是画布本身，
  预算内解码后是真正的降采样。`thum` 保留为超预算画布与旧变体的兜底。
- 移植来源：libsai `include/sai2.hpp` + `source/sai2.cpp`（MIT，SPDX 标头）；四张
  Huffman 表与算法常量按源码照抄。
- 风险：SaiThumbs 作者注明「部分旧格式 sai2 文件尚不支持」——旧变体解析失败即无
  预览，不阻塞其他格式。
- 测试：合成最小 sai2（头 + chunk 表 + 合成 JSSF blob，用 libsai 同款标准表构造
  最小 JPEG 熵流做 round-trip）；真实 `.sai2` 端到端验证待补样本（§8）。

### 4.4 `.blend`（`preview/blend.rs`，新）

两种文件头（本机样本实证，见 §5）：

- 老（≤4.x）：`BLENDER` + ptr（`-`=8 字节 / `_`=4 字节）+ endian（`v`=LE）+ 3 位
  版本号，共 12 字节；块头 24 字节（8 字节指针）或 20 字节（4 字节指针）：
  code(4) + size(4) + old_addr(4/8) + sdna(4) + count(4)。
- 新（5.x）：`BLENDER17-01vNNNN`，共 17 字节；块头 32 字节：
  code(4) + 保留(4) + old_addr(8) + size(4) + sdna(4) + count(4) + 保留(4)
  （两个保留字段样本中均为 0，按保留跳过）。
- TEST 块 payload（两种头一致）：u32 LE 宽 + u32 LE 高 + 宽*高*4 字节 straight RGBA。

流程：验魔法 → 分头型 → 顺序走块找 TEST（TEST 紧跟 REND，靠近文件头；887 MB 样本
也只需读几百字节）→ 校验 size == 8 + w*h*4 与像素预算 → 构造 RgbaImage。走到 ENDB /
超过 64 块 / 1 MiB / 取消 → None。

压缩变体：未压缩直读；gzip（`1F 8B`，<3.x 可选保存）或 zstd（3.x+ 可选保存）用
flate2 / zstd 流式解出前若干 KB 再走同样块逻辑——两个 crate 均已是依赖，零新增。
BE 字节序（`V`）→ None（罕见，degrade）。

### 4.5 `.svg`（`preview/svg.rs`，新）

新增依赖 resvg（纯 Rust，实施时锁定版本；默认特性含文本/字体支持）。usvg 默认不
加载外部资源，行为确定。按 512 最长边等比渲染；pixmap 为预乘 RGBA，反预乘后交输出
管线。解析/渲染失败 → None。

## 5. `.blend` 双样本实证记录（2026-10-20，只读读取）

| 样本 | 文件头 | 大小 | 块链 | TEST payload |
| --- | --- | --- | --- | --- |
| `F:\Models\3D\场景\崩坏：星穹铁道\仙舟罗浮.blend` | `BLENDER-v303`（老，64 位 LE） | 887 MB | REND@12（size 72）→ TEST@108 | size 65544 = 8 + 128×128×4；w=128 h=128；straight RGBA（无 alpha=0 像素、5123 个半透明、颜色范围正常） |
| `F:\Models\3D\人物\Blender\人体参考.blend` | `BLENDER17-01v0501`（新，Blender 5.1） | 254 MB | REND@17（size 264）→ TEST@313（行走步精确落位） | size 35848 = 8 + 128×70×4；w=128 h=70 |

块行走步交叉验证：老格式 12 + 24 + 72 = 108；新格式 17 + 32 + 264 = 313。两种布局
因此确证。`BLENDER17-01` 为 Blender 5.x 新头（社区 bver 脚本确认 5.0.1→v0500、
5.2.1→v0502；「17-01」含义未文档化，按保留处理）。

## 6. 测试与验证策略

- 单元测试照 `clip.rs` / `psd.rs` 模式：合成最小文件（手搓 zip 家族各容器、sai2
  头 + 表 + JSSF round-trip、blend 两种头 + TEST、svg 文本）。
- 真实样本：两个 `.blend` 可直接作 blend 回归样本；`.sai2` 端到端待样本（§8）。
- 做匹配的类型、格式和静态检查（fmt / clippy / 对应模块单测）；完整编译与 GUI
  流程由用户安排；不跑压力测试（`docs/guides/validation.md`）。
- 文档同步：`docs/modules/history-and-backup.md` 缩略图契约（支持列表、「支持 ≠
  一定有」）；CHANGELOG 进 `## Unreleased`；五处版本字段不动（`docs/guides/release-policy.md`）。
- 提交按格式组边界（位图 / zip 家族 / sai2 / blend / svg 各自独立、可分离验证），
  英文祈使句主题，文档与代码同提交，不自动 push。

## 7. 依赖变更汇总

| crate | 变更 | 说明 |
| --- | --- | --- |
| image | features += pnm, hdr, qoi, ff, exr | 纯 Rust feature flag；exr 编译增量稍重 |
| zip | 新增，default-features=false + deflate | 纯 Rust |
| resvg | 新增（默认特性） | 纯 Rust；fontdb 等传递依赖带来编译增量 |

## 8. 待确认 / 待验证

1. exr 与 resvg 的编译体积/时间增量是否可接受（实施首个 crate 时度量后再继续）。
2. `.sai2` 真实样本：**已找到**（原判断「本机未找到」只查了 `F:/Models`）。
   `F:/Artworks/创作/2023Q4/芙宁娜/` 下有 4 个：`S1/2.sai2`(3.8MB)、
   `S1/3.sai2`(4.5MB)、`S1/4.sai2`(4.7MB)、`S2/1.sai2`(18.9MB)，画布
   512×896 / 1024×1792，`thum` 与 `intg` 都在。实施时已用它们做端到端验证。
3. `.procreate`：社区四个项目文档一致，但本机网络受限未能取得
   silicate 的 `reference.procreate` 实测；合成测试 + 文档依据先行，真实样本作可选验证。
4. `.blend` 新头两个保留字段（样本均为 0）：若日后遇到非 0 变体导致行走步错位，
   按 degrade 处理并记录到模块文档。
5. cbz「第一张图」取字典序最小：对 001/002 命名即封面，特殊命名由 degrade 兜底。
6. 旧版 SAI2 变体（SaiThumbs 注明部分旧文件不支持）：解析失败即无预览。

## 9. 已实施范围（2026-10-20 第一批）

维护者确认只做第一批 + `.procreate` + 无魔数的 `.tga`，其余暂缓。落地内容：

| 项 | 结果 |
| --- | --- |
| `.sai2` | `preview/sai2.rs`：头 + chunk 表 → `intg` 优先、`thum` 兜底。libsai（MIT）移植，已记入 `THIRD_PARTY_NOTICES.md`。4 个真实样本端到端通过：3 个 512×896 样本的 `intg` 解码结果与 SAI 导出的同目录 PNG **逐像素一致**，第 4 个（1024×1792，导出为 960×1680）缩放后差异 0.99，仅重采样误差；`thum` 与导出图的均差是 3.6–7.5，正是它只作兜底的理由。JSSF→JPEG 另有合成单测覆盖 1 / 3 通道（灰与彩色）两条 DQT / DHT / SOS 形态 |
| `.procreate` | `preview/zip.rs`：候选条目表 + `QuickLook/Thumbnail.png`，大小写不敏感；未登记的扩展名不进容器扫描 |
| `.tga` | `preview/bitmap.rs`：`guess_format` 明确不识别 TGA，改为扩展名判定 + 显式格式解码。注意调色板型 TGA 头会与 ICO 魔数撞车（`00 00 01 00`），因此拒绝分支也要给 `.tga` 一次机会 |
| 依赖 | 只新增 `zip`（复用 c2pa 已带的 8.6，`deflate-flate2`，不拉 zopfli）；`image` feature 未改（tga 本来就开着） |
| 暂缓 | kra/krz、ora、cbz、ooxml、odf、blend、svg、exr/hdr/qoi/ff/pnm 均未实现，`supports_preview` 不登记 |

实施中修正的两处计划假设：`thum` 并不总比 `intg` 新/大（见 §4.3），`sai2` 样本本来就在本机（见 §8.2）。
另记一条与本批无关但值得知道的现状：`preview::encode` 会把小于 512 px 的源**放大**到 512，
这正是 `intg` 必须优先的原因，也说明 `thum` 一旦作为兜底仍会被放大。
