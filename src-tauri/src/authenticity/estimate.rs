//! 认证输出大小估算：按发布时的 JPEG 质量与背景合成参数编码一次，只统计字节数。
//!
//! 不写文件、不签名；估算结果只用于界面提示，不缓存。输入仍是仓库内受控的最终
//! 成品，不接受外部路径。

use std::{io::Write, path::Path};

use image::codecs::jpeg::JpegEncoder;

use super::{
    error::{AuthenticityError, AuthenticityResult},
    image_resource,
    model::{EstimateRequest, FileSizeEstimate},
    publication_repository,
    rendition::{flatten_to_rgb, parse_background},
};

pub(crate) fn estimate_output_size(
    root: &Path,
    request: &EstimateRequest,
) -> AuthenticityResult<FileSizeEstimate> {
    if !(1..=100).contains(&request.jpeg_quality) {
        return Err(AuthenticityError::message("JPEG 质量必须在 1 到 100 之间"));
    }
    let target = publication_repository::publication_target(root, request.branch_id.trim())
        .map_err(AuthenticityError::message)?;
    let path = Path::new(&target.artifact_path);
    let source = image_resource::open(path)?;
    let flattened = flatten_to_rgb(&source, parse_background(&request.background_color)?);
    drop(source);
    let mut output = CountingWriter::default();
    JpegEncoder::new_with_quality(&mut output, request.jpeg_quality).encode_image(&flattened)?;
    Ok(FileSizeEstimate {
        jpeg_bytes: output.bytes,
    })
}

#[derive(Default)]
struct CountingWriter {
    bytes: u64,
}

impl Write for CountingWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.bytes = self.bytes.saturating_add(buffer.len() as u64);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use sha2::Digest;

    use super::*;
    use crate::authenticity::test_support::{initialize_publication, insert_final_artifact};
    use crate::storage;

    #[test]
    fn estimate_counts_the_encoded_jpeg_without_writing_a_file() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        initialize_publication(&root);
        let relative = "artworks/final.png";
        let absolute = storage::resolve_path(&root, relative).unwrap();
        fs::create_dir_all(absolute.parent().unwrap()).unwrap();
        image::DynamicImage::ImageRgb8(image::RgbImage::new(64, 48))
            .save(&absolute)
            .unwrap();
        let sha = hex::encode_upper(sha2::Sha256::digest(fs::read(&absolute).unwrap()));
        insert_final_artifact(&root, relative, &sha);

        let estimate = estimate_output_size(
            &root,
            &EstimateRequest {
                branch_id: "branch".into(),
                jpeg_quality: 90,
                background_color: "#FFFFFF".into(),
            },
        )
        .unwrap();

        assert!(estimate.jpeg_bytes > 0);
        // 估算响应只保留编码字节数；原图大小由预览响应的 `PreviewImage` 提供。
        assert_eq!(
            serde_json::to_value(&estimate).unwrap(),
            serde_json::json!({ "jpegBytes": estimate.jpeg_bytes })
        );
        // 估算不产生输出文件，也不写入仓库 temp。
        assert!(fs::read_dir(root.join("temp")).unwrap().next().is_none());
    }

    #[test]
    fn estimate_rejects_invalid_quality_and_background() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        initialize_publication(&root);

        let mut request = EstimateRequest {
            branch_id: "branch".into(),
            jpeg_quality: 0,
            background_color: "#FFFFFF".into(),
        };
        assert!(estimate_output_size(&root, &request).is_err());

        request.jpeg_quality = 90;
        request.background_color = "white".into();
        assert!(estimate_output_size(&root, &request).is_err());
    }
}
