use rusqlite::{Connection, OptionalExtension};

use crate::storage;

#[cfg(test)]
use std::cell::Cell;

pub(super) const REPOSITORY_FORMAT: &str = "lilith-artworks";
pub(super) const SCHEMA_VERSION: i64 = 3;

/// 素材板（pin-board）三张表。v2 起：
/// - `pin_boards`：按 Artwork 平铺一层画板，`deleted_at` 为回收站软删除；
/// - `pin_board_images`：图片记录，DDS 实体存于
///   `artworks/<artwork-id>/boards/<board-id>/<image-id>.dds`；
/// - `pin_board_history`：每张图片按 step 的历史状态（撤销/恢复持久化），
///   变换（points/uv）以 JSON 存储。
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
               ('schema_version', '3');

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

pub(super) fn validate_repository_semantics(connection: &Connection) -> Result<(), String> {
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
             UNION ALL SELECT 'pending_file_cleanup.id', id FROM pending_file_cleanup",
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

    let mut paths = connection
        .prepare(
            "SELECT 'history_nodes.snapshot_path', snapshot_path FROM history_nodes
               WHERE snapshot_path IS NOT NULL
             UNION ALL SELECT 'history_nodes.delta_path', delta_path FROM history_nodes
               WHERE delta_path IS NOT NULL
             UNION ALL SELECT 'history_edges.delta_path', delta_path FROM history_edges
             UNION ALL SELECT 'final_artifacts.source_path', source_path FROM final_artifacts
             UNION ALL SELECT 'certification_records.stored_path', stored_path
               FROM certification_records
             UNION ALL SELECT 'pin_board_images.file_path', file_path
               FROM pin_board_images
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
    if version != SCHEMA_VERSION {
        return Err(format!("作品仓库版本不受支持：{version}"));
    }
    Ok(())
}

/// 打开既有仓库时执行追加式 schema 迁移。迁移不支持回退；高于当前版本的
/// 仓库直接拒绝打开。v1 → v2 追加素材板三张表；v2 → v3 为分支追加快速
/// 自动备份开关与上次全量检查记录的源文件元数据，均为追加列，不改既有数据。
pub(super) fn migrate(connection: &Connection) -> Result<(), String> {
    let version = repository_version(connection)?;
    if version == SCHEMA_VERSION {
        return Ok(());
    }
    if version < 1 || version > SCHEMA_VERSION {
        return Err(format!("作品仓库版本不受支持：{version}"));
    }
    if version < 2 {
        connection
            .execute_batch(&format!(
                "BEGIN IMMEDIATE;
                 {PIN_BOARD_TABLES_SQL}
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
    Ok(())
}

#[cfg(test)]
pub(crate) fn take_integrity_check_count() -> usize {
    INTEGRITY_CHECK_COUNT.with(|count| count.replace(0))
}
