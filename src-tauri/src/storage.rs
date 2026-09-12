use std::{
    fs::File,
    io::Read,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, OpenFlags};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub(crate) const DATABASE_NAME: &str = "lilith-artworks.sqlite3";

pub(crate) fn database_path(root: &Path) -> PathBuf {
    root.join(DATABASE_NAME)
}

pub(crate) fn open(root: &Path) -> Result<Connection, String> {
    let connection =
        Connection::open_with_flags(database_path(root), OpenFlags::SQLITE_OPEN_READ_WRITE)
            .map_err(database_error)?;
    configure(&connection)?;
    Ok(connection)
}

pub(crate) fn configure(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = FULL;
             PRAGMA busy_timeout = 5000;",
        )
        .map_err(database_error)?;
    Ok(())
}

pub(crate) fn now_ms() -> Result<i64, String> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("系统时间无效：{error}"))?;
    i64::try_from(duration.as_millis()).map_err(|_| "系统时间超出范围".into())
}

pub(crate) fn new_id() -> String {
    Uuid::new_v4().to_string()
}

pub(crate) fn database_error(error: rusqlite::Error) -> String {
    format!("作品数据库操作失败：{error}")
}

pub(crate) fn display_path(path: &Path) -> String {
    let value = path.to_string_lossy();
    value.strip_prefix(r"\\?\").unwrap_or(&value).to_owned()
}

pub(crate) fn validate_title(value: &str, label: &str) -> Result<(), String> {
    let title = value.trim();
    if title.is_empty() {
        return Err(format!("{label}不能为空"));
    }
    if title.chars().count() > 160 {
        return Err(format!("{label}不能超过 160 个字符"));
    }
    Ok(())
}

pub(crate) fn validate_sha256(value: &str) -> Result<(), String> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("SHA-256 必须是 64 位十六进制字符串".into());
    }
    Ok(())
}

pub(crate) fn validate_uuid(value: &str, label: &str) -> Result<(), String> {
    Uuid::parse_str(value)
        .map(|_| ())
        .map_err(|_| format!("{label}不是有效 UUID"))
}

pub(crate) fn validate_repository_relative_path(value: &str) -> Result<(), String> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err("仓库存储路径无效".into());
    }
    Ok(())
}

pub(crate) fn verify_file_sha256(
    path: &Path,
    expected_sha256: &str,
    label: &str,
) -> Result<(), String> {
    validate_sha256(expected_sha256)?;
    let mut file = File::open(path).map_err(|error| format!("无法读取{label}：{error}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("无法校验{label}：{error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = hex::encode_upper(hasher.finalize());
    if actual.eq_ignore_ascii_case(expected_sha256) {
        Ok(())
    } else {
        Err(format!("{label}已损坏或被替换，SHA-256 与仓库记录不匹配"))
    }
}

pub(crate) fn normalize_source_path(
    root: &Path,
    source_path: &Path,
) -> Result<(String, String), String> {
    if !source_path.is_absolute() {
        return Err("分支工作文件必须使用绝对路径".into());
    }
    if !source_path.is_file() {
        return Err("分支工作文件不存在或不是普通文件".into());
    }
    let canonical = source_path
        .canonicalize()
        .map_err(|error| format!("无法访问分支工作文件：{error}"))?;
    let repository = root
        .canonicalize()
        .map_err(|error| format!("无法访问作品仓库：{error}"))?;
    if canonical.starts_with(&repository) {
        return Err("分支工作文件不能位于作品仓库内部".into());
    }
    let display = display_path(&canonical);
    let key = if cfg!(windows) {
        display.to_lowercase()
    } else {
        display.clone()
    };
    Ok((display, key))
}

pub(crate) fn ensure_outside_repository(
    root: &Path,
    path: &Path,
    label: &str,
) -> Result<(), String> {
    if !path.is_absolute() {
        return Err(format!("{label}必须使用绝对路径"));
    }
    let repository = root
        .canonicalize()
        .map_err(|error| format!("无法访问作品仓库：{error}"))?;
    let existing = path
        .ancestors()
        .find(|candidate| candidate.exists())
        .ok_or_else(|| format!("无法解析{label}"))?;
    let canonical_existing = existing
        .canonicalize()
        .map_err(|error| format!("无法访问{label}：{error}"))?;
    let suffix = path
        .strip_prefix(existing)
        .map_err(|_| format!("无法解析{label}"))?;
    let resolved = canonical_existing.join(suffix);
    if resolved.starts_with(repository) {
        return Err(format!("{label}不能位于作品仓库内部"));
    }
    Ok(())
}

pub(crate) fn relative_path(root: &Path, path: &Path) -> Result<String, String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| "存储文件不在作品仓库内")?;
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

pub(crate) fn resolve_path(root: &Path, relative: &str) -> Result<PathBuf, String> {
    validate_repository_relative_path(relative)?;
    Ok(root.join(relative))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_missing_database_does_not_create_placeholder() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        std::fs::create_dir(&root).unwrap();
        let database = database_path(&root);

        assert!(open(&root).is_err());
        assert!(!database.exists());
    }

    #[test]
    fn rejects_nonexistent_output_below_repository() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        std::fs::create_dir(&root).unwrap();

        let output = root.join("exports").join("signed.jpg");
        let error = ensure_outside_repository(&root, &output, "发布输出路径").unwrap_err();

        assert!(error.contains("不能位于作品仓库内部"), "{error}");
    }

    #[test]
    fn allows_nonexistent_output_beside_repository() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        std::fs::create_dir(&root).unwrap();

        let output = directory.path().join("exports").join("signed.jpg");

        ensure_outside_repository(&root, &output, "发布输出路径").unwrap();
    }

    #[test]
    fn detects_replaced_controlled_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("controlled.bin");
        std::fs::write(&path, b"replacement").unwrap();

        let error = verify_file_sha256(&path, &"A".repeat(64), "受控文件").unwrap_err();

        assert!(error.contains("已损坏或被替换"), "{error}");
    }

    #[test]
    fn rejects_parent_repository_path_components() {
        assert!(validate_repository_relative_path("../outside.bin").is_err());
        assert!(validate_repository_relative_path("artworks/inside.bin").is_ok());
    }
}
