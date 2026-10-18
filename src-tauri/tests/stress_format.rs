//! M 组：ChunkFile 格式矩阵的端到端覆盖（0.3 阶段 4 的集成接缝补强）。
//!
//! 报告里的组字母 `F` 已被 `stress_input` 的「参数边界」占用，本组因此用 `M`
//! （matrix），场景编号 M1–M4 与 `docs/user/stress-test-report.md` 一一对应。
//!
//! `cargo test --lib` 的单测覆盖**格式层**：头与块记录布局、摘要、容器解析、
//! 畸形输入拒绝、v1/v2 互读、写满注入。本组补的是**集成接缝**——在真实仓库里
//! 经无头二进制走「设置/开关 → 提交 → delta → 恢复 / 检查点 / scrub」全链路，
//! 并用**磁盘事实**（ChunkFile 公共头的版本/chunker/adapter 字节）断言仓库内
//! 确实是预期的格式组合，而不是静默回退。
//!
//! 覆盖矩阵：
//!
//! - **M1 `.clip` tile 结构**：合成 CSFC 提交为按 CLIP 结构分块的 v2 快照 →
//!   只改一个 tile 的载荷再提交为 v2 delta → 两个节点恢复逐字节一致，
//!   `scrub` 全链通过。
//! - **M2 `.clip` 无 tile 记录**（screencast 形态）：Exta 整体是单一结构单元，
//!   提交 → 恢复逐字节一致。
//! - **M3 畸形/截断 `.clip`**：容器探测失败回退 CDC（chunker = CDC），提交 →
//!   恢复逐字节一致——结构路径的失败不得污染恢复正确性。
//! - **M4 v1/v2 混存仓库**：`--chunk-format v1` 写两个节点 → 默认 v2 再写两个
//!   节点（仓库内 v1/v2 并存）→ 四个节点全部恢复逐字节一致 → 对 v1 delta
//!   节点建检查点（跨格式物化成 v2 快照）→ 再次恢复一致。
//!
//! 全部夹具在运行时按确定性种子合成（见 `stress_support` 的合成器），**不提交
//! 二进制样本**；真实 Clip Studio 输出的形态由维护者手工验收覆盖，不由此组代表。
//! 与 `docs/guides/validation.md` 的分层一致：MiB 级以下、秒级完成的便宜分组，
//! 随 Windows CI 运行。
//!
//! 运行：`cargo test --features headless --test stress_format`

// 只在启用 `headless` feature 时编译。理由见 `tests/stress_support/mod.rs` 顶部说明。
#![cfg(feature = "headless")]

mod stress_support;

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde_json::json;
use stress_support::*;

/// ChunkFile 公共头布局（`src/backup/chunk_file/format.rs`）：magic(8) +
/// version u16**le** + flags u16le + chunker u16le + hash u16le，v2 再有
/// codec u16le + adapter u16le。数值与 `chunk_file/mod.rs` 的常量一一对应
/// （注意 ChunkFile 头是小端；CSFC 容器字段才是大端）。
const SNAPSHOT_MAGIC: &[u8; 8] = b"LBCHUNK\0";
const DELTA_MAGIC: &[u8; 8] = b"LBDELTA\0";
/// delta 落盘时**整体**经 zstd level 6 包装（`create_reverse_delta`），头与
/// 记录都在解压流里；未压缩的旧布局按同一魔数识别（`ChunkFileDelta::open_with`）。
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];
/// v2 chunker：0 = CDC，1 = CLIP 结构，2 = PSD 结构。
const CHUNKER_CDC: u16 = 0;
const CHUNKER_CLIP: u16 = 1;
/// v2 adapter：0 = 无，1 = CLIP，2 = PSD。
const ADAPTER_NONE: u16 = 0;
const ADAPTER_CLIP: u16 = 1;

/// 合成 `.clip` 的一个修订：6 个 8 KiB tile；`revision` 只改变第 3 个 tile 的
/// 载荷，结构布局（块序列与各记录长度）保持不变，因此修订之间是一笔「小改动」。
fn clip_revision(revision: u64) -> Vec<u8> {
    let mut bodies: Vec<Vec<u8>> = (0..6)
        .map(|index| pattern_bytes(8 * 1024, 100 + index))
        .collect();
    bodies[2] = pattern_bytes(8 * 1024, 900 + revision);
    let bodies: Vec<&[u8]> = bodies.iter().map(Vec::as_slice).collect();
    synthetic_clip(&bodies)
}

/// 磁盘事实：ChunkFile 公共头。解析失败（不是受管理的 ChunkFile）返回 `None`。
///
/// `adapter` 只对 v2 有意义；v1 布局在该偏移处是 config 字段，调用方不得解读。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChunkHead {
    kind: &'static str,
    version: u16,
    chunker: u16,
    adapter: u16,
}

/// 取文件的前 20 字节；delta 先过一遍 zstd 解压（整文件是一股压缩流）。
///
/// 与 `ChunkFileDelta::open_with` 同一手法：探测前缀后**必须回到文件起点**再交给
/// 解码器——帧魔数在偏移 0，读掉 4 字节前缀不解开，Decoder 会从帧中段开始而失败。
fn chunk_head_bytes(path: &Path) -> Option<[u8; 20]> {
    let mut file = fs::File::open(path).ok()?;
    let mut prefix = [0_u8; 4];
    file.read_exact(&mut prefix).ok()?;
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut head = [0_u8; 20];
    if prefix == ZSTD_MAGIC {
        let mut decoder = zstd::stream::read::Decoder::new(file).ok()?;
        decoder.read_exact(&mut head).ok()?;
    } else {
        file.read_exact(&mut head).ok()?;
    }
    Some(head)
}

fn read_chunk_head(path: &Path) -> Option<ChunkHead> {
    let head = chunk_head_bytes(path)?;
    let kind = if &head[..8] == SNAPSHOT_MAGIC {
        "snapshot"
    } else if &head[..8] == DELTA_MAGIC {
        "delta"
    } else {
        return None;
    };
    Some(ChunkHead {
        kind,
        version: u16::from_le_bytes([head[8], head[9]]),
        chunker: u16::from_le_bytes([head[12], head[13]]),
        adapter: u16::from_le_bytes([head[18], head[19]]),
    })
}

/// 仓库内全部受管理 ChunkFile 的磁盘事实：`(仓库相对路径, 公共头)`。
fn chunk_heads(fixture: &ArtworkFixture) -> Vec<(String, ChunkHead)> {
    let state = storage_state(&fixture.repository);
    let mut heads = Vec::new();
    for relative in state.snapshots.iter().chain(state.deltas.iter()) {
        let path = fixture.repository.join(relative);
        let head = read_chunk_head(&path)
            .unwrap_or_else(|| panic!("受管理文件缺少 ChunkFile 头：{}", path.display()));
        heads.push((relative.clone(), head));
    }
    heads
}

/// 仓库内**恰有一枚**该类型的 ChunkFile 时返回它的公共头，数量不为 1 即失败。
///
/// 「恰有一枚」是这些场景的断言前提（例如两节点链 = head 快照 + 一条边 delta），
/// 用 `find` 会放过多出来的游离文件。
fn expect_head(heads: &[(String, ChunkHead)], kind: &str) -> ChunkHead {
    let mut matched = heads.iter().filter(|(_, head)| head.kind == kind);
    let first = matched
        .next()
        .unwrap_or_else(|| panic!("仓库内应有一枚 {kind}：{heads:?}"));
    assert!(
        matched.next().is_none(),
        "仓库内应恰有一枚 {kind}：{heads:?}"
    );
    first.1
}

/// M1：`.clip` tile 结构分块的快照 + delta 全链路，两个节点恢复逐字节一致。
#[test]
fn m1_clip_structural_chunking_round_trips_through_the_real_chain() {
    let initial = clip_revision(1);
    let edited = clip_revision(2);
    let fixture = ArtworkFixture::with_work_file("artwork.clip", &initial);

    let first = fixture.commit_contents(&initial, "clip 初始提交");
    let second = fixture.commit_contents(&edited, "clip 改一个 tile");

    // 磁盘事实：快照确实是 CLIP 结构分块的 v2，没有静默回退 CDC；第二次提交
    // 写出 v2 delta（其 target_adapter 同样落在 CLIP 布局上）。
    let heads = chunk_heads(&fixture);
    let snapshot = expect_head(&heads, "snapshot");
    assert_eq!(snapshot.version, 2, "{heads:?}");
    assert_eq!(snapshot.chunker, CHUNKER_CLIP, "{heads:?}");
    assert_eq!(snapshot.adapter, ADAPTER_CLIP, "{heads:?}");
    let delta = expect_head(&heads, "delta");
    assert_eq!(delta.version, 2, "{heads:?}");
    assert_eq!(delta.chunker, CHUNKER_CLIP, "{heads:?}");
    assert_eq!(delta.adapter, ADAPTER_CLIP, "{heads:?}");

    // 两端恢复逐字节一致。
    let restored = fixture.restore_outcome(&first, "f1-first.clip");
    assert_eq!(
        fs::read(&restored.0).expect("无法读取恢复输出"),
        initial,
        "最早节点的恢复必须与提交时逐位一致"
    );
    assert_eq!(fixture.restore_to(&second, "f1-second.clip"), edited);

    // 全链 scrub（逐块复算摘要）与重开校验通过，且无残留。
    assert_eq!(fixture.healthy(), 2, "两次提交应产生两个历史节点");
    fixture.assert_repository_clean();

    record(
        "M1 clip structural round trip",
        "M",
        "small",
        &restored.1,
        json!({ "nodes": 2, "tileBytes": 8 * 1024 }),
    );
}

/// M2：无 tile 记录的 `.clip`（screencast 形态）——Exta 整体是单一结构单元，
/// 提交 → 恢复逐字节一致。
#[test]
fn m2_clip_without_tile_records_stays_one_structural_unit() {
    let contents = synthetic_clip_without_tiles(&[0x78, 0x9c, 1, 2, 3, 4, 5, 6]);
    let fixture = ArtworkFixture::with_work_file("screencast.clip", &contents);

    let node = fixture.commit_contents(&contents, "无 tile 形态");

    let heads = chunk_heads(&fixture);
    let snapshot = expect_head(&heads, "snapshot");
    assert_eq!(snapshot.version, 2, "{heads:?}");
    assert_eq!(snapshot.chunker, CHUNKER_CLIP, "{heads:?}");
    assert_eq!(snapshot.adapter, ADAPTER_CLIP, "{heads:?}");

    let restored = fixture.restore_outcome(&node, "f2.clip");
    assert_eq!(
        fs::read(&restored.0).expect("无法读取恢复输出"),
        contents,
        "无 tile 形态的恢复必须与提交时逐位一致"
    );
    assert_eq!(fixture.healthy(), 1);
    fixture.assert_repository_clean();

    record(
        "M2 clip without tile records",
        "M",
        "small",
        &restored.1,
        json!({ "nodes": 1 }),
    );
}

/// M3：截断的 `.clip`——容器探测失败，回退 CDC 后照常提交与恢复。
#[test]
fn m3_truncated_clip_falls_back_to_cdc_and_still_restores() {
    let mut contents = clip_revision(1);
    // 整块删掉 Foot 块（16 B 块头 + 0 载荷）：解析器以「缺少 Foot」拒绝，
    // 与 `malformed_container_falls_back_to_cdc` 单测同一手法。
    contents.truncate(contents.len() - 16);
    let fixture = ArtworkFixture::with_work_file("broken.clip", &contents);

    let node = fixture.commit_contents(&contents, "截断容器");

    // 磁盘事实：回退后是 CDC chunker、无 adapter 的 v2 快照。
    let heads = chunk_heads(&fixture);
    let snapshot = expect_head(&heads, "snapshot");
    assert_eq!(snapshot.version, 2, "{heads:?}");
    assert_eq!(snapshot.chunker, CHUNKER_CDC, "{heads:?}");
    assert_eq!(snapshot.adapter, ADAPTER_NONE, "{heads:?}");

    let restored = fixture.restore_outcome(&node, "f3.clip");
    assert_eq!(
        fs::read(&restored.0).expect("无法读取恢复输出"),
        contents,
        "回退 CDC 后的恢复必须与提交时逐位一致"
    );
    assert_eq!(fixture.healthy(), 1);
    fixture.assert_repository_clean();

    record(
        "M3 truncated clip falls back to CDC",
        "M",
        "small",
        &restored.1,
        json!({ "nodes": 1, "truncatedBytes": 16 }),
    );
}

/// M4：v1/v2 混存仓库——两种布局并存、跨格式链全部可恢复，检查点把跨格式链
/// 物化成 v2 快照。
///
/// 步进断言依托一个设计事实：**只有分支 head 保留完整 snapshot**，父节点快照在
/// 提交成功后按设计释放（`history_commit_release`），祖先内容经 head 快照加一串
/// 反向增量恢复。因此 v1 提交的**快照**在下一步即被回收，存活下来的是它写出的
/// v1 **delta**——断言跟着这一走-释放节奏逐提交核对。
#[test]
fn m4_v1_and_v2_nodes_coexist_and_all_restore() {
    let fixture = ArtworkFixture::new();
    let first = pattern_bytes(96 * 1024, 1);
    let second = pattern_bytes(96 * 1024, 2);
    let third = pattern_bytes(96 * 1024, 3);
    let fourth = pattern_bytes(96 * 1024, 4);

    // 提交 1（v1）：head 快照是 v1。
    let v1_first = fixture.commit_contents_with_format(&first, "v1 初始", Some("v1"));
    let heads = chunk_heads(&fixture);
    assert_eq!(heads.len(), 1, "首个提交只发布快照：{heads:?}");
    assert_eq!(heads[0].1.kind, "snapshot");
    assert_eq!(
        heads[0].1.version, 1,
        "--chunk-format v1 应写出 v1 快照：{heads:?}"
    );

    // 提交 2（v1）：新 head 快照仍是 v1，边 delta 也是 v1；提交 1 的父快照按设计
    // 被释放（不再有第二枚 v1 快照）。
    let v1_second = fixture.commit_contents_with_format(&second, "v1 改动", Some("v1"));
    let heads = chunk_heads(&fixture);
    assert_eq!(heads.len(), 2, "{heads:?}");
    assert_eq!(
        heads.iter().filter(|(_, head)| head.version == 1).count(),
        2,
        "v1 head 快照 + v1 delta 都应存活：{heads:?}"
    );

    // 提交 3（默认 v2）：新 delta 是 v2；提交 2 的 v1 快照被释放，存活的 v1 文件
    // 恰好是提交 2 写出的那条 delta。
    let v2_third = fixture.commit_contents(&third, "v2 改动");
    let heads = chunk_heads(&fixture);
    assert_eq!(heads.len(), 3, "{heads:?}");
    let v1_deltas: Vec<_> = heads
        .iter()
        .filter(|(_, head)| head.kind == "delta" && head.version == 1)
        .collect();
    assert_eq!(v1_deltas.len(), 1, "混存仓库应恰有一条 v1 delta：{heads:?}");
    assert!(
        heads
            .iter()
            .any(|(_, head)| head.kind == "delta" && head.version == 2),
        "v2 提交应写出 v2 delta：{heads:?}"
    );

    // 提交 4（默认 v2）：head 快照是 v2。
    let v2_fourth = fixture.commit_contents(&fourth, "v2 再改");
    let heads = chunk_heads(&fixture);
    assert_eq!(heads.len(), 4, "{heads:?}");
    let head_snapshot = heads
        .iter()
        .find(|(_, head)| head.kind == "snapshot")
        .expect("head 应持有快照");
    assert_eq!(
        head_snapshot.1.version, 2,
        "默认提交应写出 v2 快照：{heads:?}"
    );
    assert_eq!(
        heads
            .iter()
            .filter(|(_, head)| head.kind == "delta" && head.version == 2)
            .count(),
        2,
        "两次 v2 提交应写出两条 v2 delta：{heads:?}"
    );

    // 四个节点（含跨 v1→v2 边界的链）全部恢复逐字节一致。
    assert_eq!(fixture.restore_to(&v1_first, "f4-1.bin"), first);
    assert_eq!(fixture.restore_to(&v1_second, "f4-2.bin"), second);
    assert_eq!(fixture.restore_to(&v2_third, "f4-3.bin"), third);
    let restored = fixture.restore_outcome(&v2_fourth, "f4-4.bin");
    assert_eq!(
        fs::read(&restored.0).expect("无法读取恢复输出"),
        fourth,
        "v2 head 的恢复必须与提交时逐位一致"
    );

    // 对分支起点（v1 节点）建检查点：它的物化链是「v2 head 快照 + 两条 v2 delta
    // + 一条 v1 delta」，跨格式；按当前设置写成 v2 快照。
    let checkpoint_path = fixture
        .artwork_directory()
        .join("snapshots")
        .join(format!("{v1_first}.lbc"));
    assert!(
        !checkpoint_path.exists(),
        "该节点的快照已被释放，建检查点前不应存在：{}",
        checkpoint_path.display()
    );
    fixture
        .command("checkpoint")
        .arg("history", &v1_first)
        .finish()
        .expect_ok();
    let materialized = read_chunk_head(&checkpoint_path)
        .unwrap_or_else(|| panic!("检查点应发布 snapshot：{}", checkpoint_path.display()));
    assert_eq!(materialized.kind, "snapshot");
    assert_eq!(materialized.version, 2, "跨格式物化应按当前设置写 v2");

    // 物化之后四个节点仍然全部可恢复，逐字节一致。
    assert_eq!(fixture.restore_to(&v1_first, "f4-1-again.bin"), first);
    assert_eq!(fixture.restore_to(&v1_second, "f4-2-again.bin"), second);
    assert_eq!(fixture.restore_to(&v2_third, "f4-3-again.bin"), third);
    assert_eq!(fixture.restore_to(&v2_fourth, "f4-4-again.bin"), fourth);

    assert_eq!(fixture.healthy(), 4, "检查点不创建新节点");
    fixture.assert_repository_clean();

    record(
        "M4 v1/v2 mixed repository",
        "M",
        "small",
        &restored.1,
        json!({ "nodes": 4, "v1Nodes": 2, "v2Nodes": 2 }),
    );
}
