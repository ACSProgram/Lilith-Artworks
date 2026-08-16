use rusqlite::{Connection, OptionalExtension};

use crate::storage;

#[cfg(test)]
use std::cell::Cell;

pub(super) const REPOSITORY_FORMAT: &str = "lilith-artworks";
pub(super) const SCHEMA_VERSION: i64 = 9;

#[cfg(test)]
thread_local! {
    static INTEGRITY_CHECK_COUNT: Cell<usize> = const { Cell::new(0) };
}

pub(super) fn create(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "BEGIN IMMEDIATE;
             CREATE TABLE repository_meta (
               key TEXT PRIMARY KEY,
               value TEXT NOT NULL
             );
             INSERT INTO repository_meta (key, value) VALUES
               ('format', 'lilith-artworks'),
               ('schema_version', '9');

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
        )
        .map_err(|error| format!("无法创建作品数据库结构：{error}"))
}

pub(super) fn validate_and_migrate(connection: &Connection) -> Result<(), String> {
    #[cfg(test)]
    INTEGRITY_CHECK_COUNT.with(|count| count.set(count.get() + 1));

    let integrity: String = connection
        .query_row("PRAGMA integrity_check(1)", [], |row| row.get(0))
        .map_err(|error| format!("无法检查作品数据库完整性：{error}"))?;
    if integrity != "ok" {
        return Err(format!("作品数据库完整性检查失败：{integrity}"));
    }
    let mut version = repository_version(connection)?;
    if version == 1 {
        migrate_v1_to_v2(connection)?;
        version = 2;
    }
    if version == 2 {
        migrate_v2_to_v3(connection)?;
        version = 3;
    }
    if version == 3 {
        migrate_v3_to_v4(connection)?;
        version = 4;
    }
    if version == 4 {
        migrate_v4_to_v5(connection)?;
        version = 5;
    }
    if version == 5 {
        migrate_v5_to_v6(connection)?;
        version = 6;
    }
    if version == 6 {
        migrate_v6_to_v7(connection)?;
        version = 7;
    }
    if version == 7 {
        migrate_v7_to_v8(connection)?;
        version = 8;
    }
    if version == 8 {
        migrate_v8_to_v9(connection)?;
        version = 9;
    }
    validate_current_version(version)
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

#[cfg(test)]
pub(crate) fn take_integrity_check_count() -> usize {
    INTEGRITY_CHECK_COUNT.with(|count| count.replace(0))
}

fn migrate_v1_to_v2(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "BEGIN IMMEDIATE;
             ALTER TABLE library_nodes ADD COLUMN trashed_ms INTEGER;
             ALTER TABLE library_nodes ADD COLUMN trash_root_id TEXT;
             ALTER TABLE library_nodes ADD COLUMN restore_parent_id TEXT;
             ALTER TABLE library_nodes ADD COLUMN restore_position INTEGER;
             CREATE INDEX library_nodes_trash
               ON library_nodes(trashed_ms, trash_root_id);
             UPDATE repository_meta SET value = '2' WHERE key = 'schema_version';
             COMMIT;",
        )
        .map_err(|error| format!("无法把作品仓库迁移到版本 2：{error}"))
}

fn migrate_v2_to_v3(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "BEGIN IMMEDIATE;
             ALTER TABLE branches ADD COLUMN backup_enabled INTEGER NOT NULL DEFAULT 1
               CHECK (backup_enabled IN (0, 1));
             ALTER TABLE branches ADD COLUMN backup_interval_minutes INTEGER NOT NULL DEFAULT 5
               CHECK (backup_interval_minutes BETWEEN 1 AND 10080);
             ALTER TABLE branches ADD COLUMN last_check_ms INTEGER;
             ALTER TABLE branches ADD COLUMN last_success_ms INTEGER;
             ALTER TABLE branches ADD COLUMN last_error TEXT;
             CREATE TABLE history_edges (
               child_history_id TEXT PRIMARY KEY REFERENCES history_nodes(id) ON DELETE CASCADE,
               parent_history_id TEXT NOT NULL REFERENCES history_nodes(id) ON DELETE CASCADE,
               delta_path TEXT NOT NULL,
               delta_size INTEGER NOT NULL CHECK (delta_size >= 0)
             );
             CREATE INDEX history_edges_parent ON history_edges(parent_history_id, child_history_id);
             INSERT INTO history_edges (child_history_id, parent_history_id, delta_path, delta_size)
             SELECT id, parent_id, delta_path, chunk_file_size FROM history_nodes
             WHERE parent_id IS NOT NULL AND delta_path IS NOT NULL;
             UPDATE repository_meta SET value = '3' WHERE key = 'schema_version';
             COMMIT;",
        )
        .map_err(|error| format!("无法把作品仓库迁移到版本 3：{error}"))
}

fn migrate_v3_to_v4(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "BEGIN IMMEDIATE;
             ALTER TABLE history_nodes ADD COLUMN note TEXT NOT NULL DEFAULT ''
               CHECK (length(note) <= 500);
             ALTER TABLE history_nodes ADD COLUMN commit_kind TEXT NOT NULL DEFAULT 'manual'
               CHECK (commit_kind IN ('manual', 'automatic'));
             ALTER TABLE history_nodes ADD COLUMN is_checkpoint INTEGER NOT NULL DEFAULT 0
               CHECK (is_checkpoint IN (0, 1));
             UPDATE history_nodes
             SET note = CASE WHEN title = '自动备份' THEN '' ELSE title END,
                 commit_kind = CASE WHEN title = '自动备份' THEN 'automatic' ELSE 'manual' END;
             UPDATE history_nodes SET is_checkpoint = 1
             WHERE id IN (
               SELECT created_from_history_id FROM branches WHERE created_from_history_id IS NOT NULL
             );
             UPDATE repository_meta SET value = '4' WHERE key = 'schema_version';
             COMMIT;",
        )
        .map_err(|error| format!("无法把作品仓库迁移到版本 4：{error}"))
}

fn migrate_v4_to_v5(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "BEGIN IMMEDIATE;
             ALTER TABLE certification_configs ADD COLUMN trustmark_enabled INTEGER NOT NULL DEFAULT 1
               CHECK (trustmark_enabled IN (0, 1));
             DROP TABLE certification_records;
             ALTER TABLE final_artifacts RENAME TO final_artifacts_v4;
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
             INSERT INTO final_artifacts
               (id, branch_id, history_id, source_path, source_sha256, media_type, byte_size, created_ms)
             SELECT artifact.id, artifact.branch_id, branch.head_history_id,
                    artifact.source_path, artifact.source_sha256, artifact.media_type,
                    artifact.byte_size, artifact.created_ms
             FROM final_artifacts_v4 artifact
             JOIN branches branch ON branch.id = artifact.branch_id;
             DROP TABLE final_artifacts_v4;
             CREATE TABLE certification_records (
               id TEXT PRIMARY KEY,
               final_artifact_id TEXT NOT NULL REFERENCES final_artifacts(id) ON DELETE CASCADE,
               branch_id TEXT NOT NULL REFERENCES branches(id) ON DELETE CASCADE,
               history_id TEXT NOT NULL REFERENCES history_nodes(id) ON DELETE RESTRICT,
               watermark_id TEXT CHECK (watermark_id IS NULL OR length(watermark_id) = 40),
               trustmark_enabled INTEGER NOT NULL CHECK (trustmark_enabled IN (0, 1)),
               output_path TEXT NOT NULL,
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
             UPDATE repository_meta SET value = '5' WHERE key = 'schema_version';
             COMMIT;",
        )
        .map_err(|error| format!("无法把作品仓库迁移到版本 5：{error}"))
}

fn migrate_v5_to_v6(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "BEGIN IMMEDIATE;
             ALTER TABLE certification_records ADD COLUMN stored_path TEXT;
             UPDATE repository_meta SET value = '6' WHERE key = 'schema_version';
             COMMIT;",
        )
        .map_err(|error| format!("无法把作品仓库迁移到版本 6：{error}"))
}

fn migrate_v6_to_v7(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "BEGIN IMMEDIATE;
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
             UPDATE repository_meta SET value = '7' WHERE key = 'schema_version';
             COMMIT;",
        )
        .map_err(|error| format!("无法把作品仓库迁移到版本 7：{error}"))
}

fn migrate_v7_to_v8(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "BEGIN IMMEDIATE;
             DELETE FROM certification_records WHERE stored_path IS NULL;
             ALTER TABLE certification_records RENAME TO certification_records_v7;
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
             INSERT INTO certification_records
               (id, final_artifact_id, branch_id, history_id, watermark_id,
                trustmark_enabled, output_path, stored_path, output_sha256,
                output_bytes, title, creator, rights_statement,
                authentication_content, regions_json, c2pa_manifest_label,
                c2pa_manifest_json, validation_state, created_ms)
             SELECT id, final_artifact_id, branch_id, history_id, watermark_id,
                    trustmark_enabled, output_path, stored_path, output_sha256,
                    output_bytes, title, creator, rights_statement,
                    authentication_content, regions_json, c2pa_manifest_label,
                    c2pa_manifest_json, validation_state, created_ms
             FROM certification_records_v7;
             DROP TABLE certification_records_v7;
             CREATE INDEX certification_records_watermark
               ON certification_records(watermark_id, created_ms DESC);
             CREATE INDEX certification_records_branch
               ON certification_records(branch_id, created_ms DESC);
             UPDATE repository_meta SET value = '8' WHERE key = 'schema_version';
             COMMIT;",
        )
        .map_err(|error| format!("无法把作品仓库迁移到版本 8：{error}"))
}

fn migrate_v8_to_v9(connection: &Connection) -> Result<(), String> {
    connection
        .execute_batch(
            "BEGIN IMMEDIATE;
             ALTER TABLE branches ADD COLUMN consecutive_backup_failures INTEGER NOT NULL DEFAULT 0
               CHECK (consecutive_backup_failures >= 0);
             ALTER TABLE branches ADD COLUMN backup_retry_at_ms INTEGER;
             ALTER TABLE branches ADD COLUMN backup_disable_notice_pending INTEGER NOT NULL DEFAULT 0
               CHECK (backup_disable_notice_pending IN (0, 1));
             UPDATE repository_meta SET value = '9' WHERE key = 'schema_version';
             COMMIT;",
        )
        .map_err(|error| format!("无法把作品仓库迁移到版本 9：{error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v8_requires_repository_copies_for_certification_records() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "PRAGMA foreign_keys = ON;
                 CREATE TABLE repository_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 INSERT INTO repository_meta VALUES ('format', 'lilith-artworks');
                 INSERT INTO repository_meta VALUES ('schema_version', '7');
                 CREATE TABLE final_artifacts (id TEXT PRIMARY KEY);
                 CREATE TABLE branches (id TEXT PRIMARY KEY);
                 CREATE TABLE history_nodes (id TEXT PRIMARY KEY);
                 CREATE TABLE certification_records (
                   id TEXT PRIMARY KEY,
                   final_artifact_id TEXT NOT NULL REFERENCES final_artifacts(id) ON DELETE CASCADE,
                   branch_id TEXT NOT NULL REFERENCES branches(id) ON DELETE CASCADE,
                   history_id TEXT NOT NULL REFERENCES history_nodes(id) ON DELETE RESTRICT,
                   watermark_id TEXT,
                   trustmark_enabled INTEGER NOT NULL,
                   output_path TEXT NOT NULL,
                   stored_path TEXT,
                   output_sha256 TEXT NOT NULL,
                   output_bytes INTEGER NOT NULL,
                   title TEXT NOT NULL,
                   creator TEXT NOT NULL,
                   rights_statement TEXT NOT NULL,
                   authentication_content TEXT NOT NULL,
                   regions_json TEXT NOT NULL,
                   c2pa_manifest_label TEXT,
                   c2pa_manifest_json TEXT,
                   validation_state TEXT,
                   created_ms INTEGER NOT NULL
                 );
                 CREATE INDEX certification_records_watermark
                   ON certification_records(watermark_id, created_ms DESC);
                 CREATE INDEX certification_records_branch
                   ON certification_records(branch_id, created_ms DESC);
                 INSERT INTO final_artifacts VALUES ('artifact');
                 INSERT INTO branches VALUES ('branch');
                 INSERT INTO history_nodes VALUES ('history');
                 INSERT INTO certification_records
                   (id, final_artifact_id, branch_id, history_id, trustmark_enabled,
                    output_path, stored_path, output_sha256, output_bytes, title,
                    creator, rights_statement, authentication_content, regions_json,
                    created_ms)
                 VALUES
                   ('stored', 'artifact', 'branch', 'history', 0, 'C:/stored.jpg',
                    'artworks/stored.jpg',
                    '0000000000000000000000000000000000000000000000000000000000000000',
                    1, 'Stored', '', '', '', '[]', 0),
                   ('legacy', 'artifact', 'branch', 'history', 0, 'C:/legacy.jpg',
                    NULL,
                    '0000000000000000000000000000000000000000000000000000000000000000',
                    1, 'Legacy', '', '', '', '[]', 0);",
            )
            .unwrap();

        migrate_v7_to_v8(&connection).unwrap();

        let version: i64 = connection
            .query_row(
                "SELECT CAST(value AS INTEGER) FROM repository_meta
                 WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let records: Vec<String> = {
            let mut statement = connection
                .prepare("SELECT id FROM certification_records ORDER BY id")
                .unwrap();
            statement
                .query_map([], |row| row.get(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        let stored_path_not_null: i64 = connection
            .query_row(
                "SELECT \"notnull\" FROM pragma_table_info('certification_records')
                 WHERE name = 'stored_path'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(version, 8);
        assert_eq!(records, vec!["stored"]);
        assert_eq!(stored_path_not_null, 1);
    }
}
