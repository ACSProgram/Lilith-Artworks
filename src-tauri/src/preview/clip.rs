use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

use image::DynamicImage;
use rusqlite::{Connection, OpenFlags};
use tempfile::NamedTempFile;

use crate::cancel::CancelToken;

use super::decode_bytes;

/// A `.clip` file embeds a complete SQLite database in its `SQLi` block. Real
/// files stay around 3–5 MiB; the cap only keeps a malformed header from
/// turning preview extraction into an unbounded copy.
const MAX_SQLI_BYTES: u64 = 64 * 1024 * 1024;
const MAX_PREVIEW_ROWS: usize = 32;
const MAX_PREVIEW_BLOB_BYTES: u64 = 32 * 1024 * 1024;
const MAX_TABLES: usize = 8;
const COPY_BUFFER_BYTES: usize = 64 * 1024;

/// Read the canvas preview PNG stored in the embedded SQLite database.
pub(super) fn extract(path: &Path, cancel: &CancelToken) -> Option<DynamicImage> {
    let mut source = File::open(path).ok()?;
    let range = crate::format::container::clip_block_payload_range(&mut source, b"SQLi", &|| {
        cancel.observed()
    })
    .ok()??;
    if range.len() < 100 || range.len() > MAX_SQLI_BYTES {
        return None;
    }

    // rusqlite needs a file path; copy only the SQLi payload into an OS temp
    // file so the source preview never mutates and the copy stays bounded.
    let mut database = NamedTempFile::new().ok()?;
    source.seek(SeekFrom::Start(range.start)).ok()?;
    let mut remaining = range.len();
    let mut buffer = [0_u8; COPY_BUFFER_BYTES];
    while remaining > 0 {
        if cancel.observed() {
            return None;
        }
        let take = usize::try_from(remaining.min(COPY_BUFFER_BYTES as u64)).ok()?;
        source.read_exact(&mut buffer[..take]).ok()?;
        database.as_file_mut().write_all(&buffer[..take]).ok()?;
        remaining -= take as u64;
    }
    database.as_file_mut().flush().ok()?;

    let connection =
        Connection::open_with_flags(database.path(), OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    let mut names = preview_tables(&connection)?;
    names.truncate(MAX_TABLES);
    for table in names {
        if let Some(image) = preview_from_table(&connection, &table, cancel) {
            return Some(image);
        }
    }
    None
}

fn preview_tables(connection: &Connection) -> Option<Vec<String>> {
    let mut statement = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .ok()?;
    let mut names = statement
        .query_map([], |row| row.get::<_, String>(0))
        .ok()?
        .filter_map(Result::ok)
        .filter(|name| name.to_ascii_lowercase().contains("preview"))
        .collect::<Vec<_>>();
    // `CanvasPreview` first (`false` sorts before `true`), then discovery order.
    names.sort_by_key(|name| !name.eq_ignore_ascii_case("CanvasPreview"));
    Some(names)
}

fn preview_from_table(
    connection: &Connection,
    table: &str,
    cancel: &CancelToken,
) -> Option<DynamicImage> {
    let quoted_table = quote_identifier(table);
    let mut info = connection
        .prepare(&format!("PRAGMA table_info({quoted_table})"))
        .ok()?;
    let columns = info
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?))
        })
        .ok()?
        .filter_map(Result::ok)
        .filter(|(_, kind)| {
            let kind = kind.to_ascii_uppercase();
            kind.contains("BLOB") || kind.is_empty()
        })
        .map(|(name, _)| name)
        .collect::<Vec<_>>();
    drop(info);

    for column in columns {
        if cancel.observed() {
            return None;
        }
        let quoted_column = quote_identifier(&column);
        let sql = format!(
            "SELECT {quoted_column} FROM {quoted_table}
              WHERE {quoted_column} IS NOT NULL
                AND length({quoted_column}) BETWEEN 8 AND {MAX_PREVIEW_BLOB_BYTES}
              LIMIT {MAX_PREVIEW_ROWS}"
        );
        let Ok(mut statement) = connection.prepare(&sql) else {
            continue;
        };
        let Ok(mut rows) = statement.query([]) else {
            continue;
        };
        while let Ok(Some(row)) = rows.next() {
            if cancel.observed() {
                return None;
            }
            let Ok(bytes) = row.get::<_, Vec<u8>>(0) else {
                continue;
            };
            if let Some(image) = decode_bytes(&bytes) {
                return Some(image);
            }
        }
    }
    None
}

fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Cursor;

    use image::{ImageBuffer, Rgba};

    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image: ImageBuffer<Rgba<u8>, _> =
            ImageBuffer::from_fn(width, height, |x, y| Rgba([x as u8, y as u8, 200, 255]));
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    fn block(tag: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0_u32.to_be_bytes());
        bytes.extend_from_slice(tag);
        bytes.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        bytes.extend_from_slice(payload);
        bytes
    }

    fn container(blocks: &[Vec<u8>]) -> Vec<u8> {
        let mut body = Vec::new();
        for block in blocks {
            body.extend_from_slice(block);
        }
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"CSFCHUNK");
        bytes.extend_from_slice(&((24 + body.len()) as u64).to_be_bytes());
        bytes.extend_from_slice(&24_u64.to_be_bytes());
        bytes.extend_from_slice(&body);
        bytes
    }

    #[test]
    fn reads_the_canvas_preview_from_the_embedded_database() {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("embedded.sqlite");
        let connection = Connection::open(&database_path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE CanvasPreview (CanvasId INTEGER PRIMARY KEY, PreviewData BLOB);",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO CanvasPreview (CanvasId, PreviewData) VALUES (1, ?1)",
                [png(64, 32)],
            )
            .unwrap();
        drop(connection);
        let database = fs::read(&database_path).unwrap();

        let path = directory.path().join("artwork.clip");
        fs::write(
            &path,
            container(&[
                block(b"Head", &[0_u8; 8]),
                block(b"SQLi", &database),
                block(b"Foot", &[]),
            ]),
        )
        .unwrap();

        let image = extract(&path, &CancelToken::standalone()).unwrap();
        assert_eq!((image.width(), image.height()), (64, 32));
    }

    #[test]
    fn a_container_without_sqli_has_no_preview() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("artwork.clip");
        fs::write(
            &path,
            container(&[block(b"Head", &[0_u8; 8]), block(b"Foot", &[])]),
        )
        .unwrap();
        assert!(extract(&path, &CancelToken::standalone()).is_none());
    }

    #[test]
    fn a_database_without_a_preview_table_has_no_preview() {
        let directory = tempfile::tempdir().unwrap();
        let database_path = directory.path().join("embedded.sqlite");
        let connection = Connection::open(&database_path).unwrap();
        connection
            .execute_batch("CREATE TABLE Other (Value TEXT);")
            .unwrap();
        drop(connection);
        let database = fs::read(&database_path).unwrap();
        let path = directory.path().join("artwork.clip");
        fs::write(
            &path,
            container(&[block(b"SQLi", &database), block(b"Foot", &[])]),
        )
        .unwrap();
        assert!(extract(&path, &CancelToken::standalone()).is_none());
    }
}
