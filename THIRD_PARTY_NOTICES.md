# Third-Party Notices

Lilith Artworks 中由项目贡献者创作的代码以 GNU General Public License
v3.0 only 发布（见根目录 `LICENSE`）。第三方软件与模型保留各自版权和许可；
分发完整应用时同时适用 GPL-3.0-only 与全部第三方条款。

以下为随包分发的主要第三方组件及其许可。完整的目标安装包许可清单由发行流程生成，并随
安装包一起交付；`Cargo.lock` 与 `package-lock.json` 只锁定依赖解析，不等同于许可正文、
版权告知或目标平台依赖审计。

## 运行时与前端框架

| 组件 | 用途 | 许可 |
| --- | --- | --- |
| [Tauri 2](https://tauri.app) | 桌面应用外壳 / 系统集成 | MIT 或 Apache-2.0 |
| [React](https://react.dev) / React DOM | 前端 UI | MIT |
| [lucide-react](https://lucide.dev) | 图标 | ISC |
| [Tauri plugins](https://github.com/tauri-apps/plugins-workspace) | 对话框、文件授权、日志与单实例 | MIT 或 Apache-2.0 |

## Rust 后端依赖（主要）

| 组件 | 用途 | 许可 |
| --- | --- | --- |
| [rusqlite](https://github.com/rusqlite/rusqlite) | SQLite 绑定 | MIT |
| [c2pa-rs](https://github.com/contentauth/c2pa-rs) | C2PA 内容凭证签名/读取 | MIT 或 Apache-2.0 |
| [trustmark](https://crates.io/crates/trustmark) | TrustMark 水印（编码/解码） | MIT（Copyright Adobe） |
| [ort](https://github.com/pykeio/ort) | ONNX Runtime Rust 绑定 | MIT 或 Apache-2.0 |
| [ONNX Runtime](https://onnxruntime.ai) | TrustMark 模型推理运行时 | MIT |
| [image](https://github.com/image-rs/image) | 图片解码/编码 | MIT 或 Apache-2.0 |
| [zstd](https://github.com/gyscos/zstd-rs) | 压缩 | MIT 或 Apache-2.0 |
| [serde / serde_json](https://serde.rs) | 序列化 | MIT 或 Apache-2.0 |

## 构建与测试工具

Vite、TypeScript、Vitest、Testing Library 和 Tauri CLI 只用于构建或测试，
不作为前端 JavaScript 运行时依赖打入应用。

## 随包分发的人工智能模型（注意独立许可）

`src-tauri/resources/models/encoder_Q.onnx` 与 `decoder_Q.onnx` 是
**Adobe TrustMark** 官方预训练模型（MIT License，Copyright Adobe），
不是项目贡献者创作的代码。模型作为完整 GPL 应用的一部分聚合分发，
并保留 Adobe 的独立许可与告知。原始条款见
`src-tauri/resources/models/LICENSE`。
