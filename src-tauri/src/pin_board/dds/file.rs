//! DDS 文件的原子落盘与失败回滚。
//!
//! 先写同目录临时文件并同步，再以覆盖方式发布到最终路径；事务提交前登记的
//! 文件在 [`CreatedFiles`] 析构时删除，避免失败路径留下孤儿 DDS。

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use tempfile::NamedTempFile;

#[derive(Default)]
pub(crate) struct CreatedFiles {
    paths: Vec<PathBuf>,
    committed: bool,
}

impl CreatedFiles {
    pub(crate) fn push(&mut self, path: PathBuf) {
        self.paths.push(path);
    }

    pub(crate) fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for CreatedFiles {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        for path in &self.paths {
            let _ = fs::remove_file(path);
        }
    }
}

pub(crate) fn persist_dds_file(destination: &Path, bytes: &[u8], kind: &str) -> Result<(), String> {
    let parent = destination.parent().ok_or("DDS 保存路径无效")?;
    // 画板目录可能缺失（例如仓库数据迁移后目录未随行），先补建再落临时文件，
    // 否则 NamedTempFile 会因父目录不存在（os error 3）直接失败。
    fs::create_dir_all(parent).map_err(|error| format!("无法创建画板图片目录：{error}"))?;
    let mut temporary = NamedTempFile::new_in(parent)
        .map_err(|error| format!("无法创建{kind}临时文件：{error}"))?;
    temporary
        .write_all(bytes)
        .map_err(|error| format!("无法写入{kind}：{error}"))?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|error| format!("无法同步{kind}：{error}"))?;
    temporary
        .persist(destination)
        .map_err(|error| format!("无法保存{kind}：{}", error.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn persists_dds_file_into_a_missing_board_directory() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("boards").join("2").join("image.dds");

        persist_dds_file(&destination, b"dds-bytes", "剪贴板 DDS 图片").unwrap();

        assert_eq!(fs::read(&destination).unwrap(), b"dds-bytes");
    }
}
