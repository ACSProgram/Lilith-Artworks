//! 作品仓库的 schema 契约：格式/版本常量、当前建库 SQL 与跨版本迁移。
//!
//! - 本文件：版本常量、各版本共用的 DDL 片段、新建仓库与版本号读取；
//! - `migrate`：`v1 → v6` 迁移链，迁移测试随实现；
//! - `validate`：完整性、外键、实体与路径语义校验，以及轻量版本检查。
//!
//! 跨领域事务不在这里实现，只登记表归属：
//!
//! - `library::permanently_delete_trash` 在一个事务内写 `library_nodes`、`branches`、
//!   `final_artifacts`、`certification_records`（级联）、`history_nodes`（级联边）与
//!   `pending_file_cleanup`；
//! - `history::reset_artwork_history` 在一个事务内写 `history_nodes`、`branches`、
//!   `artworks`、`library_nodes` 与 `pending_file_cleanup`。
//!
//! 两者各自由操作模块维护，原子性由对应回滚测试锁定；schema 只定义表与约束。

mod migrate;
mod validate;

use rusqlite::{Connection, OptionalExtension};

pub(super) use migrate::migrate;
#[cfg(test)]
pub(crate) use validate::take_integrity_check_count;
pub(super) use validate::{validate, validate_current, validate_repository_semantics};

pub(super) const REPOSITORY_FORMAT: &str = "lilith-artworks";
pub(super) const SCHEMA_VERSION: i64 = 6;

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

/// 当前（v6）素材板：公共条目表 + 按 kind 的载荷子表。
///
/// - `pin_board_items`：与 kind 无关的公共列，`kind = 0` 位图、`kind = 1` 文字、
///   `kind = 2` 笔迹；
/// - `pin_board_bitmaps`：位图载荷，DDS 实体存于
///   `artworks/<artwork-id>/boards/<board-id>/<item-id>.dds`；
/// - `pin_board_texts`：文字载荷（固定字体族、度量结果随条目持久化）；
/// - `pin_board_strokes`：笔迹载荷（工具、颜色、线宽、包围盒与几何 BLOB，只存数据库）；
/// - `pin_board_history`：每个条目按 step 的变换历史，不承载载荷。笔迹的 `layer`
///   恒为 3（标注层），因此该列的取值范围是 0–3。
///
/// 笔刷与形状类型放在笔迹几何 BLOB 的头部（见 `pin_board/stroke.rs`），不新增
/// kind，也不重建 `pin_board_items`。
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
               kind INTEGER NOT NULL CHECK (kind IN (0, 1, 2)),
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

             CREATE TABLE pin_board_strokes (
               item_id INTEGER PRIMARY KEY REFERENCES pin_board_items(id) ON DELETE CASCADE,
               tool INTEGER NOT NULL CHECK (tool BETWEEN 0 AND 4),
               color TEXT NOT NULL CHECK (length(color) = 7),
               stroke_width REAL NOT NULL CHECK (stroke_width > 0 AND stroke_width <= 1000),
               width REAL NOT NULL CHECK (width > 0),
               height REAL NOT NULL CHECK (height > 0),
               geometry BLOB NOT NULL CHECK (length(geometry) BETWEEN 8 AND 20488)
             );

             CREATE TABLE pin_board_history (
               board_id INTEGER NOT NULL REFERENCES pin_boards(id) ON DELETE CASCADE,
               item_id INTEGER NOT NULL REFERENCES pin_board_items(id) ON DELETE CASCADE,
               step INTEGER NOT NULL CHECK (step >= 0),
               deleted INTEGER NOT NULL CHECK (deleted IN (0, 1)),
               layer INTEGER NOT NULL CHECK (layer BETWEEN 0 AND 3),
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
               ('schema_version', '6');

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
               last_check_elapsed_ms INTEGER,
               last_check_mode TEXT CHECK (last_check_mode IN ('quick', 'full', 'fallback')),
               quick_hit_count INTEGER NOT NULL DEFAULT 0 CHECK (quick_hit_count >= 0),
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

/// 打开「比自己新」的仓库时统一用它拒绝：迁移与校验两条路径共用同一句文案，
/// 避免旧程序改写新结构。
fn newer_version_error(version: i64) -> String {
    format!(
        "作品仓库由更新版本的 Lilith Artworks 创建（schema v{version}，当前版本支持到 \
         v{SCHEMA_VERSION}），已拒绝打开以避免损坏仓库"
    )
}
