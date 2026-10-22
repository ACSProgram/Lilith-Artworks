//! 设置的内存快照与唯一写入口。
//!
//! `SettingsService` 负责写锁、revision/CAS 与原子替换；设置文件的形状与合法性由
//! `model` 提供。它不处理仓库租约、窗口或命令。

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
};

use crate::app::command_runtime::CommandError;

use super::model::{write_json_atomic, AppSettings};

/// 设置的唯一写入口。
///
/// 设置页保存走 [`Self::update`] 并携带期望 revision；托盘、窗口几何等应用内部
/// 写入也经同一入口，只是在当前快照上打补丁（无 CAS）。写入顺序固定为：取写锁 →
/// 复核 revision → 原子替换 JSON → 更新内存快照；任何一步失败都不会留下内存与
/// 文件不一致的状态。
#[derive(Clone)]
pub(crate) struct SettingsService {
    inner: Arc<SettingsServiceInner>,
}

struct SettingsServiceInner {
    current: RwLock<AppSettings>,
    path: PathBuf,
    write: Mutex<()>,
}

impl SettingsService {
    pub(super) fn new(settings: AppSettings, path: PathBuf) -> Self {
        Self {
            inner: Arc::new(SettingsServiceInner {
                current: RwLock::new(settings),
                path,
                write: Mutex::new(()),
            }),
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.inner.path
    }

    pub(super) fn read(&self) -> Result<AppSettings, String> {
        self.inner
            .current
            .read()
            .map(|settings| settings.clone())
            .map_err(|_| "设置状态已损坏".to_owned())
    }

    pub(super) fn read_with<T>(&self, reader: impl FnOnce(&AppSettings) -> T) -> Result<T, String> {
        self.inner
            .current
            .read()
            .map(|settings| reader(&settings))
            .map_err(|_| "设置状态已损坏".to_owned())
    }

    pub(crate) fn update(
        &self,
        expected_revision: Option<u64>,
        patch: impl FnOnce(&mut AppSettings) -> Result<(), CommandError>,
    ) -> Result<AppSettings, CommandError> {
        let _writing = self
            .inner
            .write
            .lock()
            .map_err(|_| CommandError::internal("设置写入锁已损坏"))?;
        let current = self
            .inner
            .current
            .read()
            .map(|settings| settings.clone())
            .map_err(|_| CommandError::internal("设置状态已损坏"))?;
        if expected_revision.is_some_and(|expected| expected != current.revision) {
            return Err(CommandError::settings_conflict());
        }
        let mut next = current.clone();
        patch(&mut next)?;
        next.revision = current.revision.saturating_add(1);
        write_json_atomic(&self.inner.path, &next).map_err(CommandError::from)?;
        *self
            .inner
            .current
            .write()
            .map_err(|_| CommandError::internal("设置状态已损坏"))? = next.clone();
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::Arc};

    use super::*;
    use crate::app::settings::model::load_settings;

    #[test]
    fn settings_updates_use_revision_cas_and_bump_the_counter() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let initial = AppSettings::default();
        write_json_atomic(&path, &initial).unwrap();
        let service = SettingsService::new(initial, path.clone());

        let saved = service
            .update(Some(0), |settings| {
                settings.theme = "dark".into();
                Ok(())
            })
            .unwrap();
        assert_eq!(saved.revision, 1);
        assert_eq!(saved.theme, "dark");

        let conflict = service
            .update(Some(0), |settings| {
                settings.theme = "light".into();
                Ok(())
            })
            .unwrap_err();
        assert_eq!(conflict.code, "settings_conflict");
        assert!(conflict.retryable);

        // 冲突不得覆盖文件或内存中的较新设置。
        let (loaded, warning) = load_settings(&path);
        assert!(warning.is_none());
        assert_eq!(loaded.theme, "dark");
        assert_eq!(loaded.revision, 1);
        assert_eq!(service.read().unwrap().theme, "dark");
    }

    #[test]
    fn concurrent_settings_saves_allow_exactly_one_cas_winner() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let initial = AppSettings::default();
        write_json_atomic(&path, &initial).unwrap();
        let service = Arc::new(SettingsService::new(initial, path.clone()));
        let barrier = Arc::new(std::sync::Barrier::new(2));

        let handles = (0..2)
            .map(|index| {
                let service = service.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    service
                        .update(Some(0), |settings| {
                            settings.theme = if index == 0 { "dark" } else { "light" }.into();
                            Ok(())
                        })
                        .map(|_| index)
                })
            })
            .collect::<Vec<_>>();
        let results = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>();

        let winners = results.iter().filter(|result| result.is_ok()).count();
        assert_eq!(winners, 1, "同一 revision 只能有一个写入者成功");
        let conflict = results
            .iter()
            .find_map(|result| result.as_ref().err())
            .expect("另一个写入者必须得到冲突");
        assert_eq!(conflict.code, "settings_conflict");

        let (loaded, warning) = load_settings(&path);
        assert!(warning.is_none());
        assert_eq!(loaded.revision, 1);
    }

    #[test]
    fn internal_patches_preserve_concurrent_fields_and_bump_revision() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let mut initial = AppSettings::default();
        initial.theme = "dark".into();
        write_json_atomic(&path, &initial).unwrap();
        let service = SettingsService::new(initial, path.clone());

        // 托盘写入不携带期望 revision：直接在当前快照上打补丁，保留设置页已选的
        // 其他字段。
        let paused = service
            .update(None, |settings| {
                settings.pause_automatic_backups = true;
                Ok(())
            })
            .unwrap();
        assert!(paused.pause_automatic_backups);
        assert_eq!(paused.theme, "dark");
        assert_eq!(paused.revision, 1);

        // 基于旧 revision 的设置页保存必须得到冲突，而不是静默覆盖托盘改动。
        let conflict = service
            .update(Some(0), |settings| {
                settings.theme = "light".into();
                Ok(())
            })
            .unwrap_err();
        assert_eq!(conflict.code, "settings_conflict");
    }

    #[test]
    fn a_failed_settings_patch_leaves_memory_and_file_untouched() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let initial = AppSettings::default();
        write_json_atomic(&path, &initial).unwrap();
        let before = fs::read(&path).unwrap();
        let service = SettingsService::new(initial, path.clone());

        let error = service
            .update(Some(0), |_| {
                Err(CommandError::invalid_input("主题设置无效"))
            })
            .unwrap_err();
        assert_eq!(error.code, "invalid_input");
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(service.read().unwrap().revision, 0);
    }
}
