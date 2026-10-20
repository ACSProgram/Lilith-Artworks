//! 拖放导入的暂存目录生命周期。
//!
//! 桌面 WebView 在 Windows 上启用 HTML5 拖放后拿不到 `File` 的绝对路径，识别页因此把
//! 原始字节交给原生端落到系统临时目录，再复用既有的预览与识别流程。整个进程共用
//! 一个会话目录 `%TEMP%/lilith-artworks-drop-<UUID>`，它的生命周期有三条约定：
//!
//! - **按需创建**：第一次落盘才建目录，没用过拖放就不在系统临时目录里留痕；
//! - **退出必删**：进程正常退出（关窗、托盘退出与确认后的兜底强退）时整目录删除，
//!   残留的暂存图片随之一起清掉；
//! - **残留下次回收**：进程被强杀或断电时目录会留下，下一次启动按宽限期清扫。
//!
//! 只认「前缀 + 合法 UUID」的目录名，手工放在系统临时目录里的同名条目一律不动。

use std::{
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use crate::{platform::path::display_path, storage};

/// 会话目录前缀。清扫与退出删除都以它加合法 UUID 后缀判定归属。
const SESSION_PREFIX: &str = "lilith-artworks-drop-";

/// 崩溃残留的宽限期。目录的修改时间在每次落盘新图片时刷新，因此宽限期内的目录
/// 可能还属于刚退出或仍在退出的上一个进程；超过它才当作可以回收的残留。
const STALE_GRACE_MS: i64 = 10 * 60 * 1000;

static SESSION_ID: OnceLock<String> = OnceLock::new();

/// 一次启动清扫的结果：`reclaimed` 为回收的目录数，`failed` 为删除失败数。
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct DropStageSweep {
    pub(crate) reclaimed: usize,
    pub(crate) failed: usize,
}

fn session_id() -> &'static str {
    SESSION_ID.get_or_init(storage::new_id)
}

fn session_directory_in(base: &Path, session: &str) -> PathBuf {
    base.join(format!("{SESSION_PREFIX}{session}"))
}

/// 把一张待识别的图片落到本进程的暂存目录，返回落盘路径。
pub(crate) fn write_staged_image(bytes: &[u8], extension: &str) -> std::io::Result<PathBuf> {
    write_staged_image_in(&std::env::temp_dir(), session_id(), bytes, extension)
}

fn write_staged_image_in(
    base: &Path,
    session: &str,
    bytes: &[u8],
    extension: &str,
) -> std::io::Result<PathBuf> {
    let directory = session_directory_in(base, session);
    fs::create_dir_all(&directory)?;
    let path = directory.join(format!("{}.{}", storage::new_id(), extension));
    fs::write(&path, bytes)?;
    Ok(path)
}

/// 退出路径：删除本进程的暂存目录（含其中残留的图片）。
///
/// 从未执行过拖放时目录不存在，这里不做任何事，也就不会为了删除而先创建目录。
pub(crate) fn remove_session() {
    let Some(session) = SESSION_ID.get() else {
        return;
    };
    remove_session_in(&std::env::temp_dir(), session);
}

fn remove_session_in(base: &Path, session: &str) {
    let directory = session_directory_in(base, session);
    match fs::remove_dir_all(&directory) {
        Ok(()) => log::info!("drop stage removed on exit: {}", display_path(&directory)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => log::warn!("无法删除拖放暂存目录 {}：{error}", display_path(&directory)),
    }
}

/// 启动路径：回收上一个进程留下的暂存目录。
pub(crate) fn sweep_stale() -> DropStageSweep {
    let now_ms = match storage::now_ms() {
        Ok(now_ms) => now_ms,
        Err(error) => {
            log::warn!("无法读取当前时间，跳过拖放暂存目录清扫：{error}");
            return DropStageSweep::default();
        }
    };
    sweep_stale_in(&std::env::temp_dir(), now_ms)
}

/// 清扫系统临时目录下的暂存会话目录。
///
/// 只处理 `lilith-artworks-drop-<UUID>` 命名的目录，按目录自身的修改时间判断是否
/// 超过宽限期；本进程的会话目录始终保留（正常启动时它还不存在，这里只是兜底）。
/// 不跟随符号链接，删除失败只计数并写日志，不影响启动流程。
fn sweep_stale_in(base: &Path, now_ms: i64) -> DropStageSweep {
    let mut sweep = DropStageSweep::default();
    let entries = match fs::read_dir(base) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return sweep,
        Err(error) => {
            log::warn!(
                "无法读取系统临时目录 {}，跳过拖放暂存目录清扫：{error}",
                display_path(base)
            );
            return sweep;
        }
    };
    let cutoff_ms = now_ms.saturating_sub(STALE_GRACE_MS);
    let own_directory = session_directory_in(base, session_id());
    for entry in entries.flatten() {
        let path = entry.path();
        if path == own_directory {
            continue;
        }
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if !is_session_name(name) {
            continue;
        }
        // `symlink_metadata` 不跟随符号链接：指向目录的链接按链接本身判定，
        // `is_dir` 为假，不会递归删除链接目标。
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.is_dir() {
            continue;
        }
        let Some(modified_ms) = modified_ms(&metadata) else {
            continue;
        };
        if modified_ms >= cutoff_ms {
            continue;
        }
        match fs::remove_dir_all(&path) {
            Ok(()) => sweep.reclaimed += 1,
            Err(error) => {
                sweep.failed += 1;
                log::warn!("无法回收拖放暂存目录 {}：{error}", display_path(&path));
            }
        }
    }
    sweep
}

/// 会话目录命名：`lilith-artworks-drop-<UUID v4>`；后缀不是合法 UUID 的目录视为
/// 手工放置的条目，不清理。
fn is_session_name(name: &str) -> bool {
    name.strip_prefix(SESSION_PREFIX)
        .is_some_and(|session| uuid::Uuid::parse_str(session).is_ok())
}

fn modified_ms(metadata: &fs::Metadata) -> Option<i64> {
    let modified = metadata.modified().ok()?;
    let elapsed = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    i64::try_from(elapsed.as_millis()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn age_directory(path: &Path) {
        let old = filetime::FileTime::from_system_time(
            std::time::SystemTime::now() - std::time::Duration::from_secs(3_600),
        );
        filetime::set_file_mtime(path, old).unwrap();
    }

    #[test]
    fn staged_image_lands_in_the_session_directory_and_is_removed_with_it() {
        let base = tempfile::tempdir().unwrap();
        let session = storage::new_id();

        let staged = write_staged_image_in(base.path(), &session, b"image-bytes", "png").unwrap();
        let directory = session_directory_in(base.path(), &session);
        assert_eq!(staged.parent(), Some(directory.as_path()));
        assert_eq!(
            staged.extension().and_then(|value| value.to_str()),
            Some("png")
        );
        assert_eq!(fs::read(&staged).unwrap(), b"image-bytes");

        remove_session_in(base.path(), &session);
        assert!(!directory.exists(), "退出时暂存目录必须整目录删除");

        // 没落过盘时删除是无操作，也不会顺手创建目录。
        remove_session_in(base.path(), &session);
        assert!(!directory.exists());
    }

    #[test]
    fn sweep_reclaims_only_expired_session_directories() {
        let base = tempfile::tempdir().unwrap();
        let expired = session_directory_in(base.path(), &storage::new_id());
        let fresh = session_directory_in(base.path(), &storage::new_id());
        let foreign = base.path().join(format!("{SESSION_PREFIX}not-a-uuid"));
        let unrelated = base.path().join("lilith-artworks-notes");
        for path in [&expired, &fresh, &foreign, &unrelated] {
            fs::create_dir_all(path).unwrap();
            fs::write(path.join("staged.png"), b"jpeg").unwrap();
        }
        age_directory(&expired);
        age_directory(&foreign);
        age_directory(&unrelated);

        let sweep = sweep_stale_in(base.path(), storage::now_ms().unwrap());

        assert_eq!(sweep.reclaimed, 1);
        assert_eq!(sweep.failed, 0);
        // 超期的会话目录连同里面的暂存图片一起回收。
        assert!(!expired.exists());
        // 宽限期内、命名不符与手工放置的条目一律保留。
        assert!(fresh.exists());
        assert!(foreign.exists());
        assert!(unrelated.exists());

        // 目录不存在时按空处理，不报错。
        let empty = sweep_stale_in(&base.path().join("missing"), storage::now_ms().unwrap());
        assert_eq!(empty.reclaimed, 0);
        assert_eq!(empty.failed, 0);
    }
}
