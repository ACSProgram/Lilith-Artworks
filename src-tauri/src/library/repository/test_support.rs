//! Library 仓储单测共用的最小仓库夹具。
//!
//! 只供 `#[cfg(test)]` 使用：建一个仓库、一个分组、一个 Artwork 与主分支。

use std::{fs, io::Write, path::PathBuf};

use super::super::model::CreatedArtwork;
use super::{create_artwork, initialize};

pub(crate) struct Fixture {
    /// 只用于持有临时目录的生命周期：夹具存活期间仓库目录不会被删除。
    pub(crate) _directory: tempfile::TempDir,
    pub(crate) root: PathBuf,
    pub(crate) first_source: PathBuf,
    pub(crate) artwork: CreatedArtwork,
}

impl Fixture {
    pub(crate) fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let first_source = directory.path().join("first.psd");
        fs::File::create(&first_source)
            .unwrap()
            .write_all(b"first")
            .unwrap();
        initialize(&root).unwrap();
        let group = super::tree::create_group_id(&root, None, "Series").unwrap();
        let artwork = create_artwork(
            &root,
            Some(&group),
            "Portrait",
            "Main",
            &first_source,
            false,
        )
        .unwrap();
        Self {
            _directory: directory,
            root,
            first_source,
            artwork,
        }
    }
}
