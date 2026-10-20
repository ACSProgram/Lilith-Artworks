//! 笔迹条目的几何契约：BLOB 布局、校验与批量二进制打包。
//!
//! 几何只存数据库（`pin_board_strokes.geometry`），不建纹理、不落实体文件。布局
//! 小端、紧凑、带版本字节，读侧逐项校验长度——单笔 300 点约 1.5 KB：
//!
//! ```text
//! [0]      version = 1
//! [1]      tool     0=pen 1=line 2=arrow 3=rect 4=ellipse
//! [2]      flags    bit0-1 形状方向（起点落在哪个角）, bit2 是否带压感
//! [3]      reserved = 0
//! [4..7]   pointCount  u32（形状恒为 0）
//! [8..]    pen：pointCount × (u16 x, u16 y, u8 pressure)，x/y 为条目包围盒内的归一化坐标
//!          形状：无额外字节，两端点由包围盒 + 方向位推出
//! ```
//!
//! 坐标量化为 `u16`（4000 px 宽的包围盒仍有 0.06 px 分辨率），压感量化为 `u8`；
//! 点数上限 [`MAX_STROKE_POINTS`]，超限由校验拒绝；距离抽稀由采样侧在编码前完成。
//!
//! 前端解析在 `src/modules/pin-board/strokeGeometry.ts`，两边的校验口径必须一致：
//! 任一侧改动都要同步另一侧与各自测试。

/// 笔迹固定落在标注层（[`STROKE_LAYER`]），永远在所有图片与文字之上。
pub(crate) const STROKE_LAYER: u8 = 3;
/// 单笔点数上限，与前端 `MAX_STROKE_POINTS` 一致。超限由校验拒绝。
pub(crate) const MAX_STROKE_POINTS: usize = 4096;
/// 几何 BLOB 的头部长度（version / tool / flags / reserved / pointCount）。
pub(crate) const STROKE_HEADER_BYTES: usize = 8;
/// 单个笔迹点占用的字节数（u16 x + u16 y + u8 pressure）。
pub(crate) const STROKE_POINT_BYTES: usize = 5;
/// 几何 BLOB 长度上限，与 `pin_board_strokes.geometry` 的 CHECK 一致。
pub(crate) const MAX_STROKE_GEOMETRY_BYTES: usize =
    STROKE_HEADER_BYTES + MAX_STROKE_POINTS * STROKE_POINT_BYTES;
/// 批量二进制里每条目的固定头部长度（itemId / tool / color / 三个 f32 / geometryLen）。
pub(crate) const STROKE_RECORD_HEADER_BYTES: usize = 4 + 1 + 4 + 4 + 4 + 4 + 4;
pub(crate) const STROKE_GEOMETRY_VERSION: u8 = 1;
/// 包围盒逻辑尺寸上限。素材板的世界坐标不归一化，这里只挡明显异常的量级。
pub(crate) const MAX_STROKE_EXTENT: f64 = 1_000_000.0;
/// 线宽上限，与 `pin_board_strokes.stroke_width` 的 CHECK 一致。
pub(crate) const MAX_STROKE_WIDTH: f64 = 1000.0;

pub(crate) const TOOL_PEN: u8 = 0;
// 工具编号与前端 `strokeGeometry.ts` 的 `STROKE_TOOL` 一一对应。本批次只用到
// `TOOL_PEN` 的分支判断，其余编号是给阶段 2 的 SDF 渲染准备的，先按契约登记。
#[allow(dead_code)]
pub(crate) const TOOL_LINE: u8 = 1;
#[allow(dead_code)]
pub(crate) const TOOL_ARROW: u8 = 2;
#[allow(dead_code)]
pub(crate) const TOOL_RECT: u8 = 3;
#[allow(dead_code)]
pub(crate) const TOOL_ELLIPSE: u8 = 4;
const TOOL_COUNT: u8 = 5;

#[allow(dead_code)]
const FLAG_DIRECTION_MASK: u8 = 0b0000_0011;
const FLAG_PRESSURE: u8 = 0b0000_0100;
/// flags 的高 5 位保留，读侧要求为 0：将来加标志时旧程序会拒绝而不是误读。
const FLAG_RESERVED_MASK: u8 = 0b1111_1000;

/// 几何 BLOB 头部解析结果。`point_count` 已与长度校验过。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StrokeHeader {
    pub(crate) tool: u8,
    pub(crate) flags: u8,
    pub(crate) point_count: u32,
}

impl StrokeHeader {
    /// 形状方向位（`0..=3`，起点落在包围盒的哪个角）。仅形状有意义。
    #[allow(dead_code)]
    pub(crate) fn direction(self) -> u8 {
        self.flags & FLAG_DIRECTION_MASK
    }

    /// 是否带压感。仅自由笔有意义。
    pub(crate) fn has_pressure(self) -> bool {
        self.flags & FLAG_PRESSURE != 0
    }

    /// 是否为形状（矩形/椭圆/直线/箭头），即不携带点数据。
    pub(crate) fn is_shape(self) -> bool {
        self.tool != TOOL_PEN
    }
}

/// 校验几何 BLOB 并返回头部。
///
/// 长度、版本、工具、保留位、点数与逐点数据长度任一不满足都返回明确错误：几何
/// 来自渲染进程，读侧不能假设它是自己写出的形状。
pub(crate) fn validate_geometry(geometry: &[u8]) -> Result<StrokeHeader, String> {
    if geometry.len() < STROKE_HEADER_BYTES {
        return Err("笔迹几何数据不完整".into());
    }
    if geometry.len() > MAX_STROKE_GEOMETRY_BYTES {
        return Err(format!(
            "笔迹几何数据超过上限（{} 字节）",
            MAX_STROKE_GEOMETRY_BYTES
        ));
    }
    let version = geometry[0];
    if version != STROKE_GEOMETRY_VERSION {
        return Err(format!("笔迹几何版本不受支持：{version}"));
    }
    let tool = geometry[1];
    if tool >= TOOL_COUNT {
        return Err(format!("未知的笔迹工具：{tool}"));
    }
    let flags = geometry[2];
    if flags & FLAG_RESERVED_MASK != 0 {
        return Err("笔迹几何标志位无效".into());
    }
    if geometry[3] != 0 {
        return Err("笔迹几何的保留字节必须为 0".into());
    }
    let point_count = u32::from_le_bytes([geometry[4], geometry[5], geometry[6], geometry[7]]);
    if point_count as usize > MAX_STROKE_POINTS {
        return Err(format!("笔迹点数超过上限（{} 点）", MAX_STROKE_POINTS));
    }
    let header = StrokeHeader {
        tool,
        flags,
        point_count,
    };
    if header.is_shape() {
        if point_count != 0 {
            return Err("形状笔迹不应包含点数据".into());
        }
        if geometry.len() != STROKE_HEADER_BYTES {
            return Err("形状笔迹的几何长度无效".into());
        }
        if header.has_pressure() {
            return Err("形状笔迹不应带压感标志".into());
        }
        return Ok(header);
    }
    // 自由笔：点一下抬笔（单点）是合法笔迹，画出来的就是一个圆点。
    if point_count == 0 {
        return Err("笔迹至少需要一个点".into());
    }
    let expected = STROKE_HEADER_BYTES + point_count as usize * STROKE_POINT_BYTES;
    if geometry.len() != expected {
        return Err(format!(
            "笔迹几何长度与点数不符：{} 点应为 {expected} 字节，实际 {} 字节",
            point_count,
            geometry.len()
        ));
    }
    Ok(header)
}

/// 校验包围盒与线宽。笔迹、文字与位图的尺寸列同名同义，这里只管笔迹这一侧。
pub(crate) fn validate_extent(width: f64, height: f64, stroke_width: f64) -> Result<(), String> {
    if !width.is_finite()
        || !height.is_finite()
        || width <= 0.0
        || height <= 0.0
        || width > MAX_STROKE_EXTENT
        || height > MAX_STROKE_EXTENT
    {
        return Err("笔迹的包围盒尺寸无效".into());
    }
    if !stroke_width.is_finite() || stroke_width <= 0.0 || stroke_width > MAX_STROKE_WIDTH {
        return Err(format!("笔迹线宽无效（应在 0 到 {MAX_STROKE_WIDTH} 之间）"));
    }
    Ok(())
}

/// `#RRGGBB` 的形状判定（大小写不限），校验与打包共用同一份口径。
fn is_hex_color(color: &str) -> bool {
    let bytes = color.as_bytes();
    bytes.len() == 7 && bytes[0] == b'#' && bytes[1..].iter().all(u8::is_ascii_hexdigit)
}

/// 校验 `#RRGGBB` 并返回大写形式。`label` 只参与错误文案（文字/笔迹共用）。
pub(crate) fn normalise_hex_color(color: &str, label: &str) -> Result<String, String> {
    if !is_hex_color(color) {
        return Err(format!("{label}颜色必须是 #RRGGBB"));
    }
    Ok(color.to_ascii_uppercase())
}

/// `#RRGGBB` → `0x00RRGGBB`，供二进制打包使用。
pub(crate) fn color_rgb(color: &str) -> Option<u32> {
    if !is_hex_color(color) {
        return None;
    }
    u32::from_str_radix(&color[1..], 16)
        .ok()
        .map(|value| value & 0x00FF_FFFF)
}

/// 一条笔迹在批量二进制里的完整内容（几何仍然是不透明 BLOB）。
#[derive(Debug, Clone)]
pub(crate) struct StrokeRecord {
    pub(crate) item_id: i64,
    pub(crate) tool: u8,
    pub(crate) color: String,
    pub(crate) stroke_width: f64,
    pub(crate) width: f64,
    pub(crate) height: f64,
    pub(crate) geometry: Vec<u8>,
}

/// 批量打包布局：`u32 条目数` + 每条目
/// `(u32 itemId, u8 tool, u32 color, f32 strokeWidth, f32 width, f32 height,
/// u32 geometryLen, bytes)`，全部小端。每条目的固定头部 [`STROKE_RECORD_HEADER_BYTES`]。
///
/// 前端一次解析成 `Map<itemId, StrokeGeometry>`，不经过 JSON。条目顺序按 item id
/// 升序，便于两侧测试逐字节比对。
pub(crate) fn encode_batch(records: &[StrokeRecord]) -> Result<Vec<u8>, String> {
    let mut buffer = Vec::with_capacity(
        4 + records
            .iter()
            .map(|record| STROKE_RECORD_HEADER_BYTES + record.geometry.len())
            .sum::<usize>(),
    );
    let count = u32::try_from(records.len()).map_err(|_| "笔迹条目数量超过上限")?;
    buffer.extend_from_slice(&count.to_le_bytes());
    for record in records {
        let item_id = u32::try_from(record.item_id).map_err(|_| "笔迹条目编号超过上限")?;
        let color = color_rgb(&record.color)
            .ok_or_else(|| format!("笔迹条目 {} 的颜色无效", record.item_id))?;
        let geometry_len = u32::try_from(record.geometry.len())
            .map_err(|_| format!("笔迹条目 {} 的几何数据超过上限", record.item_id))?;
        buffer.extend_from_slice(&item_id.to_le_bytes());
        buffer.push(record.tool);
        buffer.extend_from_slice(&color.to_le_bytes());
        buffer.extend_from_slice(&(record.stroke_width as f32).to_le_bytes());
        buffer.extend_from_slice(&(record.width as f32).to_le_bytes());
        buffer.extend_from_slice(&(record.height as f32).to_le_bytes());
        buffer.extend_from_slice(&geometry_len.to_le_bytes());
        buffer.extend_from_slice(&record.geometry);
    }
    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pen(flags: u8, points: usize) -> Vec<u8> {
        let mut geometry = vec![STROKE_GEOMETRY_VERSION, TOOL_PEN, flags, 0];
        geometry.extend_from_slice(&(points as u32).to_le_bytes());
        geometry.resize(STROKE_HEADER_BYTES + points * STROKE_POINT_BYTES, 0);
        geometry
    }

    fn shape(tool: u8, flags: u8) -> Vec<u8> {
        let mut geometry = vec![STROKE_GEOMETRY_VERSION, tool, flags, 0];
        geometry.extend_from_slice(&0u32.to_le_bytes());
        geometry
    }

    #[test]
    fn accepts_single_point_and_full_length_strokes() {
        let single = validate_geometry(&pen(0, 1)).unwrap();
        assert_eq!(single.tool, TOOL_PEN);
        assert_eq!(single.point_count, 1);
        assert!(!single.is_shape());

        let full = validate_geometry(&pen(0b0000_0100, MAX_STROKE_POINTS)).unwrap();
        assert_eq!(full.point_count as usize, MAX_STROKE_POINTS);
        assert!(full.has_pressure());
    }

    #[test]
    fn accepts_every_shape_without_points() {
        for (tool, direction) in [
            (TOOL_LINE, 1),
            (TOOL_ARROW, 2),
            (TOOL_RECT, 2),
            (TOOL_ELLIPSE, 3),
        ] {
            let header = validate_geometry(&shape(tool, direction)).unwrap();
            assert!(header.is_shape());
            assert_eq!(header.direction(), direction);
        }
    }

    #[test]
    fn rejects_malformed_geometry() {
        assert!(validate_geometry(&[]).unwrap_err().contains("不完整"));
        assert!(validate_geometry(&pen(0, 1)[..7])
            .unwrap_err()
            .contains("不完整"));

        // 空点（自由笔没有点）
        assert!(validate_geometry(&pen(0, 0))
            .unwrap_err()
            .contains("至少需要一个点"));

        // 超上限
        let mut oversized = pen(0, 1);
        oversized[4..8].copy_from_slice(&((MAX_STROKE_POINTS as u32) + 1).to_le_bytes());
        assert!(validate_geometry(&oversized)
            .unwrap_err()
            .contains("点数超过上限"));

        // 长度与点数不符
        let mut truncated = pen(0, 2);
        truncated.pop();
        assert!(validate_geometry(&truncated)
            .unwrap_err()
            .contains("长度与点数不符"));

        // 未知版本 / 未知工具
        let mut bad_version = pen(0, 1);
        bad_version[0] = 2;
        assert!(validate_geometry(&bad_version)
            .unwrap_err()
            .contains("版本不受支持"));
        let mut bad_tool = pen(0, 1);
        bad_tool[1] = 9;
        assert!(validate_geometry(&bad_tool)
            .unwrap_err()
            .contains("未知的笔迹工具"));

        // 保留位与保留字节
        let mut bad_flags = pen(0, 1);
        bad_flags[2] = 0b1000_0000;
        assert!(validate_geometry(&bad_flags)
            .unwrap_err()
            .contains("标志位无效"));
        let mut bad_reserved = pen(0, 1);
        bad_reserved[3] = 1;
        assert!(validate_geometry(&bad_reserved)
            .unwrap_err()
            .contains("保留字节"));

        // 形状带点数据 / 形状带压感
        let mut shape_with_points = shape(TOOL_RECT, 0);
        shape_with_points[4..8].copy_from_slice(&1u32.to_le_bytes());
        assert!(validate_geometry(&shape_with_points)
            .unwrap_err()
            .contains("不应包含点数据"));
        assert!(validate_geometry(&shape(TOOL_RECT, 0b0000_0100))
            .unwrap_err()
            .contains("不应带压感"));
    }

    #[test]
    fn geometry_byte_limits_match_the_schema_check() {
        // `pin_board_strokes.geometry` 的 CHECK 写死了 8..=20488；改上限必须同步改表。
        assert_eq!(STROKE_HEADER_BYTES, 8);
        assert_eq!(MAX_STROKE_GEOMETRY_BYTES, 20_488);
    }

    #[test]
    fn rejects_invalid_extent_and_width() {
        assert!(validate_extent(10.0, 10.0, 2.0).is_ok());
        assert!(validate_extent(0.0, 10.0, 2.0).is_err());
        assert!(validate_extent(10.0, f64::NAN, 2.0).is_err());
        assert!(validate_extent(MAX_STROKE_EXTENT * 2.0, 10.0, 2.0).is_err());
        assert!(validate_extent(10.0, 10.0, 0.0).is_err());
        assert!(validate_extent(10.0, 10.0, MAX_STROKE_WIDTH + 1.0).is_err());
    }

    #[test]
    fn parses_colors_into_packed_rgb() {
        assert_eq!(normalise_hex_color("#a1b2c3", "笔迹").unwrap(), "#A1B2C3");
        assert!(normalise_hex_color("a1b2c3", "笔迹").is_err());
        assert_eq!(color_rgb("#A1B2C3"), Some(0x00A1_B2C3));
        assert_eq!(color_rgb("#FFFFFF"), Some(0x00FF_FFFF));
        assert_eq!(color_rgb("nope"), None);
    }

    #[test]
    fn encodes_batches_with_the_documented_layout() {
        let records = vec![
            StrokeRecord {
                item_id: 7,
                tool: TOOL_PEN,
                color: "#010203".into(),
                stroke_width: 2.5,
                width: 100.0,
                height: 40.0,
                geometry: pen(0, 2),
            },
            StrokeRecord {
                item_id: 9,
                tool: TOOL_RECT,
                color: "#AABBCC".into(),
                stroke_width: 1.0,
                width: 30.0,
                height: 30.0,
                geometry: shape(TOOL_RECT, 2),
            },
        ];
        let bytes = encode_batch(&records).unwrap();
        assert_eq!(u32::from_le_bytes(bytes[0..4].try_into().unwrap()), 2);

        let mut offset = 4;
        assert_eq!(
            u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()),
            7
        );
        offset += 4;
        assert_eq!(bytes[offset], TOOL_PEN);
        offset += 1;
        assert_eq!(
            u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()),
            0x0001_0203
        );
        offset += 4;
        assert_eq!(
            f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()),
            2.5
        );
        offset += 4;
        assert_eq!(
            f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()),
            100.0
        );
        offset += 4;
        assert_eq!(
            f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()),
            40.0
        );
        offset += 4;
        let geometry_len =
            u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        assert_eq!(geometry_len, STROKE_HEADER_BYTES + 2 * STROKE_POINT_BYTES);
        assert_eq!(&bytes[offset..offset + geometry_len], &pen(0, 2)[..]);

        // 第二条紧跟其后：头部 25 字节 + 几何。
        offset += geometry_len;
        assert_eq!(
            u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()),
            9
        );
        assert_eq!(
            bytes.len(),
            4 + STROKE_RECORD_HEADER_BYTES
                + geometry_len
                + STROKE_RECORD_HEADER_BYTES
                + STROKE_HEADER_BYTES
        );
    }

    #[test]
    fn rejects_unpackable_records() {
        let record = |item_id: i64, color: &str| StrokeRecord {
            item_id,
            tool: TOOL_PEN,
            color: color.to_owned(),
            stroke_width: 1.0,
            width: 1.0,
            height: 1.0,
            geometry: pen(0, 1),
        };
        let out_of_range = record(i64::from(u32::MAX) + 1, "#000000");
        assert!(encode_batch(&[out_of_range])
            .unwrap_err()
            .contains("编号超过上限"));
        assert!(encode_batch(&[record(1, "#zzzzzz")])
            .unwrap_err()
            .contains("颜色无效"));
    }
}
