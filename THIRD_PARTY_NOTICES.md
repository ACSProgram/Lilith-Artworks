# Third-Party Notices

Lilith Artworks（本项目）以 GNU General Public License v3.0 发布（见根目录 `LICENSE`）。
本文件记录本项目使用的主要第三方组件及其独立许可。这些组件**不并入** GPL，
各自保留其原始许可条款。

## 运行时与前端框架

| 组件 | 用途 | 许可 |
| --- | --- | --- |
| [Tauri 2](https://tauri.app) | 桌面应用外壳 / 系统集成 | MIT 或 Apache-2.0 |
| [React](https://react.dev) / React DOM | 前端 UI | MIT |
| [lucide-react](https://lucide.dev) | 图标 | ISC |
| [Vite](https://vite.dev) | 前端构建 | MIT |

## Rust 后端依赖（主要）

| 组件 | 用途 | 许可 |
| --- | --- | --- |
| [rusqlite](https://github.com/rusqlite/rusqlite) | SQLite 绑定 | MIT |
| [c2pa-rs](https://github.com/contentauth/c2pa-rs) | C2PA 内容凭证签名/读取 | Apache-2.0 |
| [trustmark](https://crates.io/crates/trustmark) | TrustMark 水印（编码/解码） | MIT（Copyright Adobe） |
| [image](https://github.com/image-rs/image) | 图片解码/编码 | MIT 或 Apache-2.0 |
| [zstd](https://github.com/gyscos/zstd-rs) | 压缩 | MIT 或 Apache-2.0 |
| [serde / serde_json](https://serde.rs) | 序列化 | MIT 或 Apache-2.0 |
| [tauri-plugin-dialog](https://github.com/tauri-apps/plugins-workspace) | 系统对话框 | MIT 或 Apache-2.0 |

## 随包分发的人工智能模型（注意独立许可）

`src-tauri/resources/models/encoder_Q.onnx` 与 `decoder_Q.onnx` 是
**Adobe TrustMark** 官方预训练模型（MIT License，Copyright Adobe），
**不是本项目的代码**，也不并入 GPL。条款见
`src-tauri/resources/models/LICENSE`。

---

完整依赖许可清单由包管理器维护：

- Rust：`src-tauri/Cargo.lock`（可在构建目录的 crate 源码中找到各依赖的 LICENSE）
- 前端：`node_modules/`（各包自带 LICENSE，见 `package-lock.json`）

修改依赖或引入新组件时，请同步更新本文件并核对许可兼容性。
