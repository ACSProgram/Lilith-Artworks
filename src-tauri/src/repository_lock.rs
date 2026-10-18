//! 跨进程仓库 owner lock。
//!
//! 单实例插件只覆盖 GUI 入口；无头命令、维护命令与 GUI 仍可能同时写同一个仓库。
//! 这里用仓库根目录下的专用 SQLite 数据库做**跨进程独占标记**：`BEGIN IMMEDIATE`
//! 的写锁由 SQLite 持有，进程退出或崩溃时由操作系统关闭句柄、锁自动释放。它不
//! 替代 SQLite 的 WAL + `busy_timeout`（那只保证数据库层不损坏），而是把业务层
//! 「不支持同仓库多进程写入」这条边界钉死。
//!
//! **存活判定**：锁本身是唯一的存活证明。取得 `BEGIN IMMEDIATE` 即说明旧 owner
//! 已退出或崩溃，其元数据一律视为 stale 并直接覆盖；绝不根据 PID 或心跳强行回收
//! ——Windows 的 PID 会被复用，且 SQLite 写锁无法被安全地外部打断。owner 元数据
//! （PID、启动时间、版本、角色、操作、心跳）写在同目录的 JSON 侧车文件里，只用于
//! 冲突提示与事后诊断。
//!
//! 覆盖范围：GUI、headless 与维护入口**每次写操作**在进入仓库锁序前 acquire，
//! 操作结束释放；只读命令不取锁。全库校验（`scrub`）只探测锁是否被占用，并在
//! 写者持锁时明确拒绝。

use std::{
    fs,
    path::{Path, PathBuf},
    process,
    sync::{mpsc, Arc, Mutex, OnceLock},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};

/// 专用锁数据库的仓库内相对名。点号开头，避免出现在用户可见的文件列表里。
pub(crate) const LOCK_FILE_NAME: &str = ".lilith-write-lock.sqlite3";
/// owner 元数据侧车文件。它不是锁；锁始终是上面的 SQLite 数据库。
pub(crate) const OWNER_FILE_NAME: &str = ".lilith-write-owner.json";
/// 写者持锁时第二个写进程得到的稳定文案（与 0.2 的哨兵保持一致）。
pub(crate) const BUSY_MESSAGE: &str = "仓库正在被另一个进程写入；不支持同一仓库的多进程写入";
/// 内容级只读命令在写者持锁时的拒绝文案。
pub(crate) const CONTENT_READ_BUSY_MESSAGE: &str = "仓库正在被写入，请稍后重试";
/// 心跳间隔。仅用于诊断元数据；锁的存活不依赖它。
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

static PROCESS_STARTED_MS: OnceLock<i64> = OnceLock::new();

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// 在进程启动的最早时机记录墙钟时间，供 owner 元数据使用。未调用时回退为首次
/// 取得锁的时间（单测与嵌入场景）。
pub(crate) fn record_process_start() {
    let _ = PROCESS_STARTED_MS.set(now_ms());
}

fn process_started_ms() -> i64 {
    *PROCESS_STARTED_MS.get_or_init(now_ms)
}

/// 当前或最近一次写操作的 owner 元数据。字段只用于诊断与冲突提示。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RepositoryOwner {
    pub(crate) pid: u32,
    pub(crate) process_started_ms: i64,
    pub(crate) app_version: String,
    pub(crate) role: String,
    pub(crate) operation: String,
    pub(crate) run_id: String,
    pub(crate) heartbeat_ms: i64,
}

impl RepositoryOwner {
    fn describe(&self) -> String {
        format!(
            "pid={}, process_started_ms={}, version={}, role={}, operation={}, run_id={}, heartbeat_ms={}",
            self.pid,
            self.process_started_ms,
            self.app_version,
            self.role,
            self.operation,
            self.run_id,
            self.heartbeat_ms
        )
    }
}

#[derive(Debug)]
pub(crate) enum OwnerLockError {
    /// 另一个进程正持锁；附带可读到的 owner 元数据（通常存在）。
    /// 元数据用于日志与测试断言，GUI 错误映射只需要稳定的用户文案。
    #[allow(dead_code)]
    Busy(Option<RepositoryOwner>),
    Io(String),
}

impl OwnerLockError {
    pub(crate) fn user_message(&self) -> String {
        match self {
            Self::Busy(_) => BUSY_MESSAGE.to_owned(),
            Self::Io(message) => message.clone(),
        }
    }
}

/// 持有期间独占仓库写入的 owner lock；随连接一起在 `Drop` 时释放。
pub(crate) struct RepositoryOwnerLock {
    connection: Option<Connection>,
    owner_path: PathBuf,
    heartbeat_stop: Option<mpsc::Sender<()>>,
    heartbeat_thread: Option<thread::JoinHandle<()>>,
}

impl RepositoryOwnerLock {
    /// 对一次仓库写操作取得独占锁。调用方必须保证 `root` 已存在。
    pub(crate) fn acquire(
        root: &Path,
        role: &str,
        operation: &str,
    ) -> Result<Self, OwnerLockError> {
        if !root.is_dir() {
            return Err(OwnerLockError::Io("作品仓库目录不存在".to_owned()));
        }
        let connection = open_lock_database(root).map_err(OwnerLockError::Io)?;
        match connection.execute_batch("BEGIN IMMEDIATE;") {
            Ok(()) => {}
            Err(rusqlite::Error::SqliteFailure(inner, _))
                if inner.code == rusqlite::ErrorCode::DatabaseBusy =>
            {
                let owner = read_owner(root);
                match &owner {
                    Some(owner) => {
                        log::warn!("[owner] repository write lock busy: {}", owner.describe())
                    }
                    None => {
                        log::warn!("[owner] repository write lock busy: owner metadata unavailable")
                    }
                }
                return Err(OwnerLockError::Busy(owner));
            }
            Err(error) => {
                return Err(OwnerLockError::Io(format!("无法取得仓库写锁：{error}")));
            }
        }

        // 取得内核锁即可证明旧 owner 已失效（含未清理的强杀残留）。直接覆盖其
        // 元数据并留下一条可检索的记录，不做 PID 判定。
        if let Some(stale) = read_owner(root) {
            log::warn!("[owner] replacing stale owner: {}", stale.describe());
        }
        let record = RepositoryOwner {
            pid: process::id(),
            process_started_ms: process_started_ms(),
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            role: role.to_owned(),
            operation: operation.to_owned(),
            run_id: crate::app::diagnostics::run_id().to_owned(),
            heartbeat_ms: now_ms(),
        };
        let owner_path = root.join(OWNER_FILE_NAME);
        if let Err(error) = write_owner_file(&owner_path, &record) {
            // 元数据失败不影响锁本身；只留下告警。
            log::warn!("[owner] failed to write owner metadata: {error}");
        }
        log::debug!(
            "[owner] acquired: pid={}, role={}, operation={}",
            record.pid,
            record.role,
            record.operation
        );

        let record = Arc::new(Mutex::new(record));
        let (stop_tx, stop_rx) = mpsc::channel();
        let heartbeat_path = owner_path.clone();
        let heartbeat_thread = thread::spawn(move || loop {
            match stop_rx.recv_timeout(HEARTBEAT_INTERVAL) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let next = match record.lock() {
                        Ok(mut record) => {
                            record.heartbeat_ms = now_ms();
                            record.clone()
                        }
                        Err(_) => break,
                    };
                    if let Err(error) = write_owner_file(&heartbeat_path, &next) {
                        log::warn!("[owner] failed to refresh owner heartbeat: {error}");
                    }
                }
            }
        });

        Ok(Self {
            connection: Some(connection),
            owner_path,
            heartbeat_stop: Some(stop_tx),
            heartbeat_thread: Some(heartbeat_thread),
        })
    }
}

impl Drop for RepositoryOwnerLock {
    fn drop(&mut self) {
        if let Some(stop) = self.heartbeat_stop.take() {
            let _ = stop.send(());
        }
        if let Some(handle) = self.heartbeat_thread.take() {
            let _ = handle.join();
        }
        // 先移除元数据再释放锁：下一个 owner 取得锁时不会误读本进程的残留。
        let _ = fs::remove_file(&self.owner_path);
        if let Some(connection) = self.connection.take() {
            let _ = connection.execute_batch("ROLLBACK;");
        }
    }
}

/// 探测写锁是否被占用：`BEGIN IMMEDIATE` 成功即说明此刻没有写者，随后立即回滚，
/// 不把只读命令变成写者之间的互斥来源。
pub(crate) fn probe(root: &Path) -> Result<(), String> {
    if !root.is_dir() {
        return Ok(());
    }
    let path = root.join(LOCK_FILE_NAME);
    // 哨兵库还不存在，说明没有任何进程用过写锁。
    if !path.is_file() {
        return Ok(());
    }
    let connection = open_lock_database(root)?;
    match connection.execute_batch("BEGIN IMMEDIATE;") {
        Ok(()) => connection
            .execute_batch("ROLLBACK;")
            .map_err(|error| format!("无法释放仓库哨兵探测：{error}")),
        Err(rusqlite::Error::SqliteFailure(inner, _))
            if inner.code == rusqlite::ErrorCode::DatabaseBusy =>
        {
            if let Some(owner) = read_owner(root) {
                log::warn!(
                    "[owner] content read refused while a writer holds the lock: {}",
                    owner.describe()
                );
            }
            Err(CONTENT_READ_BUSY_MESSAGE.to_owned())
        }
        Err(other) => Err(format!("无法检查仓库写锁：{other}")),
    }
}

/// 仓库根目录下的锁库、锁日志与 owner 元数据属于进程级运行时文件，不进入灾备
/// 副本，也不参与未引用文件判定。
pub(crate) fn is_runtime_artifact(relative: &Path) -> bool {
    relative
        .parent()
        .is_some_and(|parent| parent.as_os_str().is_empty())
        && relative.file_name().is_some_and(|name| {
            let name = name.to_string_lossy();
            name == OWNER_FILE_NAME || name.starts_with(".lilith-write-lock")
        })
}

fn owner_path(root: &Path) -> PathBuf {
    root.join(OWNER_FILE_NAME)
}

fn read_owner(root: &Path) -> Option<RepositoryOwner> {
    let path = owner_path(root);
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn write_owner_file(path: &Path, owner: &RepositoryOwner) -> Result<(), String> {
    // 直接覆盖写入而不是临时文件重命名：心跳发生在仓库写操作期间，临时文件会
    // 出现在根目录并被灾备扫描看到。元数据仅是诊断信息，读到半截时可安全忽略。
    let bytes = serde_json::to_vec_pretty(owner)
        .map_err(|error| format!("无法序列化 owner 元数据：{error}"))?;
    fs::write(path, bytes).map_err(|error| format!("无法写入 owner 元数据：{error}"))
}

/// 打开哨兵锁数据库并配置为零等待：这里要的是「立刻判定是否被占用」，而不是排队。
/// 真实写入仍由主库自己的 `busy_timeout` 承担。
fn open_lock_database(root: &Path) -> Result<Connection, String> {
    let path = root.join(LOCK_FILE_NAME);
    let connection = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
    )
    .map_err(|error| format!("无法打开仓库哨兵锁：{error}"))?;
    connection
        .busy_timeout(Duration::ZERO)
        .map_err(|error| format!("无法配置仓库哨兵锁：{error}"))?;
    Ok(connection)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_writer_is_refused_and_release_restores_acquisition() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();

        let writer = RepositoryOwnerLock::acquire(root, "gui", "library:write").unwrap();
        let error = match RepositoryOwnerLock::acquire(root, "headless", "commit") {
            Ok(_) => panic!("第二个写者必须被拒绝"),
            Err(error) => error,
        };
        match error {
            OwnerLockError::Busy(owner) => {
                let owner = owner.expect("侧车元数据应可读");
                assert_eq!(owner.operation, "library:write");
                assert_eq!(owner.app_version, env!("CARGO_PKG_VERSION"));
            }
            OwnerLockError::Io(message) => panic!("应得到占用错误而不是 IO 错误：{message}"),
        }

        drop(writer);
        RepositoryOwnerLock::acquire(root, "headless", "commit").expect("释放后应能重新取得锁");
    }

    #[test]
    fn stale_owner_metadata_is_overwritten_once_the_kernel_lock_is_free() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let stale = RepositoryOwner {
            pid: 999_999,
            process_started_ms: 1,
            app_version: "0.0.0-stale".into(),
            role: "headless".into(),
            operation: "crashed-commit".into(),
            run_id: "stale".into(),
            heartbeat_ms: 1,
        };
        write_owner_file(&owner_path(root), &stale).unwrap();

        // 锁未被占用：stale 只表示上一次进程崩溃后残留的元数据，取得锁即覆盖。
        let guard = RepositoryOwnerLock::acquire(root, "gui", "settings:switch").unwrap();
        let current = read_owner(root).expect("取得锁后应写入新的 owner 元数据");
        assert_eq!(current.role, "gui");
        assert_eq!(current.operation, "settings:switch");
        assert_ne!(current.pid, stale.pid);

        drop(guard);
        assert!(!owner_path(root).exists());
    }

    #[test]
    fn content_read_probe_is_refused_while_a_writer_holds_the_lock() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        assert!(probe(root).is_ok());

        let writer = RepositoryOwnerLock::acquire(root, "headless", "compact").unwrap();
        let error = probe(root).unwrap_err();
        assert!(error.contains("仓库正在被写入"), "{error}");

        drop(writer);
        assert!(probe(root).is_ok());
    }

    #[test]
    fn runtime_artifacts_are_kept_out_of_repository_content() {
        assert!(is_runtime_artifact(Path::new(LOCK_FILE_NAME)));
        assert!(is_runtime_artifact(Path::new(&format!(
            "{LOCK_FILE_NAME}-journal"
        ))));
        assert!(is_runtime_artifact(Path::new(OWNER_FILE_NAME)));
        assert!(!is_runtime_artifact(Path::new("lilith-artworks.sqlite3")));
        assert!(!is_runtime_artifact(Path::new(
            "artworks/artwork-1/.lilith-write-owner.json"
        )));
    }
}
