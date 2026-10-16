//! 跨进程仓库哨兵锁。
//!
//! 单实例插件只覆盖 GUI 入口；无头命令与清理扫描可能脱离 GUI 单独运行，因此需要
//! 一个**跨进程**的独占标记，明确拒绝「同一仓库被两个进程同时写入」。它不替代
//! SQLite 的 WAL + `busy_timeout`（那只保证数据库层不损坏），而是把业务层
//! 「不支持同仓库多进程写入」这条边界钉死，让第二个进程拿到清晰错误而不是与前者
//! 在业务层竞态。
//!
//! 实现刻意选用一个专用 SQLite 数据库上的 `BEGIN IMMEDIATE`：写锁由 SQLite 持有，
//! **进程退出或崩溃时由操作系统关闭句柄、锁自动释放**。这避免了「用锁文件、进程被
//! 强杀后锁残留、后续命令被永久拒绝」这一类哨兵锁最常见的坑——本项目恰恰有大量
//! 强杀场景，锁文件方案在这里必然出错。
//!
//! 覆盖范围仅限无头入口：[`acquire`] 只对需要独占仓库的命令真正取锁，只读命令与
//! 仓库尚未建立的场景直接通过，GUI 路径完全不受影响。

use std::{path::Path, time::Duration};

use rusqlite::{Connection, OpenFlags};

/// 专用锁数据库的仓库内相对名。点号开头，避免出现在用户可见的文件列表里。
pub(crate) const LOCK_FILE_NAME: &str = ".lilith-write-lock.sqlite3";

/// 需要独占仓库的无头命令：会写仓库的命令，以及整仓灾备——后者只写目标目录，但复制期间
/// 必须排除源仓库的并发写入者，否则副本会是一个不一致的快照。
///
/// 只读命令（`list-tree`/`search`/`list-history`/`verify`/`scrub`/`scrub-board-dds`/
/// `scan-unreferenced`/`decode-authenticity`）与建库前的 `init-repository` 不在此列：
/// 前者可以并发读，后者的仓库目录尚不存在。
const WRITE_COMMANDS: [&str; 19] = [
    "create-artwork",
    "create-group",
    "move-node",
    "trash-node",
    "empty-trash",
    "create-branch",
    "delete-branch",
    "create-board",
    "import-board-images",
    "commit",
    "restore",
    "compact",
    "checkpoint",
    "cleanup",
    "cleanup-unreferenced",
    "repository-backup",
    "enter-publication",
    "publish",
    "cancel-publication",
];

/// 持有期间独占仓库写入的哨兵锁；随连接一起在 `Drop` 时释放。
pub(crate) struct RepositoryGuard {
    /// `None` 表示该命令无需独占（只读命令，或仓库目录尚不存在）。
    _connection: Option<Connection>,
}

impl RepositoryGuard {
    /// 对会写仓库的命令取得独占锁；其余情形直接通过。
    pub(crate) fn acquire(root: &Path, command: &str) -> Result<Self, String> {
        if !WRITE_COMMANDS.contains(&command) || !root.is_dir() {
            return Ok(Self { _connection: None });
        }
        let path = root.join(LOCK_FILE_NAME);
        let connection = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )
        .map_err(|error| format!("无法打开仓库哨兵锁：{error}"))?;
        // 不等待：这里要的是「立刻判定是否被占用」，而不是排队。真实写入仍由主库
        // 自己的 `busy_timeout` 承担。
        connection
            .busy_timeout(Duration::ZERO)
            .map_err(|error| format!("无法配置仓库哨兵锁：{error}"))?;
        connection
            .execute_batch("BEGIN IMMEDIATE;")
            .map_err(|error| match error {
                rusqlite::Error::SqliteFailure(inner, _)
                    if inner.code == rusqlite::ErrorCode::DatabaseBusy =>
                {
                    "仓库正在被另一个进程写入；不支持同一仓库的多进程写入".to_owned()
                }
                other => format!("无法取得仓库写锁：{other}"),
            })?;
        Ok(Self {
            _connection: Some(connection),
        })
    }
}
