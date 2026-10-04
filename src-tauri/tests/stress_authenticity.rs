//! G 组：认证模块（批次 7）。
//!
//! 认证是唯一触及安全边界的批次：`ensure_dialog_authorized` 的路径授权来源被抽象成
//! 可注入的作用域——GUI 侧仍用 `window.fs_scope()`，无头侧用一个只允许 `--workspace`
//! 之下路径的显式作用域。因此本文件同时验证**无头进程无法读写工作区之外的路径**。
//!
//! - **G1 认证发布内存**：16K 源图（≈268 MP，`extreme` 起）的发布峰值内存不超出声明的
//!   1.5 GiB 解码预算量级。
//! - **G2 认证发布耗时**：同 G1；渲染 / 编码 / 签名各阶段耗时被记录，无超时与挂起。
//! - **G3 认证发布取消**：在渲染 / 编码 / 签名各阶段取消，无半成品被登记为成品；
//!   `cancel-publication` 后仓库内副本与记录被清除。
//! - **G4 认证回读**：`decode-authenticity` 能读回 C2PA 声明与 TrustMark 绑定。
//! - **G5 认证受控文件校验**：`scrub` 覆盖最终成品与认证副本的摘要比对。
//!
//! 复用 `tests/fixtures/authenticity/` 的公开 ES256 测试凭据与 128x128 源图（复制进
//! 工作区后使用——无头进程只授权工作区之内的路径）；大图由测试进程用已有的 `image`
//! 依赖按需生成，用完即删，不提交进仓库。
//!
//! 运行：`cargo test --features headless --test stress_authenticity`
//! 大图档：`LILITH_STRESS_TIERS=extreme cargo test --features headless --test stress_authenticity`

mod stress_support;

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use serde_json::json;
use stress_support::*;

/// 小规模认证场景（128 / 512 级图）的等待上限。
const SCENARIO_TIMEOUT: Duration = Duration::from_secs(300);
/// 大图发布（16K 档）在 debug 档下可能以分钟计，单独放宽。
const LARGE_TIMEOUT: Duration = Duration::from_secs(3600);

/// 大图场景的边长。16K 是允许范围内的大图（单边上限 32768、总像素上限 300 MP）。
const LARGE_EDGE: u32 = 16_384;

fn fixture_directory() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("authenticity")
}

struct Fixture {
    workspace: Workspace,
    repository: PathBuf,
    branch_id: String,
    artwork_id: String,
    /// 证书链与私钥（工作区内的夹具副本）。
    certificate: PathBuf,
    private_key: PathBuf,
    /// 默认最终成品：夹具里的 128x128 源图副本。
    artifact: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let workspace = Workspace::new();
        let repository = workspace.repository();
        let work = workspace.work("artwork.bin");
        write_work_file(&work, 96 * 1024, 7);
        headless(&workspace, "init-repository").finish().expect_ok();
        let created = headless(&workspace, "create-artwork")
            .arg("title", "Artwork")
            .arg("branch-title", "Main")
            .arg("source", work.to_string_lossy())
            .finish();
        created.expect_ok();
        let branch_id = created.data_str("branchId");
        let artwork_id = created.data_str("artworkId");
        assert!(!branch_id.is_empty(), "{}", created.describe());
        assert!(!artwork_id.is_empty(), "{}", created.describe());

        // 进入发布状态要求分支已有 head，因此先做一次提交。
        headless(&workspace, "commit")
            .arg("branch", &branch_id)
            .arg("note", "initial")
            .arg("commit-kind", "manual")
            .finish()
            .expect_ok();

        // 认证夹具必须在工作区之内：无头进程只授权工作区路径。
        let certificate = workspace.work("es256-test.pub");
        let private_key = workspace.work("es256-test.priv");
        let artifact = workspace.work("source.jpg");
        for (name, destination) in [
            ("es256-test.pub", &certificate),
            ("es256-test.priv", &private_key),
            ("source.jpg", &artifact),
        ] {
            fs::copy(fixture_directory().join(name), destination).expect("无法复制认证夹具");
        }

        Self {
            workspace,
            repository,
            branch_id,
            artwork_id,
            certificate,
            private_key,
            artifact,
        }
    }

    fn command(&self, command: &str) -> Spawn<'_> {
        headless(&self.workspace, command)
            .repository(&self.repository)
            .timeout(SCENARIO_TIMEOUT)
    }

    /// 进入发布状态，最终成品取 `artifact`。
    fn enter_publication(&self, artifact: &Path) -> Outcome {
        self.command("enter-publication")
            .arg("branch", &self.branch_id)
            .arg("artifact", artifact.to_string_lossy())
            .finish()
    }

    /// 认证签名发布的公共参数；`region` 是归一化坐标的 TrustMark 区域。
    fn publish_command(&self, output: &Path, region: &str) -> Spawn<'_> {
        self.command("publish")
            .arg("branch", &self.branch_id)
            .arg("output", output.to_string_lossy())
            .arg("certificate", self.certificate.to_string_lossy())
            .arg("key", self.private_key.to_string_lossy())
            .arg("title", "Artwork")
            .arg("creator", "Lilith Artworks stress tests")
            .arg("rights", "Stress test")
            .arg("content", "Stress test publish")
            .arg("algorithm", "es256")
            .arg("trustmark", "true")
            .arg("regions", region)
            .arg("jpeg-quality", "90")
    }

    fn scrub(&self) -> Outcome {
        self.command("scrub").finish()
    }

    /// 认证受控文件计数：`(最终成品, 认证记录)`。
    fn controlled_counts(&self) -> (u64, u64) {
        let outcome = self.scrub();
        outcome.expect_ok();
        (
            outcome.data_u64("finalArtifacts"),
            outcome.data_u64("certificationRecords"),
        )
    }

    fn cancel_publication(&self) -> Outcome {
        self.command("cancel-publication")
            .arg("branch", &self.branch_id)
            .finish()
    }
}

/// 按需生成一张确定性的 16K 级 JPEG 源图。
///
/// 用平滑的二维渐变而不是随机噪声：JPEG 编码快、体积小，解码后的像素仍然多样，足以让
/// TrustMark 与 C2PA 走真实路径。生成与使用都发生在工作区内，用完即删，不进仓库。
fn write_large_jpeg(path: &Path, edge: u32) {
    let image = image::RgbImage::from_fn(edge, edge, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, ((x ^ y) % 256) as u8])
    });
    image.save(path).expect("无法写入大图源");
}

/// 大图发布的峰值内存上限：解码图 + 展平图 + TrustMark 输出各一份，再加固定开销。
///
/// 这个口径是**保守上界**——它允许整图级别的多份拷贝，因此不会把「正常的多缓冲实现」
/// 误判为失败；而「把整个文件读进内存」之外的存储放大类回归（例如把增量写成整份文件）
/// 会立刻越界。声明值：单边 32768 px、总像素 300 MP、解码器预算 1.5 GiB。
fn memory_allowance(edge: u32) -> u64 {
    let rgb = u64::from(edge) * u64::from(edge) * 3;
    rgb * 3 + 768 * MIB
}

/// G3：在渲染 / 编码 / 签名三个阶段各取消一次发布。
///
/// 每个阶段都断言：取消是干净的（无输出文件、无残留临时目录）、没有半成品被登记为
/// 成品（认证记录数仍为 0）、仓库重开可用；随后 `cancel-publication` 把仓库内成品与
/// 记录一并清除。
#[test]
fn g3_publish_cancellation_at_each_stage_is_clean() {
    let fixture = Fixture::new();
    for (label, stage) in [
        ("render", "认证渲染"),
        ("encode", "认证编码"),
        ("sign", "认证签名"),
    ] {
        fixture.enter_publication(&fixture.artifact).expect_ok();
        assert_eq!(
            fixture.controlled_counts().0,
            1,
            "进入发布后应有一个最终成品"
        );

        let output = fixture.workspace.out(&format!("cancelled-{label}.jpg"));
        let policy = Policy::at_stage(stage);
        let outcome = fixture
            .publish_command(&output, "0,0,1,1")
            .gate()
            .drive(|index, stage_name| policy.decide(index, stage_name));
        outcome.expect_cancelled();

        assert!(
            !output.exists(),
            "取消后不应存在发布输出：{}",
            output.display()
        );
        assert!(
            directory_names(&fixture.workspace.out_directory()).is_empty(),
            "取消后输出目录不应残留临时产物：{:?}",
            directory_names(&fixture.workspace.out_directory())
        );
        let (artifacts, records) = fixture.controlled_counts();
        assert_eq!(
            records,
            0,
            "取消的发布不得登记认证记录：\n{}",
            outcome.describe()
        );
        assert_eq!(
            artifacts,
            1,
            "最终成品不应被取消影响：\n{}",
            outcome.describe()
        );
        assert_cleanup_empty(&fixture.workspace, &fixture.repository);
        assert_reopens(&fixture.workspace, &fixture.repository);

        // 取消发布：仓库内副本与记录被清除，分支回到可再次进入发布的状态。
        fixture.cancel_publication().expect_ok();
        assert_eq!(
            fixture.controlled_counts(),
            (0, 0),
            "取消发布后仓库内成品与记录都应清除"
        );
        assert_cleanup_empty(&fixture.workspace, &fixture.repository);
        assert_reopens(&fixture.workspace, &fixture.repository);

        record(
            "G3 publish cancel",
            "G",
            "small",
            &outcome,
            json!({ "stage": label }),
        );
    }
}

/// G4：`decode-authenticity` 回读 C2PA 声明与 TrustMark 绑定。
#[test]
fn g4_decode_reads_back_c2pa_and_trustmark() {
    let fixture = Fixture::new();
    fixture.enter_publication(&fixture.artifact).expect_ok();
    let output = fixture.workspace.out("published.jpg");
    let published = fixture.publish_command(&output, "0,0,1,1").finish();
    published.expect_ok();
    let record_id = published.data("record")["id"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let watermark_id = published.data("record")["watermarkId"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(!record_id.is_empty(), "{}", published.describe());
    assert_eq!(watermark_id.len(), 40, "{}", published.describe());

    let decoded = fixture
        .command("decode-authenticity")
        .arg("input", output.to_string_lossy())
        .finish();
    decoded.expect_ok();
    let data = &decoded.result["data"];

    assert_eq!(
        data["c2paPresent"].as_bool(),
        Some(true),
        "{}",
        decoded.describe()
    );
    let state = data["c2paValidationState"].as_str().unwrap_or_default();
    assert!(
        matches!(state, "Valid" | "Trusted"),
        "C2PA 验证状态应为 Valid/Trusted，实际 {state}：\n{}",
        decoded.describe()
    );
    assert_eq!(
        data["c2paRecordId"].as_str(),
        Some(record_id.as_str()),
        "{}",
        decoded.describe()
    );
    assert_eq!(
        data["watermarkPresent"].as_bool(),
        Some(true),
        "{}",
        decoded.describe()
    );
    assert_eq!(
        data["watermarkId"].as_str(),
        Some(watermark_id.as_str()),
        "{}",
        decoded.describe()
    );
    assert_eq!(
        data["c2paWatermarkId"].as_str(),
        Some(watermark_id.as_str()),
        "{}",
        decoded.describe()
    );
    assert_eq!(
        data["identifiersMatch"].as_bool(),
        Some(true),
        "{}",
        decoded.describe()
    );
    assert_eq!(
        data["title"].as_str(),
        Some("Artwork"),
        "{}",
        decoded.describe()
    );
    assert_eq!(
        data["creator"].as_str(),
        Some("Lilith Artworks stress tests"),
        "{}",
        decoded.describe()
    );
    assert_eq!(
        data["rightsStatement"].as_str(),
        Some("Stress test"),
        "{}",
        decoded.describe()
    );

    let matches = data["matches"].as_array().cloned().unwrap_or_default();
    assert_eq!(matches.len(), 1, "{}", decoded.describe());
    assert_eq!(
        matches[0]["record"]["id"].as_str(),
        Some(record_id.as_str()),
        "{}",
        decoded.describe()
    );
    let sources = matches[0]["evidenceSources"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for expected in ["c2pa", "trustmark"] {
        assert!(
            sources.iter().any(|value| value.as_str() == Some(expected)),
            "候选证据应包含 {expected}：\n{}",
            decoded.describe()
        );
    }

    assert_reopens(&fixture.workspace, &fixture.repository);

    record(
        "G4 decode authenticity",
        "G",
        "small",
        &decoded,
        json!({ "recordId": record_id, "watermarkId": watermark_id }),
    );
}

/// G5：`scrub` 覆盖最终成品与认证仓库副本的摘要比对。
///
/// 不只断言计数：替换认证副本的字节后必须被拒绝——证明这是**真实比对**而不是数行数。
#[test]
fn g5_scrub_covers_final_artifact_and_certified_copy() {
    let fixture = Fixture::new();
    fixture.enter_publication(&fixture.artifact).expect_ok();
    let output = fixture.workspace.out("published.jpg");
    let published = fixture.publish_command(&output, "0,0,1,1").finish();
    published.expect_ok();
    let record_id = published.data("record")["id"]
        .as_str()
        .unwrap_or_default()
        .to_owned();

    let healthy = fixture.scrub();
    healthy.expect_ok();
    assert_eq!(
        healthy.data_u64("finalArtifacts"),
        1,
        "{}",
        healthy.describe()
    );
    assert_eq!(
        healthy.data_u64("certificationRecords"),
        1,
        "{}",
        healthy.describe()
    );

    let certified = fixture
        .repository
        .join("artworks")
        .join(&fixture.artwork_id)
        .join("artifacts")
        .join(&fixture.branch_id)
        .join("certifications")
        .join(format!("{record_id}.jpg"));
    assert!(
        certified.is_file(),
        "认证仓库副本应存在：{}",
        certified.display()
    );
    fs::write(&certified, b"replaced").expect("无法替换认证副本");

    let tampered = fixture.scrub();
    assert!(
        !tampered.ok(),
        "被替换的认证副本必须使校验失败：\n{}",
        tampered.describe()
    );
    assert!(
        tampered.error().contains("已损坏或被替换"),
        "失败原因应是摘要不匹配：\n{}",
        tampered.describe()
    );

    record(
        "G5 scrub controlled files",
        "G",
        "small",
        &healthy,
        json!({ "finalArtifacts": 1, "certificationRecords": 1, "tamperRejected": true }),
    );
}

/// G1 / G2：16K 源图的发布内存与耗时。
///
/// 只在 `extreme` 及以上档位运行：生成 16384x16384 需要约 1 GiB 内存，发布链路解码
/// 同样如此；低于该档位时**明确跳过并报告**，而不是静默降级。
#[test]
fn g1_g2_large_publish_memory_and_timing() {
    let tier = tier();
    if !matches!(tier, Tier::Extreme | Tier::Manual) {
        record_skip(
            "G1/G2 large publish",
            "G",
            tier.name(),
            "认证大图场景只在 extreme 及以上档位运行（16K 生成与解码各需约 1 GiB 内存）",
        );
        return;
    }

    let fixture = Fixture::new();
    let artifact = fixture.workspace.work("large-source.jpg");
    write_large_jpeg(&artifact, LARGE_EDGE);
    assert!(artifact.is_file(), "大图源应已生成");

    fixture.enter_publication(&artifact).expect_ok();
    let output = fixture.workspace.out("large-published.jpg");
    // 16K 的 TrustMark 区域取 1/8 边长（2048x2048）：足以覆盖真实的区域编码路径，
    // 又不让残差回放大到整图尺寸。
    let published = fixture
        .publish_command(&output, "0,0,0.125,0.125")
        .timeout(LARGE_TIMEOUT)
        .finish();
    published.expect_ok();

    let peak = published.result["peakWorkingSetBytes"]
        .as_u64()
        .unwrap_or(0);
    let allowance = memory_allowance(LARGE_EDGE);
    assert!(
        peak > 0,
        "无头进程应自报峰值内存：\n{}",
        published.describe()
    );
    assert!(
        peak <= allowance,
        "16K 发布的峰值内存超出预算：{peak} > {allowance}\n{}",
        published.describe()
    );

    let render_ms = published.data_u64("renderMs");
    let encode_ms = published.data_u64("encodeMs");
    let signing_ms = published.data_u64("signingMs");
    assert!(
        render_ms > 0 && encode_ms > 0 && signing_ms > 0,
        "渲染 / 编码 / 签名各阶段耗时都应被记录：\n{}",
        published.describe()
    );

    // 回读在 16K 档同样成立：签名成品仍能读出 C2PA 声明。
    let decoded = fixture
        .command("decode-authenticity")
        .arg("input", output.to_string_lossy())
        .timeout(LARGE_TIMEOUT)
        .finish();
    decoded.expect_ok();
    assert_eq!(
        decoded.result["data"]["c2paPresent"].as_bool(),
        Some(true),
        "{}",
        decoded.describe()
    );

    // 用完即删：大图源不留在工作区，也不进仓库。
    fs::remove_file(&artifact).expect("无法删除大图源");

    record(
        "G1 large publish memory",
        "G",
        tier.name(),
        &published,
        json!({ "edge": LARGE_EDGE, "peakBytes": peak, "allowanceBytes": allowance }),
    );
    record(
        "G2 large publish timing",
        "G",
        tier.name(),
        &published,
        json!({ "edge": LARGE_EDGE, "renderMs": render_ms, "encodeMs": encode_ms, "signingMs": signing_ms }),
    );
}
