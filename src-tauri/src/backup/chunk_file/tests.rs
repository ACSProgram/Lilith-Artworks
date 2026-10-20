//! ChunkFile layout, segment-read and reverse-delta regressions.
//!
//! Covers deterministic malformed-input fuzzing, disk-full injection, the
//! randomized chain property test and v1/v2 compatibility assertions.

use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};

use crate::{
    backup::test_support::{QuotaWriter, Rng},
    cancel::CancelToken,
    fuzz_support::{mutated, mutated_header, random_bytes},
};

use super::{
    delta::delta_payload_limit,
    format::{write_delta_header, DeltaHeader},
    *,
};

fn v1_create<R: Read + Seek + Send, W: Write + Seek>(
    source: &mut R,
    snapshot: &mut W,
    config: ChunkingConfig,
) -> ChunkFileResult<ChunkFile> {
    let task = ChunkTask::untracked();
    ChunkFile::create(source, snapshot, config, ChunkWriteOptions::V1, &task)
}

fn v2_create<R: Read + Seek + Send, W: Write + Seek>(
    source: &mut R,
    snapshot: &mut W,
    config: ChunkingConfig,
    chunking: ChunkingStrategy,
) -> ChunkFileResult<ChunkFile> {
    let task = ChunkTask::untracked();
    ChunkFile::create(
        source,
        snapshot,
        config,
        ChunkWriteOptions {
            format: FormatVersion::V2,
            chunking,
        },
        &task,
    )
}

fn encode_snapshot(bytes: &[u8]) -> Vec<u8> {
    let mut output = Cursor::new(Vec::new());
    v1_create(
        &mut Cursor::new(bytes),
        &mut output,
        ChunkingConfig::default(),
    )
    .unwrap();
    output.into_inner()
}

fn encode_v2_snapshot(bytes: &[u8]) -> Vec<u8> {
    let mut output = Cursor::new(Vec::new());
    v2_create(
        &mut Cursor::new(bytes),
        &mut output,
        ChunkingConfig::default(),
        ChunkingStrategy::Cdc,
    )
    .unwrap();
    output.into_inner()
}

fn encode_reverse_delta(child: &[u8], parent: &[u8]) -> Vec<u8> {
    let config = ChunkingConfig::default();
    let task = ChunkTask::untracked();
    let mut child_snapshot = Cursor::new(Vec::new());
    let child_chunk = v1_create(&mut Cursor::new(child), &mut child_snapshot, config).unwrap();
    let mut parent_snapshot = Cursor::new(Vec::new());
    let parent_chunk = v1_create(&mut Cursor::new(parent), &mut parent_snapshot, config).unwrap();
    let mut encoded = Cursor::new(Vec::new());
    child_chunk
        .create_reverse_delta(
            &parent_chunk,
            &mut parent_snapshot,
            &mut encoded,
            ChunkWriteOptions::V1,
            &task,
        )
        .unwrap();
    encoded.into_inner()
}

fn encode_v2_reverse_delta(child: &[u8], parent: &[u8]) -> Vec<u8> {
    let config = ChunkingConfig::default();
    let task = ChunkTask::untracked();
    let mut child_snapshot = Cursor::new(Vec::new());
    let child_chunk = v2_create(
        &mut Cursor::new(child),
        &mut child_snapshot,
        config,
        ChunkingStrategy::Cdc,
    )
    .unwrap();
    let mut parent_snapshot = Cursor::new(Vec::new());
    let parent_chunk = v2_create(
        &mut Cursor::new(parent),
        &mut parent_snapshot,
        config,
        ChunkingStrategy::Cdc,
    )
    .unwrap();
    let mut encoded = Cursor::new(Vec::new());
    child_chunk
        .create_reverse_delta(
            &parent_chunk,
            &mut parent_snapshot,
            &mut encoded,
            ChunkWriteOptions::V2,
            &task,
        )
        .unwrap();
    encoded.into_inner()
}

#[test]
fn resolved_chain_matches_eager_materialization() {
    // A three-version chain whose middle revision introduces fresh chunk
    // payloads, so resolving the oldest revision has to copy chunk bytes
    // out of an earlier delta payload segment instead of the base snapshot.
    let oldest = vec![b'A'; 96 * 1024];
    let mut middle = oldest.clone();
    middle[32 * 1024..40 * 1024].copy_from_slice(&vec![b'B'; 8 * 1024]);
    let mut newest = middle.clone();
    newest[72 * 1024..84 * 1024].copy_from_slice(&vec![b'C'; 12 * 1024]);

    let newest_snapshot = encode_snapshot(&newest);
    let mut store = ChunkStore::new();
    store
        .append(Box::new(Cursor::new(newest_snapshot.clone())))
        .unwrap();
    let mut layout = ChunkFile::open(&mut Cursor::new(newest_snapshot)).unwrap();
    let task = ChunkTask::untracked();
    for (child, parent) in [(&newest, &middle), (&middle, &oldest)] {
        let delta =
            ChunkFileDelta::open(&mut Cursor::new(encode_reverse_delta(child, parent))).unwrap();
        layout = delta.resolve(&layout, &mut store, &task).unwrap();
        assert_eq!(
            layout.file_digest().to_hex(),
            ChunkFile::open(&mut Cursor::new(encode_snapshot(parent)))
                .unwrap()
                .file_digest()
                .to_hex()
        );
    }

    let mut materialized = Cursor::new(Vec::new());
    {
        let mut reader = store.reader();
        layout
            .write_snapshot(&mut reader, &mut materialized, ChunkWriteOptions::V1, &task)
            .unwrap();
    }
    assert_eq!(materialized.into_inner(), encode_snapshot(&oldest));

    let mut restored = Vec::new();
    {
        let mut reader = store.reader();
        layout
            .copy_original(&mut reader, &mut restored, &task)
            .unwrap();
    }
    assert_eq!(restored, oldest);
}

#[test]
fn v2_snapshot_round_trips_and_deduplicates_repeated_payloads() {
    let repeated = vec![b'D'; 64 * 1024];
    let bytes = [repeated.clone(), repeated.clone(), repeated].concat();

    let mut v1 = Cursor::new(Vec::new());
    v1_create(&mut Cursor::new(&bytes), &mut v1, ChunkingConfig::default()).unwrap();
    let mut v2 = Cursor::new(Vec::new());
    let layout = v2_create(
        &mut Cursor::new(&bytes),
        &mut v2,
        ChunkingConfig::default(),
        ChunkingStrategy::Cdc,
    )
    .unwrap();
    let v1 = v1.into_inner();
    let v2 = v2.into_inner();

    assert!(v2.len() < v1.len(), "v2 dedup should shrink repeated data");
    let opened = ChunkFile::open(&mut Cursor::new(v2.clone())).unwrap();
    let task = ChunkTask::untracked();
    let mut restored = Vec::new();
    opened
        .copy_original(&mut Cursor::new(v2), &mut restored, &task)
        .unwrap();
    assert_eq!(restored, bytes);
    let deduplicated = layout
        .chunk_records()
        .iter()
        .any(|record| record.stored_length == 0);
    assert!(deduplicated, "v2 去重后必须存在 stored_len = 0 的引用记录");
}

#[test]
fn v1_snapshot_stays_byte_identical_to_the_legacy_layout() {
    let bytes = vec![b'L'; 48 * 1024];
    let encoded = encode_snapshot(&bytes);
    assert_eq!(
        u16::from_le_bytes(encoded[8..10].try_into().unwrap()),
        FORMAT_VERSION_V1
    );
    assert_eq!(
        encoded.len(),
        V1_SNAPSHOT_HEADER_SIZE + V1_RECORD_HEADER_SIZE + bytes.len()
    );
    let opened = ChunkFile::open(&mut Cursor::new(encoded.clone())).unwrap();
    let task = ChunkTask::untracked();
    let mut restored = Vec::new();
    opened
        .copy_original(&mut Cursor::new(encoded), &mut restored, &task)
        .unwrap();
    assert_eq!(restored, bytes);
}

#[test]
fn v2_delta_round_trips_against_a_v1_or_v2_base() {
    let parent = vec![b'P'; 96 * 1024];
    let mut child = parent.clone();
    child[16 * 1024..20 * 1024].copy_from_slice(&[b'X'; 4 * 1024]);

    let encoded = encode_v2_reverse_delta(&child, &parent);
    let delta = ChunkFileDelta::open(&mut Cursor::new(encoded.clone())).unwrap();
    let task = ChunkTask::untracked();
    let mut child_snapshot = Cursor::new(Vec::new());
    let child_chunk = v2_create(
        &mut Cursor::new(&child),
        &mut child_snapshot,
        ChunkingConfig::default(),
        ChunkingStrategy::Cdc,
    )
    .unwrap();
    let mut restored_snapshot = Cursor::new(Vec::new());
    let restored = delta
        .apply(&child_chunk, &mut child_snapshot, &mut restored_snapshot)
        .unwrap();
    let mut restored_bytes = Vec::new();
    restored
        .copy_original(&mut restored_snapshot, &mut restored_bytes, &task)
        .unwrap();
    assert_eq!(restored_bytes, parent);

    // A v1 base remains readable underneath a v2 chain.
    let mut v1_base = Cursor::new(Vec::new());
    let v1_layout = v1_create(
        &mut Cursor::new(&child),
        &mut v1_base,
        ChunkingConfig::default(),
    )
    .unwrap();
    let mut store = ChunkStore::new();
    store
        .append(Box::new(Cursor::new(encode_snapshot(&child))))
        .unwrap();
    let resolved = ChunkFileDelta::open(&mut Cursor::new(encoded))
        .unwrap()
        .resolve(&v1_layout, &mut store, &task)
        .unwrap();
    assert_eq!(
        resolved.file_digest().to_hex(),
        ChunkFile::open(&mut Cursor::new(encode_snapshot(&parent)))
            .unwrap()
            .file_digest()
            .to_hex()
    );
}

#[test]
fn v2_container_chunking_aligns_to_clip_tiles() {
    // Synthetic CSFC with two small tile records. Structural chunking must
    // produce one chunk per unit, including the header and trailing unit.
    let file = synthetic_clip();
    let mut output = Cursor::new(Vec::new());
    let layout = v2_create(
        &mut Cursor::new(&file),
        &mut output,
        ChunkingConfig::default(),
        ChunkingStrategy::ContainerAware,
    )
    .unwrap();
    assert_eq!(layout.chunker, V2_CHUNKER_CLIP);
    assert_eq!(layout.adapter, V2_ADAPTER_CLIP);
    assert!(layout.chunk_count() >= 4);
    let mut restored = Vec::new();
    let task = ChunkTask::untracked();
    layout
        .copy_original(&mut Cursor::new(output.into_inner()), &mut restored, &task)
        .unwrap();
    assert_eq!(restored, file);
}

#[test]
fn v2_structural_snapshot_can_be_reopened() {
    // Structural units are not CDC chunks: the container header (24 B) and
    // the Exta header (56 B) are far below `min_size`, so reopening the
    // written file must not apply the CDC minimum length to them.
    let file = synthetic_clip();
    let mut output = Cursor::new(Vec::new());
    v2_create(
        &mut Cursor::new(&file),
        &mut output,
        ChunkingConfig::default(),
        ChunkingStrategy::ContainerAware,
    )
    .unwrap();
    let encoded = output.into_inner();
    let reopened = ChunkFile::open(&mut Cursor::new(encoded.clone())).unwrap();
    let mut restored = Vec::new();
    reopened
        .copy_original(
            &mut Cursor::new(encoded),
            &mut restored,
            &ChunkTask::untracked(),
        )
        .unwrap();
    assert_eq!(restored, file);
}

#[test]
fn malformed_container_falls_back_to_cdc() {
    let mut bytes = synthetic_clip();
    bytes.truncate(bytes.len() - 3);
    let mut output = Cursor::new(Vec::new());
    let layout = v2_create(
        &mut Cursor::new(&bytes),
        &mut output,
        ChunkingConfig::default(),
        ChunkingStrategy::ContainerAware,
    )
    .unwrap();
    assert_eq!(layout.chunker, V2_CHUNKER_CDC);
    assert_eq!(layout.adapter, V2_ADAPTER_NONE);
    let task = ChunkTask::untracked();
    let mut restored = Vec::new();
    layout
        .copy_original(&mut Cursor::new(output.into_inner()), &mut restored, &task)
        .unwrap();
    assert_eq!(restored, bytes);
}

#[test]
fn byte_progress_reports_a_cumulative_total() {
    // 70 KiB 按默认块大小会切成多块：每一步都要累加，而不是把本块的字节
    // 直接当成进度条的当前位置（那样进度会一直在 0 附近来回跳）。
    let bytes: Vec<u8> = (0..70_000_u32)
        .map(|value| u8::try_from(value % 251).unwrap())
        .collect();
    let mut snapshot = Cursor::new(Vec::new());
    let layout = v1_create(
        &mut Cursor::new(&bytes),
        &mut snapshot,
        ChunkingConfig::default(),
    )
    .unwrap();
    assert!(layout.chunk_count() > 1);

    let token = CancelToken::standalone();
    let mut restored = Vec::new();
    let seen = {
        let seen = std::cell::RefCell::new(Vec::new());
        let progress = |processed: u64| seen.borrow_mut().push(processed);
        let task = ChunkTask::new(&token, Some(&progress));
        layout
            .copy_original(
                &mut Cursor::new(snapshot.into_inner()),
                &mut restored,
                &task,
            )
            .unwrap();
        seen.into_inner()
    };

    assert_eq!(restored, bytes);
    assert_eq!(seen.len(), layout.chunk_count());
    assert!(
        seen.windows(2).all(|pair| pair[0] <= pair[1]),
        "累计进度不得回退：{seen:?}"
    );
    assert_eq!(seen.last().copied(), Some(layout.logical_size()));
}

#[test]
fn cancellation_stops_snapshot_creation_and_delta_reads() {
    let token = CancelToken::standalone();
    token.cancel();
    let task = ChunkTask::cancel(&token);
    let mut output = Cursor::new(Vec::new());
    let error = ChunkFile::create(
        &mut Cursor::new(vec![b'C'; 96 * 1024]),
        &mut output,
        ChunkingConfig::default(),
        ChunkWriteOptions::V2,
        &task,
    )
    .unwrap_err();
    assert!(matches!(error, ChunkFileError::Cancelled));

    let encoded = encode_reverse_delta(&[b'A'; 64 * 1024], &[b'B'; 64 * 1024]);
    let error = ChunkFileDelta::open_in(&mut Cursor::new(encoded), &std::env::temp_dir(), &task)
        .unwrap_err();
    assert!(matches!(error, ChunkFileError::Cancelled));
}

#[test]
fn unknown_versions_and_records_are_incompatible_not_corrupt() {
    let mut snapshot = encode_v2_snapshot(&[b'V'; 96 * 1024]);
    snapshot[8..10].copy_from_slice(&9_u16.to_le_bytes());
    let error = ChunkFile::open(&mut Cursor::new(snapshot)).unwrap_err();
    assert!(matches!(
        error,
        ChunkFileError::Incompatible(ErrorDimension::Format, _)
    ));

    let mut codec = encode_v2_snapshot(&[b'V'; 96 * 1024]);
    let record = V2_SNAPSHOT_HEADER_SIZE;
    codec[record + 40] = 7; // codec byte
    let error = ChunkFile::open(&mut Cursor::new(codec)).unwrap_err();
    assert!(matches!(
        error,
        ChunkFileError::Incompatible(ErrorDimension::Codec, _)
    ));
}

#[test]
fn a_corrupt_chunk_is_isolated_and_identified() {
    let bytes = vec![b'I'; 96 * 1024];
    let mut encoded = encode_v2_snapshot(&bytes);
    // Corrupt the first stored payload byte; the container itself still parses.
    let first_record = V2_SNAPSHOT_HEADER_SIZE;
    let stored_len = u32::from_le_bytes(
        encoded[first_record + 32..first_record + 36]
            .try_into()
            .unwrap(),
    ) as usize;
    assert!(stored_len > 0);
    let payload = first_record + V2_RECORD_HEADER_SIZE;
    encoded[payload] ^= 0xFF;

    let layout = ChunkFile::open(&mut Cursor::new(encoded.clone())).unwrap();
    let task = ChunkTask::untracked();
    let error = layout
        .copy_original(&mut Cursor::new(encoded), &mut std::io::sink(), &task)
        .unwrap_err();
    assert!(matches!(
        error,
        ChunkFileError::Integrity(ErrorDimension::Chunk, _)
    ));
    assert!(error.to_string().contains("第 1 个块"), "{error}");
}

#[test]
fn chunk_store_reads_across_segments_and_seeks_backwards() {
    let mut store = ChunkStore::new();
    store.append(Box::new(Cursor::new(vec![b'A'; 10]))).unwrap();
    store.append(Box::new(Cursor::new(vec![b'B'; 5]))).unwrap();
    store.append(Box::new(Cursor::new(vec![b'C'; 7]))).unwrap();
    let mut reader = store.reader();
    let mut buffer = [0_u8; 20];
    reader.read_exact(&mut buffer).unwrap();
    assert_eq!(&buffer[..10], &[b'A'; 10]);
    assert_eq!(&buffer[10..15], &[b'B'; 5]);
    assert_eq!(&buffer[15..20], &[b'C'; 5]);

    reader.seek(SeekFrom::Start(12)).unwrap();
    let mut tail = Vec::new();
    reader.read_to_end(&mut tail).unwrap();
    assert_eq!(tail, [vec![b'B'; 3], vec![b'C'; 7]].concat());
}

#[test]
fn resolved_layout_rejects_a_mismatched_base() {
    let current = vec![b'A'; 64 * 1024];
    let parent = vec![b'B'; 64 * 1024];
    let unrelated = vec![b'C'; 64 * 1024];
    let delta =
        ChunkFileDelta::open(&mut Cursor::new(encode_reverse_delta(&current, &parent))).unwrap();
    let mut unrelated_snapshot = Cursor::new(Vec::new());
    let unrelated_chunk = v1_create(
        &mut Cursor::new(&unrelated),
        &mut unrelated_snapshot,
        ChunkingConfig::default(),
    )
    .unwrap();
    let mut store = ChunkStore::new();
    store.append(Box::new(Cursor::new(Vec::new()))).unwrap();
    let task = ChunkTask::untracked();

    let error = delta
        .resolve(&unrelated_chunk, &mut store, &task)
        .unwrap_err();

    assert!(
        matches!(error, ChunkFileError::Incompatible(_, _)),
        "{error}"
    );
}

#[test]
fn compressed_delta_round_trip_uses_disk_backed_payload() {
    let config = ChunkingConfig::default();
    let current_bytes = b"current artwork bytes with a changed middle";
    let parent_bytes = b"parent artwork bytes with an older middle";
    let mut current_snapshot = Cursor::new(Vec::new());
    let current = v1_create(
        &mut Cursor::new(current_bytes),
        &mut current_snapshot,
        config,
    )
    .unwrap();
    let mut parent_snapshot = Cursor::new(Vec::new());
    let parent = v1_create(&mut Cursor::new(parent_bytes), &mut parent_snapshot, config).unwrap();
    let mut encoded_delta = Cursor::new(Vec::new());
    let task = ChunkTask::untracked();
    current
        .create_reverse_delta(
            &parent,
            &mut parent_snapshot,
            &mut encoded_delta,
            ChunkWriteOptions::V1,
            &task,
        )
        .unwrap();

    let delta = ChunkFileDelta::open(&mut encoded_delta).unwrap();
    let mut restored_snapshot = Cursor::new(Vec::new());
    let restored = delta
        .apply(&current, &mut current_snapshot, &mut restored_snapshot)
        .unwrap();
    let mut restored_bytes = Vec::new();
    restored
        .copy_original(&mut restored_snapshot, &mut restored_bytes, &task)
        .unwrap();

    assert_eq!(restored_bytes, parent_bytes);
}

#[test]
fn compressed_delta_rejects_data_beyond_declared_payload() {
    let mut payload = Vec::new();
    write_delta_header(
        &mut payload,
        FormatVersion::V1,
        V2_CHUNKER_CDC,
        V2_ADAPTER_NONE,
        ChunkingConfig::default(),
        ChunkDigest::from([0; DIGEST_SIZE]),
        ChunkDigest::from([0; DIGEST_SIZE]),
        0,
        0,
        0,
    )
    .unwrap();
    payload.extend_from_slice(&[0_u8; 4096]);
    let encoded = zstd::stream::encode_all(payload.as_slice(), 1).unwrap();

    let error = ChunkFileDelta::open(&mut Cursor::new(encoded)).unwrap_err();

    assert!(matches!(error, ChunkFileError::InvalidFormat(_, _)));
    assert!(error.to_string().contains("超过格式声明"));
}

#[test]
fn stored_format_label_reads_only_the_file_header() {
    for (bytes, expected) in [
        (encode_snapshot(b"v1 payload"), "v1"),
        (encode_v2_snapshot(b"v2 payload"), "v2"),
        // 落盘的 delta 整体是 zstd 流，版本在解压后的 LBDELTA 头里。
        (encode_reverse_delta(b"v1 child", b"v1 parent"), "v1"),
        (encode_v2_reverse_delta(b"v2 child", b"v2 parent"), "v2"),
    ] {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(&bytes).unwrap();
        assert_eq!(stored_format_label(file.path()).unwrap(), expected);
    }

    let mut foreign = tempfile::NamedTempFile::new().unwrap();
    foreign.write_all(b"not a chunk file").unwrap();
    assert!(stored_format_label(foreign.path()).is_err());
}

#[test]
fn declared_limit_allows_multi_gigabyte_targets() {
    let target_size = 4 * 1024 * 1024 * 1024_u64;
    let header = DeltaHeader {
        version: FormatVersion::V2,
        target_config: ChunkingConfig::default(),
        base_digest: ChunkDigest::from([0; DIGEST_SIZE]),
        target_digest: ChunkDigest::from([0; DIGEST_SIZE]),
        target_size,
        target_chunk_count: 262_144,
        data_count: 262_144,
        target_chunker: V2_CHUNKER_CDC,
        target_adapter: V2_ADAPTER_NONE,
    };

    assert!(delta_payload_limit(&header).unwrap() > target_size);
}

#[test]
fn large_delta_stress_round_trip_and_truncation_failure() {
    const SIZE: usize = 32 * 1024 * 1024;
    let config = ChunkingConfig::default();
    let mut state = 0x9e37_79b9_u32;
    let mut parent_bytes = vec![0_u8; SIZE];
    for byte in &mut parent_bytes {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        *byte = state as u8;
    }
    let mut current_bytes = parent_bytes.clone();
    for start in (0..SIZE).step_by(256 * 1024) {
        let end = (start + 4096).min(SIZE);
        for byte in &mut current_bytes[start..end] {
            *byte ^= 0x5a;
        }
    }

    let task = ChunkTask::untracked();
    let mut current_snapshot = Cursor::new(Vec::new());
    let current = v1_create(
        &mut Cursor::new(&current_bytes),
        &mut current_snapshot,
        config,
    )
    .unwrap();
    let mut parent_snapshot = Cursor::new(Vec::new());
    let parent = v1_create(
        &mut Cursor::new(&parent_bytes),
        &mut parent_snapshot,
        config,
    )
    .unwrap();
    let mut encoded_delta = Cursor::new(Vec::new());
    current
        .create_reverse_delta(
            &parent,
            &mut parent_snapshot,
            &mut encoded_delta,
            ChunkWriteOptions::V1,
            &task,
        )
        .unwrap();

    let delta = ChunkFileDelta::open(&mut encoded_delta).unwrap();
    let mut restored_snapshot = Cursor::new(Vec::new());
    let restored = delta
        .apply(&current, &mut current_snapshot, &mut restored_snapshot)
        .unwrap();
    let mut restored_bytes = Vec::with_capacity(SIZE);
    restored
        .copy_original(&mut restored_snapshot, &mut restored_bytes, &task)
        .unwrap();
    assert_eq!(restored_bytes, parent_bytes);

    let mut truncated = encoded_delta.into_inner();
    truncated.truncate(truncated.len().saturating_sub(17));
    assert!(ChunkFileDelta::open(&mut Cursor::new(truncated)).is_err());
}

fn materialized(layout: &ChunkFile, store: &mut ChunkStore<'_>) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut reader = store.reader();
    layout
        .copy_original(&mut reader, &mut bytes, &ChunkTask::untracked())
        .unwrap();
    bytes
}

/// 属性测试：随机版本链的任意跳恢复，都与该版本字节逐字节一致。
///
/// 每个种子构造一条 3–6 版本的链，每版本对上一版本做若干随机字节改动，逐跳
/// 生成反向增量，再从最新版本出发一路回退到最老版本，断言每一跳都精确复现
/// 对应版本。它覆盖的是「随机改动位置 × 随机链深」的组合，而不是靠手写固定
/// 场景去猜边界；失败时把种子带进消息即可复现。
#[test]
fn randomized_reverse_delta_chains_restore_every_revision() {
    let task = ChunkTask::untracked();
    for seed in 1..=16_u64 {
        let mut rng = Rng::new(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15));
        let revision_count = 3 + rng.below(4);
        let base_length = 32 * 1024 + rng.below(96 * 1024);

        let mut first = vec![0_u8; base_length];
        for byte in &mut first {
            *byte = rng.next_u64() as u8;
        }
        let mut versions = vec![first];
        for _ in 1..revision_count {
            let mut next = versions.last().unwrap().clone();
            let length = next.len();
            for _ in 0..(1 + rng.below(4)) {
                let start = rng.below(length);
                let span = 1 + rng.below((length - start).min(8 * 1024));
                let mask = rng.nonzero_byte();
                for byte in &mut next[start..start + span] {
                    *byte ^= mask;
                }
            }
            versions.push(next);
        }

        let snapshots: Vec<Vec<u8>> = versions
            .iter()
            .map(|version| encode_snapshot(version))
            .collect();
        let deltas: Vec<Vec<u8>> = (0..revision_count - 1)
            .map(|index| encode_reverse_delta(&versions[index + 1], &versions[index]))
            .collect();

        let newest = revision_count - 1;
        let mut store = ChunkStore::new();
        store
            .append(Box::new(Cursor::new(snapshots[newest].clone())))
            .unwrap();
        let mut layout = ChunkFile::open(&mut Cursor::new(snapshots[newest].clone())).unwrap();
        assert_eq!(
            materialized(&layout, &mut store),
            versions[newest],
            "seed={seed} 最新版本不一致"
        );

        for index in (0..newest).rev() {
            let delta = ChunkFileDelta::open(&mut Cursor::new(deltas[index].clone())).unwrap();
            layout = delta.resolve(&layout, &mut store, &task).unwrap();
            assert_eq!(
                materialized(&layout, &mut store),
                versions[index],
                "seed={seed} 回退到版本 {index} 时字节不一致"
            );
        }
    }
}

/// 磁盘写满时创建快照必须干净失败：返回真实的「空间不足」错误、不 panic，
/// 且写到一半的半成品无法被当成完整快照打开。
#[test]
fn snapshot_creation_reports_a_full_disk_cleanly() {
    let source_bytes = vec![b'A'; 96 * 1024];
    let quota = 16 * 1024;
    let mut writer = QuotaWriter::new(Cursor::new(Vec::new()), quota);
    let task = ChunkTask::untracked();

    let error = ChunkFile::create(
        &mut Cursor::new(&source_bytes),
        &mut writer,
        ChunkingConfig::default(),
        ChunkWriteOptions::V1,
        &task,
    )
    .unwrap_err();

    let ChunkFileError::Io(io_error) = &error else {
        panic!("预期 I/O 错误，实际为 {error}");
    };
    assert_eq!(io_error.kind(), io::ErrorKind::StorageFull, "{io_error}");
    let written = writer.written();
    assert!(written > 0 && written <= quota, "written={written}");
    let partial = writer.into_inner().into_inner();
    assert_eq!(partial.len() as u64, written);
    assert!(ChunkFile::open(&mut Cursor::new(partial)).is_err());
}

/// 物化快照（恢复路径的落盘）遇到磁盘写满时同样干净失败。
#[test]
fn snapshot_materialization_reports_a_full_disk_cleanly() {
    let source_bytes = vec![b'B'; 128 * 1024];
    let encoded = encode_snapshot(&source_bytes);
    let layout = ChunkFile::open(&mut Cursor::new(encoded.clone())).unwrap();
    let quota = 8 * 1024;
    let mut writer = QuotaWriter::new(Cursor::new(Vec::new()), quota);
    let task = ChunkTask::untracked();

    let error = layout
        .write_snapshot(
            &mut Cursor::new(encoded),
            &mut writer,
            ChunkWriteOptions::V1,
            &task,
        )
        .unwrap_err();

    let ChunkFileError::Io(io_error) = &error else {
        panic!("预期 I/O 错误，实际为 {error}");
    };
    assert_eq!(io_error.kind(), io::ErrorKind::StorageFull, "{io_error}");
    let written = writer.written();
    assert!(written > 0 && written <= quota, "written={written}");
}

/// 畸形输入模糊（快照容器）：任意字节都必须被 `open` 干净地拒绝或正确地接受，
/// **绝不 panic、绝不因为头部声明的巨大数量而无界分配**。
#[test]
fn fuzzed_snapshot_containers_are_rejected_without_panicking() {
    const MINIMUM_RECORD_BYTES: usize = DIGEST_SIZE + size_of::<u32>();
    let task = ChunkTask::untracked();
    for (label, valid) in [
        ("v1", encode_snapshot(&random_bytes(0xF0_0D, 96 * 1024))),
        ("v2", encode_v2_snapshot(&random_bytes(0xF0_0D, 96 * 1024))),
    ] {
        let mut accepted = 0_usize;
        for seed in 0..384_u64 {
            let bytes = match seed % 3 {
                0 => random_bytes(seed, 1 + (seed as usize * 37) % 4096),
                1 => mutated(seed, &valid, 1 + (seed as usize % 24)),
                _ => mutated_header(seed, &valid, 64, 1 + (seed as usize % 12)),
            };
            let Ok(layout) = ChunkFile::open(&mut Cursor::new(bytes.clone())) else {
                continue;
            };
            accepted += 1;
            assert!(
                layout.chunk_count().saturating_mul(MINIMUM_RECORD_BYTES) <= bytes.len(),
                "{label} seed={seed}：接受了超出文件容量的块数量"
            );
            let mut materialized = Cursor::new(Vec::new());
            if let Ok(restored) = layout.write_snapshot(
                &mut Cursor::new(bytes.clone()),
                &mut materialized,
                ChunkWriteOptions::V1,
                &task,
            ) {
                assert_eq!(
                    materialized.into_inner().len() as u64,
                    layout.logical_size(),
                    "{label} seed={seed}：物化产物长度必须等于声明的大小"
                );
                assert_eq!(restored.logical_size(), layout.logical_size());
            }
        }
        assert!(
            accepted > 0,
            "{label} 固定种子下没有任何一次扰动保持在结构之内"
        );
        assert!(valid.len() > 256);
        for drop in [1_usize, 9, 64, valid.len() / 2, valid.len() - 1] {
            assert!(
                ChunkFile::open(&mut Cursor::new(valid[..valid.len() - drop].to_vec())).is_err(),
                "{label} 截断 {drop} 字节的快照必须被拒绝"
            );
        }
        let mut extended = valid.clone();
        extended.push(0);
        assert!(
            ChunkFile::open(&mut Cursor::new(extended)).is_err(),
            "{label} 续写字节的快照必须被拒绝"
        );
    }
}

/// 畸形输入模糊（反向增量容器）：任意字节都必须被干净地拒绝或接受，绝不 panic；
/// 并额外断言截断与续写这两条确定的拒绝性质。
#[test]
fn fuzzed_delta_containers_are_rejected_without_panicking() {
    let child = random_bytes(0xC1, 96 * 1024);
    let parent = random_bytes(0xB2, 96 * 1024);
    let valid = encode_reverse_delta(&child, &parent);
    assert!(valid.len() > 256);
    assert!(
        ChunkFileDelta::open(&mut Cursor::new(valid.clone())).is_ok(),
        "合法反向增量必须能打开"
    );

    for seed in 0..384_u64 {
        let bytes = match seed % 2 {
            0 => random_bytes(seed, 1 + (seed as usize * 53) % 4096),
            _ => mutated(seed, &valid, 1 + (seed as usize % 24)),
        };
        let _ = ChunkFileDelta::open(&mut Cursor::new(bytes));
    }

    for drop in [1_usize, 7, 64, valid.len() / 2, valid.len() - 1] {
        assert!(
            ChunkFileDelta::open(&mut Cursor::new(valid[..valid.len() - drop].to_vec())).is_err(),
            "截断 {drop} 字节的增量必须被拒绝"
        );
    }
    let mut extended = valid.clone();
    extended.extend_from_slice(b"trailing");
    assert!(
        ChunkFileDelta::open(&mut Cursor::new(extended)).is_err(),
        "续写字节的增量必须被拒绝"
    );
}

/// 并行管道只改变块摘要的计算位置，不改变布局：同一输入下与串行路径逐字节一致。
#[test]
fn parallel_snapshot_creation_matches_the_serial_layout() {
    let task = ChunkTask::untracked();
    let config = ChunkingConfig::default();
    let cases = [
        (
            "v1-cdc",
            ChunkWriteOptions::V1,
            random_bytes(0x51, 512 * 1024),
        ),
        (
            "v2-cdc",
            ChunkWriteOptions {
                format: FormatVersion::V2,
                chunking: ChunkingStrategy::Cdc,
            },
            random_bytes(0x52, 512 * 1024),
        ),
        ("v2-structural", ChunkWriteOptions::V2, synthetic_clip()),
    ];
    for (label, options, bytes) in cases {
        let mut serial = Cursor::new(Vec::new());
        let serial_layout = ChunkFile::create_serially(
            &mut Cursor::new(&bytes),
            &mut serial,
            config,
            options,
            &task,
        )
        .unwrap();
        let mut parallel = Cursor::new(Vec::new());
        let parallel_layout = ChunkFile::create_with_workers(
            &mut Cursor::new(&bytes),
            &mut parallel,
            config,
            options,
            &task,
            4,
        )
        .unwrap();
        let serial_bytes = serial.into_inner();
        let parallel_bytes = parallel.into_inner();

        assert_eq!(serial_bytes, parallel_bytes, "{label}：产物字节不一致");
        assert_eq!(
            serial_layout.file_digest(),
            parallel_layout.file_digest(),
            "{label}"
        );
        assert_eq!(
            serial_layout.chunk_count(),
            parallel_layout.chunk_count(),
            "{label}"
        );
        assert_eq!(
            serial_layout.logical_size(),
            parallel_layout.logical_size(),
            "{label}"
        );

        let mut restored = Vec::new();
        parallel_layout
            .copy_original_with_workers(&mut Cursor::new(parallel_bytes), &mut restored, &task, 4)
            .unwrap();
        assert_eq!(restored, bytes, "{label}：并行导出未能还原输入");
    }
}

/// 并行创建与并行导出都只上报「已完成前缀」：进度单调不减，终点等于逻辑大小。
#[test]
fn parallel_pipelines_report_only_the_completed_prefix() {
    let bytes = random_bytes(0xA7, 512 * 1024);
    let token = CancelToken::standalone();

    let mut snapshot = Cursor::new(Vec::new());
    let created = {
        let seen = std::cell::RefCell::new(Vec::new());
        let progress = |processed: u64| seen.borrow_mut().push(processed);
        let task = ChunkTask::new(&token, Some(&progress));
        let layout = ChunkFile::create_with_workers(
            &mut Cursor::new(&bytes),
            &mut snapshot,
            ChunkingConfig::default(),
            ChunkWriteOptions::V2,
            &task,
            4,
        )
        .unwrap();
        (layout, seen.into_inner())
    };
    let (layout, created_seen) = created;
    assert!(
        created_seen.windows(2).all(|pair| pair[0] <= pair[1]),
        "创建进度不得回退：{created_seen:?}"
    );
    assert_eq!(
        created_seen.last().copied(),
        Some(layout.logical_size()),
        "创建进度终点必须等于逻辑大小"
    );

    let encoded = snapshot.into_inner();
    let mut restored = Vec::new();
    let restored_seen = {
        let seen = std::cell::RefCell::new(Vec::new());
        let progress = |processed: u64| seen.borrow_mut().push(processed);
        let task = ChunkTask::new(&token, Some(&progress));
        layout
            .copy_original_with_workers(&mut Cursor::new(encoded), &mut restored, &task, 4)
            .unwrap();
        seen.into_inner()
    };
    assert_eq!(restored, bytes);
    assert!(
        restored_seen.windows(2).all(|pair| pair[0] <= pair[1]),
        "导出进度不得回退：{restored_seen:?}"
    );
    assert_eq!(
        restored_seen.last().copied(),
        Some(layout.logical_size()),
        "导出进度终点必须等于逻辑大小"
    );

    // 小文件与单核机器走串行兜底；它同样按累计已写字节上报。
    let mut serial = Cursor::new(Vec::new());
    let serial_seen = {
        let seen = std::cell::RefCell::new(Vec::new());
        let progress = |processed: u64| seen.borrow_mut().push(processed);
        let task = ChunkTask::new(&token, Some(&progress));
        ChunkFile::create_serially(
            &mut Cursor::new(&bytes),
            &mut serial,
            ChunkingConfig::default(),
            ChunkWriteOptions::V1,
            &task,
        )
        .unwrap();
        seen.into_inner()
    };
    assert!(
        serial_seen.windows(2).all(|pair| pair[0] <= pair[1]),
        "串行创建进度不得回退：{serial_seen:?}"
    );
    assert_eq!(
        serial_seen.last().copied(),
        Some(bytes.len() as u64),
        "串行创建进度终点必须等于输入大小"
    );
}

/// 块摘要校验在执行块的线程里完成；并行导出仍必须定位到具体块并报告损坏。
#[test]
fn parallel_restore_rejects_a_corrupt_chunk() {
    let bytes = random_bytes(0xC3, 512 * 1024);
    let mut encoded = encode_v2_snapshot(&bytes);
    let payload = V2_SNAPSHOT_HEADER_SIZE + V2_RECORD_HEADER_SIZE;
    encoded[payload] ^= 0xff;

    let layout = ChunkFile::open(&mut Cursor::new(encoded.clone())).unwrap();
    let error = layout
        .copy_original_with_workers(
            &mut Cursor::new(encoded),
            &mut std::io::sink(),
            &ChunkTask::untracked(),
            4,
        )
        .unwrap_err();

    assert!(matches!(
        error,
        ChunkFileError::Integrity(ErrorDimension::Chunk, _)
    ));
    assert!(error.to_string().contains("第 1 个块"), "{error}");
}

/// 取消在管道每一段都可见：已取消的令牌不会让线程挂起或 panic。
#[test]
fn cancelled_parallel_pipelines_stop_cleanly() {
    let token = CancelToken::standalone();
    token.cancel();
    let task = ChunkTask::cancel(&token);

    let mut output = Cursor::new(Vec::new());
    let error = ChunkFile::create_with_workers(
        &mut Cursor::new(random_bytes(0x61, 512 * 1024)),
        &mut output,
        ChunkingConfig::default(),
        ChunkWriteOptions::V1,
        &task,
        4,
    )
    .unwrap_err();
    assert!(matches!(error, ChunkFileError::Cancelled), "{error}");

    let bytes = random_bytes(0x62, 512 * 1024);
    let encoded = encode_v2_snapshot(&bytes);
    let layout = ChunkFile::open(&mut Cursor::new(encoded.clone())).unwrap();
    let error = layout
        .copy_original_with_workers(
            &mut Cursor::new(encoded),
            &mut Cursor::new(Vec::new()),
            &task,
            4,
        )
        .unwrap_err();
    assert!(matches!(error, ChunkFileError::Cancelled), "{error}");
}

/// 管道写到一半遇到磁盘写满：返回真实 I/O 错误，其余段退出而不是阻塞。
#[test]
fn parallel_snapshot_creation_reports_a_full_disk_cleanly() {
    let bytes = random_bytes(0x71, 512 * 1024);
    let quota = 16 * 1024;
    let mut writer = QuotaWriter::new(Cursor::new(Vec::new()), quota);

    let error = ChunkFile::create_with_workers(
        &mut Cursor::new(&bytes),
        &mut writer,
        ChunkingConfig::default(),
        ChunkWriteOptions::V1,
        &ChunkTask::untracked(),
        4,
    )
    .unwrap_err();

    let ChunkFileError::Io(io_error) = &error else {
        panic!("预期 I/O 错误，实际为 {error}");
    };
    assert_eq!(io_error.kind(), io::ErrorKind::StorageFull, "{io_error}");
    assert!(writer.written() > 0 && writer.written() <= quota);
}

fn synthetic_clip() -> Vec<u8> {
    fn block(tag: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut output = Vec::new();
        output.extend_from_slice(&[0, 0, 0, 0]);
        output.extend_from_slice(tag);
        output.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        output.extend_from_slice(payload);
        output
    }
    fn utf16be(name: &str) -> Vec<u8> {
        let mut output = Vec::new();
        for byte in name.bytes() {
            output.push(0);
            output.push(byte);
        }
        output
    }
    fn tile(body: &[u8]) -> Vec<u8> {
        let begin = utf16be("BlockDataBeginChunk");
        let end = utf16be("BlockDataEndChunk");
        let size = 4 + 4 + begin.len() + body.len() + 4 + end.len();
        let mut output = Vec::new();
        output.extend_from_slice(&(size as u32).to_be_bytes());
        output.extend_from_slice(&(19_u32).to_be_bytes());
        output.extend_from_slice(&begin);
        output.extend_from_slice(body);
        output.extend_from_slice(&(17_u32).to_be_bytes());
        output.extend_from_slice(&end);
        output
    }
    let mut exta_payload = Vec::new();
    exta_payload.extend_from_slice(&40_u64.to_be_bytes());
    exta_payload.extend_from_slice(b"extrnlid");
    exta_payload.extend_from_slice(b"0123456789ABCDEF0123456789ABCDEF");
    exta_payload.extend_from_slice(&0_u32.to_be_bytes());
    exta_payload.extend_from_slice(&0_u32.to_be_bytes());
    exta_payload.extend_from_slice(&tile(&[1_u8; 4096]));
    exta_payload.extend_from_slice(&tile(&[2_u8; 4096]));
    exta_payload.extend_from_slice(&[0xAA; 12]);

    let mut body = Vec::new();
    body.extend_from_slice(&block(b"Head", &[0_u8; 8]));
    body.extend_from_slice(&block(b"Exta", &exta_payload));
    body.extend_from_slice(&block(b"Foot", &[]));
    let mut output = Vec::new();
    output.extend_from_slice(b"CSFCHUNK");
    output.extend_from_slice(&((24 + body.len()) as u64).to_be_bytes());
    output.extend_from_slice(&24_u64.to_be_bytes());
    output.extend_from_slice(&body);
    output
}
