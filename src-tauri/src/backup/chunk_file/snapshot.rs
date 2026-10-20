//! Snapshot creation, opening and materialization.
//!
//! Creating a snapshot picks the chunk layout (container-aligned units when the
//! file is recognised, CDC otherwise), writes 44 B/36 B records and deduplicates
//! repeated v2 payloads. Opening and materializing reverse the layout without
//! trusting any declared length. Inputs above a size threshold move through a
//! bounded reader/hasher/writer pipeline; small inputs and single-core machines
//! keep the serial path.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::{BufReader, Read, Seek, SeekFrom, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Condvar, Mutex,
    },
    time::Duration,
};

use sha2::{Digest, Sha256};

use crate::{cancel::CancelToken, format::container::StructuralUnit};

use super::{
    chunking::{plan_container_chunks, stream_cdc},
    format::{
        finalize_sha256, read_chunk_key, read_snapshot_header, read_v2_record_header,
        require_empty_output, sha256, stream_length, u64_to_usize, usize_to_u64,
        validate_record_length, validate_v2_codec, write_chunk_key, write_delta_header,
        write_snapshot_header, write_u8, write_v2_record_header,
    },
    integrity, invalid_format, ChunkDigest, ChunkFileError, ChunkFileResult, ChunkKey, ChunkRecord,
    ChunkTask, ChunkWriteOptions, ChunkingConfig, ChunkingStrategy, ErrorDimension, FormatVersion,
    CANCEL_CHECK_INTERVAL, CODEC_NONE, DIGEST_SIZE, RECIPE_NONE, V2_ADAPTER_CLIP, V2_ADAPTER_NONE,
    V2_CHUNKER_CDC, V2_CHUNKER_CLIP, V2_RECORD_FLAG_STRUCTURAL,
};

use super::delta::{DeltaOp, DeltaOpKind};

/// 低于这个大小不做并行：线程与队列的开销会盖过重叠收益。
const PIPELINE_MIN_BYTES: u64 = 4 * 1024 * 1024;
/// 同时在途（已切块但尚未按序写出）的字节预算。取 16 MiB 而不是 64 MiB：
/// 大文件的压力测试要求峰值内存增量保持在「逻辑大小/8 + 24 MiB」之内。
const PIPELINE_MEMORY_BUDGET: usize = 16 * 1024 * 1024;
/// 块哈希是短任务，超过 4 个 worker 的调度收益有限，也避免小文件占用过多线程。
const MAX_PIPELINE_WORKERS: usize = 4;
/// 窗口与队列按字节预算折算；极小块的配置再给一个消息数上限，避免按字节
/// 算出的槽位数本身占掉过多内存。
const MAX_PIPELINE_QUEUE_MESSAGES: usize = 1024;
/// 单次读取的最大切片，保证取消检查的粒度不随 `max_size` 变大而变粗。
const READ_SLICE_BYTES: usize = 64 * 1024;

fn pipeline_worker_count() -> usize {
    std::thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(1)
        .clamp(1, MAX_PIPELINE_WORKERS)
}

/// 同时在途（已切块但尚未按序写出）的块数上限。队列容量与它一致，真正的内存
/// 上界由 [`PipelineThrottle`] 的窗口保证：乱序完成的块会被写线程收进待写表，
/// 仅靠队列容量挡不住待写表随文件增长。
fn pipeline_window(max_size: usize) -> usize {
    (PIPELINE_MEMORY_BUDGET / max_size.max(1)).clamp(1, MAX_PIPELINE_QUEUE_MESSAGES)
}

/// 等待窗口推进时的轮询上限。取消与停止标志没有自己的唤醒信号，超时让等待者
/// 能定期重新检查它们；正常推进仍由 `advance` 立即唤醒。
const THROTTLE_WAIT_POLL: Duration = Duration::from_millis(20);

/// 读线程只有在「已写前缀」推进后才会继续生产块，从而把待写表的规模也限制在
/// 同一窗口内。写线程每写出一块就推进一次并唤醒读线程；失败或取消先置停止位
/// 再唤醒，等待者会在重新检查条件时退出。
struct PipelineThrottle {
    written: Mutex<u64>,
    advanced: Condvar,
    window: u64,
}

impl PipelineThrottle {
    fn new(window: usize) -> Self {
        Self {
            written: Mutex::new(0),
            advanced: Condvar::new(),
            window: window as u64,
        }
    }

    fn wait_for_slot(&self, sequence: u64, stop: &AtomicBool, cancel: Option<&CancelToken>) {
        let mut written = self.written.lock().expect("pipeline throttle poisoned");
        while sequence >= *written + self.window {
            if stop.load(Ordering::Relaxed) || cancel.is_some_and(|token| token.observed()) {
                return;
            }
            written = self
                .advanced
                .wait_timeout(written, THROTTLE_WAIT_POLL)
                .expect("pipeline throttle poisoned")
                .0;
        }
    }

    fn advance(&self, written: u64) {
        *self.written.lock().expect("pipeline throttle poisoned") = written;
        self.advanced.notify_all();
    }

    fn wake(&self) {
        self.advanced.notify_all();
    }
}

struct ChunkJob {
    sequence: u64,
    data: Vec<u8>,
    flags: u16,
}

struct HashedChunk {
    sequence: u64,
    data: Vec<u8>,
    digest: ChunkDigest,
    flags: u16,
}

struct RestoreJob {
    /// 块在布局中的位置；它同时是保序写的唯一键。
    index: usize,
    data: Vec<u8>,
    expected: ChunkDigest,
}

struct RestoredChunk {
    index: usize,
    data: Vec<u8>,
}

fn prepare_snapshot<R, W>(
    source: &mut R,
    snapshot: &mut W,
    config: ChunkingConfig,
    options: ChunkWriteOptions,
    task: &ChunkTask<'_>,
) -> ChunkFileResult<(Option<Vec<StructuralUnit>>, u16, u16)>
where
    R: Read + Seek,
    W: Write + Seek,
{
    let config = config.validate()?;
    require_empty_output(snapshot, "snapshot")?;
    let structural_units = if options.format == FormatVersion::V2
        && options.chunking == ChunkingStrategy::ContainerAware
    {
        plan_container_chunks(source, task)?
    } else {
        None
    };
    let (chunker, adapter) = if structural_units.is_some() {
        (V2_CHUNKER_CLIP, V2_ADAPTER_CLIP)
    } else {
        (V2_CHUNKER_CDC, V2_ADAPTER_NONE)
    };
    write_snapshot_header(
        snapshot,
        options.format,
        chunker,
        adapter,
        config,
        0,
        0,
        ChunkDigest::from([0; DIGEST_SIZE]),
    )?;
    Ok((structural_units, chunker, adapter))
}

/// 按顺序产生块并交给 `emit`，不负责哈希或写入。
///
/// 结构单元在文件内连续铺开，顺序读与逐单元 seek 等价；这里用一块容量不小于
/// `max_size` 的缓冲顺序读，单元缓冲复用，不再逐块 seek 与新分配。超出
/// `max_size` 的单元仍交给 CDC 切分。
fn produce_chunks<R, F>(
    source: &mut R,
    structural_units: Option<Vec<StructuralUnit>>,
    config: ChunkingConfig,
    cancel: Option<&CancelToken>,
    stop: &AtomicBool,
    mut emit: F,
) -> ChunkFileResult<u64>
where
    R: Read + Seek,
    F: FnMut(&[u8], u16) -> ChunkFileResult<()>,
{
    source.seek(SeekFrom::Start(0))?;
    let cancel = cancel.cloned().unwrap_or_else(CancelToken::standalone);
    let reader_task = ChunkTask::cancel(&cancel);
    let mut emitted = 0_u64;
    match structural_units {
        Some(units) => {
            let mut buffered =
                BufReader::with_capacity(config.max_size.max(super::STREAM_BUFFER_SIZE), source);
            let mut unit_buffer = Vec::new();
            for unit in units {
                if stop.load(Ordering::Relaxed) {
                    return Err(ChunkFileError::Cancelled);
                }
                reader_task.check()?;
                let length = unit.len();
                if length > config.max_size as u64 {
                    stream_cdc(
                        &mut buffered,
                        Some(length),
                        config,
                        &reader_task,
                        &mut |data, flags| {
                            emit(data, flags)?;
                            emitted += 1;
                            Ok(())
                        },
                    )?;
                } else {
                    let length = usize::try_from(length).map_err(|_| {
                        invalid_format(ErrorDimension::Chunk, "结构单元长度超出平台范围")
                    })?;
                    if unit_buffer.len() < length {
                        unit_buffer.resize(length, 0);
                    }
                    buffered.read_exact(&mut unit_buffer[..length])?;
                    emit(&unit_buffer[..length], V2_RECORD_FLAG_STRUCTURAL)?;
                    emitted += 1;
                }
            }
        }
        None => {
            stream_cdc(source, None, config, &reader_task, &mut |data, flags| {
                emit(data, flags)?;
                emitted += 1;
                Ok(())
            })?;
        }
    }
    Ok(emitted)
}

#[derive(Debug, Clone)]
pub(crate) struct ChunkFile {
    config: ChunkingConfig,
    logical_size: u64,
    file_digest: ChunkDigest,
    chunks: Vec<ChunkRecord>,
    chunk_index: HashMap<ChunkKey, usize>,
    /// Canonical v2 ids (0=CDC, 1=CLIP, 2=PSD); v1 layouts use 0.
    pub(super) chunker: u16,
    pub(super) adapter: u16,
}

impl ChunkFile {
    pub(crate) fn create<R, W>(
        source: &mut R,
        snapshot: &mut W,
        config: ChunkingConfig,
        options: ChunkWriteOptions,
        task: &ChunkTask<'_>,
    ) -> ChunkFileResult<Self>
    where
        R: Read + Seek + Send,
        W: Write + Seek,
    {
        // 小文件与单核机器没有收益，保留串行路径：线程启动、队列与一次额外的
        // 载荷拷贝不值得；大文件走下面三段重叠的管道。
        let workers = pipeline_worker_count();
        let length = stream_length(source)?;
        source.seek(SeekFrom::Start(0))?;
        if length < PIPELINE_MIN_BYTES || workers <= 1 {
            return Self::create_serially(source, snapshot, config, options, task);
        }
        Self::create_with_workers(source, snapshot, config, options, task, workers)
    }

    pub(super) fn create_serially<R, W>(
        source: &mut R,
        snapshot: &mut W,
        config: ChunkingConfig,
        options: ChunkWriteOptions,
        task: &ChunkTask<'_>,
    ) -> ChunkFileResult<Self>
    where
        R: Read + Seek,
        W: Write + Seek,
    {
        let (structural_units, chunker, adapter) =
            prepare_snapshot(source, snapshot, config, options, task)?;
        let mut writer = SnapshotWriter::new(snapshot, options.format);
        let stop = AtomicBool::new(false);
        source.seek(SeekFrom::Start(0))?;
        produce_chunks(
            source,
            structural_units,
            config,
            task.cancel_token(),
            &stop,
            |data, flags| {
                writer.emit(data, flags)?;
                task.report(data.len() as u64);
                Ok(())
            },
        )?;
        writer.finish_file(config, options, chunker, adapter)
    }

    /// 三段式有界管道：读线程切块，`workers` 个线程计算块摘要，主线程按序写记录
    /// 与载荷。整文件摘要仍在主线程按序喂入，因此产物与串行路径逐字节一致。
    ///
    /// 读线程最多领先已写前缀约 16 MiB；两个队列按同一窗口限长，因此乱序完成
    /// 的块进入待写表后也不会让内存随文件增长。取消、写盘失败与读盘失败都会让
    /// 所有段退出并汇入同一个错误，不会留下仍在阻塞的线程。进度只由主线程在写完
    /// 一个顺序块后上报，因此始终是「已完成前缀」且单调不减。
    pub(super) fn create_with_workers<R, W>(
        source: &mut R,
        snapshot: &mut W,
        config: ChunkingConfig,
        options: ChunkWriteOptions,
        task: &ChunkTask<'_>,
        workers: usize,
    ) -> ChunkFileResult<Self>
    where
        R: Read + Seek + Send,
        W: Write + Seek,
    {
        let (structural_units, chunker, adapter) =
            prepare_snapshot(source, snapshot, config, options, task)?;
        let capacity = pipeline_window(config.max_size);
        let (job_sender, job_receiver) = mpsc::sync_channel::<ChunkJob>(capacity);
        let (result_sender, result_receiver) = mpsc::sync_channel::<HashedChunk>(capacity);
        let job_receiver = Arc::new(Mutex::new(job_receiver));
        let stop = AtomicBool::new(false);
        let stop = &stop;
        let throttle = PipelineThrottle::new(capacity);
        let throttle = &throttle;
        let cancel = task.cancel_token();
        let workers = workers.max(1);
        let mut writer = SnapshotWriter::new(snapshot, options.format);

        let (failure, produced, written) = std::thread::scope(|scope| {
            let reader = scope.spawn(move || {
                let mut sequence = 0_u64;
                produce_chunks(
                    source,
                    structural_units,
                    config,
                    cancel,
                    &stop,
                    |data, flags| {
                        if stop.load(Ordering::Relaxed) {
                            return Err(ChunkFileError::Cancelled);
                        }
                        throttle.wait_for_slot(sequence, stop, cancel);
                        if stop.load(Ordering::Relaxed)
                            || cancel.is_some_and(|token| token.observed())
                        {
                            return Err(ChunkFileError::Cancelled);
                        }
                        job_sender
                            .send(ChunkJob {
                                sequence,
                                data: data.to_vec(),
                                flags,
                            })
                            .map_err(|_| ChunkFileError::Cancelled)?;
                        sequence += 1;
                        Ok(())
                    },
                )
            });

            for _ in 0..workers {
                let receiver = Arc::clone(&job_receiver);
                let sender = result_sender.clone();
                scope.spawn(move || loop {
                    let job = match receiver.lock().expect("chunk job queue poisoned").recv() {
                        Ok(job) => job,
                        Err(_) => break,
                    };
                    // 取消或上游失败后继续消费队列，避免读线程阻塞在 send 上；
                    // 只是不再计算摘要。
                    if stop.load(Ordering::Relaxed) || cancel.is_some_and(|token| token.observed())
                    {
                        continue;
                    }
                    let digest = ChunkDigest::from(sha256(&job.data));
                    if sender
                        .send(HashedChunk {
                            sequence: job.sequence,
                            data: job.data,
                            digest,
                            flags: job.flags,
                        })
                        .is_err()
                    {
                        stop.store(true, Ordering::Relaxed);
                    }
                });
            }
            drop(result_sender);

            let mut pending = BTreeMap::new();
            let mut next = 0_u64;
            let mut failure = None;
            loop {
                let chunk = match result_receiver.recv() {
                    Ok(chunk) => chunk,
                    Err(_) => break,
                };
                if failure.is_some() {
                    continue;
                }
                pending.insert(chunk.sequence, chunk);
                while let Some(chunk) = pending.remove(&next) {
                    // 取消检查与写入合并成一次结果处理：任何一步失败都走同一条
                    // 退出路径（置停止位、唤醒读线程、丢弃待写表）。
                    let outcome = task
                        .check()
                        .and_then(|()| writer.emit_hashed(&chunk.data, chunk.digest, chunk.flags));
                    if let Err(error) = outcome {
                        failure = Some(error);
                        stop.store(true, Ordering::Relaxed);
                        throttle.wake();
                        pending.clear();
                        break;
                    }
                    task.report(chunk.data.len() as u64);
                    next += 1;
                    throttle.advance(next);
                }
            }
            let produced = reader.join().expect("chunk reader thread panicked");
            (failure, produced, next)
        });

        if let Some(error) = failure {
            return Err(error);
        }
        let produced = produced?;
        if written != produced {
            // worker 在取消后不再发送结果，因此缺失的块在这里是预期的取消结果，
            // 不是布局损坏。
            if let Err(error) = task.check() {
                return Err(error);
            }
            return Err(integrity(
                ErrorDimension::Chunk,
                format!("快照块数量不一致：写入 {written}，读取 {produced}"),
            ));
        }
        writer.finish_file(config, options, chunker, adapter)
    }

    pub(crate) fn open<R>(snapshot: &mut R) -> ChunkFileResult<Self>
    where
        R: Read + Seek,
    {
        let file_length = stream_length(snapshot)?;
        snapshot.seek(SeekFrom::Start(0))?;
        let header = read_snapshot_header(snapshot)?;
        let chunk_count = u64_to_usize(header.chunk_count, "块数量")?;
        let minimum_record_bytes = header.version.record_header_size() as u64;
        let possible_records = file_length
            .saturating_sub(snapshot.stream_position()?)
            .checked_div(minimum_record_bytes)
            .unwrap_or(0);
        if header.chunk_count > possible_records {
            return Err(invalid_format(
                ErrorDimension::Chunk,
                "块数量超过文件能够容纳的范围",
            ));
        }

        let mut chunks = Vec::with_capacity(chunk_count);
        let mut logical_size = 0_u64;
        // v2 dedup references point at an earlier payload with the same key.
        let mut stored_payloads: HashMap<ChunkKey, u64> = HashMap::with_capacity(chunk_count);
        // Structural chunking follows the container layout, so its units are not
        // bound by the CDC minimum length. v1 always reports the CDC chunker.
        let structural_chunking = header.chunker != V2_CHUNKER_CDC;
        for index in 0..chunk_count {
            let record = match header.version {
                FormatVersion::V1 => {
                    let key = read_chunk_key(snapshot)?;
                    validate_record_length(key.length, header.config, index + 1 == chunk_count)?;
                    let data_offset = snapshot.stream_position()?;
                    let data_end = data_offset
                        .checked_add(u64::from(key.length))
                        .ok_or_else(|| invalid_format(ErrorDimension::Chunk, "块偏移溢出"))?;
                    if data_end > file_length {
                        return Err(invalid_format(
                            ErrorDimension::Chunk,
                            format!("第 {} 个块超出文件边界", index + 1),
                        ));
                    }
                    snapshot.seek(SeekFrom::Start(data_end))?;
                    ChunkRecord {
                        key,
                        data_offset,
                        stored_length: key.length,
                    }
                }
                FormatVersion::V2 => {
                    let record_header = read_v2_record_header(snapshot)?;
                    let key = record_header.key;
                    if record_header.logical_len != key.length {
                        return Err(invalid_format(
                            ErrorDimension::Chunk,
                            format!(
                                "第 {} 个块记录的逻辑长度 {} 与块长度 {} 不一致",
                                index + 1,
                                record_header.logical_len,
                                key.length
                            ),
                        ));
                    }
                    validate_record_length(
                        key.length,
                        header.config,
                        structural_chunking || index + 1 == chunk_count,
                    )?;
                    validate_v2_codec(record_header.codec, record_header.recipe, index)?;
                    if record_header.stored_len == 0 {
                        let data_offset = stored_payloads.get(&key).copied().ok_or_else(|| {
                            invalid_format(
                                ErrorDimension::Chunk,
                                format!("第 {} 个块引用了未存储的重复块 {}", index + 1, key.digest),
                            )
                        })?;
                        ChunkRecord {
                            key,
                            data_offset,
                            stored_length: 0,
                        }
                    } else {
                        if record_header.stored_len != key.length {
                            return Err(integrity(
                                ErrorDimension::Codec,
                                format!(
                                    "第 {} 个块的存储长度 {} 与逻辑长度 {} 不一致",
                                    index + 1,
                                    record_header.stored_len,
                                    key.length
                                ),
                            ));
                        }
                        let data_offset = snapshot.stream_position()?;
                        let data_end = data_offset
                            .checked_add(u64::from(record_header.stored_len))
                            .ok_or_else(|| invalid_format(ErrorDimension::Chunk, "块偏移溢出"))?;
                        if data_end > file_length {
                            return Err(invalid_format(
                                ErrorDimension::Chunk,
                                format!("第 {} 个块超出文件边界", index + 1),
                            ));
                        }
                        snapshot.seek(SeekFrom::Start(data_end))?;
                        stored_payloads.entry(key).or_insert(data_offset);
                        ChunkRecord {
                            key,
                            data_offset,
                            stored_length: record_header.stored_len,
                        }
                    }
                }
            };
            logical_size = logical_size
                .checked_add(u64::from(record.key.length))
                .ok_or_else(|| invalid_format(ErrorDimension::File, "逻辑文件大小溢出"))?;
            chunks.push(record);
        }

        if snapshot.stream_position()? != file_length {
            return Err(invalid_format(
                ErrorDimension::Chunk,
                "snapshot 末尾存在未识别数据",
            ));
        }
        if logical_size != header.logical_size {
            return Err(invalid_format(
                ErrorDimension::File,
                format!(
                    "逻辑大小不匹配：头部为 {}，块合计为 {logical_size}",
                    header.logical_size
                ),
            ));
        }

        Ok(Self::from_records(
            header.config,
            logical_size,
            header.file_digest,
            chunks,
            header.chunker,
            header.adapter,
        ))
    }

    pub(crate) fn logical_size(&self) -> u64 {
        self.logical_size
    }

    pub(crate) fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    pub(crate) fn file_digest(&self) -> ChunkDigest {
        self.file_digest
    }

    pub(crate) fn copy_original<R, W>(
        &self,
        snapshot: &mut R,
        output: &mut W,
        task: &ChunkTask<'_>,
    ) -> ChunkFileResult<()>
    where
        R: Read + Seek + Send,
        W: Write,
    {
        let workers = pipeline_worker_count();
        if self.logical_size < PIPELINE_MIN_BYTES || workers <= 1 {
            return self.copy_original_serially(snapshot, output, task);
        }
        self.copy_original_with_workers(snapshot, output, task, workers)
    }

    fn copy_original_serially<R, W>(
        &self,
        snapshot: &mut R,
        output: &mut W,
        task: &ChunkTask<'_>,
    ) -> ChunkFileResult<()>
    where
        R: Read + Seek,
        W: Write,
    {
        let mut file_hasher = Sha256::new();
        let mut copied = 0_u64;
        let mut buffer = Vec::new();
        for (index, record) in self.chunks.iter().enumerate() {
            task.check()?;
            load_verified_chunk(snapshot, record, &mut buffer, index, task)?;
            output.write_all(&buffer)?;
            file_hasher.update(&buffer);
            copied = copied
                .checked_add(u64::from(record.key.length))
                .ok_or_else(|| integrity(ErrorDimension::File, "还原大小溢出"))?;
            task.report(u64::from(record.key.length));
        }
        output.flush()?;
        let digest = ChunkDigest::from(finalize_sha256(file_hasher));
        self.verify_restored(copied, digest)
    }

    /// 恢复导出的有界管道：读线程按逻辑顺序读块，`workers` 个线程验块摘要，
    /// 主线程按序写输出。输出始终是单文件顺序写，整文件摘要也只在主线程按序
    /// 更新；进度只上报已完成前缀。
    ///
    /// 块摘要错误、读错误与写错误都会让其余段退出；写失败后主线程仍继续消费
    /// 结果队列，避免 worker 因队列满而永远阻塞。取消的块已写入临时输出，但
    /// 调用方只有在 `copy_original` 成功后才发布它。
    pub(super) fn copy_original_with_workers<R, W>(
        &self,
        snapshot: &mut R,
        output: &mut W,
        task: &ChunkTask<'_>,
        workers: usize,
    ) -> ChunkFileResult<()>
    where
        R: Read + Seek + Send,
        W: Write,
    {
        let capacity = pipeline_window(self.config.max_size);
        let (job_sender, job_receiver) = mpsc::sync_channel::<RestoreJob>(capacity);
        let (result_sender, result_receiver) =
            mpsc::sync_channel::<Result<RestoredChunk, ChunkFileError>>(capacity);
        let job_receiver = Arc::new(Mutex::new(job_receiver));
        let stop = AtomicBool::new(false);
        let stop = &stop;
        let throttle = PipelineThrottle::new(capacity);
        let throttle = &throttle;
        let cancel = task.cancel_token();
        let workers = workers.max(1);

        let (failure, reader_result, written, copied, file_hasher) = std::thread::scope(|scope| {
            let reader = scope.spawn(move || {
                let cancel_owned = cancel.cloned().unwrap_or_else(CancelToken::standalone);
                let reader_task = ChunkTask::cancel(&cancel_owned);
                let mut buffer = Vec::new();
                for (index, record) in self.chunks.iter().enumerate() {
                    if stop.load(Ordering::Relaxed) {
                        return Err(ChunkFileError::Cancelled);
                    }
                    throttle.wait_for_slot(index as u64, stop, cancel);
                    if stop.load(Ordering::Relaxed) || cancel.is_some_and(|token| token.observed())
                    {
                        return Err(ChunkFileError::Cancelled);
                    }
                    reader_task.check()?;
                    load_chunk(snapshot, record, &mut buffer, &reader_task)?;
                    // 块的字节所有权要交给 worker，缓冲无法复用，直接移出比再复制
                    // 一次省一遍全块拷贝。
                    let data = std::mem::take(&mut buffer);
                    job_sender
                        .send(RestoreJob {
                            index,
                            data,
                            expected: record.key.digest,
                        })
                        .map_err(|_| ChunkFileError::Cancelled)?;
                }
                Ok(())
            });

            for _ in 0..workers {
                let receiver = Arc::clone(&job_receiver);
                let sender = result_sender.clone();
                scope.spawn(move || loop {
                    let job = match receiver.lock().expect("restore job queue poisoned").recv() {
                        Ok(job) => job,
                        Err(_) => break,
                    };
                    if stop.load(Ordering::Relaxed) || cancel.is_some_and(|token| token.observed())
                    {
                        continue;
                    }
                    let digest = ChunkDigest::from(sha256(&job.data));
                    let result = if digest == job.expected {
                        Ok(RestoredChunk {
                            index: job.index,
                            data: job.data,
                        })
                    } else {
                        stop.store(true, Ordering::Relaxed);
                        Err(integrity(
                            ErrorDimension::Chunk,
                            format!(
                                "第 {} 个块哈希不匹配：预期 {}，实际 {digest}",
                                job.index + 1,
                                job.expected
                            ),
                        ))
                    };
                    if sender.send(result).is_err() {
                        stop.store(true, Ordering::Relaxed);
                    }
                });
            }
            drop(result_sender);

            let mut pending = BTreeMap::new();
            let mut next = 0_usize;
            let mut copied = 0_u64;
            let mut file_hasher = Sha256::new();
            let mut failure = None;
            loop {
                match result_receiver.recv() {
                    Ok(Ok(chunk)) => {
                        if failure.is_some() {
                            continue;
                        }
                        pending.insert(chunk.index, chunk.data);
                        while let Some(data) = pending.remove(&next) {
                            // 取消、写盘与大小累加共用一条失败路径，避免三段重复的
                            // 「置停止位 + 丢弃待写表」。
                            let outcome = task.check().and_then(|()| {
                                output.write_all(&data)?;
                                file_hasher.update(&data);
                                copied =
                                    copied.checked_add(data.len() as u64).ok_or_else(|| {
                                        integrity(ErrorDimension::File, "还原大小溢出")
                                    })?;
                                Ok(())
                            });
                            if let Err(error) = outcome {
                                failure = Some(error);
                                stop.store(true, Ordering::Relaxed);
                                throttle.wake();
                                pending.clear();
                                break;
                            }
                            task.report(data.len() as u64);
                            next += 1;
                            throttle.advance(next as u64);
                        }
                    }
                    Ok(Err(error)) => {
                        if failure.is_none() {
                            failure = Some(error);
                            stop.store(true, Ordering::Relaxed);
                            throttle.wake();
                        }
                        pending.clear();
                    }
                    Err(_) => break,
                }
            }
            let reader_result = reader.join().expect("restore reader thread panicked");
            (failure, reader_result, next, copied, file_hasher)
        });

        if let Some(error) = failure {
            return Err(error);
        }
        reader_result?;
        output.flush()?;
        if written != self.chunks.len() {
            if let Err(error) = task.check() {
                return Err(error);
            }
            return Err(integrity(
                ErrorDimension::File,
                format!(
                    "恢复块数量不一致：写入 {written}，布局为 {}",
                    self.chunks.len()
                ),
            ));
        }
        let digest = ChunkDigest::from(finalize_sha256(file_hasher));
        self.verify_restored(copied, digest)
    }

    /// 串行与并行导出共用的收尾校验：累计字节与整文件摘要都必须与布局一致。
    fn verify_restored(&self, copied: u64, digest: ChunkDigest) -> ChunkFileResult<()> {
        if copied != self.logical_size {
            return Err(integrity(
                ErrorDimension::File,
                format!("还原大小不匹配：预期 {}，实际 {copied}", self.logical_size),
            ));
        }
        if digest != self.file_digest {
            return Err(integrity(
                ErrorDimension::File,
                format!("整文件哈希不匹配：预期 {}，实际 {digest}", self.file_digest),
            ));
        }
        Ok(())
    }

    /// Writes a resolved layout as a complete snapshot file and returns the
    /// layout of the written file, verifying every chunk payload and the
    /// whole-file digest while streaming.
    pub(crate) fn write_snapshot<R, W>(
        &self,
        source: &mut R,
        output: &mut W,
        options: ChunkWriteOptions,
        task: &ChunkTask<'_>,
    ) -> ChunkFileResult<ChunkFile>
    where
        R: Read + Seek,
        W: Write + Seek,
    {
        require_empty_output(output, "目标 snapshot")?;
        let (chunker, adapter) = match options.format {
            FormatVersion::V1 => (V2_CHUNKER_CDC, V2_ADAPTER_NONE),
            FormatVersion::V2 => (self.chunker, self.adapter),
        };
        write_snapshot_header(
            output,
            options.format,
            chunker,
            adapter,
            self.config,
            self.logical_size,
            usize_to_u64(self.chunks.len(), "目标块数量")?,
            self.file_digest,
        )?;
        let mut writer = SnapshotWriter::new(output, options.format);
        let mut buffer = Vec::new();
        for (index, record) in self.chunks.iter().enumerate() {
            task.check()?;
            load_verified_chunk(source, record, &mut buffer, index, task)?;
            writer.emit(&buffer, 0)?;
            task.report(u64::from(record.key.length));
        }
        let end = writer.end_position()?;
        let (logical_size, chunks, file_digest) = writer.finish();
        if logical_size != self.logical_size || file_digest != self.file_digest {
            return Err(integrity(
                ErrorDimension::File,
                "物化 snapshot 的整文件摘要与布局不一致",
            ));
        }
        output.seek(SeekFrom::Start(0))?;
        write_snapshot_header(
            output,
            options.format,
            chunker,
            adapter,
            self.config,
            self.logical_size,
            usize_to_u64(chunks.len(), "目标块数量")?,
            self.file_digest,
        )?;
        output.seek(SeekFrom::Start(end))?;
        Ok(ChunkFile::from_records(
            self.config,
            logical_size,
            self.file_digest,
            chunks,
            chunker,
            adapter,
        ))
    }

    /// Chunk records in file order; the index of every record equals its
    /// position in the logical file.
    pub(super) fn chunk_records(&self) -> &[ChunkRecord] {
        &self.chunks
    }

    /// Position of the first record holding `key`, if the layout contains it.
    pub(super) fn chunk_position(&self, key: &ChunkKey) -> Option<usize> {
        self.chunk_index.get(key).copied()
    }

    /// [`ChunkFile::create_reverse_delta`] 将要写入的 DATA 载荷字节数。
    ///
    /// 反向增量只携带父版本相对自身变化的块，所以它的真实工作量是这些块的
    /// 逻辑字节，而不是父文件的 `logical_size`。这里按与写入时一致的规则
    /// （父块不在自身索引中、同一块只计一次）求和，只做索引比对，不读载荷，
    /// 让调用方能把进度归一到一个能真正走满的总量。
    pub(crate) fn delta_payload_bytes(&self, parent: &ChunkFile) -> u64 {
        let mut seen = HashSet::new();
        let mut bytes = 0_u64;
        for record in &parent.chunks {
            if !self.chunk_index.contains_key(&record.key) && seen.insert(record.key) {
                bytes = bytes.saturating_add(u64::from(record.key.length));
            }
        }
        bytes
    }

    pub(crate) fn create_reverse_delta<R, W>(
        &self,
        parent: &ChunkFile,
        parent_snapshot: &mut R,
        delta_output: &mut W,
        options: ChunkWriteOptions,
        task: &ChunkTask<'_>,
    ) -> ChunkFileResult<()>
    where
        R: Read + Seek,
        W: Write + Seek,
    {
        require_empty_output(delta_output, "delta")?;
        let mut data_keys = Vec::new();
        let mut seen_data = HashSet::new();
        let mut ops = Vec::with_capacity(parent.chunks.len());

        for (index, record) in parent.chunks.iter().enumerate() {
            if index % CANCEL_CHECK_INTERVAL == 0 {
                task.check()?;
            }
            let kind = if self.chunk_index.contains_key(&record.key) {
                DeltaOpKind::Copy
            } else {
                if seen_data.insert(record.key) {
                    data_keys.push(record.key);
                }
                DeltaOpKind::Data
            };
            ops.push(DeltaOp {
                kind,
                key: record.key,
            });
        }

        let chunker = match options.format {
            FormatVersion::V1 => V2_CHUNKER_CDC,
            FormatVersion::V2 => parent.chunker,
        };
        let adapter = match options.format {
            FormatVersion::V1 => V2_ADAPTER_NONE,
            FormatVersion::V2 => parent.adapter,
        };
        let mut encoded = zstd::stream::write::Encoder::new(&mut *delta_output, 6)?;
        write_delta_header(
            &mut encoded,
            options.format,
            chunker,
            adapter,
            parent.config,
            self.file_digest,
            parent.file_digest,
            parent.logical_size,
            usize_to_u64(parent.chunks.len(), "目标块数量")?,
            usize_to_u64(data_keys.len(), "delta 数据块数量")?,
        )?;

        let mut buffer = Vec::new();
        for (index, key) in data_keys.iter().enumerate() {
            task.check()?;
            let parent_index = parent.chunk_index.get(key).ok_or_else(|| {
                integrity(
                    ErrorDimension::Chunk,
                    format!("父版本缺少声明的块 {}", key.digest),
                )
            })?;
            let record = parent.chunks[*parent_index];
            match options.format {
                FormatVersion::V1 => write_chunk_key(&mut encoded, record.key)?,
                FormatVersion::V2 => write_v2_record_header(
                    &mut encoded,
                    record.key.length,
                    record.key,
                    CODEC_NONE,
                    RECIPE_NONE,
                    0,
                )?,
            }
            load_verified_chunk(parent_snapshot, &record, &mut buffer, index, task)?;
            encoded.write_all(&buffer)?;
            task.report(u64::from(record.key.length));
        }

        for op in &ops {
            write_u8(&mut encoded, op.kind as u8)?;
            write_chunk_key(&mut encoded, op.key)?;
        }
        encoded.finish()?;
        delta_output.flush()?;
        Ok(())
    }

    pub(super) fn from_records(
        config: ChunkingConfig,
        logical_size: u64,
        file_digest: ChunkDigest,
        chunks: Vec<ChunkRecord>,
        chunker: u16,
        adapter: u16,
    ) -> Self {
        let mut chunk_index = HashMap::with_capacity(chunks.len());
        for (index, record) in chunks.iter().enumerate() {
            chunk_index.entry(record.key).or_insert(index);
        }
        Self {
            config,
            logical_size,
            file_digest,
            chunks,
            chunk_index,
            chunker,
            adapter,
        }
    }
}

struct SnapshotWriter<'a, W> {
    output: &'a mut W,
    version: FormatVersion,
    dedup: HashMap<ChunkKey, u64>,
    chunks: Vec<ChunkRecord>,
    file_hasher: Sha256,
    logical_size: u64,
}

impl<'a, W: Write + Seek> SnapshotWriter<'a, W> {
    fn new(output: &'a mut W, version: FormatVersion) -> Self {
        Self {
            output,
            version,
            dedup: HashMap::new(),
            chunks: Vec::new(),
            file_hasher: Sha256::new(),
            logical_size: 0,
        }
    }

    fn emit(&mut self, data: &[u8], flags: u16) -> ChunkFileResult<()> {
        let digest = ChunkDigest::from(sha256(data));
        self.emit_hashed(data, digest, flags)
    }

    /// 写入一个已经由 worker 计算好摘要的块。摘要必须来自同一份 `data`；调用方
    /// 只负责保证这一点，写入顺序与整文件摘要仍在这里按序推进。
    fn emit_hashed(&mut self, data: &[u8], digest: ChunkDigest, flags: u16) -> ChunkFileResult<()> {
        if data.is_empty() {
            return Err(invalid_format(ErrorDimension::Chunk, "不能写入空数据块"));
        }
        let length = u32::try_from(data.len())
            .map_err(|_| ChunkFileError::InvalidConfig("单块大小超过 u32".into()))?;
        let key = ChunkKey { digest, length };
        let record = match self.version {
            FormatVersion::V1 => {
                write_chunk_key(self.output, key)?;
                let data_offset = self.output.stream_position()?;
                self.output.write_all(data)?;
                ChunkRecord {
                    key,
                    data_offset,
                    stored_length: length,
                }
            }
            FormatVersion::V2 => match self.dedup.get(&key).copied() {
                Some(data_offset) => {
                    write_v2_record_header(self.output, 0, key, CODEC_NONE, RECIPE_NONE, flags)?;
                    ChunkRecord {
                        key,
                        data_offset,
                        stored_length: 0,
                    }
                }
                None => {
                    write_v2_record_header(
                        self.output,
                        length,
                        key,
                        CODEC_NONE,
                        RECIPE_NONE,
                        flags,
                    )?;
                    // The payload starts after the 44-byte v2 record header.
                    let data_offset = self.output.stream_position()?;
                    self.output.write_all(data)?;
                    self.dedup.insert(key, data_offset);
                    ChunkRecord {
                        key,
                        data_offset,
                        stored_length: length,
                    }
                }
            },
        };
        self.file_hasher.update(data);
        self.logical_size = self
            .logical_size
            .checked_add(u64::from(length))
            .ok_or_else(|| ChunkFileError::InvalidConfig("逻辑文件大小溢出".into()))?;
        self.chunks.push(record);
        Ok(())
    }

    fn end_position(&mut self) -> ChunkFileResult<u64> {
        Ok(self.output.stream_position()?)
    }

    fn finish(self) -> (u64, Vec<ChunkRecord>, ChunkDigest) {
        (
            self.logical_size,
            self.chunks,
            ChunkDigest::from(finalize_sha256(self.file_hasher)),
        )
    }

    /// 回填头部并返回布局。`create` 的两条路径（串行兜底与管道）共用这里，
    /// 保证无论块以什么顺序完成哈希，最终写出的字节布局只由顺序写入决定。
    fn finish_file(
        self,
        config: ChunkingConfig,
        options: ChunkWriteOptions,
        chunker: u16,
        adapter: u16,
    ) -> ChunkFileResult<ChunkFile> {
        let end = self.output.stream_position()?;
        let logical_size = self.logical_size;
        let chunks = self.chunks;
        let file_digest = ChunkDigest::from(finalize_sha256(self.file_hasher));
        self.output.seek(SeekFrom::Start(0))?;
        write_snapshot_header(
            self.output,
            options.format,
            chunker,
            adapter,
            config,
            logical_size,
            usize_to_u64(chunks.len(), "块数量")?,
            file_digest,
        )?;
        self.output.seek(SeekFrom::Start(end))?;
        self.output.flush()?;
        Ok(ChunkFile::from_records(
            config,
            logical_size,
            file_digest,
            chunks,
            chunker,
            adapter,
        ))
    }
}

/// 读入一个块的载荷，不做哈希。并行导出把读取与校验拆到不同线程，串行路径
/// 仍通过 [`load_verified_chunk`] 组合两者。
pub(super) fn load_chunk<R: Read + Seek + ?Sized>(
    source: &mut R,
    record: &ChunkRecord,
    buffer: &mut Vec<u8>,
    task: &ChunkTask<'_>,
) -> ChunkFileResult<()> {
    buffer.resize(record.key.length as usize, 0);
    source.seek(SeekFrom::Start(record.data_offset))?;
    // A chunk is at most 64 KiB in the default configuration. Reading it in
    // 64 KiB slices keeps the cancellation floor at the block/tile boundary
    // even if a caller raises `max_size`.
    let mut offset = 0;
    while offset < buffer.len() {
        task.check()?;
        let want = (buffer.len() - offset).min(READ_SLICE_BYTES);
        source.read_exact(&mut buffer[offset..offset + want])?;
        offset += want;
    }
    Ok(())
}

pub(super) fn load_verified_chunk<R: Read + Seek + ?Sized>(
    source: &mut R,
    record: &ChunkRecord,
    buffer: &mut Vec<u8>,
    index: usize,
    task: &ChunkTask<'_>,
) -> ChunkFileResult<()> {
    load_chunk(source, record, buffer, task)?;
    let digest = ChunkDigest::from(sha256(buffer));
    if digest != record.key.digest {
        return Err(integrity(
            ErrorDimension::Chunk,
            format!(
                "第 {} 个块哈希不匹配：预期 {}，实际 {digest}",
                index + 1,
                record.key.digest
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{sync::atomic::AtomicBool, time::Instant};

    use super::*;

    /// 窗口推进要有自己的唤醒信号，而取消与停止没有；等待者必须在超时后
    /// 重新检查它们，否则「读线程等窗口、写线程等结果」会互锁。
    #[test]
    fn throttle_wait_observes_stop_after_the_poll_timeout() {
        let throttle = PipelineThrottle::new(1);
        let stop = AtomicBool::new(false);
        std::thread::scope(|scope| {
            let handle = scope.spawn(|| throttle.wait_for_slot(1, &stop, None));
            std::thread::sleep(THROTTLE_WAIT_POLL + Duration::from_millis(10));
            stop.store(true, Ordering::Relaxed);
            handle.join().expect("throttle waiter panicked");
        });
    }

    #[test]
    fn throttle_wait_returns_immediately_when_already_cancelled() {
        let throttle = PipelineThrottle::new(1);
        let token = CancelToken::standalone();
        token.cancel();
        let stop = AtomicBool::new(false);
        let started = Instant::now();
        throttle.wait_for_slot(1, &stop, Some(&token));
        assert!(started.elapsed() < THROTTLE_WAIT_POLL);
    }
}
