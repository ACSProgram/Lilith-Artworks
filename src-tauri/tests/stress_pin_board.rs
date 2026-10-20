//! H 组：画板 DDS 完整性（批次 6）与笔迹载荷（批次 alpha5 阶段 5）。
//!
//! - **H1 双向检查**：记录 → 文件（缺失/损坏）与文件 → 记录（孤儿）四类计数；
//!   缺失与损坏**报告不失败**（命令仍成功返回）。
//! - **H2 孤儿 DDS 回收**：复用统一清理体系的「扫描发现 + 确认清理」闭环，被引用 DDS
//!   不被误删（扫描只报告孤儿，即便被引用文件同样「过期」）。
//! - **H3 规模与取消**：单画板数百张图片下完整性检查仍正确、逐条响应取消、内存有界。
//! - **H4 笔迹 BLOB 边界**：空点、超上限、长度与点数不符、未知版本/工具、形状带
//!   点或带压感、超长 BLOB 全部明确拒绝；上限内的 4096 点必须被接受（单点是合法的
//!   圆点笔迹，不在拒绝之列）。
//! - **H5 笔迹批量读写**：`create-board-strokes` 一次事务写入后，`read-board-strokes`
//!   的载荷逐字节等于按契约拼出的期望值（含摘要比对）；批量里有一条不合法时一条也不写。
//!
//! 画板记录只能由导入/创建流程写入（测试进程不直接访问数据库），因此 H 组先给无头入口
//! 补上 `create-board` / `import-board-images` / `create-board-stroke` /
//! `create-board-strokes` / `read-board-strokes` / `scrub-board-dds` 子命令——薄映射、
//! feature 门控、不进发布产物。断言只用「子命令返回的 JSON」与「磁盘事实」。
//!
//! 运行：`cargo test --features headless --test stress_pin_board`

// 只在启用 `headless` feature 时编译。理由见 `tests/stress_support/mod.rs` 顶部说明。
#![cfg(feature = "headless")]

mod stress_support;

use std::{fs, path::PathBuf, time::Duration};

use serde_json::json;
use sha2::{Digest, Sha256};
use stress_support::*;

/// 画板规模场景会导入数百张图片并做全量校验，留足余量。
const SCENARIO_TIMEOUT: Duration = Duration::from_secs(300);

/// H3 的规模：个人素材板真实上限「数百张」，用 300 张覆盖。单次导入上限为 256
/// （`dds::MAX_IMPORT_FILES`），因此按 100 张一批分三次导入。
const SCALE_TOTAL: usize = 300;
const SCALE_BATCH: usize = 100;

/// 与 `pin_board::stroke` 同一份契约：版本、工具编号与单笔点数上限。
const GEOMETRY_VERSION: u8 = 1;
const TOOL_PEN: u8 = 0;
const TOOL_RECT: u8 = 3;
const MAX_STROKE_POINTS: usize = 4096;
const STROKE_HEADER_BYTES: usize = 8;
const STROKE_POINT_BYTES: usize = 5;
/// 批量二进制里每条目的固定头部长度（itemId / tool / color / 三个 f32 / geometryLen）。
const STROKE_RECORD_HEADER_BYTES: usize = 25;

struct Fixture {
    workspace: Workspace,
    repository: PathBuf,
    artwork_id: String,
    board_id: i64,
    revision: String,
    /// `<repository>/artworks/<artwork-id>/boards/<board-id>`：DDS 的落盘位置。
    board_dir: PathBuf,
    /// 源图（PNG）目录，位于仓库之外的工作区内。
    images_dir: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let workspace = Workspace::new();
        let repository = workspace.repository();
        let work = workspace.work("artwork.bin");
        write_work_file(&work, 96 * 1024, 1);
        headless(&workspace, "init-repository").finish().expect_ok();
        let created = headless(&workspace, "create-artwork")
            .arg("title", "Artwork")
            .arg("branch-title", "Main")
            .arg("source", work.to_string_lossy())
            .finish();
        created.expect_ok();
        let artwork_id = created.data_str("artworkId");
        assert!(!artwork_id.is_empty(), "{}", created.describe());

        let board = headless(&workspace, "create-board")
            .repository(&repository)
            .timeout(SCENARIO_TIMEOUT)
            .arg("artwork", &artwork_id)
            .arg("title", "Board")
            .finish();
        board.expect_ok();
        let board_id = board
            .data("boardId")
            .as_i64()
            .unwrap_or_else(|| panic!("创建画板未返回 boardId：\n{}", board.describe()));
        let revision = board.data_str("revision");
        assert!(!revision.is_empty(), "{}", board.describe());

        let board_dir = repository
            .join("artworks")
            .join(&artwork_id)
            .join("boards")
            .join(board_id.to_string());
        let images_dir = workspace.work("board");
        fs::create_dir_all(&images_dir).expect("无法创建源图目录");
        Self {
            workspace,
            repository,
            artwork_id,
            board_id,
            revision,
            board_dir,
            images_dir,
        }
    }

    fn command(&self, command: &str) -> Spawn<'_> {
        headless(&self.workspace, command)
            .repository(&self.repository)
            .timeout(SCENARIO_TIMEOUT)
    }

    /// 生成一张确定性内容的 PNG 源图（尺寸取 4 的倍数，BC7 编码无边界余量问题）。
    fn write_png(&self, index: usize, size: u32) -> PathBuf {
        let path = self.images_dir.join(format!("img_{index:04}.png"));
        let seed = index as u32;
        let image = image::RgbaImage::from_fn(size, size, |x, y| {
            let value = (x.wrapping_mul(31) ^ y.wrapping_mul(17) ^ seed.wrapping_mul(7)) as u8;
            image::Rgba([value, value.wrapping_mul(3), value.wrapping_add(11), 255])
        });
        image.save(&path).expect("无法写入 PNG 源图");
        path
    }

    /// 导入一批源图，返回新建图片的 id；同时推进本地记录的修订号。
    fn import(&mut self, paths: &[PathBuf]) -> Vec<i64> {
        let joined = paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(",");
        let outcome = self
            .command("import-board-images")
            .arg("board", self.board_id.to_string())
            .arg("revision", &self.revision)
            .arg("paths", joined)
            .finish();
        outcome.expect_ok();
        let revision = outcome.data_str("revision");
        assert!(!revision.is_empty(), "{}", outcome.describe());
        self.revision = revision;
        outcome
            .data("imageIds")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|value| value.as_i64().unwrap_or_default())
            .collect()
    }

    fn dds(&self, image_id: i64) -> PathBuf {
        self.board_dir.join(format!("{image_id}.dds"))
    }

    fn scrub(&self) -> Outcome {
        self.command("scrub-board-dds").finish()
    }

    /// 创建一笔笔迹；几何按十六进制直传（小 BLOB 不必落盘）。
    fn create_stroke_hex(
        &self,
        revision: &str,
        geometry: &[u8],
        width: f64,
        height: f64,
    ) -> Outcome {
        self.command("create-board-stroke")
            .arg("board", self.board_id.to_string())
            .arg("revision", revision)
            .arg("tool", TOOL_PEN.to_string())
            .arg("geometry-hex", hex::encode(geometry))
            .arg("stroke-width", "6")
            .arg("width", width.to_string())
            .arg("height", height.to_string())
            .arg("center-x", "0")
            .arg("center-y", "0")
            .finish()
    }

    /// 读取整板笔迹载荷的摘要。
    fn read_strokes(&self) -> Outcome {
        self.command("read-board-strokes")
            .arg("board", self.board_id.to_string())
            .finish()
    }
}

/// 一笔自由笔的几何 BLOB（布局见 `pin_board::stroke`）。
fn pen_geometry(points: &[(u16, u16, u8)]) -> Vec<u8> {
    let mut bytes = vec![
        GEOMETRY_VERSION,
        TOOL_PEN,
        if points.iter().any(|point| point.2 > 0) {
            0b0000_0100
        } else {
            0
        },
        0,
    ];
    bytes.extend_from_slice(&(points.len() as u32).to_le_bytes());
    for (x, y, pressure) in points {
        bytes.extend_from_slice(&x.to_le_bytes());
        bytes.extend_from_slice(&y.to_le_bytes());
        bytes.push(*pressure);
    }
    bytes
}

/// 形状只写头部：两端点由包围盒与方向位推出。
fn shape_geometry(tool: u8, direction: u8) -> Vec<u8> {
    vec![GEOMETRY_VERSION, tool, direction, 0, 0, 0, 0, 0]
}

/// 批量清单里的一条记录。
#[derive(Clone)]
struct SpecRecord {
    tool: u8,
    stroke_width: f32,
    width: f32,
    height: f32,
    center_x: f32,
    center_y: f32,
    geometry: Vec<u8>,
}

impl SpecRecord {
    fn pen(points: &[(u16, u16, u8)], width: f32, height: f32) -> Self {
        Self {
            tool: TOOL_PEN,
            stroke_width: 6.0,
            width,
            height,
            center_x: 0.0,
            center_y: 0.0,
            geometry: pen_geometry(points),
        }
    }

    fn shape(tool: u8, direction: u8, width: f32, height: f32) -> Self {
        Self {
            tool,
            stroke_width: 4.0,
            width,
            height,
            center_x: 100.0,
            center_y: 100.0,
            geometry: shape_geometry(tool, direction),
        }
    }
}

/// `create-board-strokes` 的 `--spec` 布局（小端）。
fn stroke_spec(records: &[SpecRecord]) -> Vec<u8> {
    let mut bytes = (records.len() as u32).to_le_bytes().to_vec();
    for record in records {
        bytes.push(record.tool);
        for value in [
            record.stroke_width,
            record.width,
            record.height,
            record.center_x,
            record.center_y,
        ] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&(record.geometry.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&record.geometry);
    }
    bytes
}

/// 按 `stroke::encode_batch` 的契约拼出期望载荷：读取侧的字节布局由此逐字节核对。
fn expected_payload(records: &[SpecRecord], item_ids: &[i64], color: &str) -> Vec<u8> {
    let rgb = u32::from_str_radix(color.trim_start_matches('#'), 16).expect("颜色应为 #RRGGBB");
    let mut bytes = (records.len() as u32).to_le_bytes().to_vec();
    for (record, item_id) in records.iter().zip(item_ids) {
        bytes.extend_from_slice(&(*item_id as u32).to_le_bytes());
        bytes.push(record.tool);
        bytes.extend_from_slice(&rgb.to_le_bytes());
        for value in [record.stroke_width, record.width, record.height] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&(record.geometry.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&record.geometry);
    }
    bytes
}

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// H1：双向检查报告「正常 / 缺失 / 损坏 / 孤儿」四类计数，缺失与损坏不使命令失败。
#[test]
fn h1_board_dds_scan_reports_four_classes() {
    let mut fixture = Fixture::new();
    let pngs = (0..3)
        .map(|index| fixture.write_png(index, 16))
        .collect::<Vec<_>>();
    let ids = fixture.import(&pngs);
    assert_eq!(ids.len(), 3, "导入数量不符：{ids:?}");
    for id in &ids {
        assert!(
            fixture.dds(*id).is_file(),
            "导入后 DDS 应存在：{}",
            fixture.dds(*id).display()
        );
    }

    // 缺失：删除第二张的 DDS。
    fs::remove_file(fixture.dds(ids[1])).expect("无法删除 DDS");
    // 损坏：把第三张的 DDS 截断成无效头（不再是合法 DDS）。
    fs::write(fixture.dds(ids[2]), b"DDS ").expect("无法写入损坏 DDS");
    // 孤儿：磁盘上多出一份 DDS，但没有任何记录指向它。
    let orphan = fixture.board_dir.join("999999.dds");
    fs::copy(fixture.dds(ids[0]), &orphan).expect("无法复制孤儿 DDS");

    let report = fixture.scrub();
    // 缺失与损坏**报告不失败**：命令照常成功返回。
    report.expect_ok();
    assert_eq!(report.data_u64("images"), 3, "{}", report.describe());
    assert_eq!(report.data_u64("missing"), 1, "{}", report.describe());
    assert_eq!(report.data_u64("corrupt"), 1, "{}", report.describe());
    assert_eq!(report.data_u64("orphans"), 1, "{}", report.describe());

    assert_reopens(&fixture.workspace, &fixture.repository);

    record(
        "H1 board DDS scan",
        "H",
        "small",
        &report,
        json!({ "images": 3, "missing": 1, "corrupt": 1, "orphans": 1 }),
    );
}

/// H2：孤儿 DDS 经「扫描发现 + 确认清理」被回收，被引用 DDS 不受影响。
#[test]
fn h2_orphan_board_dds_is_discovered_and_reclaimed() {
    let mut fixture = Fixture::new();
    let pngs = (0..2)
        .map(|index| fixture.write_png(index, 16))
        .collect::<Vec<_>>();
    let ids = fixture.import(&pngs);
    assert_eq!(ids.len(), 2, "{ids:?}");
    let referenced = fixture.dds(ids[0]);

    // 孤儿：复制一份被引用 DDS，命名成一个没有记录的 id。
    let orphan_name = "888888.dds";
    let orphan = fixture.board_dir.join(orphan_name);
    fs::copy(&referenced, &orphan).expect("无法复制孤儿 DDS");
    let orphan_relative = format!(
        "artworks/{}/boards/{}/{}",
        fixture.artwork_id, fixture.board_id, orphan_name
    );

    // 扫描有 30 分钟宽限期（避免与进行中的写入赛跑）；把两份 DDS 都回拨到宽限期之外，
    // 让「过期」不再是变量——此时扫描**只**应报告孤儿，被引用 DDS 即便同样过期也不报告，
    // 从而证明引用复查生效。
    backdate(&referenced, 2 * 60 * 60);
    backdate(&orphan, 2 * 60 * 60);

    let scan = fixture.command("scan-unreferenced").finish();
    scan.expect_ok();
    assert_eq!(
        scan.data_u64("count"),
        1,
        "扫描只应报告孤儿 DDS：\n{}",
        scan.describe()
    );
    let candidates = scan
        .data("candidates")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(candidates.len(), 1, "{}", scan.describe());
    assert_eq!(
        candidates[0]["path"].as_str(),
        Some(orphan_relative.as_str()),
        "{}",
        scan.describe()
    );
    assert_eq!(
        candidates[0]["reason"].as_str(),
        Some("画板图片未被引用"),
        "{}",
        scan.describe()
    );

    // 确认清理：入队 + 单遍重放删除孤儿。
    let cleanup = fixture
        .command("cleanup-unreferenced")
        .arg("ids", &orphan_relative)
        .finish();
    cleanup.expect_ok();
    assert_eq!(
        cleanup.data_u64("cleanedCount"),
        1,
        "{}",
        cleanup.describe()
    );
    assert!(!orphan.exists(), "孤儿 DDS 应被删除");
    assert!(referenced.exists(), "被引用 DDS 不得被误删");
    assert_cleanup_empty(&fixture.workspace, &fixture.repository);

    // 再扫描无候选（幂等）。
    let rescan = fixture.command("scan-unreferenced").finish();
    rescan.expect_ok();
    assert_eq!(rescan.data_u64("count"), 0, "{}", rescan.describe());

    // 完整性检查确认仓库自洽：只剩被引用的两张，无孤儿。
    let report = fixture.scrub();
    report.expect_ok();
    assert_eq!(report.data_u64("images"), 2, "{}", report.describe());
    assert_eq!(report.data_u64("missing"), 0, "{}", report.describe());
    assert_eq!(report.data_u64("corrupt"), 0, "{}", report.describe());
    assert_eq!(report.data_u64("orphans"), 0, "{}", report.describe());

    assert_reopens(&fixture.workspace, &fixture.repository);

    record(
        "H2 orphan board DDS reclaimed",
        "H",
        "small",
        &cleanup,
        json!({ "orphan": orphan_relative, "scannedCandidates": 1 }),
    );
}

/// H3：单画板数百张图片下完整性检查仍正确、逐条响应取消、峰值内存有界。
#[test]
fn h3_board_dds_scale_and_cancellation() {
    let mut fixture = Fixture::new();
    let mut ids = Vec::new();
    let mut index = 0;
    while index < SCALE_TOTAL {
        let count = SCALE_BATCH.min(SCALE_TOTAL - index);
        let batch = (0..count)
            .map(|offset| fixture.write_png(index + offset, 32))
            .collect::<Vec<_>>();
        ids.extend(fixture.import(&batch));
        index += count;
    }
    assert_eq!(ids.len(), SCALE_TOTAL, "导入数量不符：{}", ids.len());

    // 规模下完整性检查仍正确：全部记录都命中，无缺失/损坏/孤儿。
    let report = fixture.scrub();
    report.expect_ok();
    assert_eq!(
        report.data_u64("images"),
        SCALE_TOTAL as u64,
        "{}",
        report.describe()
    );
    assert_eq!(report.data_u64("missing"), 0, "{}", report.describe());
    assert_eq!(report.data_u64("corrupt"), 0, "{}", report.describe());
    assert_eq!(report.data_u64("orphans"), 0, "{}", report.describe());

    // 内存有界：debug 档基线约十几 MiB，数百张小图的检查不应出现与图片数成比例的放大。
    if let Some(peak) = report.result["peakWorkingSetBytes"].as_u64() {
        assert!(
            peak < 512 * MIB,
            "画板 DDS 检查峰值内存异常：{peak} 字节\n{}",
            report.describe()
        );
    }

    // 逐条响应取消：在第一个检查点取消（此时只检查了第一张），随后可立即重试成功。
    let cancelled = fixture
        .command("scrub-board-dds")
        .gate()
        .drive(|index, _stage| index == 1);
    cancelled.expect_cancelled();

    let retried = fixture.scrub();
    retried.expect_ok();
    assert_eq!(
        retried.data_u64("images"),
        SCALE_TOTAL as u64,
        "{}",
        retried.describe()
    );
    assert_eq!(retried.data_u64("missing"), 0, "{}", retried.describe());
    assert_eq!(retried.data_u64("orphans"), 0, "{}", retried.describe());

    assert_reopens(&fixture.workspace, &fixture.repository);

    record(
        "H3 board DDS scale and cancel",
        "H",
        "small",
        &retried,
        json!({ "images": SCALE_TOTAL, "cancelledAtCheckpoint": 1 }),
    );
}

/// H4：笔迹几何 BLOB 的边界全部明确拒绝，上限内的 4096 点被接受。
#[test]
fn h4_stroke_geometry_boundaries() {
    let fixture = Fixture::new();
    let mut revision = fixture.revision.clone();

    // 正面边界：4096 点（上限）必须写进去。BLOB 有 20 KB，十六进制放不进命令行
    // （Windows 命令行上限 32767 字符），因此走文件。
    let points = (0..MAX_STROKE_POINTS)
        .map(|index| ((index % 1000) as u16, ((index / 1000) as u16) * 10, 128))
        .collect::<Vec<_>>();
    let max_points = pen_geometry(&points);
    assert_eq!(
        max_points.len(),
        STROKE_HEADER_BYTES + MAX_STROKE_POINTS * STROKE_POINT_BYTES,
        "上限几何长度应等于 8 + 4096 × 5"
    );
    let max_path = fixture.workspace.work("max-stroke.bin");
    fs::write(&max_path, &max_points).expect("无法写入几何文件");
    let accepted = fixture
        .command("create-board-stroke")
        .arg("board", fixture.board_id.to_string())
        .arg("revision", &revision)
        .arg("tool", TOOL_PEN.to_string())
        .arg("geometry", max_path.to_string_lossy())
        .arg("stroke-width", "6")
        .arg("width", "1000")
        .arg("height", "40")
        .arg("center-x", "0")
        .arg("center-y", "0")
        .finish();
    accepted.expect_ok();
    assert!(
        accepted.data("itemId").as_i64().unwrap_or_default() > 0,
        "{}",
        accepted.describe()
    );
    revision = accepted.data_str("revision");
    assert!(!revision.is_empty(), "{}", accepted.describe());

    // 反面边界：每一种畸形几何都要拿到自己的错误文案，而不是被静默接受。
    let mut oversized = max_points.clone();
    oversized.push(0);
    let mut over_limit_points = pen_geometry(&[(0, 0, 0)]);
    over_limit_points[4..8].copy_from_slice(&((MAX_STROKE_POINTS as u32) + 1).to_le_bytes());
    let mut length_mismatch = pen_geometry(&[(0, 0, 0), (10, 10, 0)]);
    length_mismatch.pop();
    let mut bad_version = pen_geometry(&[(0, 0, 0)]);
    bad_version[0] = 2;
    let mut bad_tool = pen_geometry(&[(0, 0, 0)]);
    bad_tool[1] = 9;
    let mut bad_flags = pen_geometry(&[(0, 0, 0)]);
    bad_flags[2] = 0b1000_0000;
    let mut bad_reserved = pen_geometry(&[(0, 0, 0)]);
    bad_reserved[3] = 1;
    let mut shape_with_points = shape_geometry(TOOL_RECT, 0);
    shape_with_points[4..8].copy_from_slice(&1u32.to_le_bytes());

    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("不完整", vec![0u8; 1]),
        ("不完整", pen_geometry(&[(0, 0, 0)])[..7].to_vec()),
        ("至少需要一个点", pen_geometry(&[])),
        ("点数超过上限", over_limit_points),
        ("长度与点数不符", length_mismatch),
        ("版本不受支持", bad_version),
        ("未知的笔迹工具", bad_tool),
        ("标志位无效", bad_flags),
        ("保留字节", bad_reserved),
        ("不应包含点数据", shape_with_points),
        ("不应带压感", shape_geometry(TOOL_RECT, 0b0000_0100)),
    ];
    for (expected, geometry) in &cases {
        let outcome = fixture.create_stroke_hex(&revision, geometry, 100.0, 100.0);
        assert!(!outcome.ok(), "应拒绝：{expected}\n{}", outcome.describe());
        assert!(
            outcome.error().contains(expected),
            "错误文案应包含 {expected:?}，实际 {:?}\n{}",
            outcome.error(),
            outcome.describe()
        );
    }

    // 超长 BLOB 走文件：长度先于其它校验被拒绝。
    let oversized_path = fixture.workspace.work("oversized-stroke.bin");
    fs::write(&oversized_path, &oversized).expect("无法写入超长几何文件");
    let too_long = fixture
        .command("create-board-stroke")
        .arg("board", fixture.board_id.to_string())
        .arg("revision", &revision)
        .arg("tool", TOOL_PEN.to_string())
        .arg("geometry", oversized_path.to_string_lossy())
        .arg("stroke-width", "6")
        .arg("width", "1000")
        .arg("height", "40")
        .finish();
    assert!(!too_long.ok(), "{}", too_long.describe());
    assert!(
        too_long.error().contains("超过上限"),
        "{}",
        too_long.describe()
    );

    // 被拒绝的写入不得改变画板内容：整板仍然只有那一笔。
    let read = fixture.read_strokes();
    read.expect_ok();
    assert_eq!(read.data_u64("count"), 1, "{}", read.describe());
    assert_eq!(
        read.data_u64("bytes") as usize,
        4 + STROKE_RECORD_HEADER_BYTES + max_points.len(),
        "{}",
        read.describe()
    );

    assert_reopens(&fixture.workspace, &fixture.repository);

    record(
        "H4 stroke geometry boundaries",
        "H",
        "small",
        &read,
        json!({ "rejected": cases.len() + 1, "acceptedPoints": MAX_STROKE_POINTS }),
    );
}

/// H5：批量写入后整板载荷逐字节等于按契约拼出的期望值；一条不合法则一条都不写。
#[test]
fn h5_stroke_batch_read_write_roundtrip() {
    let fixture = Fixture::new();
    let color = "#E5484D";
    let records = vec![
        SpecRecord::pen(&[(0, 0, 0), (100, 0, 128), (100, 100, 255)], 100.0, 100.0),
        SpecRecord::pen(&[(0, 0, 0), (50, 50, 64)], 50.0, 50.0),
        SpecRecord::pen(
            &[(0, 0, 0), (20, 0, 0), (40, 20, 0), (60, 0, 0)],
            60.0,
            20.0,
        ),
        SpecRecord::shape(TOOL_RECT, 2, 80.0, 60.0),
        SpecRecord::shape(1, 1, 120.0, 90.0),
    ];
    let spec_path = fixture.workspace.work("strokes.bin");
    fs::write(&spec_path, stroke_spec(&records)).expect("无法写入笔迹清单");

    let created = fixture
        .command("create-board-strokes")
        .arg("board", fixture.board_id.to_string())
        .arg("revision", &fixture.revision)
        .arg("spec", spec_path.to_string_lossy())
        .arg("color", color)
        .finish();
    created.expect_ok();
    assert_eq!(
        created.data_u64("count"),
        records.len() as u64,
        "{}",
        created.describe()
    );
    let item_ids = created
        .data("itemIds")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|value| value.as_i64().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(item_ids.len(), records.len(), "{}", created.describe());
    let revision = created.data_str("revision");
    assert!(!revision.is_empty(), "{}", created.describe());

    // 载荷逐字节核对：布局、条目顺序、颜色打包与几何原样回读。
    let expected = expected_payload(&records, &item_ids, color);
    let read = fixture.read_strokes();
    read.expect_ok();
    assert_eq!(
        read.data_u64("count"),
        records.len() as u64,
        "{}",
        read.describe()
    );
    assert_eq!(
        read.data_u64("bytes") as usize,
        expected.len(),
        "载荷长度不符\n{}",
        read.describe()
    );
    assert_eq!(
        read.data_str("digest"),
        digest(&expected),
        "载荷摘要不符（布局或字段顺序与契约不一致）\n{}",
        read.describe()
    );

    // 批量里有一条畸形几何（空点）：整批拒绝，画板内容不变。单点是合法的圆点笔迹，
    // 不能拿单点当畸形样本。
    let mut broken = records.clone();
    broken.push(SpecRecord::pen(&[], 10.0, 10.0));
    let broken_spec = fixture.workspace.work("strokes-broken.bin");
    fs::write(&broken_spec, stroke_spec(&broken)).expect("无法写入畸形清单");
    let rejected = fixture
        .command("create-board-strokes")
        .arg("board", fixture.board_id.to_string())
        .arg("revision", &revision)
        .arg("spec", broken_spec.to_string_lossy())
        .arg("color", color)
        .finish();
    assert!(!rejected.ok(), "{}", rejected.describe());
    let after = fixture.read_strokes();
    after.expect_ok();
    assert_eq!(
        after.data_u64("count"),
        records.len() as u64,
        "整批应回滚\n{}",
        after.describe()
    );
    assert_eq!(
        after.data_str("digest"),
        digest(&expected),
        "{}",
        after.describe()
    );

    assert_reopens(&fixture.workspace, &fixture.repository);

    record(
        "H5 stroke batch roundtrip",
        "H",
        "small",
        &read,
        json!({
            "strokes": records.len(),
            "payloadBytes": expected.len(),
            "rejectedBatchRolledBack": true,
        }),
    );
}
