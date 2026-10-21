//! 整画布导出的原生侧：把前端逐块送来的 RGBA 行带流式拼成一张透明 PNG。
//!
//! 设计要点（口径见 `docs/modules/pin-board.md` 的「整画布导出」）：
//!
//! - **不取仓库写租约、不取仓库操作锁**：导出只写用户选的输出目录，读的只是仓库
//!   路径本身（用于 `ensure_outside_repository` 的边界校验）。因此导出期间后台的
//!   完整性扫描、自动备份与清理照常运行，不会互相把对方挡成 `repository_busy`；
//! - **内存不随输出尺寸增长**：PNG 必须逐行顺序写，所以按「行带」推进——一条行带
//!   的整行缓冲由前端的分块拼满后立刻写盘并丢弃，常驻内存只有一个行带；
//! - **临时文件在输出目录内**：`begin` 建临时文件、`finish` 用改名发布，保证最后
//!   一步是同卷原子操作，与 `publish_export_png` 同一口径；进程被强杀留下的临时
//!   文件在下次 `begin` 时按前缀清掉。
//!
//! 会话是单并发的（`Mutex<Option<CanvasExportSession>>`）：同时只允许一次导出。

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tauri::ipc::{InvokeBody, Request};
use tauri::State;
use tauri_plugin_fs::FsExt;

use crate::{app::AppState, path_scope, storage};

/// 临时文件名前缀：`begin` 按它清理强杀留下的残留。
const TEMP_PREFIX: &str = "lilith-pin-board-export-";
const TEMP_SUFFIX: &str = ".png.tmp";
/// 单块上限：8192 是设备 `maxTextureDimension2D` 的通用上限，行带最高 2048。
const MAX_TILE_WIDTH: u32 = 8192;
const MAX_TILE_HEIGHT: u32 = 2048;
const MAX_TILE_PIXELS: u64 = MAX_TILE_WIDTH as u64 * MAX_TILE_HEIGHT as u64;
/// 输出像素的防御上界。计划刻意**只按最长边**限制（30000 最长边仍可发起），这个值
/// 只用来挡住荒谬的入参（例如宽高各 10 亿）导致的无意义分配，正常上限远低于它。
const MAX_OUTPUT_PIXELS: u64 = 1_000_000_000;
const DEFAULT_FILE_STEM: &str = "素材板";
const PNG_STREAM_BUFFER_BYTES: usize = 1 << 20;

/// 导出会话表：单并发，`None` 表示没有正在进行的导出。
#[derive(Default)]
pub(crate) struct PinBoardCanvasExportState {
    session: Mutex<Option<CanvasExportSession>>,
}

struct CanvasExportSession {
    width: u32,
    height: u32,
    target: PathBuf,
    temp: tempfile::NamedTempFile,
    /// 已打开 PNG 头的流写入器。`finish` 时它内部的 `Writer` 在 Drop 里写 IEND。
    stream: png::StreamWriter<'static, File>,
    /// 当前行带的行数；0 表示还没有行带在进行中。
    band_rows: u32,
    /// 当前行带顶部的 y（没有行带时是下一条行带的 y）。
    band_y: u32,
    /// 行内下一块的起始 x；0 表示这一行还没收到块。
    next_x: u32,
    /// 当前行带的整行缓冲（`width × band_rows × 4`），写盘后丢弃。
    band: Vec<u8>,
}

impl CanvasExportSession {
    fn new(
        temp: tempfile::NamedTempFile,
        target: PathBuf,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        let file = temp
            .as_file()
            .try_clone()
            .map_err(|error| format!("无法打开导出临时文件：{error}"))?;
        let mut encoder = png::Encoder::new(file, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        // 大图的成本几乎全在编码上：用面向 PNG 调优的快速档 + 上向预测，写盘快一个
        // 量级，文件略大。导出的是交付图而不是仓库存储，这个取舍更划算。
        encoder.set_compression(png::Compression::Fast);
        encoder.set_filter(png::Filter::Up);
        let writer = encoder
            .write_header()
            .map_err(|error| format!("无法写入 PNG 头：{error}"))?;
        let stream = writer
            .into_stream_writer_with_size(PNG_STREAM_BUFFER_BYTES)
            .map_err(|error| format!("无法创建 PNG 流写入器：{error}"))?;
        Ok(Self {
            width,
            height,
            target,
            temp,
            stream,
            band_rows: 0,
            band_y: 0,
            next_x: 0,
            band: Vec::new(),
        })
    }

    /// 收下一块 RGBA。块必须按「行带优先、行内从左到右」到达，一条行带拼满即写盘。
    fn write_tile(
        &mut self,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Result<(), String> {
        if width == 0 || height == 0 {
            return Err("导出块尺寸无效".into());
        }
        if width > MAX_TILE_WIDTH
            || height > MAX_TILE_HEIGHT
            || (width as u64) * (height as u64) > MAX_TILE_PIXELS
        {
            return Err(format!("导出块 {width}×{height} 超过单块上限"));
        }
        let end_x = x.checked_add(width).ok_or("导出块越界")?;
        let end_y = y.checked_add(height).ok_or("导出块越界")?;
        if end_x > self.width || end_y > self.height {
            return Err("导出块超出输出范围".into());
        }
        let expected = width as usize * height as usize * 4;
        if rgba.len() != expected {
            return Err(format!(
                "导出块字节数不符：期望 {expected}，实际 {}",
                rgba.len()
            ));
        }
        if self.band_rows == 0 {
            if x != 0 {
                return Err("导出块必须从行带左端开始".into());
            }
            if y != self.band_y {
                return Err(format!(
                    "导出块的行带位置不符：期望 y={}，实际 y={y}",
                    self.band_y
                ));
            }
            let capacity = self.width as usize * height as usize * 4;
            self.band.clear();
            self.band.resize(capacity, 0);
            self.band_rows = height;
        } else if x != self.next_x || y != self.band_y || height != self.band_rows {
            return Err(format!(
                "导出块的到达顺序不符合行带布局：期望 x={} y={} 高 {}，实际 x={x} y={y} 高 {height}",
                self.next_x, self.band_y, self.band_rows
            ));
        }
        let row_bytes = self.width as usize * 4;
        let tile_row_bytes = width as usize * 4;
        for row in 0..height as usize {
            let destination = row * row_bytes + x as usize * 4;
            let source = row * tile_row_bytes;
            self.band[destination..destination + tile_row_bytes]
                .copy_from_slice(&rgba[source..source + tile_row_bytes]);
        }
        self.next_x = end_x;
        if self.next_x == self.width {
            self.flush_band()?;
        }
        Ok(())
    }

    fn flush_band(&mut self) -> Result<(), String> {
        let rows = self.band_rows as usize;
        let bytes = rows * self.width as usize * 4;
        self.stream
            .write_all(&self.band[..bytes])
            .map_err(|error| format!("无法写入导出数据：{error}"))?;
        self.band_y += self.band_rows;
        self.band_rows = 0;
        self.next_x = 0;
        Ok(())
    }

    /// 结束导出：校验数据写满、写 IEND、`sync_all`，最后原子改名到最终文件。
    fn finish(self) -> Result<PathBuf, String> {
        if self.band_rows != 0 || self.next_x != 0 {
            return Err("导出尚未写完当前行带".into());
        }
        if self.band_y != self.height {
            return Err(format!(
                "导出缺少 {} 行数据",
                self.height.saturating_sub(self.band_y)
            ));
        }
        // `finish` 会确认所有行都已写入并 flush；内层 `Writer` 随后在 Drop 里写 IEND。
        self.stream
            .finish()
            .map_err(|error| format!("无法结束 PNG 编码：{error}"))?;
        self.temp
            .as_file()
            .sync_all()
            .map_err(|error| format!("无法同步导出文件：{error}"))?;
        let target = self.target.clone();
        self.temp
            .persist(&target)
            .map_err(|error| format!("无法保存导出文件：{}", error.error))?;
        Ok(target)
    }
}

/// 开始一次导出：校验输出位置、建临时文件、写 PNG 头。
///
/// 单独抽成函数（而不是写在命令里）是为了让单测能在没有 Tauri 运行时的条件下覆盖
/// 行带校验、文件命名与仓库边界——命令层只做参数搬运与授权检查。
fn prepare_export(
    root: &Path,
    directory: &Path,
    file_name: &str,
    width: u32,
    height: u32,
) -> Result<CanvasExportSession, String> {
    if width == 0 || height == 0 {
        return Err("导出尺寸无效".into());
    }
    if (width as u64) * (height as u64) > MAX_OUTPUT_PIXELS {
        return Err("导出像素数超过上限".into());
    }
    if !directory.is_dir() {
        return Err("导出位置必须是文件夹".into());
    }
    // 与单条导出一致：不得写进仓库目录（那会让画板 DDS 与用户导出文件混在同一棵树上）。
    storage::ensure_outside_repository(root, directory, "导出位置")?;
    sweep_stale_temps(directory);
    let target = unique_target(directory, file_name);
    let temp = tempfile::Builder::new()
        .prefix(TEMP_PREFIX)
        .suffix(TEMP_SUFFIX)
        .tempfile_in(directory)
        .map_err(|error| format!("无法创建导出临时文件：{error}"))?;
    CanvasExportSession::new(temp, target, width, height)
}

/// 清掉输出目录里同前缀的临时文件（上一次进程被强杀留下的）。只删自己的前缀，
/// 且会话是单并发的，因此不会误删别的程序的文件。
fn sweep_stale_temps(directory: &Path) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with(TEMP_PREFIX) || !name.ends_with(TEMP_SUFFIX) {
            continue;
        }
        if let Err(error) = std::fs::remove_file(entry.path()) {
            log::warn!(
                "pin-board canvas export stale temp kept: path={}, error={error}",
                entry.path().display()
            );
        }
    }
}

/// 文件名：去掉非法字符与扩展名，空则回落到「素材板」。
fn export_file_stem(raw: &str) -> String {
    let trimmed = raw.trim();
    let without_extension = trimmed
        .strip_suffix(".png")
        .or_else(|| trimmed.strip_suffix(".PNG"))
        .unwrap_or(trimmed);
    let sanitised: String = without_extension
        .chars()
        .map(|character| {
            if character.is_control() || "\\/:*?\"<>|".contains(character) {
                '_'
            } else {
                character
            }
        })
        .collect();
    let cleaned = sanitised.trim().trim_end_matches('.').trim();
    if cleaned.is_empty() {
        DEFAULT_FILE_STEM.to_string()
    } else {
        cleaned.chars().take(80).collect()
    }
}

/// 最终文件名：同目录已存在同名文件时追加 `-2`、`-3`…，绝不覆盖用户已有文件。
fn unique_target(directory: &Path, file_name: &str) -> PathBuf {
    let stem = export_file_stem(file_name);
    let first = directory.join(format!("{stem}.png"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|suffix| directory.join(format!("{stem}-{suffix}.png")))
        .find(|path| !path.exists())
        .unwrap_or(first)
}

/// 块的坐标与尺寸走 header（短字符串），像素走原始请求体，避免大块 JSON 编码。
struct TileHeaders {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl TileHeaders {
    fn parse(headers: &tauri::http::HeaderMap) -> Result<Self, String> {
        Ok(Self {
            x: header_u32(headers, "x")?,
            y: header_u32(headers, "y")?,
            width: header_u32(headers, "width")?,
            height: header_u32(headers, "height")?,
        })
    }
}

fn header_u32(headers: &tauri::http::HeaderMap, name: &str) -> Result<u32, String> {
    let value = headers
        .get(name)
        .ok_or_else(|| format!("导出块缺少 {name} 头"))?;
    value
        .to_str()
        .map_err(|_| format!("导出块的 {name} 头不是有效文本"))?
        .trim()
        .parse::<u32>()
        .map_err(|_| format!("导出块的 {name} 头不是有效整数"))
}

fn lock_state(
    state: &PinBoardCanvasExportState,
) -> Result<std::sync::MutexGuard<'_, Option<CanvasExportSession>>, String> {
    state
        .session
        .lock()
        .map_err(|_| "导出会话已损坏".to_owned())
}

/// 开始整画布导出：在输出目录建临时文件、写 PNG 头，返回前会话已就绪。
///
/// 输出目录必须已由文件选择器授权（`path_scope`），且位于仓库之外
/// （`ensure_outside_repository`）。这一步**不取仓库写租约与操作锁**。
#[tauri::command]
pub(crate) async fn begin_pin_board_canvas_export(
    window: tauri::WebviewWindow,
    app: State<'_, AppState>,
    export: State<'_, PinBoardCanvasExportState>,
    output_directory: String,
    file_name: String,
    width: u32,
    height: u32,
) -> Result<(), String> {
    let directory = PathBuf::from(output_directory.trim());
    path_scope::ensure_authorized(&window.fs_scope(), &directory, "导出位置")?;
    let root = app.ready_repository_path()?;
    let mut guard = lock_state(&export)?;
    // 单并发：新会话直接顶掉旧会话（旧临时文件随 Drop 删除），同时清掉强杀残留。
    *guard = None;
    let session = prepare_export(&root, &directory, &file_name, width, height)?;
    log::info!(
        "pin-board canvas export started: width={width}, height={height}, target={}",
        session.target.display()
    );
    *guard = Some(session);
    Ok(())
}

/// 收一块像素：坐标与尺寸走 header，RGBA 走原始请求体。
#[tauri::command]
pub(crate) async fn write_pin_board_canvas_export_tile(
    request: Request<'_>,
    export: State<'_, PinBoardCanvasExportState>,
) -> Result<(), String> {
    let tile = TileHeaders::parse(request.headers())?;
    let body = match request.body() {
        InvokeBody::Raw(bytes) => bytes.as_slice(),
        InvokeBody::Json(_) => return Err("导出块必须以原始字节发送".into()),
    };
    let mut guard = lock_state(&export)?;
    let session = guard
        .as_mut()
        .ok_or_else(|| "当前没有进行中的画布导出".to_owned())?;
    session.write_tile(tile.x, tile.y, tile.width, tile.height, body)
}

/// 结束导出：写 IEND、`sync_all`、原子改名，返回最终文件路径。
#[tauri::command]
pub(crate) async fn finish_pin_board_canvas_export(
    export: State<'_, PinBoardCanvasExportState>,
) -> Result<String, String> {
    let session = lock_state(&export)?
        .take()
        .ok_or_else(|| "当前没有进行中的画布导出".to_owned())?;
    let target = session.finish()?;
    log::info!(
        "pin-board canvas export finished: path={}",
        target.display()
    );
    Ok(target.to_string_lossy().to_string())
}

/// 放弃导出：会话与临时文件一起丢弃。
#[tauri::command]
pub(crate) async fn cancel_pin_board_canvas_export(
    export: State<'_, PinBoardCanvasExportState>,
) -> Result<(), String> {
    let mut guard = lock_state(&export)?;
    if guard.take().is_some() {
        log::info!("pin-board canvas export cancelled");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 输出目录与仓库根：`ensure_outside_repository` 只做路径规范化，不需要可用的数据库。
    fn workspace() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("repository");
        let output = directory.path().join("out");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&output).unwrap();
        (directory, root, output)
    }

    fn solid(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
        rgba.iter()
            .copied()
            .cycle()
            .take(width as usize * height as usize * 4)
            .collect()
    }

    #[test]
    fn streams_bands_into_a_transparent_png() {
        let (_guard, root, output) = workspace();
        let mut session = prepare_export(&root, &output, "画板", 4, 3).unwrap();
        session
            .write_tile(0, 0, 4, 2, &solid(4, 2, [255, 0, 0, 255]))
            .unwrap();
        session
            .write_tile(0, 2, 4, 1, &solid(4, 1, [0, 0, 255, 128]))
            .unwrap();
        let target = session.finish().unwrap();

        assert_eq!(target.file_name().unwrap(), "画板.png");
        let decoded = image::open(&target).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), (4, 3));
        assert_eq!(decoded.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(decoded.get_pixel(3, 1).0, [255, 0, 0, 255]);
        assert_eq!(decoded.get_pixel(0, 2).0, [0, 0, 255, 128]);
        assert_eq!(decoded.get_pixel(3, 2).0, [0, 0, 255, 128]);
    }

    #[test]
    fn assembles_a_band_from_several_tiles() {
        let (_guard, root, output) = workspace();
        let mut session = prepare_export(&root, &output, "多块", 4, 1).unwrap();
        session
            .write_tile(0, 0, 2, 1, &solid(2, 1, [10, 20, 30, 255]))
            .unwrap();
        session
            .write_tile(2, 0, 2, 1, &solid(2, 1, [40, 50, 60, 255]))
            .unwrap();
        let target = session.finish().unwrap();

        let decoded = image::open(&target).unwrap().to_rgba8();
        assert_eq!(decoded.get_pixel(1, 0).0, [10, 20, 30, 255]);
        assert_eq!(decoded.get_pixel(2, 0).0, [40, 50, 60, 255]);
    }

    #[test]
    fn rejects_tiles_that_break_the_band_layout() {
        let (_guard, root, output) = workspace();
        let mut session = prepare_export(&root, &output, "顺序", 4, 2).unwrap();
        // 不从行带左端开始。
        assert!(session
            .write_tile(2, 0, 2, 1, &solid(2, 1, [1, 2, 3, 255]))
            .is_err());
        // 行带位置不符。
        assert!(session
            .write_tile(0, 1, 2, 1, &solid(2, 1, [1, 2, 3, 255]))
            .is_err());
        session
            .write_tile(0, 0, 2, 1, &solid(2, 1, [1, 2, 3, 255]))
            .unwrap();
        // 同一行带内跳块（期望 x=2）。
        assert!(session
            .write_tile(3, 0, 1, 1, &solid(1, 1, [1, 2, 3, 255]))
            .is_err());
        // 行高与行带不一致。
        assert!(session
            .write_tile(2, 0, 2, 2, &solid(2, 2, [1, 2, 3, 255]))
            .is_err());
    }

    #[test]
    fn rejects_out_of_bounds_and_mismatched_payloads() {
        let (_guard, root, output) = workspace();
        let mut session = prepare_export(&root, &output, "越界", 4, 2).unwrap();
        assert!(session
            .write_tile(0, 0, 5, 1, &solid(5, 1, [0, 0, 0, 0]))
            .is_err());
        assert!(session
            .write_tile(0, 1, 4, 2, &solid(4, 2, [0, 0, 0, 0]))
            .is_err());
        // 字节数与声明尺寸不符。
        assert!(session
            .write_tile(0, 0, 2, 1, &solid(2, 2, [0, 0, 0, 0]))
            .is_err());
        // 单块上限。
        assert!(session
            .write_tile(0, 0, 1, MAX_TILE_HEIGHT + 1, &[])
            .is_err());
    }

    #[test]
    fn finish_requires_every_row() {
        let (_guard, root, output) = workspace();
        let mut session = prepare_export(&root, &output, "缺行", 4, 3).unwrap();
        session
            .write_tile(0, 0, 4, 1, &solid(4, 1, [9, 9, 9, 255]))
            .unwrap();
        let error = session.finish().unwrap_err();
        assert!(error.contains("缺少 2 行"), "{error}");
    }

    #[test]
    fn dropping_the_session_removes_the_temporary_file() {
        let (_guard, root, output) = workspace();
        let session = prepare_export(&root, &output, "取消", 4, 4).unwrap();
        let temps: Vec<PathBuf> = std::fs::read_dir(&output)
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .collect();
        assert_eq!(temps.len(), 1, "开始导出应当只留下一个临时文件");
        drop(session);
        assert!(
            !temps[0].exists(),
            "丢弃会话必须删掉临时文件：{}",
            temps[0].display()
        );
    }

    #[test]
    fn refuses_output_inside_the_repository() {
        let (_guard, root, _output) = workspace();
        let inside = root.join("exports");
        std::fs::create_dir(&inside).unwrap();
        let error = prepare_export(&root, &inside, "画板", 4, 4)
            .err()
            .expect("导出不得写进仓库目录");
        assert!(error.contains("作品仓库内部"), "{error}");
    }

    #[test]
    fn never_overwrites_an_existing_export() {
        let (_guard, root, output) = workspace();
        for expected in ["画板.png", "画板-2.png"] {
            let mut session = prepare_export(&root, &output, "画板", 2, 1).unwrap();
            session
                .write_tile(0, 0, 2, 1, &solid(2, 1, [0, 0, 0, 0]))
                .unwrap();
            let target = session.finish().unwrap();
            assert_eq!(target.file_name().unwrap(), expected);
        }
    }

    #[test]
    fn sanitises_the_requested_file_name() {
        assert_eq!(export_file_stem("画板"), "画板");
        assert_eq!(export_file_stem("  画板.png "), "画板");
        assert_eq!(export_file_stem("a/b:c*?"), "a_b_c__");
        assert_eq!(export_file_stem("   "), DEFAULT_FILE_STEM);
        assert_eq!(export_file_stem("...."), DEFAULT_FILE_STEM);
        assert_eq!(export_file_stem(&"长".repeat(200)).chars().count(), 80);
    }

    #[test]
    fn clears_temporary_files_left_by_a_killed_process() {
        let (_guard, root, output) = workspace();
        let stale = output.join(format!("{TEMP_PREFIX}dead{TEMP_SUFFIX}"));
        std::fs::write(&stale, b"leftover").unwrap();
        let session = prepare_export(&root, &output, "画板", 2, 1).unwrap();
        assert!(!stale.exists(), "开始导出时应清掉上次强杀的残留");
        drop(session);
    }
}
