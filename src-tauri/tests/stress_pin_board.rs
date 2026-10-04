//! H 组：画板 DDS 完整性（批次 6）。
//!
//! - **H1 双向检查**：记录 → 文件（缺失/损坏）与文件 → 记录（孤儿）四类计数；
//!   缺失与损坏**报告不失败**（命令仍成功返回）。
//! - **H2 孤儿 DDS 回收**：复用统一清理体系的「扫描发现 + 确认清理」闭环，被引用 DDS
//!   不被误删（扫描只报告孤儿，即便被引用文件同样「过期」）。
//! - **H3 规模与取消**：单画板数百张图片下完整性检查仍正确、逐条响应取消、内存有界。
//!
//! 画板记录只能由导入流程写入（测试进程不直接访问数据库），因此 H 组先给无头入口补上
//! `create-board` / `import-board-images` / `scrub-board-dds` 三个子命令——1:1 薄映射、
//! feature 门控、不进发布产物。断言只用「子命令返回的 JSON」与「磁盘事实」。
//!
//! 运行：`cargo test --features headless --test stress_pin_board`

mod stress_support;

use std::{fs, path::PathBuf, time::Duration};

use serde_json::json;
use stress_support::*;

/// 画板规模场景会导入数百张图片并做全量校验，留足余量。
const SCENARIO_TIMEOUT: Duration = Duration::from_secs(300);

/// H3 的规模：个人素材板真实上限「数百张」，用 300 张覆盖。单次导入上限为 256
/// （`dds::MAX_IMPORT_FILES`），因此按 100 张一批分三次导入。
const SCALE_TOTAL: usize = 300;
const SCALE_BATCH: usize = 100;

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
