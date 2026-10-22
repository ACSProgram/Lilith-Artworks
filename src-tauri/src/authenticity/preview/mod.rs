//! 认证图片的 IPC 预览：质量预览、高清局部与固定尺寸缩略图。
//!
//! 三个子模块共享资源预算与源图解析规则，但输出用途不同：`quality` 生成发布
//! 预览的基础层，`tile` 按源分辨率补齐放大后的细节，`thumbnail` 提供记录/识别
//! 预览与质量预览的缩略图层。

mod quality;
mod thumbnail;
mod tile;

pub(crate) use quality::preview as preview_publication;
pub(crate) use thumbnail::make_preview;
pub(crate) use tile::preview_tile;
