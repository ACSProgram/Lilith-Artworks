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
/// 前者可以并发读，后者的仓库目录尚不存在。其中 `scrub` 另受
/// [`CONTENT_READ_COMMANDS`] 中的「写者持锁时明确拒绝」约束。
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

/// 只读、但要求**内容级一致性**的无头命令：写哨兵被持有时明确拒绝，提示用户稍后重试。
///
/// 目前只有 `scrub`（全库校验历史链）。它与精简真正并发时，只靠 SQLite 快照隔离与
/// 文件系统语义会遇到「节点在读取过程中被移除」，报出一个与原因无关的错误；不损坏
/// 数据，但对用户毫无指导意义。**只**给这一类命令加拒绝，`verify`、`list-*` 等轻量
/// 只读命令保持「写进程持锁期间照常作答」的既有承诺不变。
///
/// 这里只**探测**而不持有哨兵：全库校验可能跑很久，持有会把只读命令变成写者之间的
/// 互斥来源，使真正要写的进程反而失败。已知这留下一个「探测后写者才开始」的窗口，
/// 那是本方案明确接受的取舍——拒绝发生在能给出有意义提示的时刻，而不是把校验对象
/// 固定在一个正在变化的目标上。
const CONTENT_READ_COMMANDS: [&str; 1] = ["scrub"];

/// 持有期间独占仓库写入的哨兵锁；随连接一起在 `Drop` 时释放。
pub(crate) struct RepositoryGuard {
    /// `None` 表示该命令无需独占（只读命令，或仓库目录尚不存在）。
    _connection: Option<Connection>,
}

impl RepositoryGuard {
    /// 对会写仓库的命令取得独占锁；对内容级只读命令探测写者；其余情形直接通过。
    pub(crate) fn acquire(root: &Path, command: &str) -> Result<Self, String> {
        if CONTENT_READ_COMMANDS.contains(&command) {
            refuse_when_being_written(root)?;
            return Ok(Self { _connection: None });
        }
        if !WRITE_COMMANDS.contains(&command) || !root.is_dir() {
            return Ok(Self { _connection: None });
        }
        let connection = open_lock_database(root)?;
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

/// 探测写哨兵是否被占用：`BEGIN IMMEDIATE` 成功即说明此刻没有写者，随后立即回滚，
/// 不把只读命令变成写者之间的互斥来源。
fn refuse_when_being_written(root: &Path) -> Result<(), String> {
    if !root.is_dir() {
        return Ok(());
    }
    let path = root.join(LOCK_FILE_NAME);
    // 哨兵库还不存在，说明没有任何进程用过写锁（`Connection::open` 之外的写命令
    // 都会先创建它）。
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
            Err("仓库正在被写入，请稍后重试".to_owned())
        }
        Err(other) => Err(format!("无法检查仓库写锁：{other}")),
    }
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
    fn content_read_is_refused_while_a_writer_holds_the_sentinel() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        // 没有任何进程写过仓库时哨兵库还不存在，全库校验照常进行。
        assert!(RepositoryGuard::acquire(root, "scrub").is_ok());

        let writer = RepositoryGuard::acquire(root, "commit").unwrap();
        let error = match RepositoryGuard::acquire(root, "scrub") {
            Ok(_) => panic!("写者持锁时全库校验必须被拒绝"),
            Err(error) => error,
        };
        assert!(error.contains("仓库正在被写入"), "{error}");

        drop(writer);
        assert!(RepositoryGuard::acquire(root, "scrub").is_ok());
    }

    #[test]
    fn light_read_commands_still_answer_while_a_writer_holds_the_sentinel() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let _writer = RepositoryGuard::acquire(root, "compact").unwrap();

        // J1/J2 的承诺不变：轻量只读命令在写进程持锁期间照常作答。
        assert!(RepositoryGuard::acquire(root, "verify").is_ok());
        assert!(RepositoryGuard::acquire(root, "list-tree").is_ok());
        assert!(RepositoryGuard::acquire(root, "list-history").is_ok());
    }

    #[test]
    fn content_read_probe_does_not_take_the_sentinel() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let probe = RepositoryGuard::acquire(root, "scrub").unwrap();

        // 全库校验只探测不持有：写命令在它「运行期间」仍能取得写锁。
        let writer = RepositoryGuard::acquire(root, "commit");
        assert!(writer.is_ok());
        drop(probe);
    }
}
