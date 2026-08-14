# 随包分发的 TrustMark 模型

本目录包含 Rust `trustmark` crate 使用的官方 Adobe TrustMark Q/BCH_SUPER ONNX 编码器与解码器。Tauri 将两个文件作为离线打包资源随应用分发，运行时不下载模型。

| 文件 | 大小 | SHA-256 |
| --- | --- | --- |
| `encoder_Q.onnx` | 16.5 MB | `19b3d1b25836130ffd78775a8f61539f993375d1823ef0e59ba5b8dffb4f892d` |
| `decoder_Q.onnx` | 45.2 MB | `ee3268f057c9dabef680e169302f5973d0589feea86189ed229a896cc3aa88df` |

模型变体与哈希摘要由后端在运行时读取，显示于应用的发布/识别页面；不在前端写死。
