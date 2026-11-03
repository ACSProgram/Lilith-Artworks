//! 仓库 schema 迁移链与迁移回归测试。
//!
//! 迁移不支持回退；高于当前版本的仓库直接拒绝打开。`v4 → v5` 与 `v5 → v6`
//! 都在 `PRAGMA foreign_keys = OFF` 下用 `BEGIN IMMEDIATE` 重建表，提交前做
//! `PRAGMA foreign_key_check`，任一步失败整笔回滚。

use rusqlite::{Connection, TransactionBehavior};

use crate::storage;

use super::{
    newer_version_error, repository_version, PATH_INDEXES_SQL, PIN_BOARD_TABLES_V2_SQL,
    REPOSITORY_BACKUP_RUNS_SQL, SCHEMA_VERSION,
};

/// 打开既有仓库时执行 schema 迁移。迁移不支持回退；高于当前版本的仓库直接
/// 拒绝打开。v1 → v2 追加素材板三张表；v2 → v3 为分支追加快速自动备份开关与
/// 上次全量检查记录的源文件元数据；v3 → v4 为分支追加空闲链路校验状态；
/// v4 → v5 重建画板条目为多态结构，`history_nodes` 追加缩略图与节点级校验列，
/// 删除 0.2 遗留的 `branches` 校验三列，并建立路径索引与 `repository_backup_runs`；
/// v5 → v6 为素材板加入笔迹条目（并重建 `pin_board_items` / `pin_board_history`
/// 以放开 kind 与 layer 的取值范围），同时为分支追加备份检查元数据三列。
pub(in crate::library) fn migrate(connection: &mut Connection) -> Result<(), String> {
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
    if version < 6 {
        migrate_v5_to_v6(connection)?;
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

/// v5 → v6：为素材板加入笔迹条目，并为分支补上备份检查元数据三列。
///
/// 素材板四张表必须一起重建：`pin_board_bitmaps` / `pin_board_texts` /
/// `pin_board_history` 的外键都指向 `pin_board_items`，而 SQLite 的表重命名会
/// 连带改写这些外键——只重建父表会留下指向 `pin_board_items_v5` 的悬空引用，
/// 而它随后会被删除。因此先把四张表全部改名，再建新表、复制数据、删除旧表。
///
/// 新表比旧表只放宽了取值范围（`kind IN (0,1,2)`、`layer BETWEEN 0 AND 3`），
/// 因此复制数据不需要任何转换。索引创建放在旧表删除之后：索引名不随表重命名
/// 改变，提前创建会与旧表上的同名索引冲突。
///
/// 分支三列都是可空或带默认值，旧行迁移后不需要回填：
/// - `last_check_elapsed_ms`：上一次成功自动检查的耗时，调度器据此放宽间隔
///   （重启后仍然记得，不必重新学习）；
/// - `last_check_mode`：上一次成功自动检查的方式（`quick` / `full` / `fallback`），
///   供分支状态行显示；
/// - `quick_hit_count`：连续快速命中次数；达到调度器按当前负载算出的阈值时
///   跳过一次指纹检查、改为全量确认，用于收窄整秒盲区。
fn migrate_v5_to_v6(connection: &mut Connection) -> Result<(), String> {
    connection
        .execute_batch("PRAGMA foreign_keys = OFF;")
        .map_err(|error| format!("无法暂停外键检查进行迁移：{error}"))?;
    let migration = (|| -> Result<(), String> {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage::database_error)?;
        transaction
            .execute_batch(
                "ALTER TABLE pin_board_bitmaps RENAME TO pin_board_bitmaps_v5;
                 ALTER TABLE pin_board_texts RENAME TO pin_board_texts_v5;
                 ALTER TABLE pin_board_history RENAME TO pin_board_history_v5;
                 ALTER TABLE pin_board_items RENAME TO pin_board_items_v5;

                 CREATE TABLE pin_board_items (
                   id INTEGER PRIMARY KEY AUTOINCREMENT,
                   board_id INTEGER NOT NULL REFERENCES pin_boards(id) ON DELETE CASCADE,
                   kind INTEGER NOT NULL CHECK (kind IN (0, 1, 2)),
                   created_ms INTEGER NOT NULL
                 );

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
                 );

                 INSERT INTO pin_board_items (id, board_id, kind, created_ms)
                   SELECT id, board_id, kind, created_ms FROM pin_board_items_v5;
                 INSERT INTO pin_board_bitmaps (item_id, file_path, width, height)
                   SELECT item_id, file_path, width, height FROM pin_board_bitmaps_v5;
                 INSERT INTO pin_board_texts
                   (item_id, text, font_family, font_size, color, align,
                    measured_width, measured_height)
                   SELECT item_id, text, font_family, font_size, color, align,
                          measured_width, measured_height
                     FROM pin_board_texts_v5;
                 INSERT INTO pin_board_history
                   (board_id, item_id, step, deleted, layer, sort_order, transform_json)
                   SELECT board_id, item_id, step, deleted, layer, sort_order, transform_json
                     FROM pin_board_history_v5;

                 DROP TABLE pin_board_history_v5;
                 DROP TABLE pin_board_texts_v5;
                 DROP TABLE pin_board_bitmaps_v5;
                 DROP TABLE pin_board_items_v5;

                 CREATE INDEX pin_board_items_board ON pin_board_items(board_id, id);

                 ALTER TABLE branches ADD COLUMN last_check_elapsed_ms INTEGER;
                 ALTER TABLE branches ADD COLUMN last_check_mode TEXT
                   CHECK (last_check_mode IN ('quick', 'full', 'fallback'));
                 ALTER TABLE branches ADD COLUMN quick_hit_count
                   INTEGER NOT NULL DEFAULT 0 CHECK (quick_hit_count >= 0);",
            )
            .map_err(|error| format!("无法迁移仓库结构到 v6：{error}"))?;
        {
            let mut foreign_keys = transaction
                .prepare("PRAGMA foreign_key_check")
                .map_err(storage::database_error)?;
            if foreign_keys.exists([]).map_err(storage::database_error)? {
                return Err("v6 迁移后的外键完整性检查失败".into());
            }
        }
        transaction
            .execute(
                "UPDATE repository_meta SET value = '6' WHERE key = 'schema_version'",
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
mod migration_tests {
    use std::path::Path;

    use rusqlite::{params, Connection};

    fn initialize(root: &Path) {
        std::fs::create_dir(root).unwrap();
        crate::library::initialize(root).unwrap();
    }

    fn downgrade_to_v1(connection: &Connection) {
        connection
            .execute_batch(
                "PRAGMA foreign_keys = OFF;
                 DROP TABLE pin_board_strokes;
                 DROP TABLE pin_board_history;
                 DROP TABLE pin_board_texts;
                 DROP TABLE pin_board_bitmaps;
                 DROP TABLE pin_board_items;
                 DROP TABLE pin_boards;
                 CREATE TABLE branches_v1 (
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
                   created_ms INTEGER NOT NULL,
                   updated_ms INTEGER NOT NULL,
                   UNIQUE (artwork_id, source_path_key)
                 );
                 INSERT INTO branches_v1 SELECT
                   id, artwork_id, title, source_path, source_path_key, head_history_id,
                   created_from_history_id, backup_enabled, backup_interval_minutes,
                   last_check_ms, last_success_ms, last_error, consecutive_backup_failures,
                   backup_retry_at_ms, backup_disable_notice_pending, created_ms, updated_ms
                 FROM branches;
                 DROP TABLE branches;
                 ALTER TABLE branches_v1 RENAME TO branches;
                 CREATE INDEX branches_artwork_created ON branches(artwork_id, created_ms, id);
                 DROP TABLE repository_backup_runs;
                 DROP INDEX history_nodes_snapshot_path;
                 DROP INDEX history_nodes_delta_path;
                 DROP INDEX certification_records_stored_path;
                 DROP INDEX certification_records_output_path;
                 ALTER TABLE history_nodes DROP COLUMN thumbnail;
                 ALTER TABLE history_nodes DROP COLUMN thumbnail_version;
                 ALTER TABLE history_nodes DROP COLUMN verified_ms;
                 ALTER TABLE history_nodes DROP COLUMN verify_error;
                 UPDATE repository_meta SET value = '1' WHERE key = 'schema_version';",
            )
            .unwrap();
    }

    /// 把 v5 画板表按 v4 形状重建并保留数据：v4 只有图片载荷与 `image_id`
    /// 历史，也没有路径索引、备份运行表和节点级新列。
    fn downgrade_to_v4(connection: &Connection) {
        connection
            .execute_batch(
                "PRAGMA foreign_keys = OFF;
                 DROP TABLE pin_board_strokes;
                 CREATE TABLE pin_board_images (
                   id INTEGER PRIMARY KEY AUTOINCREMENT,
                   board_id INTEGER NOT NULL REFERENCES pin_boards(id) ON DELETE CASCADE,
                   file_path TEXT NOT NULL,
                   width INTEGER NOT NULL CHECK (width > 0),
                   height INTEGER NOT NULL CHECK (height > 0),
                   created_ms INTEGER NOT NULL
                 );
                 CREATE INDEX pin_board_images_board ON pin_board_images(board_id, id);
                 INSERT INTO pin_board_images (id, board_id, file_path, width, height, created_ms)
                   SELECT item.id, item.board_id, bitmap.file_path, bitmap.width,
                          bitmap.height, item.created_ms
                     FROM pin_board_items item
                     JOIN pin_board_bitmaps bitmap ON bitmap.item_id = item.id;
                 CREATE TABLE pin_board_history_v4 (
                   board_id INTEGER NOT NULL REFERENCES pin_boards(id) ON DELETE CASCADE,
                   image_id INTEGER NOT NULL REFERENCES pin_board_images(id) ON DELETE CASCADE,
                   step INTEGER NOT NULL CHECK (step >= 0),
                   deleted INTEGER NOT NULL CHECK (deleted IN (0, 1)),
                   layer INTEGER NOT NULL CHECK (layer BETWEEN 0 AND 2),
                   sort_order INTEGER NOT NULL CHECK (sort_order >= 0),
                   transform_json TEXT NOT NULL,
                   PRIMARY KEY (board_id, image_id, step)
                 );
                 INSERT INTO pin_board_history_v4
                   (board_id, image_id, step, deleted, layer, sort_order, transform_json)
                   SELECT board_id, item_id, step, deleted, layer, sort_order, transform_json
                     FROM pin_board_history;
                 DROP TABLE pin_board_history;
                 DROP TABLE pin_board_texts;
                 DROP TABLE pin_board_bitmaps;
                 DROP TABLE pin_board_items;
                 ALTER TABLE pin_board_history_v4 RENAME TO pin_board_history;

                 DROP TABLE repository_backup_runs;
                 DROP INDEX history_nodes_snapshot_path;
                 DROP INDEX history_nodes_delta_path;
                 DROP INDEX certification_records_stored_path;
                 DROP INDEX certification_records_output_path;
                 ALTER TABLE history_nodes DROP COLUMN thumbnail;
                 ALTER TABLE history_nodes DROP COLUMN thumbnail_version;
                 ALTER TABLE history_nodes DROP COLUMN verified_ms;
                 ALTER TABLE history_nodes DROP COLUMN verify_error;
                 ALTER TABLE branches ADD COLUMN verified_history_id TEXT;
                 ALTER TABLE branches ADD COLUMN verified_ms INTEGER;
                 ALTER TABLE branches ADD COLUMN verify_error TEXT;
                 ALTER TABLE branches DROP COLUMN last_check_elapsed_ms;
                 ALTER TABLE branches DROP COLUMN last_check_mode;
                 ALTER TABLE branches DROP COLUMN quick_hit_count;
                 UPDATE repository_meta SET value = '4' WHERE key = 'schema_version';",
            )
            .unwrap();
    }

    /// 把 v6 画板表按 v5 形状重建并保留数据：v5 没有笔迹表，`kind` 只允许 0/1、
    /// `layer` 只允许 0–2，分支表也没有备份检查元数据三列。
    fn downgrade_to_v5(connection: &Connection) {
        connection
            .execute_batch(
                "PRAGMA foreign_keys = OFF;
                 DROP TABLE pin_board_strokes;
                 ALTER TABLE pin_board_bitmaps RENAME TO pin_board_bitmaps_v6;
                 ALTER TABLE pin_board_texts RENAME TO pin_board_texts_v6;
                 ALTER TABLE pin_board_history RENAME TO pin_board_history_v6;
                 ALTER TABLE pin_board_items RENAME TO pin_board_items_v6;

                 CREATE TABLE pin_board_items (
                   id INTEGER PRIMARY KEY AUTOINCREMENT,
                   board_id INTEGER NOT NULL REFERENCES pin_boards(id) ON DELETE CASCADE,
                   kind INTEGER NOT NULL CHECK (kind IN (0, 1)),
                   created_ms INTEGER NOT NULL
                 );
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
                   SELECT id, board_id, kind, created_ms FROM pin_board_items_v6;
                 INSERT INTO pin_board_bitmaps (item_id, file_path, width, height)
                   SELECT item_id, file_path, width, height FROM pin_board_bitmaps_v6;
                 INSERT INTO pin_board_texts
                   (item_id, text, font_family, font_size, color, align,
                    measured_width, measured_height)
                   SELECT item_id, text, font_family, font_size, color, align,
                          measured_width, measured_height
                     FROM pin_board_texts_v6;
                 INSERT INTO pin_board_history
                   (board_id, item_id, step, deleted, layer, sort_order, transform_json)
                   SELECT board_id, item_id, step, deleted, layer, sort_order, transform_json
                     FROM pin_board_history_v6;

                 DROP TABLE pin_board_history_v6;
                 DROP TABLE pin_board_texts_v6;
                 DROP TABLE pin_board_bitmaps_v6;
                 DROP TABLE pin_board_items_v6;
                 CREATE INDEX pin_board_items_board ON pin_board_items(board_id, id);

                 ALTER TABLE branches DROP COLUMN last_check_elapsed_ms;
                 ALTER TABLE branches DROP COLUMN last_check_mode;
                 ALTER TABLE branches DROP COLUMN quick_hit_count;
                 UPDATE repository_meta SET value = '5' WHERE key = 'schema_version';",
            )
            .unwrap();
    }

    fn table_exists(connection: &Connection, name: &str) -> bool {
        connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master
                 WHERE type = 'table' AND name = ?1)",
                [name],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn index_exists(connection: &Connection, name: &str) -> bool {
        connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master
                 WHERE type = 'index' AND name = ?1)",
                [name],
                |row| row.get(0),
            )
            .unwrap()
    }

    fn columns(connection: &Connection, table: &str) -> Vec<String> {
        let mut statement = connection
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap();
        statement
            .query_map([], |row| row.get(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    }

    fn insert_board_fixture(connection: &Connection, artwork_id: &str) {
        let now = crate::storage::now_ms().unwrap();
        connection
            .execute(
                "INSERT INTO library_nodes (id, kind, title, position, created_ms, updated_ms)
                 VALUES (?1, 'artwork', '迁移作品', 0, ?2, ?2)",
                params![artwork_id, now],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO artworks (id, created_ms, updated_ms) VALUES (?1, ?2, ?2)",
                params![artwork_id, now],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO pin_boards
                   (id, artwork_id, name, sort_order, now_step, max_step, revision,
                    created_ms, updated_ms)
                 VALUES (1, ?1, '迁移画板', 0, 2, 2, ?2, ?3, ?3)",
                params![artwork_id, "0".repeat(64), now],
            )
            .unwrap();
        let transform = r#"{"points":[[0.0,0.0],[1.0,0.0],[1.0,1.0],[0.0,1.0]],"uv":[[0.0,0.0],[1.0,0.0],[1.0,1.0],[0.0,1.0]]}"#;
        for (item_id, path) in [(11, "one.dds"), (12, "two.dds")] {
            connection
                .execute(
                    "INSERT INTO pin_board_items (id, board_id, kind, created_ms)
                     VALUES (?1, 1, 0, ?2)",
                    params![item_id, now],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO pin_board_bitmaps (item_id, file_path, width, height)
                     VALUES (?1, ?2, 8, 8)",
                    params![item_id, path],
                )
                .unwrap();
        }
        for step in 0..3 {
            connection
                .execute(
                    "INSERT INTO pin_board_history
                       (board_id, item_id, step, deleted, layer, sort_order, transform_json)
                     VALUES (1, 11, ?1, 0, 1, ?1, ?2)",
                    params![step, transform],
                )
                .unwrap();
        }
        connection
            .execute(
                "INSERT INTO pin_board_history
                   (board_id, item_id, step, deleted, layer, sort_order, transform_json)
                 VALUES (1, 12, 0, 1, 2, 0, ?1)",
                [transform],
            )
            .unwrap();
    }

    /// v1 仓库（无素材板表、分支表也没有 v2/v3/v4 追加列）打开时必须通过追加式
    /// 迁移升级到当前版本 v6，且 v1 形状的既有结构保持可读。
    #[test]
    fn migrates_a_v1_repository_to_the_current_version_append_only() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        initialize(&root);
        {
            let connection = crate::storage::open(&root).unwrap();
            downgrade_to_v1(&connection);
        }

        crate::library::open_existing(&root).unwrap();

        let connection = crate::storage::open(&root).unwrap();
        let version: i64 = connection
            .query_row(
                "SELECT CAST(value AS INTEGER) FROM repository_meta
                 WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, 6);
        for table in [
            "pin_boards",
            "pin_board_items",
            "pin_board_bitmaps",
            "pin_board_texts",
            "pin_board_strokes",
            "pin_board_history",
            "repository_backup_runs",
        ] {
            assert!(
                table_exists(&connection, table),
                "表 {table} 应在迁移后存在"
            );
        }
        assert!(!table_exists(&connection, "pin_board_images"));
        let branch_columns = columns(&connection, "branches");
        for column in [
            "backup_quick_enabled",
            "last_source_size",
            "last_source_modified_ms",
            "last_check_elapsed_ms",
            "last_check_mode",
            "quick_hit_count",
        ] {
            assert!(branch_columns.iter().any(|item| item == column), "{column}");
        }
        for column in ["verified_history_id", "verified_ms", "verify_error"] {
            assert!(
                !branch_columns.iter().any(|item| item == column),
                "{column}"
            );
        }
        let node_columns = columns(&connection, "history_nodes");
        for column in [
            "thumbnail",
            "thumbnail_version",
            "verified_ms",
            "verify_error",
        ] {
            assert!(node_columns.iter().any(|item| item == column), "{column}");
        }
    }

    /// v4 → v5 必须保留图片记录、条目 ID 与每一步变换历史，并补齐索引、
    /// 节点级校验列和整仓备份记录表，同时移除 v4 的分支校验列。
    #[test]
    fn migrates_v4_pin_board_items_and_step_history() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        initialize(&root);
        let artwork_id = crate::storage::new_id();
        {
            let connection = crate::storage::open(&root).unwrap();
            insert_board_fixture(&connection, &artwork_id);
            downgrade_to_v4(&connection);
        }

        crate::library::open_existing(&root).unwrap();

        let connection = crate::storage::open(&root).unwrap();
        assert_eq!(
            connection
                .query_row::<i64, _, _>(
                    "SELECT CAST(value AS INTEGER) FROM repository_meta
                     WHERE key = 'schema_version'",
                    [],
                    |row| row.get(0)
                )
                .unwrap(),
            6
        );
        assert!(!table_exists(&connection, "pin_board_images"));
        let items: Vec<(i64, i64, i64)> = {
            let mut statement = connection
                .prepare("SELECT id, board_id, kind FROM pin_board_items ORDER BY id")
                .unwrap();
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(items, vec![(11, 1, 0), (12, 1, 0)]);
        let bitmaps: Vec<(i64, String, i64, i64)> = {
            let mut statement = connection
                .prepare("SELECT item_id, file_path, width, height FROM pin_board_bitmaps ORDER BY item_id")
                .unwrap();
            statement
                .query_map([], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(
            bitmaps,
            vec![
                (11, "one.dds".to_owned(), 8, 8),
                (12, "two.dds".to_owned(), 8, 8)
            ]
        );
        let history: Vec<(i64, i64, i64, i64)> = {
            let mut statement = connection
                .prepare(
                    "SELECT item_id, step, deleted, layer FROM pin_board_history
                     ORDER BY item_id, step",
                )
                .unwrap();
            statement
                .query_map([], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(
            history,
            vec![(11, 0, 0, 1), (11, 1, 0, 1), (11, 2, 0, 1), (12, 0, 1, 2)]
        );
        for index in [
            "history_nodes_snapshot_path",
            "history_nodes_delta_path",
            "certification_records_stored_path",
            "certification_records_output_path",
        ] {
            assert!(index_exists(&connection, index), "索引 {index}");
        }
        assert!(table_exists(&connection, "repository_backup_runs"));
        let node_columns = columns(&connection, "history_nodes");
        for column in [
            "thumbnail",
            "thumbnail_version",
            "verified_ms",
            "verify_error",
        ] {
            assert!(node_columns.iter().any(|item| item == column), "{column}");
        }
        let branch_columns = columns(&connection, "branches");
        for column in ["verified_history_id", "verified_ms", "verify_error"] {
            assert!(
                !branch_columns.iter().any(|item| item == column),
                "{column}"
            );
        }

        // 外键级联在新表上继续生效。
        connection
            .execute("DELETE FROM pin_boards WHERE id = 1", [])
            .unwrap();
        for table in [
            "pin_board_items",
            "pin_board_bitmaps",
            "pin_board_texts",
            "pin_board_history",
        ] {
            let count: i64 = connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "{table}");
        }
    }

    /// 迁移中若外键检查失败，整笔事务回滚：版本仍是 v4，旧表仍在。
    #[test]
    fn failed_v4_migration_rolls_back_to_v4() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        initialize(&root);
        let artwork_id = crate::storage::new_id();
        {
            let connection = crate::storage::open(&root).unwrap();
            insert_board_fixture(&connection, &artwork_id);
            downgrade_to_v4(&connection);
            connection
                .execute_batch(
                    "PRAGMA foreign_keys = OFF;
                     INSERT INTO pin_board_history
                       (board_id, image_id, step, deleted, layer, sort_order, transform_json)
                     VALUES (1, 999, 0, 0, 1, 0, '{}');",
                )
                .unwrap();
        }

        let error = crate::library::open_existing(&root).unwrap_err();
        assert!(error.contains("外键完整性检查失败"), "{error}");

        let connection = crate::storage::open(&root).unwrap();
        assert_eq!(
            connection
                .query_row::<i64, _, _>(
                    "SELECT CAST(value AS INTEGER) FROM repository_meta
                     WHERE key = 'schema_version'",
                    [],
                    |row| row.get(0)
                )
                .unwrap(),
            4
        );
        assert!(table_exists(&connection, "pin_board_images"));
        assert!(!table_exists(&connection, "pin_board_items"));
    }

    /// v5 → v6 必须保留位图、文字与每一步变换历史，新建笔迹表并放开 kind / layer
    /// 的取值，同时为分支补上备份检查元数据三列（旧行为空或取默认值）。
    #[test]
    fn migrates_v5_boards_and_adds_backup_check_columns() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        initialize(&root);
        let artwork_id = crate::storage::new_id();
        let branch_id = crate::storage::new_id();
        {
            let connection = crate::storage::open(&root).unwrap();
            insert_board_fixture(&connection, &artwork_id);
            let transform = r#"{"points":[[0.0,0.0],[1.0,0.0],[1.0,1.0],[0.0,1.0]],"uv":[[0.0,0.0],[1.0,0.0],[1.0,1.0],[0.0,1.0]]}"#;
            connection
                .execute(
                    "INSERT INTO pin_board_items (id, board_id, kind, created_ms)
                     VALUES (13, 1, 1, 1)",
                    [],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO pin_board_texts
                       (item_id, text, font_family, font_size, color, align,
                        measured_width, measured_height)
                     VALUES (13, '迁移文字', 'sans-serif', 20.0, '#112233', 'left', 60.0, 30.0)",
                    [],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO pin_board_history
                       (board_id, item_id, step, deleted, layer, sort_order, transform_json)
                     VALUES (1, 13, 1, 0, 1, 0, ?1)",
                    [transform],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO branches
                       (id, artwork_id, title, source_path, source_path_key, created_ms, updated_ms)
                     VALUES (?1, ?2, '迁移分支', 'C:/outside.psd', 'c:/outside.psd', 1, 1)",
                    [&branch_id, &artwork_id],
                )
                .unwrap();
            downgrade_to_v5(&connection);
        }

        crate::library::open_existing(&root).unwrap();

        let connection = crate::storage::open(&root).unwrap();
        assert_eq!(
            connection
                .query_row::<i64, _, _>(
                    "SELECT CAST(value AS INTEGER) FROM repository_meta
                     WHERE key = 'schema_version'",
                    [],
                    |row| row.get(0)
                )
                .unwrap(),
            6
        );
        assert!(table_exists(&connection, "pin_board_strokes"));
        let items: Vec<(i64, i64)> = {
            let mut statement = connection
                .prepare("SELECT id, kind FROM pin_board_items ORDER BY id")
                .unwrap();
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(items, vec![(11, 0), (12, 0), (13, 1)]);
        assert_eq!(
            connection
                .query_row::<i64, _, _>(
                    "SELECT COUNT(*) FROM pin_board_history WHERE item_id = 11",
                    [],
                    |row| row.get(0)
                )
                .unwrap(),
            3
        );
        assert_eq!(
            connection
                .query_row::<String, _, _>(
                    "SELECT text FROM pin_board_texts WHERE item_id = 13",
                    [],
                    |row| row.get(0)
                )
                .unwrap(),
            "迁移文字"
        );

        // 分支三列：旧行不填也是合法状态（两列 NULL，计数取默认 0）。
        let columns_in_branches = columns(&connection, "branches");
        for column in [
            "last_check_elapsed_ms",
            "last_check_mode",
            "quick_hit_count",
        ] {
            assert!(
                columns_in_branches.iter().any(|item| item == column),
                "{column}"
            );
        }
        let defaults: (Option<i64>, Option<String>, i64) = connection
            .query_row(
                "SELECT last_check_elapsed_ms, last_check_mode, quick_hit_count
                   FROM branches WHERE id = ?1",
                [&branch_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(defaults, (None, None, 0));

        // 新结构可写笔迹条目：kind = 2 与 layer = 3 都被接受。
        connection
            .execute(
                "INSERT INTO pin_board_items (id, board_id, kind, created_ms)
                 VALUES (14, 1, 2, 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO pin_board_strokes
                   (item_id, tool, color, stroke_width, width, height, geometry)
                 VALUES (14, 0, '#000000', 1.0, 10.0, 10.0,
                         X'01000000010000000000000080')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO pin_board_history
                   (board_id, item_id, step, deleted, layer, sort_order, transform_json)
                 VALUES (1, 14, 1, 0, 3, 0, '{}')",
                [],
            )
            .unwrap();
        // 越界取值仍被新表的 CHECK 挡住。
        assert!(connection
            .execute(
                "INSERT INTO pin_board_items (id, board_id, kind, created_ms)
                 VALUES (15, 1, 3, 1)",
                [],
            )
            .is_err());
        assert!(connection
            .execute(
                "INSERT INTO pin_board_history
                   (board_id, item_id, step, deleted, layer, sort_order, transform_json)
                 VALUES (1, 14, 2, 0, 4, 0, '{}')",
                [],
            )
            .is_err());

        // 外键级联在新表上继续生效（笔迹与文字都随画板级联删除）。
        connection
            .execute("DELETE FROM pin_boards WHERE id = 1", [])
            .unwrap();
        for table in [
            "pin_board_items",
            "pin_board_bitmaps",
            "pin_board_texts",
            "pin_board_strokes",
            "pin_board_history",
        ] {
            let count: i64 = connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "{table}");
        }
    }

    /// 迁移中若外键检查失败，整笔事务回滚：版本仍是 v5，v5 形状的表仍在。
    #[test]
    fn failed_v6_migration_rolls_back_to_v5() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        initialize(&root);
        let artwork_id = crate::storage::new_id();
        {
            let connection = crate::storage::open(&root).unwrap();
            insert_board_fixture(&connection, &artwork_id);
            downgrade_to_v5(&connection);
            connection
                .execute_batch(
                    "PRAGMA foreign_keys = OFF;
                     INSERT INTO pin_board_history
                       (board_id, item_id, step, deleted, layer, sort_order, transform_json)
                     VALUES (1, 999, 0, 0, 1, 0, '{}');",
                )
                .unwrap();
        }

        let error = crate::library::open_existing(&root).unwrap_err();
        assert!(error.contains("外键完整性检查失败"), "{error}");

        let connection = crate::storage::open(&root).unwrap();
        assert_eq!(
            connection
                .query_row::<i64, _, _>(
                    "SELECT CAST(value AS INTEGER) FROM repository_meta
                     WHERE key = 'schema_version'",
                    [],
                    |row| row.get(0)
                )
                .unwrap(),
            5
        );
        assert!(!table_exists(&connection, "pin_board_strokes"));
        assert!(!table_exists(&connection, "pin_board_items_v6"));
        assert!(!columns(&connection, "branches")
            .iter()
            .any(|item| item == "quick_hit_count"));
    }

    /// 旧程序（支持到 v6）必须拒绝打开 v7 仓库，且不改动版本与表结构。
    #[test]
    fn rejects_repositories_from_newer_versions() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        initialize(&root);
        {
            let connection = crate::storage::open(&root).unwrap();
            connection
                .execute(
                    "UPDATE repository_meta SET value = '7' WHERE key = 'schema_version'",
                    [],
                )
                .unwrap();
        }

        let error = crate::library::open_existing(&root).unwrap_err();
        assert!(error.contains("更新版本"), "{error}");
        let connection = crate::storage::open(&root).unwrap();
        let version: String = connection
            .query_row(
                "SELECT value FROM repository_meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, "7");
        assert!(table_exists(&connection, "pin_board_items"));
    }
}
