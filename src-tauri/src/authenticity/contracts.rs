use std::{fs, path::Path};

use rusqlite::Connection;

use crate::{
    contracts::{
        collect_candidate_files, ManagedFile, ManagedFileOwner, ReferenceInventory,
        ScanCandidateFile,
    },
    storage,
};

use super::publication_repository::{artwork_artifacts_directory, certification_copy_directory};

/// 最终成品与认证输出的引用清单。认证导出路径在仓库之外，失去引用后仍需用户在
/// 清理队列里逐条确认，不能进入自动重放。
///
/// 仓库内的两类文件（`artifacts/<branch>/` 下的成品副本与 `certifications/` 下的
/// 认证副本）由 [`Self::scan_candidates`] 负责发现：取消发布时若副本内容已被替换，
/// 命令按设计不登记清理、把文件原样留在仓库里，只有垃圾扫描能把它们找回来。
pub(crate) struct ArtifactReferenceInventory;

impl ReferenceInventory for ArtifactReferenceInventory {
    fn collect_targets(&self, connection: &Connection) -> Result<Vec<ManagedFile>, String> {
        let mut statement = connection
            .prepare(
                "SELECT source_path, 0 FROM final_artifacts
                   WHERE TRIM(source_path) <> ''
                 UNION ALL
                 SELECT stored_path, 1 FROM certification_records
                   WHERE TRIM(stored_path) <> ''
                 UNION ALL
                 SELECT output_path, 2 FROM certification_records
                   WHERE TRIM(output_path) <> ''",
            )
            .map_err(storage::database_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(storage::database_error)?;
        let mut targets = Vec::new();
        for row in rows {
            let (path, kind) = row.map_err(storage::database_error)?;
            targets.push(ManagedFile {
                path,
                owner: match kind {
                    0 => ManagedFileOwner::FinalArtifact,
                    1 => ManagedFileOwner::CertificationStored,
                    _ => ManagedFileOwner::CertificationOutput,
                },
            });
        }
        Ok(targets)
    }

    /// 成品副本与认证副本的候选：`artifacts/<branch>/` 的直属普通文件，以及其中
    /// `certifications/` 的直属普通文件。
    ///
    /// 不按文件名过滤：崩溃残留的 `.tmp*` 暂存文件与用户手工改名过的副本正是最
    /// 需要被扫到的两类，而文件名白名单恰好会漏掉它们。正在使用的成品与副本由扫描
    /// 主流程的引用 anti-join 排除；宽限期与确认清理前的 SHA-256 复查兜住误删风险。
    fn scan_candidates(
        &self,
        artwork_directory: &Path,
        files: &mut Vec<ScanCandidateFile>,
    ) -> Result<(), String> {
        let artifacts = artwork_artifacts_directory(artwork_directory);
        let branches = match fs::read_dir(&artifacts) {
            Ok(branches) => branches,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(format!("无法读取成品目录 {}：{error}", artifacts.display())),
        };
        for entry in branches {
            let entry = entry.map_err(|error| format!("无法读取成品目录项：{error}"))?;
            // `DirEntry::file_type` 不跟随符号链接：指向目录的链接不算分支目录。
            let is_directory = entry
                .file_type()
                .map_err(|error| format!("无法读取成品目录项类型：{error}"))?
                .is_dir();
            if !is_directory {
                continue;
            }
            let branch_directory = entry.path();
            collect_candidate_files(&branch_directory, "最终成品副本未被引用", |_| true, files)?;
            collect_candidate_files(
                &certification_copy_directory(&branch_directory),
                "认证副本未被引用",
                |_| true,
                files,
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashSet, fs, path::PathBuf};

    use super::*;
    use crate::authenticity::test_support::{initialize_publication, insert_final_artifact};
    use crate::digest::ZERO_SHA256;

    /// 造三个文件：被成品记录引用的副本、没有任何记录引用的成品副本、孤儿认证副本。
    /// 返回 `(作品目录, 被引用, 孤儿成品, 孤儿认证副本)`。
    fn seed_artifact_copies(root: &Path) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
        initialize_publication(root);
        let artwork_directory = root.join("artworks").join("artwork");
        let artifact_directory = artwork_artifacts_directory(&artwork_directory).join("branch");
        let certifications = certification_copy_directory(&artifact_directory);
        fs::create_dir_all(&certifications).unwrap();
        let referenced = artifact_directory.join("final.png");
        let orphan = artifact_directory.join("abandoned.png");
        let orphan_certification = certifications.join("record.jpg");
        for path in [&referenced, &orphan, &orphan_certification] {
            fs::write(path, b"payload").unwrap();
        }
        insert_final_artifact(
            root,
            &storage::relative_path(root, &referenced).unwrap(),
            ZERO_SHA256,
        );
        (artwork_directory, referenced, orphan, orphan_certification)
    }

    #[test]
    fn lists_artifact_and_certification_copies_as_candidates() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let (artwork_directory, _, orphan, orphan_certification) = seed_artifact_copies(&root);

        let mut files = Vec::new();
        ArtifactReferenceInventory
            .scan_candidates(&artwork_directory, &mut files)
            .unwrap();

        let mut reported = files
            .iter()
            .map(|file| (file.path.clone(), file.reason))
            .collect::<Vec<_>>();
        reported.sort_by(|left, right| left.0.cmp(&right.0));

        assert_eq!(
            reported,
            vec![
                (orphan.clone(), "最终成品副本未被引用"),
                (orphan_certification.clone(), "认证副本未被引用"),
                (
                    artwork_directory
                        .join("artifacts")
                        .join("branch")
                        .join("final.png"),
                    "最终成品副本未被引用"
                ),
            ]
        );
    }

    /// 扫描主流程用 `collect_targets` 做 anti-join：被成品记录指向的副本必须出现在
    /// 引用清单里，否则清理会把正在使用的文件删掉。
    #[test]
    fn referenced_copy_stays_in_the_reference_list() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let (artwork_directory, referenced, orphan, orphan_certification) =
            seed_artifact_copies(&root);

        let referenced_paths = ArtifactReferenceInventory
            .collect_targets(&storage::open(&root).unwrap())
            .unwrap()
            .into_iter()
            .map(|target| target.path)
            .collect::<HashSet<_>>();
        assert!(referenced_paths.contains(&storage::relative_path(&root, &referenced).unwrap()));

        let mut files = Vec::new();
        ArtifactReferenceInventory
            .scan_candidates(&artwork_directory, &mut files)
            .unwrap();
        let unreferenced = files
            .into_iter()
            .filter(|file| {
                storage::relative_path(&root, &file.path)
                    .is_ok_and(|relative| !referenced_paths.contains(&relative))
            })
            .map(|file| file.path)
            .collect::<HashSet<_>>();

        assert_eq!(unreferenced, HashSet::from([orphan, orphan_certification]));
    }

    #[test]
    fn reports_nothing_when_the_artwork_has_no_artifacts_directory() {
        let directory = tempfile::tempdir().unwrap();
        let artwork_directory = directory.path().join("artworks").join("artwork");
        fs::create_dir_all(&artwork_directory).unwrap();

        let mut files = Vec::new();
        ArtifactReferenceInventory
            .scan_candidates(&artwork_directory, &mut files)
            .unwrap();

        assert!(files.is_empty());
    }

    /// 只碰两层直属文件：分支目录下的未知子目录与其内容不递归，避免把用户自己放进
    /// 仓库的目录整片报成孤儿。
    #[test]
    fn does_not_descend_into_unknown_subdirectories() {
        let directory = tempfile::tempdir().unwrap();
        let artwork_directory = directory.path().join("artworks").join("artwork");
        let nested = artwork_artifacts_directory(&artwork_directory)
            .join("branch")
            .join("notes");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("draft.png"), b"payload").unwrap();

        let mut files = Vec::new();
        ArtifactReferenceInventory
            .scan_candidates(&artwork_directory, &mut files)
            .unwrap();

        assert!(files.is_empty());
    }
}
