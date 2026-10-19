use std::path::Path;

use rusqlite::{Connection, OptionalExtension, TransactionBehavior};

use crate::storage;

#[cfg(test)]
use std::cell::Cell;

pub(super) const REPOSITORY_FORMAT: &str = "lilith-artworks";
pub(super) const SCHEMA_VERSION: i64 = 5;

/// v2 的素材板三张表必须冻结为旧形状：`v1 → v2` 迁移会先建出这些表，再由
/// `v4 → v5` 重建。若复用更新后的建表 SQL，v1 仓库会在 v2 步骤直接建出 v5 表，
/// 随后的 v5 重建就找不到旧表。
const PIN_BOARD_TABLES_V2_SQL: &str = "
             CREATE TABLE pin_boards (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               artwork_id TEXT NOT NULL REFERENCES artworks(id) ON DELETE CASCADE,
               name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 120),
               sort_order INTEGER NOT NULL CHECK (sort_order >= 0),
               now_step INTEGER NOT NULL DEFAULT 0 CHECK (now_step >= 0),
               max_step INTEGER NOT NULL DEFAULT 0 CHECK (max_step >= 0),
               revision TEXT NOT NULL CHECK (length(revision) = 64),
               deleted_at INTEGER,
               created_ms INTEGER NOT NULL,
               updated_ms INTEGER NOT NULL
             );
             CREATE INDEX pin_boards_artwork
               ON pin_boards(artwork_id, deleted_at, sort_order, id);
             CREATE INDEX pin_boards_trash ON pin_boards(deleted_at, id);

             CREATE TABLE pin_board_images (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               board_id INTEGER NOT NULL REFERENCES pin_boards(id) ON DELETE CASCADE,
               file_path TEXT NOT NULL,
               width INTEGER NOT NULL CHECK (width > 0),
               height INTEGER NOT NULL CHECK (height > 0),
               created_ms INTEGER NOT NULL
             );
             CREATE INDEX pin_board_images_board ON pin_board_images(board_id, id);

             CREATE TABLE pin_board_history (
               board_id INTEGER NOT NULL REFERENCES pin_boards(id) ON DELETE CASCADE,
               image_id INTEGER NOT NULL REFERENCES pin_board_images(id) ON DELETE CASCADE,
               step INTEGER NOT NULL CHECK (step >= 0),
               deleted INTEGER NOT NULL CHECK (deleted IN (0, 1)),
               layer INTEGER NOT NULL CHECK (layer BETWEEN 0 AND 2),
               sort_order INTEGER NOT NULL CHECK (sort_order >= 0),
               transform_json TEXT NOT NULL,
               PRIMARY KEY (board_id, image_id, step)
             );";

/// v5 素材板：公共条目表 + 按 kind 的载荷子表。
///
/// - `pin_board_items`：与 kind 无关的公共列，`kind = 0` 位图、`kind = 1` 文字；
/// - `pin_board_bitmaps`：位图载荷，DDS 实体存于
///   `artworks/<artwork-id>/boards/<board-id>/<item-id>.dds`；
/// - `pin_board_texts`：文字载荷（固定字体族、度量结果随条目持久化）；
/// - `pin_board_history`：每个条目按 step 的变换历史，不承载载荷。
const PIN_BOARD_TABLES_SQL: &str = "
             CREATE TABLE pin_boards (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               artwork_id TEXT NOT NULL REFERENCES artworks(id) ON DELETE CASCADE,
               name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 120),
               sort_order INTEGER NOT NULL CHECK (sort_order >= 0),
               now_step INTEGER NOT NULL DEFAULT 0 CHECK (now_step >= 0),
               max_step INTEGER NOT NULL DEFAULT 0 CHECK (max_step >= 0),
               revision TEXT NOT NULL CHECK (length(revision) = 64),
               deleted_at INTEGER,
               created_ms INTEGER NOT NULL,
               updated_ms INTEGER NOT NULL
             );
             CREATE INDEX pin_boards_artwork
               ON pin_boards(artwork_id, deleted_at, sort_order, id);
             CREATE INDEX pin_boards_trash ON pin_boards(deleted_at, id);

             CREATE TABLE pin_board_items (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               board_id INTEGER NOT NULL REFERENCES pin_boards(id) ON DELETE CASCADE,
               kind INTEGER NOT NULL CHECK (kind IN (0, 1)),
               created_ms INTEGER NOT NULL
             );
             CREATE INDEX pin_board_items_board ON pin_board_items(board_id, id);

             CREATE TABLE pin_board_bitmaps (
               item_id INTEGER PRIMARY KEY REFERENCES pin_board_items(id) ON DELETE CASCADE,
               file_path TEXT NOT NULL,
               width INTEGER NOT NULL CHECK (width > 0),
               height INTEGER NOT NULL CHECK (height > 0)
             );

             CREATE TABLE pin_board_texts (
               item_id INTEGER PRIMARY KEY REFERENCES pin_board_items(id) ON DELETE CASCADE,
               text TEXT NOT NULL CHECK (length(text) BETWEEN 1 AND 5000),
               font_family TEXT NOT NULL CHECK (length(font_family) BETWEEN 1 AND 200),
               font_size REAL NOT NULL CHECK (font_size > 0 AND font_size <= 1000),
               color TEXT NOT NULL CHECK (length(color) = 7),
               align TEXT NOT NULL CHECK (align IN ('left', 'center', 'right')),
               measured_width REAL NOT NULL CHECK (measured_width > 0),
               measured_height REAL NOT NULL CHECK (measured_height > 0)
             );

             CREATE TABLE pin_board_history (
               board_id INTEGER NOT NULL REFERENCES pin_boards(id) ON DELETE CASCADE,
               item_id INTEGER NOT NULL REFERENCES pin_board_items(id) ON DELETE CASCADE,
               step INTEGER NOT NULL CHECK (step >= 0),
               deleted INTEGER NOT NULL CHECK (deleted IN (0, 1)),
               layer INTEGER NOT NULL CHECK (layer BETWEEN 0 AND 2),
               sort_order INTEGER NOT NULL CHECK (sort_order >= 0),
               transform_json TEXT NOT NULL,
               PRIMARY KEY (board_id, item_id, step)
             );";

const PATH_INDEXES_SQL: &str = "
             CREATE INDEX IF NOT EXISTS history_nodes_snapshot_path
               ON history_nodes(snapshot_path) WHERE snapshot_path IS NOT NULL;
             CREATE INDEX IF NOT EXISTS history_nodes_delta_path
               ON history_nodes(delta_path) WHERE delta_path IS NOT NULL;
             CREATE INDEX IF NOT EXISTS certification_records_stored_path
               ON certification_records(stored_path);
             CREATE INDEX IF NOT EXISTS certification_records_output_path
               ON certification_records(output_path);";

const REPOSITORY_BACKUP_RUNS_SQL: &str = "
             CREATE TABLE repository_backup_runs (
               id TEXT PRIMARY KEY,
               backup_path TEXT NOT NULL,
               repository_path TEXT NOT NULL,
               file_count INTEGER NOT NULL CHECK (file_count >= 0),
               total_bytes INTEGER NOT NULL CHECK (total_bytes >= 0),
               history_nodes INTEGER NOT NULL CHECK (history_nodes >= 0),
               final_artifacts INTEGER NOT NULL CHECK (final_artifacts >= 0),
               certification_records INTEGER NOT NULL CHECK (certification_records >= 0),
               created_ms INTEGER NOT NULL
             );
             CREATE INDEX repository_backup_runs_created
               ON repository_backup_runs(created_ms DESC, id);";

#[cfg(test)]
thread_local! {
    static INTEGRITY_CHECK_COUNT: Cell<usize> = const { Cell::new(0) };
}

pub(super) fn create(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(&format!(
            "BEGIN IMMEDIATE;
             CREATE TABLE repository_meta (
               key TEXT PRIMARY KEY,
               value TEXT NOT NULL
             );
               INSERT INTO repository_meta (key, value) VALUES
               ('format', 'lilith-artworks'),
               ('schema_version', '5');

             CREATE TABLE library_nodes (
               id TEXT PRIMARY KEY,
               parent_id TEXT REFERENCES library_nodes(id) ON DELETE CASCADE,
               kind TEXT NOT NULL CHECK (kind IN ('group', 'artwork')),
               title TEXT NOT NULL CHECK (length(title) BETWEEN 1 AND 160),
               position INTEGER NOT NULL CHECK (position >= 0),
               created_ms INTEGER NOT NULL,
               updated_ms INTEGER NOT NULL,
               trashed_ms INTEGER,
               trash_root_id TEXT,
               restore_parent_id TEXT,
               restore_position INTEGER
             );
             CREATE INDEX library_nodes_parent_position
               ON library_nodes(parent_id, position, id);
             CREATE INDEX library_nodes_trash
               ON library_nodes(trashed_ms, trash_root_id);

             CREATE TABLE artworks (
               id TEXT PRIMARY KEY REFERENCES library_nodes(id) ON DELETE CASCADE,
               description TEXT NOT NULL DEFAULT '',
               created_ms INTEGER NOT NULL,
               updated_ms INTEGER NOT NULL
             );

             CREATE TABLE branches (
               id TEXT PRIMARY KEY,
               artwork_id TEXT NOT NULL REFERENCES artworks(id) ON DELETE CASCADE,
               title TEXT NOT NULL CHECK (length(title) BETWEEN 1 AND 160),
               source_path TEXT NOT NULL,
               source_path_key TEXT NOT NULL,
               head_history_id TEXT REFERENCES history_nodes(id) ON DELETE SET NULL,
               created_from_history_id TEXT REFERENCES history_nodes(id) ON DELETE SET NULL,
               backup_enabled INTEGER NOT NULL DEFAULT 1 CHECK (backup_enabled IN (0, 1)),
               backup_interval_minutes INTEGER NOT NULL DEFAULT 5 CHECK (backup_interval_minutes BETWEEN 1 AND 10080),
               last_check_ms INTEGER,
               last_success_ms INTEGER,
               last_error TEXT,
               consecutive_backup_failures INTEGER NOT NULL DEFAULT 0 CHECK (consecutive_backup_failures >= 0),
               backup_retry_at_ms INTEGER,
               backup_disable_notice_pending INTEGER NOT NULL DEFAULT 0 CHECK (backup_disable_notice_pending IN (0, 1)),
               backup_quick_enabled INTEGER NOT NULL DEFAULT 0 CHECK (backup_quick_enabled IN (0, 1)),
               last_source_size INTEGER,
               last_source_modified_ms INTEGER,
               created_ms INTEGER NOT NULL,
               updated_ms INTEGER NOT NULL,
               UNIQUE (artwork_id, source_path_key)
             );
             CREATE INDEX branches_artwork_created ON branches(artwork_id, created_ms, id);

             CREATE TABLE history_nodes (
               id TEXT PRIMARY KEY,
               artwork_id TEXT NOT NULL REFERENCES artworks(id) ON DELETE CASCADE,
               created_on_branch_id TEXT NOT NULL REFERENCES branches(id) ON DELETE RESTRICT,
               parent_id TEXT REFERENCES history_nodes(id) ON DELETE CASCADE,
               title TEXT NOT NULL CHECK (length(title) BETWEEN 1 AND 160),
               note TEXT NOT NULL DEFAULT '' CHECK (length(note) <= 500),
               commit_kind TEXT NOT NULL DEFAULT 'manual' CHECK (commit_kind IN ('manual', 'automatic')),
               is_checkpoint INTEGER NOT NULL DEFAULT 0 CHECK (is_checkpoint IN (0, 1)),
               created_ms INTEGER NOT NULL,
               logical_size INTEGER NOT NULL CHECK (logical_size >= 0),
               chunk_file_size INTEGER NOT NULL CHECK (chunk_file_size >= 0),
               sha256 TEXT NOT NULL CHECK (length(sha256) = 64),
               chunk_count INTEGER NOT NULL CHECK (chunk_count >= 0),
               snapshot_path TEXT,
               delta_path TEXT,
               thumbnail BLOB,
               thumbnail_version INTEGER,
               verified_ms INTEGER,
               verify_error TEXT,
               CHECK (snapshot_path IS NOT NULL OR delta_path IS NOT NULL)
             );
             CREATE INDEX history_nodes_parent ON history_nodes(parent_id, created_ms, id);
             CREATE INDEX history_nodes_artwork_created ON history_nodes(artwork_id, created_ms, id);

             CREATE TABLE history_edges (
               child_history_id TEXT PRIMARY KEY REFERENCES history_nodes(id) ON DELETE CASCADE,
               parent_history_id TEXT NOT NULL REFERENCES history_nodes(id) ON DELETE CASCADE,
               delta_path TEXT NOT NULL,
               delta_size INTEGER NOT NULL CHECK (delta_size >= 0)
             );
             CREATE INDEX history_edges_parent ON history_edges(parent_history_id, child_history_id);
             INSERT INTO history_edges (child_history_id, parent_history_id, delta_path, delta_size)
             SELECT id, parent_id, delta_path, 0 FROM history_nodes
             WHERE parent_id IS NOT NULL AND delta_path IS NOT NULL;

             CREATE TABLE final_artifacts (
               id TEXT PRIMARY KEY,
               branch_id TEXT NOT NULL UNIQUE REFERENCES branches(id) ON DELETE CASCADE,
               history_id TEXT NOT NULL REFERENCES history_nodes(id) ON DELETE RESTRICT,
               source_path TEXT NOT NULL,
               source_sha256 TEXT NOT NULL CHECK (length(source_sha256) = 64),
               media_type TEXT NOT NULL,
               byte_size INTEGER NOT NULL CHECK (byte_size >= 0),
               created_ms INTEGER NOT NULL
             );

             CREATE TABLE certification_configs (
               branch_id TEXT PRIMARY KEY REFERENCES branches(id) ON DELETE CASCADE,
               title TEXT NOT NULL DEFAULT '',
               creator TEXT NOT NULL DEFAULT '',
               rights_statement TEXT NOT NULL DEFAULT '',
               authentication_content TEXT NOT NULL DEFAULT '',
               trustmark_enabled INTEGER NOT NULL DEFAULT 1 CHECK (trustmark_enabled IN (0, 1)),
               certificate_path TEXT NOT NULL DEFAULT '',
               signing_algorithm TEXT NOT NULL DEFAULT 'es256',
               timestamp_url TEXT,
               jpeg_quality INTEGER NOT NULL DEFAULT 92 CHECK (jpeg_quality BETWEEN 1 AND 100),
               background_color TEXT NOT NULL DEFAULT '#FFFFFF',
               watermark_strength REAL NOT NULL DEFAULT 1.0,
               additional_regions_json TEXT NOT NULL DEFAULT '[]',
               updated_ms INTEGER NOT NULL
             );

             CREATE TABLE certification_records (
               id TEXT PRIMARY KEY,
               final_artifact_id TEXT NOT NULL REFERENCES final_artifacts(id) ON DELETE CASCADE,
               branch_id TEXT NOT NULL REFERENCES branches(id) ON DELETE CASCADE,
               history_id TEXT NOT NULL REFERENCES history_nodes(id) ON DELETE RESTRICT,
               watermark_id TEXT CHECK (watermark_id IS NULL OR length(watermark_id) = 40),
               trustmark_enabled INTEGER NOT NULL CHECK (trustmark_enabled IN (0, 1)),
               output_path TEXT NOT NULL,
               stored_path TEXT NOT NULL,
               output_sha256 TEXT NOT NULL CHECK (length(output_sha256) = 64),
               output_bytes INTEGER NOT NULL CHECK (output_bytes >= 0),
               title TEXT NOT NULL,
               creator TEXT NOT NULL,
               rights_statement TEXT NOT NULL,
               authentication_content TEXT NOT NULL,
               regions_json TEXT NOT NULL DEFAULT '[]',
               c2pa_manifest_label TEXT,
               c2pa_manifest_json TEXT,
               validation_state TEXT,
               created_ms INTEGER NOT NULL
             );
             CREATE INDEX certification_records_watermark
               ON certification_records(watermark_id, created_ms DESC);
             CREATE INDEX certification_records_branch
               ON certification_records(branch_id, created_ms DESC);

             {PIN_BOARD_TABLES_SQL}

             CREATE TABLE pending_file_cleanup (
               id TEXT PRIMARY KEY,
               path_kind TEXT NOT NULL CHECK (path_kind IN ('repository_file', 'repository_directory', 'external_file')),
               path TEXT NOT NULL,
               expected_sha256 TEXT CHECK (expected_sha256 IS NULL OR length(expected_sha256) = 64),
               reason TEXT NOT NULL,
               created_ms INTEGER NOT NULL,
               last_attempt_ms INTEGER,
               last_error TEXT,
               UNIQUE (path_kind, path)
             );
             CREATE INDEX pending_file_cleanup_created
               ON pending_file_cleanup(created_ms, id);

             {PATH_INDEXES_SQL}

             {REPOSITORY_BACKUP_RUNS_SQL}

             CREATE TRIGGER artwork_nodes_only
             BEFORE INSERT ON artworks
             WHEN (SELECT kind FROM library_nodes WHERE id = NEW.id) <> 'artwork'
             BEGIN
               SELECT RAISE(ABORT, 'artwork metadata requires an artwork node');
             END;

             CREATE TRIGGER groups_cannot_have_artwork_metadata
             BEFORE UPDATE OF kind ON library_nodes
             WHEN NEW.kind = 'group' AND EXISTS (SELECT 1 FROM artworks WHERE id = NEW.id)
             BEGIN
               SELECT RAISE(ABORT, 'artwork node kind cannot be changed');
             END;

             CREATE TRIGGER artwork_nodes_are_leaves_on_insert
             BEFORE INSERT ON library_nodes
             WHEN NEW.parent_id IS NOT NULL
               AND (SELECT kind FROM library_nodes WHERE id = NEW.parent_id) = 'artwork'
             BEGIN
               SELECT RAISE(ABORT, 'artwork nodes cannot contain children');
             END;

             CREATE TRIGGER artwork_nodes_are_leaves_on_move
             BEFORE UPDATE OF parent_id ON library_nodes
             WHEN NEW.parent_id IS NOT NULL
               AND (SELECT kind FROM library_nodes WHERE id = NEW.parent_id) = 'artwork'
             BEGIN
               SELECT RAISE(ABORT, 'artwork nodes cannot contain children');
             END;

             CREATE TRIGGER branch_head_matches_artwork
             BEFORE INSERT ON branches
             WHEN NEW.head_history_id IS NOT NULL AND NOT EXISTS (
               SELECT 1 FROM history_nodes
               WHERE id = NEW.head_history_id AND artwork_id = NEW.artwork_id
             )
             BEGIN
               SELECT RAISE(ABORT, 'branch head belongs to another artwork');
             END;

             CREATE TRIGGER branch_head_update_matches_artwork
             BEFORE UPDATE OF head_history_id ON branches
             WHEN NEW.head_history_id IS NOT NULL AND NOT EXISTS (
               SELECT 1 FROM history_nodes
               WHERE id = NEW.head_history_id AND artwork_id = NEW.artwork_id
             )
             BEGIN
               SELECT RAISE(ABORT, 'branch head belongs to another artwork');
             END;

             CREATE TRIGGER fork_origin_matches_artwork
             BEFORE INSERT ON branches
             WHEN NEW.created_from_history_id IS NOT NULL AND NOT EXISTS (
               SELECT 1 FROM history_nodes
               WHERE id = NEW.created_from_history_id AND artwork_id = NEW.artwork_id
             )
             BEGIN
               SELECT RAISE(ABORT, 'fork origin belongs to another artwork');
             END;

             CREATE TRIGGER fork_origin_update_matches_artwork
             BEFORE UPDATE OF created_from_history_id ON branches
             WHEN NEW.created_from_history_id IS NOT NULL AND NOT EXISTS (
               SELECT 1 FROM history_nodes
               WHERE id = NEW.created_from_history_id AND artwork_id = NEW.artwork_id
             )
             BEGIN
               SELECT RAISE(ABORT, 'fork origin belongs to another artwork');
             END;

             COMMIT;",
        ))
        .map_err(|error| format!("无法创建作品数据库结构：{error}"))
}

pub(super) fn validate(connection: &Connection) -> Result<(), String> {
    #[cfg(test)]
    INTEGRITY_CHECK_COUNT.with(|count| count.set(count.get() + 1));

    let integrity: String = connection
        .query_row("PRAGMA integrity_check(1)", [], |row| row.get(0))
        .map_err(|error| format!("无法检查作品数据库完整性：{error}"))?;
    if integrity != "ok" {
        return Err(format!("作品数据库完整性检查失败：{integrity}"));
    }
    validate_current_version(repository_version(connection)?)
}

pub(super) fn validate_current(connection: &Connection) -> Result<(), String> {
    validate_current_version(repository_version(connection)?)
}

pub(super) fn validate_repository_semantics(
    connection: &Connection,
    root: &Path,
) -> Result<(), String> {
    let mut foreign_keys = connection
        .prepare("PRAGMA foreign_key_check")
        .map_err(storage::database_error)?;
    if foreign_keys.exists([]).map_err(storage::database_error)? {
        return Err("作品数据库外键完整性检查失败".into());
    }
    drop(foreign_keys);

    let mut ids = connection
        .prepare(
            "SELECT 'library_nodes.id', id FROM library_nodes
             UNION ALL SELECT 'artworks.id', id FROM artworks
             UNION ALL SELECT 'branches.id', id FROM branches
             UNION ALL SELECT 'history_nodes.id', id FROM history_nodes
             UNION ALL SELECT 'final_artifacts.id', id FROM final_artifacts
             UNION ALL SELECT 'certification_records.id', id FROM certification_records
             UNION ALL SELECT 'pending_file_cleanup.id', id FROM pending_file_cleanup
             UNION ALL SELECT 'repository_backup_runs.id', id FROM repository_backup_runs",
        )
        .map_err(storage::database_error)?;
    let id_rows = ids
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(storage::database_error)?;
    for row in id_rows {
        let (label, id) = row.map_err(storage::database_error)?;
        storage::validate_uuid(&id, &label)?;
    }
    drop(ids);

    // 空串在这些列上是「没有文件」的记账值，不是待校验的路径：检查点父节点名下的
    // 边按设计登记 `delta_path = ''`（取消检查点时重建），兼容列 `delta_path` 也
    // 可能在节点持有 snapshot 时被清空。按路径校验会把这类合法仓库挡在门外。
    // `snapshot_path` 没有这种记账语义（读取侧把非 NULL 一律当作可用快照），所以仍
    // 要求它非 NULL，不放过空串。
    let mut paths = connection
        .prepare(
            "SELECT 'history_nodes.snapshot_path', snapshot_path FROM history_nodes
               WHERE snapshot_path IS NOT NULL
             UNION ALL SELECT 'history_nodes.delta_path', delta_path FROM history_nodes
               WHERE delta_path IS NOT NULL AND TRIM(delta_path) <> ''
             UNION ALL SELECT 'history_edges.delta_path', delta_path FROM history_edges
               WHERE TRIM(delta_path) <> ''
             UNION ALL SELECT 'final_artifacts.source_path', source_path FROM final_artifacts
             UNION ALL SELECT 'certification_records.stored_path', stored_path
               FROM certification_records
             UNION ALL SELECT 'pin_board_bitmaps.file_path', file_path
               FROM pin_board_bitmaps
             UNION ALL SELECT 'pending_file_cleanup.path', path FROM pending_file_cleanup
               WHERE path_kind <> 'external_file'",
        )
        .map_err(storage::database_error)?;
    let path_rows = paths
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(storage::database_error)?;
    for row in path_rows {
        let (label, path) = row.map_err(storage::database_error)?;
        storage::validate_repository_relative_path(&path)
            .map_err(|error| format!("{label} 无效：{error}"))?;
    }
    drop(paths);

    // 分支工作文件按设计保存仓库之外的绝对路径，因此单独做结构校验：绝对路径、
    // 无设备命名空间、不在仓库目录内，但不检查文件是否存在，也不拒绝网络位置。
    // 读取侧必须挡住的是「打开别人的仓库后，无人值守流程直接对这条路径取元数据
    // 或整份复制」——这些使用点另外按 `unattended_source_path_rejection` 跳过。
    let mut branch_paths = connection
        .prepare(
            "SELECT 'branches.source_path', source_path FROM branches
             UNION ALL SELECT 'branches.source_path_key', source_path_key FROM branches",
        )
        .map_err(storage::database_error)?;
    let branch_rows = branch_paths
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(storage::database_error)?;
    for row in branch_rows {
        let (label, path) = row.map_err(storage::database_error)?;
        storage::validate_branch_source_path(root, &path, &label)?;
    }
    drop(branch_paths);

    let mut hashes = connection
        .prepare(
            "SELECT 'history_nodes.sha256', sha256 FROM history_nodes
             UNION ALL SELECT 'final_artifacts.source_sha256', source_sha256 FROM final_artifacts
             UNION ALL SELECT 'certification_records.output_sha256', output_sha256
               FROM certification_records
             UNION ALL SELECT 'pending_file_cleanup.expected_sha256', expected_sha256
               FROM pending_file_cleanup WHERE expected_sha256 IS NOT NULL",
        )
        .map_err(storage::database_error)?;
    let hash_rows = hashes
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(storage::database_error)?;
    for row in hash_rows {
        let (label, hash) = row.map_err(storage::database_error)?;
        storage::validate_sha256(&hash).map_err(|error| format!("{label} 无效：{error}"))?;
    }
    Ok(())
}

fn repository_version(connection: &Connection) -> Result<i64, String> {
    let format: Option<String> = connection
        .query_row(
            "SELECT value FROM repository_meta WHERE key = 'format'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| format!("无法读取仓库格式：{error}"))?;
    if format.as_deref() != Some(REPOSITORY_FORMAT) {
        return Err("所选数据库不是 Lilith Artworks 仓库".into());
    }
    let version: Option<i64> = connection
        .query_row(
            "SELECT CAST(value AS INTEGER) FROM repository_meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| format!("无法读取仓库版本：{error}"))?;
    version.ok_or_else(|| "作品仓库版本未知".to_owned())
}

fn validate_current_version(version: i64) -> Result<(), String> {
    if version > SCHEMA_VERSION {
        return Err(newer_version_error(version));
    }
    if version != SCHEMA_VERSION {
        return Err(format!("作品仓库版本不受支持：{version}"));
    }
    Ok(())
}

/// 打开「比自己新」的仓库时统一用它拒绝：迁移与校验两条路径共用同一句文案，
/// 避免旧程序改写新结构。
fn newer_version_error(version: i64) -> String {
    format!(
        "作品仓库由更新版本的 Lilith Artworks 创建（schema v{version}，当前版本支持到 \
         v{SCHEMA_VERSION}），已拒绝打开以避免损坏仓库"
    )
}

/// 打开既有仓库时执行 schema 迁移。迁移不支持回退；高于当前版本的仓库直接
/// 拒绝打开。v1 → v2 追加素材板三张表；v2 → v3 为分支追加快速自动备份开关与
/// 上次全量检查记录的源文件元数据；v3 → v4 为分支追加空闲链路校验状态；
/// v4 → v5 是 0.3 唯一一次表重建：画板条目改为多态结构，`history_nodes`
/// 追加缩略图与节点级校验列，删除 0.2 遗留的 `branches` 校验三列，并建立路径
/// 索引与 `repository_backup_runs`。
pub(super) fn migrate(connection: &mut Connection) -> Result<(), String> {
    let version = repository_version(connection)?;
    if version == SCHEMA_VERSION {
        return Ok(());
    }
    if version > SCHEMA_VERSION {
        return Err(newer_version_error(version));
    }
    if version < 1 {
        return Err(format!("作品仓库版本不受支持：{version}"));
    }
    if version < 2 {
        connection
            .execute_batch(&format!(
                "BEGIN IMMEDIATE;
                 {PIN_BOARD_TABLES_V2_SQL}
                 UPDATE repository_meta SET value = '2' WHERE key = 'schema_version';
                 COMMIT;"
            ))
            .map_err(|error| format!("无法迁移作品数据库结构：{error}"))?;
    }
    if version < 3 {
        connection
            .execute_batch(
                "BEGIN IMMEDIATE;
                 ALTER TABLE branches ADD COLUMN backup_quick_enabled
                   INTEGER NOT NULL DEFAULT 0 CHECK (backup_quick_enabled IN (0, 1));
                 ALTER TABLE branches ADD COLUMN last_source_size INTEGER;
                 ALTER TABLE branches ADD COLUMN last_source_modified_ms INTEGER;
                 UPDATE repository_meta SET value = '3' WHERE key = 'schema_version';
                 COMMIT;",
            )
            .map_err(|error| format!("无法迁移作品数据库结构：{error}"))?;
    }
    if version < 4 {
        connection
            .execute_batch(
                "BEGIN IMMEDIATE;
                 ALTER TABLE branches ADD COLUMN verified_history_id TEXT;
                 ALTER TABLE branches ADD COLUMN verified_ms INTEGER;
                 ALTER TABLE branches ADD COLUMN verify_error TEXT;
                 UPDATE repository_meta SET value = '4' WHERE key = 'schema_version';
                 COMMIT;",
            )
            .map_err(|error| format!("无法迁移作品数据库结构：{error}"))?;
    }
    if version < 5 {
        migrate_v4_to_v5(connection)?;
    }
    Ok(())
}

/// v4 → v5：在 `PRAGMA foreign_keys = OFF` 下重建画板表并一次性补齐 0.3 的
/// 持久结构。外键检查在事务内、提交前执行，任一步失败整笔回滚；提交后恢复
/// `foreign_keys = ON`。
fn migrate_v4_to_v5(connection: &mut Connection) -> Result<(), String> {
    connection
        .execute_batch("PRAGMA foreign_keys = OFF;")
        .map_err(|error| format!("无法暂停外键检查进行迁移：{error}"))?;
    let migration = (|| -> Result<(), String> {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage::database_error)?;
        transaction
            .execute_batch(
                "ALTER TABLE pin_board_history RENAME TO pin_board_history_v4;
                 ALTER TABLE pin_board_images RENAME TO pin_board_images_v4;

                 CREATE TABLE pin_board_items (
                   id INTEGER PRIMARY KEY AUTOINCREMENT,
                   board_id INTEGER NOT NULL REFERENCES pin_boards(id) ON DELETE CASCADE,
                   kind INTEGER NOT NULL CHECK (kind IN (0, 1)),
                   created_ms INTEGER NOT NULL
                 );
                 CREATE INDEX pin_board_items_board ON pin_board_items(board_id, id);

                 CREATE TABLE pin_board_bitmaps (
                   item_id INTEGER PRIMARY KEY REFERENCES pin_board_items(id) ON DELETE CASCADE,
                   file_path TEXT NOT NULL,
                   width INTEGER NOT NULL CHECK (width > 0),
                   height INTEGER NOT NULL CHECK (height > 0)
                 );

                 CREATE TABLE pin_board_texts (
                   item_id INTEGER PRIMARY KEY REFERENCES pin_board_items(id) ON DELETE CASCADE,
                   text TEXT NOT NULL CHECK (length(text) BETWEEN 1 AND 5000),
                   font_family TEXT NOT NULL CHECK (length(font_family) BETWEEN 1 AND 200),
                   font_size REAL NOT NULL CHECK (font_size > 0 AND font_size <= 1000),
                   color TEXT NOT NULL CHECK (length(color) = 7),
                   align TEXT NOT NULL CHECK (align IN ('left', 'center', 'right')),
                   measured_width REAL NOT NULL CHECK (measured_width > 0),
                   measured_height REAL NOT NULL CHECK (measured_height > 0)
                 );

                 CREATE TABLE pin_board_history (
                   board_id INTEGER NOT NULL REFERENCES pin_boards(id) ON DELETE CASCADE,
                   item_id INTEGER NOT NULL REFERENCES pin_board_items(id) ON DELETE CASCADE,
                   step INTEGER NOT NULL CHECK (step >= 0),
                   deleted INTEGER NOT NULL CHECK (deleted IN (0, 1)),
                   layer INTEGER NOT NULL CHECK (layer BETWEEN 0 AND 2),
                   sort_order INTEGER NOT NULL CHECK (sort_order >= 0),
                   transform_json TEXT NOT NULL,
                   PRIMARY KEY (board_id, item_id, step)
                 );

                 INSERT INTO pin_board_items (id, board_id, kind, created_ms)
                   SELECT id, board_id, 0, created_ms FROM pin_board_images_v4;
                 INSERT INTO pin_board_bitmaps (item_id, file_path, width, height)
                   SELECT id, file_path, width, height FROM pin_board_images_v4;
                 INSERT INTO pin_board_history
                   (board_id, item_id, step, deleted, layer, sort_order, transform_json)
                   SELECT board_id, image_id, step, deleted, layer, sort_order, transform_json
                     FROM pin_board_history_v4;

                 DROP TABLE pin_board_history_v4;
                 DROP TABLE pin_board_images_v4;

                 ALTER TABLE history_nodes ADD COLUMN thumbnail BLOB;
                 ALTER TABLE history_nodes ADD COLUMN thumbnail_version INTEGER;
                 ALTER TABLE history_nodes ADD COLUMN verified_ms INTEGER;
                 ALTER TABLE history_nodes ADD COLUMN verify_error TEXT;

                 ALTER TABLE branches DROP COLUMN verified_history_id;
                 ALTER TABLE branches DROP COLUMN verified_ms;
                 ALTER TABLE branches DROP COLUMN verify_error;",
            )
            .map_err(|error| format!("无法重建素材板结构：{error}"))?;
        transaction
            .execute_batch(PATH_INDEXES_SQL)
            .map_err(|error| format!("无法建立路径索引：{error}"))?;
        transaction
            .execute_batch(REPOSITORY_BACKUP_RUNS_SQL)
            .map_err(|error| format!("无法建立整仓备份记录表：{error}"))?;
        {
            let mut foreign_keys = transaction
                .prepare("PRAGMA foreign_key_check")
                .map_err(storage::database_error)?;
            if foreign_keys.exists([]).map_err(storage::database_error)? {
                return Err("v5 迁移后的外键完整性检查失败".into());
            }
        }
        transaction
            .execute(
                "UPDATE repository_meta SET value = '5' WHERE key = 'schema_version'",
                [],
            )
            .map_err(storage::database_error)?;
        transaction.commit().map_err(storage::database_error)
    })();
    let restored = connection
        .execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(|error| format!("无法恢复外键检查：{error}"));
    migration.and(restored)
}

#[cfg(test)]
pub(crate) fn take_integrity_check_count() -> usize {
    INTEGRITY_CHECK_COUNT.with(|count| count.replace(0))
}
