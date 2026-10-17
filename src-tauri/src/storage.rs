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

/// 路径的命名空间分类。只做**词法**判断，不访问文件系统：读取侧校验要能在文件
/// 已被移动或删除、甚至从未存在的情况下照常完成。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathNamespace {
    /// 本地卷或普通挂载点。
    Local,
    /// UNC 网络位置：`\\server\share\...` 与 verbatim 形式 `\\?\UNC\server\share\...`。
    /// NAS 与共享目录是合法工作方式，[`PathNamespace::Network`] 本身不是错误。
    Network,
    /// Windows 设备命名空间：`\\.\`、verbatim 前缀 `\\?\`，以及 NT 对象路径
    /// `\Device\`。读取这类路径可绕过盘符与访问控制，因此一律拒绝。
    Device,
}

/// 分类路径命名空间。分隔符 `\` 与 `/` 视为等价，前缀匹配不区分大小写——
/// Windows 对两者都接受，只按 `/` 或原始大小写判断会被绕过。
pub(crate) fn path_namespace(value: &str) -> PathNamespace {
    let normalized = value.trim().replace('/', "\\");
    let lowered = normalized.to_lowercase();
    if lowered.starts_with(r"\\?\unc\") {
        return PathNamespace::Network;
    }
    if lowered.starts_with(r"\\?\")
        || lowered.starts_with(r"\\.\")
        || lowered.starts_with(r"\device\")
    {
        return PathNamespace::Device;
    }
    if lowered.starts_with(r"\\") {
        return PathNamespace::Network;
    }
    PathNamespace::Local
}

/// 无人值守流程（自动备份）可以安全使用的分支工作文件路径。
///
/// 自动备份与快速检查会对该路径直接取元数据、打开并整份复制，用户不在场也无法
/// 取消；网络位置会在这一步静默向远端主机发起认证，设备命名空间则是另一类对象。
/// 返回 `Some(原因)` 表示该分支不参与自动备份：跳过它，并把原因写进分支记录，
/// 与「未选择工作文件的分支不参与自动备份」沿用同一条口径。
pub(crate) fn unattended_source_path_rejection(value: &str) -> Option<String> {
    match path_namespace(value) {
        PathNamespace::Local => None,
        PathNamespace::Network => Some("工作文件位于网络位置，已跳过自动备份".into()),
        PathNamespace::Device => Some("工作文件位于设备命名空间，已跳过自动备份".into()),
    }
}

/// 拒绝设备命名空间路径；网络位置照常通过。用于用户在场、可自行判断的手动操作
/// （手动提交、在文件夹中显示）。
pub(crate) fn ensure_not_device_path(value: &str, label: &str) -> Result<(), String> {
    if path_namespace(value) == PathNamespace::Device {
        return Err(format!("{label}不能是设备命名空间路径"));
    }
    Ok(())
}

/// 分支工作文件路径的**读取侧**结构校验。
///
/// 该字段按设计保存仓库之外的绝对路径，因此不能复用
/// [`validate_repository_relative_path`]，而在打开别人的仓库时又必须挡住手工写入
/// 的路径。这里只判三件事：绝对路径、没有设备命名空间、不在仓库目录内部。
/// **不检查文件是否存在**——工作文件会被移动或删除，而打开仓库必须仍然成功；
/// 网络位置也放行（NAS 是合法用法），是否参与无人值守流程由
/// [`unattended_source_path_rejection`] 在使用处判断。空字符串表示该分支尚未
/// 选择工作文件，属于合法状态。
pub(crate) fn validate_branch_source_path(
    root: &Path,
    value: &str,
    label: &str,
) -> Result<(), String> {
    if value.trim().is_empty() {
        return Ok(());
    }
    if !Path::new(value).is_absolute() {
        return Err(format!("{label}必须是绝对路径"));
    }
    if path_namespace(value) == PathNamespace::Device {
        return Err(format!("{label}不能是设备命名空间路径"));
    }
    if is_inside_repository(root, value) {
        return Err(format!("{label}不能位于作品仓库内部"));
    }
    Ok(())
}

/// 词法判断 `value` 是否落在仓库目录内。不解析符号链接、不做 canonicalize：
/// 目标文件可能不存在，而读取侧校验必须在任何磁盘状态下都能给出结论。
fn is_inside_repository(root: &Path, value: &str) -> bool {
    let root = display_path(root);
    let root_key = comparable_path(root.trim_end_matches(['\\', '/']));
    if root_key.is_empty() {
        return false;
    }
    let value_key = comparable_path(value);
    value_key == root_key
        || value_key.starts_with(&format!("{root_key}{}", std::path::MAIN_SEPARATOR))
}

/// 把路径归一成可比较的形式：统一分隔符，Windows 上再统一大小写。
fn comparable_path(value: &str) -> String {
    let normalized = if std::path::MAIN_SEPARATOR == '\\' {
        value.replace('/', "\\")
    } else {
        value.replace('\\', "/")
    };
    if cfg!(windows) {
        normalized.to_lowercase()
    } else {
        normalized
    }
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

    #[test]
    fn classifies_local_network_and_device_namespaces() {
        assert_eq!(path_namespace(r"C:\work\artwork.psd"), PathNamespace::Local);
        assert_eq!(
            path_namespace(r"\\nas\share\artwork.psd"),
            PathNamespace::Network
        );
        assert_eq!(
            path_namespace(r"//nas/share/artwork.psd"),
            PathNamespace::Network
        );
        assert_eq!(
            path_namespace(r"\\?\UNC\nas\share\artwork.psd"),
            PathNamespace::Network
        );
        // verbatim 与设备前缀一律按设备命名空间处理：它们绕过盘符与访问控制，
        // 而写入侧早已把 `\\?\` 前缀剥掉（见 `display_path`），正常数据不会带它。
        assert_eq!(
            path_namespace(r"\\?\C:\work\artwork.psd"),
            PathNamespace::Device
        );
        assert_eq!(path_namespace(r"\\.\PhysicalDrive0"), PathNamespace::Device);
        assert_eq!(
            path_namespace(r"\\?\GLOBALROOT\Device\HarddiskVolumeShadowCopy1\x"),
            PathNamespace::Device
        );
        assert_eq!(
            path_namespace(r"\Device\HarddiskVolume1\x"),
            PathNamespace::Device
        );
    }

    #[test]
    fn unattended_paths_skip_network_and_device_namespaces() {
        assert!(unattended_source_path_rejection(r"C:\work\artwork.psd").is_none());
        let network = unattended_source_path_rejection(r"\\nas\share\artwork.psd").unwrap();
        assert!(network.contains("网络位置"), "{network}");
        let device = unattended_source_path_rejection(r"\\.\PhysicalDrive0").unwrap();
        assert!(device.contains("设备命名空间"), "{device}");
    }

    #[test]
    fn branch_source_path_structure_rejects_devices_and_repository_paths() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        std::fs::create_dir(&root).unwrap();

        // 空路径表示尚未选择工作文件，是合法状态。
        validate_branch_source_path(&root, "", "分支工作文件").unwrap();
        let outside = directory.path().join("artwork.psd");
        validate_branch_source_path(&root, &display_path(&outside), "分支工作文件").unwrap();
        // 网络位置放行：打开仓库不因 NAS 拒绝，是否参与自动备份由使用处决定。
        validate_branch_source_path(&root, r"\\nas\share\artwork.psd", "分支工作文件").unwrap();
        // 文件不存在也必须通过——工作文件可被移动或删除。
        assert!(!outside.exists());

        let relative =
            validate_branch_source_path(&root, "artwork.psd", "分支工作文件").unwrap_err();
        assert!(relative.contains("绝对路径"), "{relative}");
        let device =
            validate_branch_source_path(&root, r"\\.\PhysicalDrive0", "分支工作文件").unwrap_err();
        assert!(device.contains("设备命名空间"), "{device}");
        let inside = root.join("artworks").join("artwork.psd");
        let inside_error =
            validate_branch_source_path(&root, &display_path(&inside), "分支工作文件").unwrap_err();
        assert!(inside_error.contains("作品仓库内部"), "{inside_error}");
    }

    #[test]
    fn manual_paths_reject_only_the_device_namespace() {
        ensure_not_device_path(r"\\nas\share\artwork.psd", "工作文件").unwrap();
        let error = ensure_not_device_path(r"\\.\PhysicalDrive0", "工作文件").unwrap_err();
        assert!(error.contains("设备命名空间"), "{error}");
    }
}
