# Third-Party Notices

Lilith Artworks 中由项目贡献者创作的代码以 GNU General Public License
v3.0 only 发布（见根目录 `LICENSE`）。第三方软件与模型保留各自版权和许可；
分发完整应用时仍需同时满足 GPL-3.0-only 和全部适用的第三方条款。

本文件是人工维护的主要组件摘要，**不是完整的目标安装包许可清单**。
`Cargo.lock` 和 `package-lock.json` 用于锁定依赖解析，不能替代许可证正文、
版权告知或目标平台依赖审计。正式安装包发布前必须生成 Windows 目标专用、
包含实际版本和必要许可正文的第三方许可包，并将它与本文件一起随包交付。

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
不作为前端 JavaScript 运行时依赖打入应用；发行流程仍需审计构建工具及其
供应链风险。

## 随包分发的人工智能模型（注意独立许可）

`src-tauri/resources/models/encoder_Q.onnx` 与 `decoder_Q.onnx` 是
**Adobe TrustMark** 官方预训练模型（MIT License，Copyright Adobe），
不是项目贡献者创作的代码。模型仍作为完整 GPL 应用的一部分聚合分发，
并保留 Adobe 的独立许可与告知。原始条款见
`src-tauri/resources/models/LICENSE`。

修改依赖、模型或打包目标时，需要重新生成目标许可清单、核对许可兼容性，
并检查安装包中实际存在根 `LICENSE`、本文件、模型 `LICENSE` 和生成的许可包。
