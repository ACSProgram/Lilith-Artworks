//! 认证记录再次导出：把仓库内认证副本以不覆盖方式写到用户选择的外部路径。
//!
//! 源文件由调用方经 `repository::record_source_path` 解析并核验哈希；本模块只做
//! 目标路径校验与复制，不读取数据库，也不改变记录。

use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::storage;

use super::{
    error::{AuthenticityError, AuthenticityResult},
    file_ops::copy_noclobber,
    rendition::is_jpeg_extension,
};

pub(crate) fn export_record(
    root: &Path,
    source: &Path,
    output_path: &str,
) -> AuthenticityResult<()> {
    let destination = PathBuf::from(output_path.trim());
    if !destination.is_absolute() {
        return Err(AuthenticityError::message("导出路径必须是绝对路径"));
    }
    if destination
        .extension()
        .and_then(|value| value.to_str())
        .is_none_or(|value| !is_jpeg_extension(&value.to_ascii_lowercase()))
    {
        return Err(AuthenticityError::message(
            "认证图片必须导出为 .jpg 或 .jpeg",
        ));
    }
    if destination.exists() {
        return Err(AuthenticityError::message(
            "导出目标已存在；请选择新的文件名",
        ));
    }
    storage::ensure_outside_repository(root, &destination, "再次导出路径")
        .map_err(AuthenticityError::message)?;
    let directory = destination
        .parent()
        .ok_or_else(|| AuthenticityError::message("导出目录无效"))?;
    fs::create_dir_all(directory)
        .map_err(|error| AuthenticityError::message(format!("无法创建导出目录：{error}")))?;
    copy_noclobber(source, &destination)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn export_copies_bytes_without_overwriting_an_existing_file() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        crate::library::initialize(&root).unwrap();
        let source = directory.path().join("certified.jpg");
        fs::write(&source, b"signed bytes").unwrap();
        let destination = directory.path().join("export.jpg");

        export_record(&root, &source, &destination.to_string_lossy()).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"signed bytes");

        assert!(export_record(&root, &source, &destination.to_string_lossy()).is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"signed bytes");
    }

    #[test]
    fn export_rejects_relative_and_repository_paths() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        crate::library::initialize(&root).unwrap();
        let source = directory.path().join("certified.jpg");
        fs::write(&source, b"signed bytes").unwrap();

        assert!(export_record(&root, &source, "relative.jpg").is_err());
        let inside = root.join("artworks").join("inside.jpg");
        assert!(export_record(&root, &source, &inside.to_string_lossy()).is_err());
    }
}
