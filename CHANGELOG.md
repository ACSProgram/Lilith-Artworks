# Changelog

All notable changes are recorded here. The project uses semantic versioning
before and after the first stable release.

Changes since the last stamped version are collected under `Unreleased`. When
the maintainer cuts a version, that section becomes a dated version heading and
the five version fields move together, so the version number lags the work until
the cut. A version can exist without a tag: tagging is a separate, later release
decision, so an untagged version is transitional.

## Unreleased

### Added

- Reference inventories can report files that records still point at but the
  storage model no longer reads (`redundant_targets`), so the junk scan can name
  the reason instead of calling every candidate unreferenced.
- Pin-board items are now polymorphic (`pin_board_items` plus per-kind payload
  tables) instead of images only. A text item is the first non-bitmap kind:
  `create_pin_board_text` stores text, font, size, colour, alignment and the
  front-end's measured logical size; the renderer rasterizes it on demand at the
  requested texture long edge with the same font metrics used for export. Export
  rasterizes text at twice its logical long edge, encodes PNG in the webview and
  hands the bytes to `write_pin_board_item_png`, which validates the payload and
  publishes it atomically; shipping compressed PNG instead of raw RGBA keeps the
  IPC payload in the hundreds of kilobytes. Text has no DDS, so copying it inside
  the board or to the system clipboard remains bitmap-only for now, while export
  handles both kinds.
- The repository schema moves to v5 with a single v4 → v5 migration. It rebuilds
  the pin-board tables while preserving item IDs and step history, adds
  `history_nodes.thumbnail` / `thumbnail_version` / `verified_ms` /
  `verify_error`, removes the legacy `branches` verification columns, creates
  snapshot/delta/certification path indexes and adds `repository_backup_runs`.
  Failed migrations roll back completely, and repositories from a newer schema
  are refused before any write so an older build cannot corrupt them.
- Repository backups record their result in `repository_backup_runs`, and
  `get_last_repository_backup_run` lets the settings page show the latest backup
  after a restart instead of only within the current run.
- History nodes now carry a derived thumbnail so the branch inspector can show
  a preview without restoring the work file. A changed commit extracts it from
  the same file it already reads: `.clip` uses the canvas preview stored in the
  embedded SQLite database, `.psd` / `.psb` prefer the PS 4.0/5.0 thumbnail
  resource (the Photoshop 4.0 resource is BGR and is swapped back to RGB) and
  otherwise decode the merged composite (raw, RLE or ZIP), and
  png/jpg/webp/tiff/bmp/gif are decoded directly. Previews are stored as
  versioned, 512 px lossless WebP in `history_nodes.thumbnail` /
  `thumbnail_version`, never inside ChunkFile content; unchanged checks,
  checkpoints, forks and snapshot repairs keep the existing preview, and any
  unsupported or malformed input only falls back to a placeholder.
  `get_history_node_thumbnail` serves the selected node's preview on demand
  without selecting the BLOB in graph or list reads.
- A node preview can be re-extracted from the inspector. The refresh button sits
  next to "无可用预览" and over an existing preview (so a cached thumbnail can be
  replaced); `refresh_history_node_thumbnail` materializes the selected node's
  own stored content into `temp/`, runs the same format dispatch and writes the
  result back to `thumbnail` / `thumbnail_version`. It never reads the current
  work file, so old nodes keep a preview that matches their stored version; the
  command is a foreground write, verifies chunks while materializing, and leaves
  the repository untouched when extraction fails.
- Idle integrity checking is available as an opt-in background task. When
  `idleIntegrityCheckEnabled` is on and the main window is hidden or minimized,
  the scheduler verifies up to eight history nodes (or 30 seconds) per idle
  window after ten minutes of repository silence, oldest first. It reuses the
  full-scan per-node materialization kernel, records `verified_ms` /
  `verify_error` per node, yields at node boundaries to foreground work, pending
  manual commits and due automatic backups, and consumes at most one batch per
  idle window. The settings page shows the switch, its behaviour and the pending
  node count; the tray pause also pauses this task. Manual branch re-verification
  now verifies the whole chain immediately in the foreground, and the full scan
  refreshes every node's verification state while continuing past individual
  failures.

### Changed

- The pin-board add-text dialog is rebuilt around a live text preview, a
  stepper-controlled font size and an embedded colour picker (saturation/value
  area, hue slider, hex input and preset swatches) instead of the native colour
  dialog. Layout, focus rings and dark-theme colours follow the rest of the
  application.
- Checkpoints keep only their snapshot. A commit or compaction whose parent is a
  checkpoint no longer writes a reverse delta: that node's snapshot is permanent,
  so no read path could ever consume the delta. The edge is registered with an
  empty path and the delta is rebuilt on demand when the checkpoint is cancelled.
  The skip condition mirrors the snapshot-release rule (a checkpoint that holds a
  snapshot), so a parent that only keeps its snapshot because it is another
  branch's head still gets its delta.
- Cancelling a checkpoint rebuilds the child-to-parent reverse delta outside the
  transaction when the edge has none, then swaps the snapshot in a single
  transaction; the new delta is removed if that transaction fails. The rebuild
  reuses the edge's own file name, so a stale edge path is only queued for
  cleanup when it names a different file. Repositories that already hold the
  delta keep using it unchanged.
- The settings cleanup entry is a junk scan now: one pass reports checkpoint
  deltas, unreferenced files, pin-board leftovers and interrupted temp files. The
  history reference inventory only protects deltas a read path can reach, so
  checkpoint deltas can actually be collected instead of being counted as
  referenced forever.
- Cleanup, unreferenced scans, DDS scrubbing, storage accounting and reference
  inventories now read `pin_board_bitmaps`; text items have no file path and are
  excluded by construction.
- Branch verification status is derived from the entire head chain: any node's
  `verify_error` marks the branch as failed, otherwise `verified_ms` is the
  oldest timestamp provided every node has been verified. Compaction, checkpoint
  changes and snapshot replacement clear `verified_ms` / `verify_error` for the
  artwork in the same transaction, so affected nodes rejoin the pending set.
- The full repository integrity scan now records per-node verification state, so
  it runs as a foreground write command holding the cross-process owner lock
  rather than through the read-only path; it keeps scanning after an individual
  node fails and returns a summary error.

### Fixed

- PSD previews are no longer dropped or blurry for large canvases. The parser
  used to reject any document over the 64 MP shared decode budget before it read
  the embedded thumbnail, so an 84 MP file with a valid 1033/1036 resource showed
  "无可用预览". The header now always reaches the resource section, and a
  thumbnail whose longest edge is under 256 px is treated as too soft: the merged
  composite is streamed and sampled into the 512 px target instead, with the
  embedded resource kept as the fallback when the composite is unavailable. The
  composite budget is 256 MP because sampling is row-based, so memory stays tied
  to the 512 px output rather than the source canvas.
- Pin-board text textures centre the glyph ink inside each 1.3× line box.
  Drawing with `textBaseline = "top"` left all extra line height below the
  glyphs, so text sat above the centre of its selection quad and its top edge
  was clipped by the texture. Display and export share the corrected path, so
  existing text items are recentred when their texture is rebuilt.
- Byte progress is cumulative again. `ChunkTask::report` hands the callback the
  total logical bytes processed so far instead of the size of the step that just
  finished, so restore, checkpoint and reverse-delta bars climb towards 100%
  instead of oscillating between a few kilobytes on a multi-chunk file. The
  reverse-delta phase also reports against the payload bytes it really copies
  (`delta_payload_bytes`) rather than the parent file size, which kept that bar
  near zero even when the work was finished.
- Progress logging is sampled instead of written per step. `operation progress:`
  was emitted once per chunk, so restoring a 2 GiB node flooded the log with
  ~114,600 lines in a minute and rotated five files; it now writes one line when
  the phase changes and otherwise at most every five seconds.
- The headless `--marker` channel appends one `stage` line per phase instead of
  one per chunk. Progress callbacks fire per chunk, so a single large restore
  added megabytes of stage lines and every stress poll re-read the whole marker
  file; checkpoint lines still carry the latest progress numbers.
- Checkpoint deltas and stale `history_nodes.delta_path` entries were invisible to
  the scan because the reference inventory counted every recorded path, including
  deltas that no read path can reach. They are now reported as redundant files and
  can be cleaned.
- `history::load_node` no longer returns an empty delta path as `Some("")`; a
  checkpoint edge without a delta is reported as `None`, and chain resolution
  rejects an empty path with the node id instead of opening the repository root.
- Opening a repository no longer rejects the blank edge paths the cleanup flow
  leaves behind. A checkpoint's child edge records `delta_path = ''` to mean "no
  reverse delta", but the read-side path validation treated every row as a real
  relative path and refused to open the repository with
  `history_edges.delta_path 无效：仓库存储路径无效` right after a cleanup run.
  Blank entries in `history_edges.delta_path` and in the compatible
  `history_nodes.delta_path` column are now skipped as "no file", matching the
  reference inventory and `load_node`. `snapshot_path` keeps its non-null
  requirement, since readers treat any non-null value as a usable snapshot.
- The stress suite's commit crash/cancel points line up with the current
  cancellation boundaries again. Adding the thumbnail-extraction boundary
  shifted every later checkpoint by one, so `stress_crash` B1/B4 and
  `stress_cleanup` B5 killed at the wrong place: B1/B5 stopped before the
  publish step and were left with two `.tmp*` files instead of the expected
  orphans, and B4 never reached the transaction marker. The boundary indexes now
  live in `stress_support` as `COMMIT_AFTER_PUBLISH_CHECKPOINT` /
  `COMMIT_TRANSACTION_CHECKPOINT` / `COMMIT_CHECKPOINTS`, B4 selects the marker
  by its stage name instead of a number, the A1 and `stress_baseline` loops
  cover all six boundaries, J1 parks at the post-publish boundary again, and a
  new A0 contract test fails first whenever the boundary list changes.
- `stress_concurrency` J6 expects the refusal the full scan actually returns.
  Since the scan became a foreground write command it takes the cross-process
  owner lock, so a second process gets "仓库正在被另一个进程写入；不支持同一仓库的
  多进程写入" instead of the probe-era "仓库正在被写入，请稍后重试".

## 0.3.0-alpha.2 - 2026-10-19

### Added

- The repository now has a read-only storage dashboard. `get_repository_storage_overview`
  aggregates the database-recorded size (snapshot files, `history_edges` deltas,
  final artifacts and certification outputs) and `scan_repository_storage_disk`
  optionally walks the repository directory for actual disk usage; the two views
  are shown side by side and never silently merged. Non-snapshot nodes'
  `chunk_file_size` (rebuild read amount) is excluded from stored totals, final
  artifacts and certification outputs are deduplicated by normalized path, and
  board DDS classification depends on a non-empty `file_path` rather than the
  pin-board item `kind`. The dashboard breaks sizes down by category and by
  artwork → branch → node, reports snapshot and delta compression layers
  separately, and marks database-recorded files that are missing on disk. The
  disk scan is an on-demand, cancellable foreground read task; it counts only
  the repository directory and excludes external certification exports.
- Publishing reports a small-selection watermark warning, without blocking the
  publish. Before embedding TrustMark, each framed region's pixel bounds on the
  rendition are measured with the same conversion the encoder uses
  (`trustmark::region_bounds`); when either edge is under the stable threshold
  (`WATERMARK_MIN_STABLE_EDGE`, 256 px, the measured minimum stable edge), the
  publish result carries a structured `watermarkWarnings` entry (region index,
  rendition pixel size, threshold) and a `log::warn!` line, while the publish
  still succeeds — a hint, not an error. The publish page echoes the same warning
  in place while framing (computed from the source pixel size, which equals the
  rendition size) and in the publish result area; the headless `publish` result
  JSON carries the same field for stress assertions. The threshold is a risk
  reduction, not a guarantee: larger selections or smoother content may still
  fail pixel-domain readback, while C2PA claims and local record matching are
  unaffected. `stress_authenticity` gains `G6`, asserting that 96/128/192-level
  regions warn, 256/512-level regions do not, and the warning count matches the
  number of sub-threshold regions.
- A new `stress_format` stress group closes the format-layer integration gap
  that `cargo test --lib` cannot reach: end-to-end commit → delta → restore
  through the real headless binary for `.clip` container-aware chunking (tile
  structure, the screencast shape without tile records, and a truncated
  container falling back to CDC) and for v1/v2 mixed repositories. Assertions
  use disk facts only: the ChunkFile common header (little-endian version /
  chunker / adapter, delta frames decompressed through zstd) is checked after
  every commit, so a silent fallback to CDC or a dropped format flag fails the
  suite. Because only the branch head keeps a snapshot, the v1 commit's delta
  is the surviving v1 evidence; checkpointing the branch start then
  materializes a chain mixing v2 and v1 deltas into a v2 snapshot. All
  fixtures are synthesized at runtime with deterministic seeds; no binary
  samples are committed.
- The headless entry gains `--chunk-format v1|v2`, a process-level override of
  the `chunkFileFormatV2` setting (default unchanged). It only affects the
  snapshots/deltas written by that process, which is what lets one test
  repository hold both layouts; the GUI path is untouched.
- The cheap headless stress groups in Windows CI grow from eight to nine with
  `stress_format`, keeping the format matrix under daily regression
  protection.
- The history branch-view inspector now labels the selected node's ChunkFile
  format (`ChunkFile v1` / `ChunkFile v2`). `get_history_node_format` resolves
  the node's current storage file the same way as `chunk_file_size` (snapshot
  when the node owns one, otherwise the cheapest child-edge delta) and reads
  only the 10-byte common header; missing or unreadable files show 未知 without
  surfacing a global error. No schema change and no extra work on periodic
  history polling.
- `stress_bench` records each measured operation's process CPU time (user +
  kernel) next to the wall clock. Each child process is queried once after exit
  via `GetProcessTimes`; the cycle counter (`QueryProcessCycleTime`) is kept for
  failure diagnostics only. Results go to `target/stress-report.jsonl` and
  `tests/perf-baseline.json` — `scenario::label` is wall clock,
  `scenario::label#cpu` is CPU time. Because CPU time excludes the time a
  process is descheduled, it stays comparable under `cargo test` parallelism,
  where the same baseline's wall clock stretched by 2.3–4.1×. It cannot see disk
  waits, and it is quantized to the 15.625 ms Windows accounting tick, so
  baselines below 50 ms are recorded but not gated. Strict mode now compares
  both, and `LILITH_STRESS_PROFILE` names the build profile a baseline belongs
  to when a debug harness drives a release binary. The measurement reuses the
  existing `windows-sys` dependency (one feature enabled) and adds no crate.

### Changed

- The `stress_large` C4 ledger bound now accounts for snapshot growth. Its expected
  delta still had the v1 shape — v1 snapshots are a constant size, so replacing the
  head snapshot cost nothing — while v2 snapshots grow with newly distinct content;
  the `heavy` and `extreme` tiers therefore reported a 12.8 MB / 10.4 MB excess that
  matched snapshot growth exactly. The expected value now includes
  `snapshots_after - snapshots_before` (recorded as `snapshotGrowthBytes` in
  `ledger.jsonl`), and both tiers pass after the fix.
- `stress_bench` now samples the P2 publish/decode and P8 orphan scan three times
  and keeps the fastest round, like the other scenarios. Those three were
  single-shot, and repeated sampling shows their wall clock swinging 4–6× on the
  same idle machine (signing 8–50 ms against an 11 ms baseline, orphan scan
  51–203 ms against 71 ms), so the 2.5× strict gate was failing on scheduler noise
  rather than on a regression.
- Frontend changes (including test files) now require `npx tsc --noEmit` in the
  lightweight verification set, alongside `npm test`. Vitest transpiles with
  esbuild and strips types without checking them, so type-only errors in tests
  previously surfaced only when `npm run build` ran as the tauri
  `beforeBuildCommand`, blocking release builds. Documented in
  `docs/guides/validation.md`.
- `AGENTS.md` adds a decision-and-questioning rule: when a choice is
  significantly uncertain and materially affects behavior, performance or
  architecture, the agent asks the user first, stating each option's
  consequences in plain language (e.g. "noticeably raises CPU/memory use of a
  feature", "makes the architecture more complex and coupled"); clear-cut,
  reversible choices are made directly with the reasoning stated, so thinking
  is not offloaded to the user.

- The settings dialog is split into `仓储`, `空间` and `备份` pages. The
  repository page keeps the location, integrity check and storage format; the
  space page hosts the storage dashboard and the file cleanup queue/scan; the
  backup page owns automatic backup scheduling, the default check mode and
  repository backup, and shows the last backup result for the current run (not
  persisted yet). The in-page `设置 / 空间` switch is gone. Command names, DTOs,
  settings writes, repository switching and tray refresh keep their previous
  semantics.
- Cancellation observation is split into two layers. Hot loops (`ChunkTask` in
  the chunk-file writer/reader) now perform only a **silent** query —
  `CancelToken::observed`, which reads whether cancellation has already been
  decided without writing an interlock marker or blocking — while explicit
  boundaries keep the interactive checkpoint (`is_cancelled`). Interactive
  checkpoints therefore appear only at boundaries: their count no longer scales
  with how many chunks a file produces, checkpoint marker files stay in the
  hundreds of bytes instead of reaching megabytes, and the fixed checkpoint
  indices the crash/cancel stress tests rely on address the positions their
  comments claim. Cancellation latency does not regress, because a cancellation
  decided at a boundary is immediately visible to the next silent query.
  Release builds are unchanged bit for bit: `CancelToken::for_backup`, the GUI's
  only source, uses the same closure for both layers, and the layered token is
  constructed only in the headless entry, which is not part of release
  artifacts.
- The `stress_large` snapshot-size assertions follow the v2 format instead of
  the v1 one. They now assert "not larger than the logical size plus per-chunk
  overhead", assert separately that v2 dedup/compression really takes effect on
  the sparse fixtures, and add a dense, incompressible case (`C8`) asserting the
  snapshot tracks the logical size, so "the snapshot got smaller" can no longer
  hide content loss. The two `deltas * 4 <= snapshots` ratio assertions, which
  only tracked the dedup ratio, are replaced by change-volume-based bounds.
- `stress_large`'s disk ledger counts only the repository, the work file and the
  product-facing outputs; the fixture's own bookkeeping (`scratch/` markers,
  results and stderr) no longer inflates the measured workspace usage. Repository
  temporary files under the repository root `temp/` (the delta payload overflow
  directory) are now visible to the stray-file assertion as well.
- `stress_authenticity` G4 publishes from a 512-level generated source instead
  of the 128x128 fixture. Pixel-domain TrustMark readback is unstable at the
  96/128 level — an inherent robustness limit of the external watermark engine,
  about 5% failures at 128 — while C2PA soft binding always matches. The
  readback rate per selection edge is recorded as an observation, and the
  guarantee boundary is stated in the report.
- The `stress_concurrency` `J5` assertion is aligned with the repository's
  actual write semantics instead of demanding an outcome the product never
  promised. `compact` now passes either by succeeding or by taking the explicit
  cross-process write refusal: the full-library scrub's sentinel *probe* also
  uses `BEGIN IMMEDIATE`, so under a race the probe can win the lock and the
  concurrent write is refused. A refused compaction is retried once on the same
  repository (by then uncontended, hence deterministic) and asserted to succeed;
  both refusal kinds are counted per round, and the round count rises from 4 to
  6. The lock and the probe are untouched, so no product behavior changes.

### Fixed

- The history node inspector now shows the real format version for delta
  nodes. `stored_format_label` read the stored file's first 10 raw bytes and
  required `LBCHUNK\0`/`LBDELTA\0` magic, but published delta files are whole
  zstd streams — the `LBDELTA\0` header (which carries the version) lives
  inside the stream — so every delta, v1 and v2 alike, failed the magic check
  and the UI showed 未知 while snapshots showed their version. The label reader
  now recognizes the zstd frame magic and streams the first 10 bytes through a
  zstd decoder before checking the version; plaintext snapshot reads are
  unchanged. The zstd frame magic is shared as `ZSTD_FRAME_MAGIC` instead of a
  hardcoded array in the delta opener, and the header-label unit test now
  covers zstd-wrapped v1 and v2 deltas.
- Stale entries in the repository-root `temp/` are swept automatically when a
  repository opens. `prepare_repository` now runs `cleanup::sweep_repository_temp`
  once the database is ready: it removes expired `authenticity-preview-<UUID>`
  preview cache directories (`authenticity::pipeline` keeps one per process,
  about 40 MB observed in long-lived repositories) and stray `.tmp*`
  `NamedTempFile` crash residue from restore-chain resolution, both under the
  shared 30-minute grace period. The current process's own session directory is
  always kept, entries with other names are untouched, symlinks are not
  followed, and deletion failures are only logged, so opening never blocks.
  Losing a preview cache only costs one preview recomputation; per-artwork
  `artworks/<artwork>/temp/` residue keeps its report-then-confirm path from
  the previous batch.
- The pin-board finalize busy-retry test compiles again. Its success-case mock
  resolved only `{ revision: "r2" }` while `finalizePinBoard` is typed as
  `SavePinBoardResult` (`saved`, `revision`, `nowStep`), so the first
  `npm run build` after the write-gate batch failed at `tsc` with TS2345. The
  mock now returns the full result shape; `npm test` had passed because vitest
  strips types without checking them.
- Repository writes inside one process no longer refuse each other. Every write
  command now queues on a per-repository in-process write gate before taking the
  cross-process owner lock: `Shutdown` (pin-board settlement) overtakes
  `Foreground` (restore, compact, checkpoint, full backup, publish, scrub,
  repository switch) overtakes `Interactive` (normal writes, pin-board writes,
  cleanup replay), while `Background` (automatic backup, idle verify) only tries
  and yields without queuing (the scheduler records that as `Skipped`, not a
  branch failure). The cross-process boundary is unchanged: another process
  holding the owner lock is still refused immediately with the existing
  "仓库正在被另一个进程写入；不支持同一仓库的多进程写入" message. A
  same-process loser now waits (bounded and cancellable) instead of reporting
  that foreign-writer message: interactive writes try first, request background
  yield only on real contention, then queue up to 10 s; foreground commands
  register the foreground wait and request background yield *before* taking the
  gate (yielding only after the lock had already been refused was the 0.2 -> 0.3
  regression) and get a 60 s cap; pin-board settlement uses `Shutdown` with a 10 s
  cap. An in-process wait that does time out still reports `repository_busy`
  (retryable) but with the text "仓库正在写入，请稍后重试", and a queued wait
  cancelled by shutdown maps to `operation_cancelled`. The gate is released only
  after the SQLite owner lock in `RepositoryOwnerLock::drop`, keeps its zero-wait
  `acquire` for `probe` and headless, and its idle entry is retired under the
  registry lock so a writer that fetched it just before can never hold a removed
  gate alongside a newly created one.
- Startup no longer loses the repository state to a busy cleanup replay.
  `useSettingsController.load` backfills the settings snapshot and repository
  status as soon as both reads return, then runs the cleanup-queue replay as an
  independent best-effort step (`void replayCleanupQueue()`): it first lists the
  queue with the read-only command and returns immediately when it is empty (the
  normal case), and a refused replay only writes an info diagnostic instead of
  notifying or blocking the interface. `settingsBusy` now covers only the two
  reads. Previously the write command ran inside `load`, so a same-process
  `repository_busy` made `load` throw and left the window on
  `EMPTY_REPOSITORY_STATUS` showing the repository as unavailable.
- Pin-board settlement survives an automatic backup holding the repository.
  `finalize_pin_board` runs through the `Shutdown`-priority entry (10 s cap, well
  inside the native 15 s force-quit fallback), and the renderer retries a
  settlement that is still `repository_busy` once after 300 ms before reporting a
  warning, so closing the window or switching boards during a backup settles the
  board instead of failing it.
- Crash residue in the per-artwork staging directory is now reported and
  classified correctly. Backups, restores and checkpoints write their snapshot
  and delta staging files as `NamedTempFile` under
  `artworks/<artwork>/temp/`; a killed process or power loss skips their `Drop`
  and leaves `.tmp*` files that no database row references. The disk scan
  counted them as unrecognized files (1.69 GiB in a real repository) because it
  only recognized the repository-root `temp/`, and the unreferenced-file scan
  never looked into the directory, so the application could not reclaim them
  either. `scan_files` now classifies `artworks/<artwork>/temp/` as repository
  temp, and `HistoryReferenceInventory::scan_candidates` reports stale `.tmp*`
  files there (keeping the existing 30-minute grace period and hash-verified,
  user-confirmed deletion), so they appear in the cleanup scan with the reason
  "备份或恢复中断后残留的临时文件" instead of hiding in the unrecognized total.
  Non-`.tmp` files placed in that directory by hand are not reported.
- The settings page now shows the live stage, counts and percentage of its long
  operations. It matched `BackupRuntimeStatus.operation` against command names
  (`repository-scrub`, `repository-backup`, `unreferenced-scan`), but the
  backend reports the serialized `OperationDomain` (`maintenance`, `cleanup`),
  so the check never matched and the dialog stayed on its "preparing"
  placeholder until the operation returned. The page now accepts a foreground
  `userOperation` whose reported domain is `maintenance` or `cleanup`, which
  keeps background scheduling and unrelated foreground work (publish, pin
  board) out of the settings progress area.

## 0.3.0-alpha.1 - 2026-10-18

### Added

- The 0.3 stage 1 command runtime: `CommandContext`, `OperationCoordinator`,
  typed `CommandError` (stable `code`, `kind`, `retryable`, operation id),
  `CancelToken`, and the frozen `AppEvent` v1 envelope with
  `operationStarted`, `operationProgress`, `operationFinished`,
  `repositoryChanged` and `cleanupQueueChanged`. Commands now run through
  GUI/headless shareable use-cases; Tauri keeps returning string errors so
  user-facing text is unchanged, while structured fields go to events and
  logs, and `invokeCommand` reconstructs known structured errors as `Error`
  objects without altering string rejections.
- A shared `src/shared/appEvents.ts` contract for the v1 event channel. Stage 3
  will add the subscription provider on top of it.
- A new `stress_baseline` group (stage 0 of the 0.3 plan) records the 0.2
  baseline for lock wait, task duration, cancel latency, deep-chain restore,
  scheduling scan, cleanup query and backup I/O. Its real-repository scenarios
  run only when `LILITH_BASELINE_REPO` points at a temporary repository copy
  and skip themselves otherwise, so plain `cargo test` and CI are unaffected.
  That copy is temporary external material that is dropped once the 0.3 plan
  lands: the group creates no baseline file, sets no threshold, and is not a
  long-term test standard. The headless entry gains a read-only
  `scheduler-scan` probe, and the test gate now measures cancel latency (an
  upper bound limited by its 5 ms poll interval) and stamps the build profile
  into each record. No product behavior changes. Results are summarized in
  `docs/planning/0.3-baseline-2026-10-18.md`.
- A single `SettingsService` write entry for the settings page, tray toggle and
  window-geometry capture. The settings file gains a monotonic `revision`
  counter; saves compare it (CAS) and return `settings_conflict` instead of
  silently overwriting a concurrent change, while internal patches preserve
  the current snapshot and bump the same counter.
- A repository-level cross-process owner lock shared by GUI, headless and
  maintenance commands. Every write operation acquires it before the runtime
  lock and releases it afterwards; read-only commands still take no lock, and
  full-library/board scrubs keep the probe-only "仓库正在被写入" refusal. A
  concurrent writer gets `repository_busy` instead of queueing, and that
  includes a foreground command arriving while the scheduler's automatic
  backup or idle verify holds the lock in the same process. The existing
  SQLite sentinel remains the lock, with a sidecar owner record
  (PID, process start, version, role, operation, heartbeat) for diagnostics.
  Kernel release is the liveness proof: metadata left by a crashed process is
  overwritten on the next acquisition and is never reclaimed by PID. Lock
  artifacts are excluded from repository backups and from database discovery.
- Shutdown settlement is now visible in structured diagnostics: the webview
  records settlement outcome, elapsed time and a `[watchdog]` warning when it
  is still running after 5 s; the native fallback logs its 15 s confirmation
  timeout, and `confirm_app_shutdown` logs `[watchdog]` entries when an
  in-flight operation or the repository operation lock delays exit.
- The App frontend is split into `AppRuntimeProvider`,
  `useSettingsController`, `useRepositoryMaintenance`,
  `RuntimeProgressProvider`, `SettingsDialog` and `NoticeCenter`. `App` only
  composes providers and routes the workspace; settings, maintenance progress
  and notices no longer share one God component.
- One `AppEvent` v1 subscription bridge with a single native listener, filtered
  by event version, active repository key and event timestamp. The shared
  `useRequestEpoch` hook now provides request generations for the History,
  Library, Authenticity and Pin-board controllers, and `useAsyncOperation`
  provides their busy/finally and error-reporting strategy except for the
  pin-board import/export transfers, which own their busy flag because only one
  transfer may run at a time.
- A `usePinBoardController` that owns board loading and selection, lifecycle
  settlement registration and save/import/export state. The WebGPU renderer
  stays a pure renderer; pin-board import/export progress now travels through
  `CommandContext` as `operationProgress` events instead of a `Channel`.
- ChunkFile v2 as an opt-out write format: 80 B snapshot and 120 B delta
  headers record version, chunker, hash, codec and adapter; 44 B chunk records
  carry stored/logical lengths, codec, recipe and flags. Snapshots store
  duplicate chunks once as `stored_len = 0` references (tile-level dedup), and
  the v1 reader stays in place so a repository can mix both versions. Errors
  now separate `InvalidFormat`, `Integrity` and `Incompatible` from the
  file/container/chunker/hash/codec/recipe/adapter/chunk dimension.
- A shared container parser in `src-tauri/src/format/container.rs` that detects
  `.clip` (`CSFCHUNK`) and `.psd` (`8BPS`) and walks `.clip` CSFC blocks, tiles
  and Exta records without loading payloads. v2 snapshot and delta writes use
  its structural units for tile-aligned chunking, split oversized units with
  CDC and fall back to CDC when parsing fails; `.psd` is detect-only for now.
- A sorted virtual-segment index and cached reader cursor in `ChunkStore`.
  Sequential reads stay in the current segment, jumps use binary search, and a
  read that crosses segment boundaries completes in one call instead of
  rescanning the segment list per chunk.
- A versioned quick-check fingerprint for automatic backups (version 1:
  SHA-256 over a version byte, size, nanosecond mtime and 64 KiB head/tail
  samples, high bit cleared and stored in the existing
  `branches.last_source_modified_ms` column). Baselines holding a raw mtime,
  timestamps without subsecond precision, missing files or failed sampling
  fall back to a full check; manual commits stay full.
- A `chunkFileFormatV2` application setting (default on) and a settings-page
  switch. It only selects the format of newly written snapshots/deltas; old v1
  nodes remain readable and switching off restores v1 writes without rewriting
  existing files. Commit, checkpoint and compaction logs record the format.
- A `stress_bench` P1B benchmark that builds a 4 MiB × 6-node chain, restores
  the oldest and newest nodes repeatedly and reports p50/p95 plus throughput
  for multi-segment reads. It is release-time decision evidence and does not
  run in daily checks or CI.
- A neutral `contracts` module with `ReferenceInventory` and
  `RepositoryIntegrityProvider`. History, authenticity and pin-board each
  implement the reference inventory for their managed files; the application
  registers the list and the disaster-recovery provider. `cleanup` no longer
  imports pin-board path constants, and the backup core no longer imports
  Library or Authenticity.
- Batch cleanup backed by a connection-local `TEMP` reference table and an
  anti-join. Confirmed cleanup commits every 512 entries, is cancellable
  between batches, stays idempotent, and skips the reference inventory when
  the queue is empty; unreferenced scans now collect candidates through the
  registered inventories.
- Per-task `OperationId` and `CancelToken` for background backup and idle
  verification. `cancel_background` only signals background tasks, while
  `request_cancel` still cancels everything. Tasks are registered either as
  the serialized run-lock holder or as a read-only parallel probe, and only
  the former counts for `cancelled()`, so a background yield request can no
  longer abort the foreground command that asked for it.
- Deadline-scheduled automatic backups: candidates are ordered by due time,
  last check and branch id, per-branch failure backoff no longer blocks other
  branches, and the idle-verification aging rule runs a ready verification
  after every 4 consecutive automatic backups.
- Idle verification now prepares up to 4 targets in parallel, one per Artwork,
  and writes results back serially under the shared runtime lock. Each worker
  has its own task guard and cancellation token.
- Repository backup manifest v2 records both the full SHA-256 and a
  64 KiB head/tail sample digest computed during the copy pass. Manifest
  generation reuses those digests instead of re-reading every file, and the
  pre-publish check defaults to sampled verification (`Sample`) while a full
  verification remains available internally.
- `stress_bench` gains P7 (scheduler candidate scan over 10 artworks ×
  12 branches, 121 branches with the fixture's own main branch) and P8 (scan
  and confirmed cleanup of 120 orphan board DDS files); P5 now records file
  count, throughput and the sample verification mode.

### Changed

- Library, History/Backup, Authenticity, Cleanup, Settings and Pin-board
  repository entries now go through `OperationCoordinator` and one shared
  use-case per operation. The lock order is fixed as cross-process repository
  owner lock → shared runtime lock (write/foreground/background) → repository
  lease → repository operation lock → SQLite transaction. Pin-board writes
  keep their existing semantics via `run_repository_write`: lease plus
  repository operation lock, no shared runtime lock. Headless is now a context
  assembler over the same use-cases and keeps its marker/cancel-checkpoint
  format.
- Every Tauri command is registered from one domain-grouped table in
  `app/command_registry.rs`, so `lib.rs` no longer carries the command list;
  adapters stay limited to DTO parsing, picker authorization and error
  mapping, and final-artifact materialization plus publish orchestration moved
  out of the authenticity adapter into `authenticity/artifact.rs`. Long
  history operations log their target with the operation id so the line can be
  correlated with the coordinator's start/finish entries.
- `main` is frozen as the 0.2 release line and `dev` is the 0.3 development
  line. Windows CI now also runs on pushes to `dev`, and the release
  metadata check maps versions to repository schemas (`0.2.x` → v4,
  `0.3.x` → v5) instead of requiring v4 unconditionally.
- The 0.3 architecture and algorithm plan is finalized for implementation:
  alpha1 keeps the persistent schema at v4, alpha2 performs the single
  v4 → v5 migration in stage 7, pin-board entries use a common item table plus
  per-kind payload tables, and storage totals no longer double-count
  `history_nodes.chunk_file_size` for delta nodes.
- History/backup runtime progress is event-driven: history and maintenance
  consumers refresh on progress/finish events and `repositoryChanged`, keeping a
  3 s snapshot poll as the lost-event and background-scheduler fallback and
  replacing the 350 ms high-frequency loop. The library tree refreshes on
  successful write events and keeps its 30 s fallback for scheduler-only writes.
  Event delivery failures are logged through the existing diagnostics channel
  and never change business behavior.
- Pin-board commands no longer accept a progress `Channel`; import, clipboard
  import and export report progress through the coordinator. The
  `PinBoardRenderer` API and all command names/DTOs are unchanged.
- `history::materialization_chain` resolves the nearest descendant snapshot
  and the whole parent chain in one recursive CTE instead of one query per
  node. The returned chain still starts at the base snapshot and ends at the
  requested node, so compaction, checkpoints, restore and scrub share the same
  resolution semantics.
- Chunk creation, reverse deltas, payload copies, chain resolution, export,
  checkpoints and scrubs now take the crate-root `CancelToken` directly.
  Cancellation is checked at every structural unit (tile) and CDC chunk,
  progress is reported in processed logical bytes, and cancelled temporary
  outputs are still removed by their guards.
- The backup scheduler no longer parks the whole queue for 60 s after one
  branch fails: each branch carries its own retry deadline and the loop moves
  straight to other due branches. Idle verification is selected in a batch
  (deduplicated per Artwork), yields without writing results when paused or
  when foreground/manual work is pending, and remains paused together with
  automatic backups.
- Repository backup publication now verifies the staged bundle with sampled
  digests by default, while still checking the exact file set, sizes and total
  bytes. The copy-phase metadata check that rejects work files changed during
  the copy is unchanged.

### Fixed

- v2 snapshots written with container-aligned chunking can be reopened. The
  reader applied the CDC minimum chunk length to every non-final record, but
  structural units follow the container layout (a 24 B file header, a 56 B
  `Exta` header), so every `.clip` snapshot written with the default v2
  settings failed to open and blocked restore, checkpoints and scrubs for that
  node. The minimum now only binds CDC layouts; `max_size` still bounds every
  record, and a regression test reopens a structurally chunked snapshot and
  restores it byte-identically.
- Permanently deleting or emptying the library trash no longer hides the
  "N files are still pending cleanup" warning: the trash reload that follows the
  mutation does not touch the shared error channel anymore, so a partial file
  cleanup stays visible together with its retry entry point.

## 0.2.0 - 2026-10-17

### Added

- Headless commands that write a repository now take a **cross-process write
  sentinel**: an exclusive lock held by a dedicated SQLite database
  (`BEGIN IMMEDIATE`, `busy_timeout = 0`), so a second writer gets an explicit
  refusal instead of racing the first in the business layer. The rejection reads
  "仓库正在被另一个进程写入；不支持同一仓库的多进程写入". SQLite releases the lock
  when the handle closes, so a killed process cannot leave the repository
  locked — a plain lock file would, and this project has many kill scenarios.
  Only the headless entry is gated, only for the commands that write, and only
  when the repository directory already exists; read-only commands,
  `init-repository`, and the whole GUI path are unchanged (the GUI keeps relying
  on the single-instance plugin). Covered by the new `stress_concurrency` group.
- New tests close the gaps listed in `docs/planning/todo.md` under "待补强":
  a deterministic property test over the reverse-delta chain (any revision in a
  random chain restores byte-identically), a random commit/restore sequence on a
  real repository, disk-full injection on the real snapshot write path
  (`ErrorKind::StorageFull` returns a clean error and leaves the repository
  intact and retryable, with the partial bytes actually on disk), and
  multi-process contention plus 20 rapid sequential commits. The `chunk_file`
  snapshot writer was already generic over `Write + Seek`, so disk-full injects
  at the real data path rather than a mock; the commit path's snapshot write was
  extracted into `create_snapshot` for the same reason, and the shared fixtures
  live in the new `backup/test_support.rs`. Coverage and run commands are
  documented in `docs/guides/validation.md`.
- The remaining review gaps are closed without touching product code:
  a **large-file interruption** scenario (every commit cancel checkpoint plus
  one real kill on a tier-sized work file, asserting no residue, no history
  advance, and a byte-identical head restore, since the cancel and kill groups
  previously only used 96 KiB files); **main-database corruption** (the
  `.sqlite3` header overwritten with non-database bytes, and the file emptied —
  both must fail cleanly with an actionable error, and the emptied file must
  not be rebuilt into a fresh empty repository, which would silently discard
  history); **write + read interleaving** (read commands still answer, and see
  a consistent pre-write state, while a writer holds the sentinel lock,
  including the "snapshot published, transaction not yet committed" window;
  concurrent readers do not block each other); and a **performance baseline**
  for core algorithms (node chain, authenticity publish/decode, board DDS
  import/scan) recorded to `src-tauri/tests/perf-baseline.json`, with an opt-in
  strict comparison (`LILITH_STRESS_PERF_STRICT=1`, `LILITH_STRESS_PERF_FACTOR`,
  default 2.5x) so a slow regression is caught instead of only an
  order-of-magnitude blow-up.
- `tests/gate_guard.rs` guards the one line Cargo cannot: it scans every
  `stress_*.rs` and fails if `#![cfg(feature = "headless")]` is missing, so a
  new stress target can no longer silently rejoin a bare `cargo test` and pop
  a GUI window.
- `app/settings.rs` gains a unit test for the long-standing concurrency
  promise that read-only repository commands keep answering while a long write
  (restore/compaction) holds the exclusive operation lock. A regression now
  fails by timeout instead of hanging the board.
- The review list's remaining items are closed with tests only:
  - **Input-domain extremes** (`stress_input`, group K): non-ASCII, emoji, ZWJ
    sequences, non-BMP code points, RTL, combining marks, zero-width characters,
    embedded newlines/tabs and titles containing path separators must round-trip
    character for character (which also proves a title is never used as a file
    name); title length is counted in **characters** (160 emoji = 640 bytes is
    accepted, 161 is rejected) after trimming; only-whitespace titles are
    rejected; non-ASCII, spaced and deep source paths work end to end; relative,
    out-of-workspace and missing sources are all refused with a stated reason.
  - **Malformed-input fuzzing** for the four parsers — snapshot/delta
    containers, DDS headers, C2PA manifests and the settings file — driven by a
    deterministic generator in the new `src/fuzz_support.rs` (`cfg(test)` only,
    also the single implementation behind `backup::test_support::Rng`). No new
    dependency. The contract asserted is that untrusted bytes can never buy a
    panic, an out-of-bounds access or an unbounded allocation: an accepted
    container must be self-consistent (chunk count within the file's capacity,
    materialisable, length equal to the declared size), an accepted DDS size
    must sit inside the declared limits, and a settings file that fails to parse
    must be **left byte-identical** rather than overwritten with defaults.
  - **Randomised corruption** (`stress_damage` R5/R6): every managed file gets
    several seeded single-point rewrites, each of which must be detected by the
    content-level check with exit code 1 and no panic while the database-level
    check stays unaffected; board DDS gets six structural corruptions (magic,
    DX10 fourcc, BC7 format, zero width, oversized height, truncation) which
    must be reported as `corrupt`, plus 24 random rewrites after which the scan
    must still return a report.
  - **Multi-operation interleaving** (`stress_concurrency` J3–J6): reads answer
    while a *long* write holds the lock (repository backup and compaction,
    parked at every checkpoint); a genuinely concurrent compaction ×
    full-repository scrub run (four rounds, no window constructed) must not
    hang and must leave the repository complete with every remaining node
    byte-restorable, and a scrub started while the writer holds the sentinel
    is rejected explicitly with "仓库正在被写入，请稍后重试" (J6 asserts the
    refusal deterministically; two of four unwindowed J5 rounds hit it).
  - **Performance baseline extended to six scenarios**: library tree assembly
    and two search queries, whole-repository backup (throughput as well as
    time), and cleanup scan / queue replay join the node chain, authenticity and
    board DDS baselines. Each of the new ones takes the minimum of three runs and
    asserts correctness on every repeat, so the number cannot drift away from a
    correct result.
- All eight cheap stress groups now run in Windows CI (the seven from before
  plus `stress_input`), which brings the input-domain and path-boundary
  regressions into everyday runs instead of pre-release ones.

### Fixed

- Documentation aligned with the actual CI and stress-test split: the release
  policy and the history/backup and authenticity module docs now state that the
  eight cheap stress groups run in Windows CI (only `stress_large`,
  `stress_scale`, and `stress_bench` still run manually before a release), the
  Rust audit ignore-list description matches both workflows, and the
  stress-test report's item count, J5 rerun conclusion, and 4.18 section
  coverage were corrected.
- TrustMark models are now verified before they are loaded. `model_info` computed
  the encoder and decoder SHA-256 but never compared them with anything, and
  `model_files_ready` only checked that the files existed, so a replaced
  `*_Q.onnx` in the install directory was loaded silently — which undermines the
  authenticity guarantees the application itself advertises. Both digests are
  pinned as constants next to the loader (matching
  `src-tauri/resources/models/README.md`), the check runs once and is cached with
  the engine, and a missing or mismatched model makes `model_files_ready` false
  and makes the loader fail with an explicit message naming the encoder or
  decoder instead of loading it. `pinned_digests_match_the_shipped_models`
  compares the constants with the models in this repository, so a model swap that
  forgets the constants fails before the commit rather than after the release.
  The compile-time `CARGO_MANIFEST_DIR` fallback in `lib.rs` is gone: a
  developer-machine path does not belong in a release artifact, and the packaged
  resource directory is the only place the models are resolved from.
- Pin-board import and export now run the same two path gates the authenticity
  module runs. `import_pin_board_images` handed renderer-supplied absolute paths
  straight to `dds::read_limited` and the raster decoder with neither a
  filesystem-scope check nor a repository-boundary check, and
  `export_pin_board_images` only required the target directory to exist; the
  "外部源绝对路径必须已由文件选择器授权" rule in the module documentation was
  enforced by the UI alone and never reused in Rust. Both commands now call the
  shared scope gate (moved out of `authenticity/commands.rs` into
  `src-tauri/src/path_scope.rs`, so the two domain modules still do not import
  each other), and both domain functions refuse paths inside the repository,
  which also covers the headless entry. "Copy files in Explorer, paste into the
  board" keeps working: the clipboard-reading command adds the paths it read
  itself to the scope, so the import still passes the same gate. Pasting raw
  **text** paths no longer imports — such a path has no authorization source, so
  the Rust layer rejects it; paths now come from the picker or the clipboard
  files, while drag-and-drop still uses the byte channel. Covered by
  `pin_board::repository` and `path_scope` tests.
- External cleanup entries no longer replay themselves on startup. The
  `pending_file_cleanup` queue kept `external_file` rows (an absolute path
  outside the repository) out of the path validation on purpose, and the
  frontend called `retryFileCleanup([])` as soon as a repository became ready,
  where an empty id list meant "process everything". Opening a poisoned
  repository was therefore an automatic, unconfirmed "delete any absolute path"
  primitive, with the target's SHA-256 as the only prerequisite. An empty id
  list now means **repository entries only** (`repository_file` /
  `repository_directory`), so the startup replay and the settings page's
  "retry all" both leave external rows alone; those are deleted only from the
  settings page, one row at a time, through an explicit id. Each external row
  now shows the full path, the owning artwork (looked up through the export
  path's certification record, empty when the publish failed before the record
  existed) and the expected SHA-256, and the button opens a slide-to-confirm
  dialog before the delete is queued. The existing "keep the file when the
  digest does not match" guard is unchanged. A per-session directory allowlist
  was rejected as the alternative: confirmation already removes the automatic,
  unconfirmed primitive, an allowlist would only weaken the gate, and it would
  expire on restart, which reads as a bug. Covered by `cleanup::run` and `App`
  tests.
- Opening a repository no longer trusts the branch work-file paths recorded in
  its database. `validate_repository_semantics` checked every controlled
  relative path but not `branches.source_path`, whose value only had to pass
  the write side; a crafted or shared repository could therefore carry
  `\\host\share\x` or a device path, and the automatic backup's quick check
  would call `metadata()` on it (a silent SMB authentication towards that host)
  while a full backup would open and copy the file into the history. The read
  side now runs a **separate structural check** for `source_path` /
  `source_path_key` — absolute, no device namespace, outside the repository
  directory — deliberately **not** the controlled-relative-path check (the
  field stores an absolute path by design), **without** testing whether the
  file exists (work files get moved or deleted, and opening a repository must
  still succeed), and **without** rejecting network paths (a NAS is a legal
  setup, and refusing it would lock users out of their existing repositories).
  What gets refused is only what hand-written data can produce: the device
  namespace (`\\.\`, `\\?\`, `\Device\`) and paths inside the repository. The
  unattended use sites — scheduled-branch selection, the quick check, and the
  auto-triggered `run_backup` — skip network and device paths and record a clear
  reason on the branch (`mark_automatic_backup_skipped`, which does not count a
  failure, set a retry deadline, or disable automation, and skips the write when
  the reason is unchanged), reusing the existing "does not take part in
  automatic backup" rule; the settings page's backup work-file count follows the
  same filter. Manual commits and "show in folder" still allow network paths but
  refuse the device namespace. The same 2026-10-16 review also flagged the
  external cleanup replay, the pin-board path gates, the unverified TrustMark
  models, and `scrub` racing a writer; those are fixed in their own commits.
- The pin board no longer fails silently when the environment offers no usable
  WebGPU adapter. The renderer's creation error used to reach only the transient
  status line, and every status is cleared after 4.5 seconds, so such a machine
  (a virtual machine without 3D acceleration, some remote-desktop sessions)
  ended up with a blank board, every editing control disabled, and a
  "画板已锁定" marker that pointed away from the real cause. The failure now
  renders a **persistent** notice in the stage area that names the original
  reason and states what the environment needs, and the lock marker is
  suppressed while the renderer is absent (the board's default state is locked,
  which is not what the user did). The pin board is the only module that needs
  WebGPU, so the notice also says the rest of the application is unaffected.
  Found during the clean Windows user-environment acceptance run and fixed the
  same day. The requirement itself — a graphics adapter supporting WebGPU and
  BC7 texture compression, which recent integrated graphics meet — is now
  stated in `README.md`'s runtime section and in the manual gate list of
  `docs/guides/release-policy.md`, neither of which mentioned it before. The
  fix was re-verified on 2026-10-17 in an environment without a usable adapter:
  the persistent notice appears with the original reason.
- The three new RustSec advisories against the `zip` crate
  (RUSTSEC-2026-0367/0368/0369, symlink-escape and permission-preservation flaws
  in extract-to-disk paths) are added to the dependency-audit ignore list in
  both workflows, unblocking Windows CI run #19 without any code change. The
  locked zip 8.6.0 is a transitive dependency of c2pa 0.90.11, which only reads
  archive-based assets and is never handed an untrusted ZIP file, so the
  vulnerable code is unreachable from this application. The proper fix —
  upgrading c2pa to 0.91.3, which uses zip 9.0.4 — was evaluated and deferred:
  0.91's validator rejects manifests signed by 0.90 or earlier (soft binding
  values stored as CBOR text strings now fail `claim.malformed`), which would
  fail closed the integrity verification of every previously published record.
  The decision and the rationale are recorded in the workflow files and
  `docs/planning/todo.md`.

### Changed

- The application version is incremented to `0.2.0`, the first stable-tagged
  release of the 0.2 line. It carries the same product code as `0.2.0-rc.4`
  (tagged `v0.2.0-rc.4` and pushed on 2026-10-17): no source change landed
  between the two cuts, so this release ships exactly the candidate that
  passed Windows CI, the clean-environment desktop acceptance and the full
  pre-release stress suite. The cut itself adds only the version bump, the
  release-infrastructure fixes and the documentation updates listed below.
  Cutting the version does not by itself tag or release anything; tagging
  `v0.2.0` stays a separate, later maintainer decision, so until then this
  cut is an untagged version.
- The release workflow now flags the draft GitHub release as a prerelease only
  when the version carries a prerelease suffix (`-alpha.N` / `-rc.N`); a stable
  tag such as `v0.2.0` produces a plain draft, so the flag no longer has to be
  flipped manually before publishing. The workflow is renamed from "Windows
  Release Candidate" to "Windows Release".
- `tools/release/verify-metadata.mjs` now also verifies the application version
  recorded in `src-tauri/Cargo.lock`, the one version field that was previously
  unchecked.
- The full pre-release stress suite was run on `0.2.0-rc.4`, whose product
  code is identical to this release's, as this release's stress evidence: the
  eleven groups with the large-file group across all five tiers up to
  `manual` (8 GiB work files). The measured numbers, disk budgets and
  wall-clock times are recorded in `docs/user/stress-test-report.md`.
- The desktop acceptance for this content is complete. The full manual-gate
  checklist passed on a clean Windows user environment on 2026-10-16 (the
  pin-board item re-verified on a machine with a usable WebGPU adapter), and
  the remaining task-control, idle-verification and security-batch UI items
  passed on 2026-10-17 against `0.2.0-rc.4`. Because this release and
  `0.2.0-rc.4` are the same build, that acceptance carries over unchanged;
  the records are archived in
  `docs/planning/archive/handoff-log-2026-10-16.md`.
- Standing limitations carried by this release: a repository written
  concurrently by the GUI and a headless process is not protected against
  races (headless versus headless is, through the cross-process write
  sentinel); files orphaned by a crash are reclaimed through scan and
  explicit confirmation rather than automatically; and power-loss durability
  is not automatically verified. See `docs/planning/todo.md` and
  `docs/user/stress-test-report.md`.
- The README now opens with a product screenshot (`.github/assets/demo.png`,
  the version-history view of a real artwork) and its sections are reordered
  and tightened: the feature list leads with the module names, the usage
  statement and its runtime requirements keep their own headings, and the
  version line follows the current lagging-version model. No behavior change.
- The per-branch quick-check switch now actually controls that branch's
  automatic backups, and the branch status line shows the check mode that
  takes effect. Previously the switch only forced quick checks on: turning it
  off meant "follow the global default" (quick by default), so a new artwork
  displayed the switch as off while its backups still ran quick checks. The
  scheduler now reads only the branch flag; the global
  `automaticBackupCheckMode` setting is purely the default written into new
  artworks and new branches at creation time (GUI and headless alike). Existing
  branches that never touched the switch stored `0` and therefore switch to
  full checks — matching what their switch now says; re-enable it per branch to
  get quick checks back. The switch's off-state hint now reads "每次全量校验"
  instead of "跟随全局设置". A scheduler regression test corrupts the head
  snapshot and asserts a quick-disabled branch repairs it on an unchanged
  file, the mirror of the existing quick-enabled test.
- The headless full-repository content check (`scrub`) now refuses to run
  while another process holds the write sentinel, with "仓库正在被写入，请稍后
  重试". Running it during a compaction used to surface "找不到历史节点" — clean
  and lossless, but an error unrelated to its cause; J5 measured it in one of
  four rounds. Only content-level checks are gated: `verify`, `list-*` and the
  other light read commands keep the promise that they answer while a writer
  holds the lock (J1/J2 assert exactly that and must not regress). The probe
  takes `BEGIN IMMEDIATE` with a zero busy timeout and rolls back at once, so
  the read command never becomes a mutual-exclusion source between writers; the
  window between the probe and a writer starting is accepted and documented,
  since the alternative — re-reading the node set and continuing — would verify
  a moving target and hide the real concurrency window. J6 pins the refusal
  deterministically (writer parked on a checkpoint, `scrub` refused with exit
  code 1, `verify` still answering, `scrub` passing once the writer finishes),
  and J5 now records refusals separately from other failures instead of treating
  them as a boundary (2 of 4 rounds refused and none failing otherwise on the
  re-run).
- The planning set now follows one rule instead of two behaviours: the two
  surface documents (`docs/planning/current-handoff.md` and
  `docs/planning/todo.md`) carry only the current state, open work, and plans,
  while everything finished — completed batches, passed manual acceptance,
  superseded plans — moves into `docs/planning/archive/` and is deleted from
  the surface. A pass used to be appended to the handoff beside the open items,
  which grew the file without bound and mixed history into what a reader needs
  for the current batch. The rule is stated in `AGENTS.md`, `docs/README.md`,
  `docs/architecture/ai-reading-guide.md`, `docs/guides/release-policy.md` and
  in the headers of both surface documents; the acceptance records and the batch
  rows they used to carry now live in the archive (the newest in
  `archive/handoff-log-2026-10-16.md`), with the `archive/README.md` index
  updated to match.
- `rustls` moved to 0.23.46 in `src-tauri/Cargo.lock`, which clears the four
  advisories RustSec published on 2026-10-14 against 0.23.45
  (RUSTSEC-2026-0346 client session `Debug` output leaking TLS secrets,
  RUSTSEC-2026-0347 application data sent to the outer server after ECH
  rejection, RUSTSEC-2026-0348 a brotli certificate that can allocate up to
  1 GiB before authentication, RUSTSEC-2026-0349 an unauthenticated ServerHello
  downgrading later connections to classical key exchange). None of the four was
  in the audit ignore list, so `windows-ci.yml` and `release.yml` both failed at
  "Audit Rust dependencies" — the first run after the advisories landed, not a
  regression from the commits before it. `rustls` comes in through `ureq`'s
  `rustls` feature and that requirement is `^0.23`, so nothing else moved;
  `licenses/THIRD_PARTY_LICENSES.html` was regenerated for the new version.
  One audit warning stays and cannot be fixed here: every `zip` 8.x release,
  including the locked `8.6.0`, is yanked on crates.io, and `c2pa` up to 0.91.2
  still requires `zip ^8.6.0`. The warning does not fail the audit check.
- The pre-release stress suite was re-run in full on `0.2.0-rc.3` — all eight
  groups, 31 cases, all passing — and its C group now covers all five tiers,
  including `manual` (8 GiB), which the report previously listed as designed
  but never measured. `docs/user/stress-test-report.md` now carries the numbers
  from that run instead of the older `0.2.0-alpha.4` ones. No shipped behavior
  changed. Running `manual` needs an explicit `LILITH_STRESS_DISK_BUDGET`,
  because its own peak-disk floor (3 x 8 GiB + 64 MiB) already exceeds the
  24 GiB default — leaving it alone records the whole C group as skipped rather
  than failing. `docs/guides/validation.md` now states that prerequisite and
  the measured cost of a full run (about 95 minutes on a 16-thread desktop).
- `windows-ci.yml` now also builds the headless binary and runs the cheap
  stress groups (`stress_cancel`, `stress_crash`, `stress_cleanup`,
  `stress_damage`, `stress_pin_board`, `stress_authenticity`,
  `stress_concurrency`, `stress_input` — about 30 seconds in total) plus
  `tests/gate_guard.rs`. They previously ran only before a release, so everyday
  changes had no automatic regression coverage. `stress_large`,
  `stress_scale`, and `stress_bench` stay manual (`docs/guides/validation.md`).
- Two documentation claims were corrected. The security policy no longer calls
  `0.2.0` "the first stable release" — `0.1.0` already is one — and instead
  says that fixes go to the newest stable release and the current development
  line, with the `0.2.0` release candidates superseded. Statements that
  `0.2.0` is the *same build* as `0.2.0-rc.4` now read "the same product code":
  only the version fields, the release workflow and the documentation differ
  between the two cuts.

## 0.2.0-rc.4 - 2026-10-17

### Added

- Headless commands that write a repository now take a **cross-process write
  sentinel**: an exclusive lock held by a dedicated SQLite database
  (`BEGIN IMMEDIATE`, `busy_timeout = 0`), so a second writer gets an explicit
  refusal instead of racing the first in the business layer. The rejection reads
  "仓库正在被另一个进程写入；不支持同一仓库的多进程写入". SQLite releases the lock
  when the handle closes, so a killed process cannot leave the repository
  locked — a plain lock file would, and this project has many kill scenarios.
  Only the headless entry is gated, only for the commands that write, and only
  when the repository directory already exists; read-only commands,
  `init-repository`, and the whole GUI path are unchanged (the GUI keeps relying
  on the single-instance plugin). Covered by the new `stress_concurrency` group.
- New tests close the gaps listed in `docs/planning/todo.md` under "待补强":
  a deterministic property test over the reverse-delta chain (any revision in a
  random chain restores byte-identically), a random commit/restore sequence on a
  real repository, disk-full injection on the real snapshot write path
  (`ErrorKind::StorageFull` returns a clean error and leaves the repository
  intact and retryable, with the partial bytes actually on disk), and
  multi-process contention plus 20 rapid sequential commits. The `chunk_file`
  snapshot writer was already generic over `Write + Seek`, so disk-full injects
  at the real data path rather than a mock; the commit path's snapshot write was
  extracted into `create_snapshot` for the same reason, and the shared fixtures
  live in the new `backup/test_support.rs`. Coverage and run commands are
  documented in `docs/guides/validation.md`.
- The remaining review gaps are closed without touching product code:
  a **large-file interruption** scenario (every commit cancel checkpoint plus
  one real kill on a tier-sized work file, asserting no residue, no history
  advance, and a byte-identical head restore, since the cancel and kill groups
  previously only used 96 KiB files); **main-database corruption** (the
  `.sqlite3` header overwritten with non-database bytes, and the file emptied —
  both must fail cleanly with an actionable error, and the emptied file must
  not be rebuilt into a fresh empty repository, which would silently discard
  history); **write + read interleaving** (read commands still answer, and see
  a consistent pre-write state, while a writer holds the sentinel lock,
  including the "snapshot published, transaction not yet committed" window;
  concurrent readers do not block each other); and a **performance baseline**
  for core algorithms (node chain, authenticity publish/decode, board DDS
  import/scan) recorded to `src-tauri/tests/perf-baseline.json`, with an opt-in
  strict comparison (`LILITH_STRESS_PERF_STRICT=1`, `LILITH_STRESS_PERF_FACTOR`,
  default 2.5x) so a slow regression is caught instead of only an
  order-of-magnitude blow-up.
- `tests/gate_guard.rs` guards the one line Cargo cannot: it scans every
  `stress_*.rs` and fails if `#![cfg(feature = "headless")]` is missing, so a
  new stress target can no longer silently rejoin a bare `cargo test` and pop
  a GUI window.
- `app/settings.rs` gains a unit test for the long-standing concurrency
  promise that read-only repository commands keep answering while a long write
  (restore/compaction) holds the exclusive operation lock. A regression now
  fails by timeout instead of hanging the board.
- The review list's remaining items are closed with tests only:
  - **Input-domain extremes** (`stress_input`, group K): non-ASCII, emoji, ZWJ
    sequences, non-BMP code points, RTL, combining marks, zero-width characters,
    embedded newlines/tabs and titles containing path separators must round-trip
    character for character (which also proves a title is never used as a file
    name); title length is counted in **characters** (160 emoji = 640 bytes is
    accepted, 161 is rejected) after trimming; only-whitespace titles are
    rejected; non-ASCII, spaced and deep source paths work end to end; relative,
    out-of-workspace and missing sources are all refused with a stated reason.
  - **Malformed-input fuzzing** for the four parsers — snapshot/delta
    containers, DDS headers, C2PA manifests and the settings file — driven by a
    deterministic generator in the new `src/fuzz_support.rs` (`cfg(test)` only,
    also the single implementation behind `backup::test_support::Rng`). No new
    dependency. The contract asserted is that untrusted bytes can never buy a
    panic, an out-of-bounds access or an unbounded allocation: an accepted
    container must be self-consistent (chunk count within the file's capacity,
    materialisable, length equal to the declared size), an accepted DDS size
    must sit inside the declared limits, and a settings file that fails to parse
    must be **left byte-identical** rather than overwritten with defaults.
  - **Randomised corruption** (`stress_damage` R5/R6): every managed file gets
    several seeded single-point rewrites, each of which must be detected by the
    content-level check with exit code 1 and no panic while the database-level
    check stays unaffected; board DDS gets six structural corruptions (magic,
    DX10 fourcc, BC7 format, zero width, oversized height, truncation) which
    must be reported as `corrupt`, plus 24 random rewrites after which the scan
    must still return a report.
  - **Multi-operation interleaving** (`stress_concurrency` J3–J5): reads answer
    while a *long* write holds the lock (repository backup and compaction,
    parked at every checkpoint), and a genuinely concurrent
    compaction × full-repository scrub run (four rounds, no window
    constructed) must not hang, must leave the repository complete and every
    remaining node byte-restorable. J5 surfaced a real boundary: in one of four
    rounds the scrub failed cleanly with "找不到历史节点" because `scrub` is a
    read command and is not covered by the write sentinel, so it is not
    mutually exclusive with `compact`. No data loss; the error just does not
    match its cause. Recorded as a P2 item in `docs/planning/todo.md` and in the
    report's boundary table.
  - **Performance baseline extended to six scenarios**: library tree assembly
    and two search queries, whole-repository backup (throughput as well as
    time), and cleanup scan / queue replay join the node chain, authenticity and
    board DDS baselines. Each of the new ones takes the minimum of three runs and
    asserts correctness on every repeat, so the number cannot drift away from a
    correct result.
- Eight of the cheap stress groups now run in Windows CI (the seven from before
  plus `stress_input`), which brings the input-domain and path-boundary
  regressions into everyday runs instead of pre-release ones.

### Fixed

- TrustMark models are now verified before they are loaded. `model_info` computed
  the encoder and decoder SHA-256 but never compared them with anything, and
  `model_files_ready` only checked that the files existed, so a replaced
  `*_Q.onnx` in the install directory was loaded silently — which undermines the
  authenticity guarantees the application itself advertises. Both digests are
  pinned as constants next to the loader (matching
  `src-tauri/resources/models/README.md`), the check runs once and is cached with
  the engine, and a missing or mismatched model makes `model_files_ready` false
  and makes the loader fail with an explicit message naming the encoder or
  decoder instead of loading it. `pinned_digests_match_the_shipped_models`
  compares the constants with the models in this repository, so a model swap that
  forgets the constants fails before the commit rather than after the release.
  The compile-time `CARGO_MANIFEST_DIR` fallback in `lib.rs` is gone: a
  developer-machine path does not belong in a release artifact, and the packaged
  resource directory is the only place the models are resolved from.
- Pin-board import and export now run the same two path gates the authenticity
  module runs. `import_pin_board_images` handed renderer-supplied absolute paths
  straight to `dds::read_limited` and the raster decoder with neither a
  filesystem-scope check nor a repository-boundary check, and
  `export_pin_board_images` only required the target directory to exist; the
  "外部源绝对路径必须已由文件选择器授权" rule in the module documentation was
  enforced by the UI alone and never reused in Rust. Both commands now call the
  shared scope gate (moved out of `authenticity/commands.rs` into
  `src-tauri/src/path_scope.rs`, so the two domain modules still do not import
  each other), and both domain functions refuse paths inside the repository,
  which also covers the headless entry. "Copy files in Explorer, paste into the
  board" keeps working: the clipboard-reading command adds the paths it read
  itself to the scope, so the import still passes the same gate. Pasting raw
  **text** paths no longer imports — such a path has no authorization source, so
  the Rust layer rejects it; paths now come from the picker or the clipboard
  files, while drag-and-drop still uses the byte channel. Covered by
  `pin_board::repository` and `path_scope` tests.
- External cleanup entries no longer replay themselves on startup. The
  `pending_file_cleanup` queue kept `external_file` rows (an absolute path
  outside the repository) out of the path validation on purpose, and the
  frontend called `retryFileCleanup([])` as soon as a repository became ready,
  where an empty id list meant "process everything". Opening a poisoned
  repository was therefore an automatic, unconfirmed "delete any absolute path"
  primitive, with the target's SHA-256 as the only prerequisite. An empty id
  list now means **repository entries only** (`repository_file` /
  `repository_directory`), so the startup replay and the settings page's
  "retry all" both leave external rows alone; those are deleted only from the
  settings page, one row at a time, through an explicit id. Each external row
  now shows the full path, the owning artwork (looked up through the export
  path's certification record, empty when the publish failed before the record
  existed) and the expected SHA-256, and the button opens a slide-to-confirm
  dialog before the delete is queued. The existing "keep the file when the
  digest does not match" guard is unchanged. A per-session directory allowlist
  was rejected as the alternative: confirmation already removes the automatic,
  unconfirmed primitive, an allowlist would only weaken the gate, and it would
  expire on restart, which reads as a bug. Covered by `cleanup::run` and `App`
  tests.
- Opening a repository no longer trusts the branch work-file paths recorded in
  its database. `validate_repository_semantics` checked every controlled
  relative path but not `branches.source_path`, whose value only had to pass
  the write side; a crafted or shared repository could therefore carry
  `\\host\share\x` or a device path, and the automatic backup's quick check
  would call `metadata()` on it (a silent SMB authentication towards that host)
  while a full backup would open and copy the file into the history. The read
  side now runs a **separate structural check** for `source_path` /
  `source_path_key` — absolute, no device namespace, outside the repository
  directory — deliberately **not** the controlled-relative-path check (the
  field stores an absolute path by design), **without** testing whether the
  file exists (work files get moved or deleted, and opening a repository must
  still succeed), and **without** rejecting network paths (a NAS is a legal
  setup, and refusing it would lock users out of their existing repositories).
  What gets refused is only what hand-written data can produce: the device
  namespace (`\\.\`, `\\?\`, `\Device\`) and paths inside the repository. The
  unattended use sites — scheduled-branch selection, the quick check, and the
  auto-triggered `run_backup` — skip network and device paths and record a clear
  reason on the branch (`mark_automatic_backup_skipped`, which does not count a
  failure, set a retry deadline, or disable automation, and skips the write when
  the reason is unchanged), reusing the existing "does not take part in
  automatic backup" rule; the settings page's backup work-file count follows the
  same filter. Manual commits and "show in folder" still allow network paths but
  refuse the device namespace. The same 2026-10-16 review also flagged the
  external cleanup replay, the pin-board path gates, the unverified TrustMark
  models, and `scrub` racing a writer; those are fixed in their own commits.
- The pin board no longer fails silently when the environment offers no usable
  WebGPU adapter. The renderer's creation error used to reach only the transient
  status line, and every status is cleared after 4.5 seconds, so such a machine
  (a virtual machine without 3D acceleration, some remote-desktop sessions)
  ended up with a blank board, every editing control disabled, and a
  "画板已锁定" marker that pointed away from the real cause. The failure now
  renders a **persistent** notice in the stage area that names the original
  reason and states what the environment needs, and the lock marker is
  suppressed while the renderer is absent (the board's default state is locked,
  which is not what the user did). The pin board is the only module that needs
  WebGPU, so the notice also says the rest of the application is unaffected.
  Found during the clean Windows user-environment acceptance run and fixed the
  same day. The requirement itself — a graphics adapter supporting WebGPU and
  BC7 texture compression, which recent integrated graphics meet — is now
  stated in `README.md`'s runtime section and in the manual gate list of
  `docs/guides/release-policy.md`, neither of which mentioned it before. The
  fix was re-verified on 2026-10-17 in an environment without a usable adapter:
  the persistent notice appears with the original reason.
- The three new RustSec advisories against the `zip` crate
  (RUSTSEC-2026-0367/0368/0369, symlink-escape and permission-preservation flaws
  in extract-to-disk paths) are added to the dependency-audit ignore list in
  both workflows, unblocking Windows CI run #19 without any code change. The
  locked zip 8.6.0 is a transitive dependency of c2pa 0.90.11, which only reads
  archive-based assets and is never handed an untrusted ZIP file, so the
  vulnerable code is unreachable from this application. The proper fix —
  upgrading c2pa to 0.91.3, which uses zip 9.0.4 — was evaluated and deferred:
  0.91's validator rejects manifests signed by 0.90 or earlier (soft binding
  values stored as CBOR text strings now fail `claim.malformed`), which would
  fail closed the integrity verification of every previously published record.
  The decision and the rationale are recorded in the workflow files and
  `docs/planning/todo.md`.

### Changed

- The application version is incremented to `0.2.0-rc.4`. The bump changes only
  the version string and does not by itself tag or release anything; tagging
  `v0.2.0-rc.4` and publishing it stays a separate, later maintainer decision,
  so this cut is a transitional, untagged version.
- The README now opens with a product screenshot (`.github/assets/demo.png`,
  the version-history view of a real artwork) and its sections are reordered
  and tightened: the feature list leads with the module names, the usage
  statement and its runtime requirements keep their own headings, and the
  version line follows the current lagging-version model. No behavior change.
- The per-branch quick-check switch now actually controls that branch's
  automatic backups, and the branch status line shows the check mode that
  takes effect. Previously the switch only forced quick checks on: turning it
  off meant "follow the global default" (quick by default), so a new artwork
  displayed the switch as off while its backups still ran quick checks. The
  scheduler now reads only the branch flag; the global
  `automaticBackupCheckMode` setting is purely the default written into new
  artworks and new branches at creation time (GUI and headless alike). Existing
  branches that never touched the switch stored `0` and therefore switch to
  full checks — matching what their switch now says; re-enable it per branch to
  get quick checks back. The switch's off-state hint now reads "每次全量校验"
  instead of "跟随全局设置". A scheduler regression test corrupts the head
  snapshot and asserts a quick-disabled branch repairs it on an unchanged
  file, the mirror of the existing quick-enabled test.
- The headless full-repository content check (`scrub`) now refuses to run
  while another process holds the write sentinel, with "仓库正在被写入，请稍后
  重试". Running it during a compaction used to surface "找不到历史节点" — clean
  and lossless, but an error unrelated to its cause; J5 measured it in one of
  four rounds. Only content-level checks are gated: `verify`, `list-*` and the
  other light read commands keep the promise that they answer while a writer
  holds the lock (J1/J2 assert exactly that and must not regress). The probe
  takes `BEGIN IMMEDIATE` with a zero busy timeout and rolls back at once, so
  the read command never becomes a mutual-exclusion source between writers; the
  window between the probe and a writer starting is accepted and documented,
  since the alternative — re-reading the node set and continuing — would verify
  a moving target and hide the real concurrency window. J6 pins the refusal
  deterministically (writer parked on a checkpoint, `scrub` refused with exit
  code 1, `verify` still answering, `scrub` passing once the writer finishes),
  and J5 now records refusals separately from other failures instead of treating
  them as a boundary (2 of 4 rounds refused and none failing otherwise on the
  re-run).
- The planning set now follows one rule instead of two behaviours: the two
  surface documents (`docs/planning/current-handoff.md` and
  `docs/planning/todo.md`) carry only the current state, open work, and plans,
  while everything finished — completed batches, passed manual acceptance,
  superseded plans — moves into `docs/planning/archive/` and is deleted from
  the surface. A pass used to be appended to the handoff beside the open items,
  which grew the file without bound and mixed history into what a reader needs
  for the current batch. The rule is stated in `AGENTS.md`, `docs/README.md`,
  `docs/architecture/ai-reading-guide.md`, `docs/guides/release-policy.md` and
  in the headers of both surface documents; the acceptance records and the batch
  rows they used to carry now live in the archive (the newest in
  `archive/handoff-log-2026-10-16.md`), with the `archive/README.md` index
  updated to match.
- `rustls` moved to 0.23.46 in `src-tauri/Cargo.lock`, which clears the four
  advisories RustSec published on 2026-10-14 against 0.23.45
  (RUSTSEC-2026-0346 client session `Debug` output leaking TLS secrets,
  RUSTSEC-2026-0347 application data sent to the outer server after ECH
  rejection, RUSTSEC-2026-0348 a brotli certificate that can allocate up to
  1 GiB before authentication, RUSTSEC-2026-0349 an unauthenticated ServerHello
  downgrading later connections to classical key exchange). None of the four was
  in the audit ignore list, so `windows-ci.yml` and `release.yml` both failed at
  "Audit Rust dependencies" — the first run after the advisories landed, not a
  regression from the commits before it. `rustls` comes in through `ureq`'s
  `rustls` feature and that requirement is `^0.23`, so nothing else moved;
  `licenses/THIRD_PARTY_LICENSES.html` was regenerated for the new version.
  One audit warning stays and cannot be fixed here: every `zip` 8.x release,
  including the locked `8.6.0`, is yanked on crates.io, and `c2pa` up to 0.91.2
  still requires `zip ^8.6.0`. The warning does not fail the audit check.
- The pre-release stress suite was re-run in full on `0.2.0-rc.3` — all eight
  groups, 31 cases, all passing — and its C group now covers all five tiers,
  including `manual` (8 GiB), which the report previously listed as designed
  but never measured. `docs/user/stress-test-report.md` now carries the numbers
  from that run instead of the older `0.2.0-alpha.4` ones. No shipped behavior
  changed. Running `manual` needs an explicit `LILITH_STRESS_DISK_BUDGET`,
  because its own peak-disk floor (3 x 8 GiB + 64 MiB) already exceeds the
  24 GiB default — leaving it alone records the whole C group as skipped rather
  than failing. `docs/guides/validation.md` now states that prerequisite and
  the measured cost of a full run (about 95 minutes on a 16-thread desktop).
- `windows-ci.yml` now also builds the headless binary and runs the cheap
  stress groups (`stress_cancel`, `stress_crash`, `stress_cleanup`,
  `stress_damage`, `stress_pin_board`, `stress_authenticity`,
  `stress_concurrency`, `stress_input` — about 30 seconds in total) plus
  `tests/gate_guard.rs`. They previously ran only before a release, so everyday
  changes had no automatic regression coverage. `stress_large`,
  `stress_scale`, and `stress_bench` stay manual (`docs/guides/validation.md`).

## 0.2.0-rc.3 - 2026-10-16

### Added

- Shift now works as an additive-selection modifier in the pin board, matching
  Ctrl/Cmd. Holding it while clicking an image toggles that image, and holding
  it while dragging on empty canvas starts a marquee that adds to the current
  selection instead of replacing it, so several marquees can be chained:
  already-selected images stay selected and newly framed images are added. A
  plain drag still replaces the selection. The modifier predicate and the
  marquee hit test live in a new `pin-board/selection.ts` helper.
- The pin-board settings page now includes an operation manual. It documents
  viewing, selection, editing, and board/file operations as grouped rows with
  key chips, placed next to the lock and fullscreen shortcut fields.
- Pin-board images that are visible but whose texture is not loaded yet now
  show a placeholder over their footprint: a neutral grey box tracing the
  image's on-screen quad with a spinning accent arc in the centre, so an
  opening board no longer shows blank gaps while textures stream in. The
  placeholder follows pan, zoom and rotation, keeps the icon upright by
  cancelling the image angle, and shrinks the icon with the box (dropped
  entirely once an edge falls below 18 px). An image whose load failed switches
  to a red-toned box with a static `!` badge instead of spinning forever.
- Confirmations for the most dangerous actions now use a slide control instead
  of a button. `src/shared/SlideToConfirm.tsx` fires only after its thumb is
  dragged to the end of the track (or stepped there with the right arrow key);
  a click or a half-drag springs back, so a second click in the same place can
  no longer confirm by accident. Dialogs opt in with
  `ConfirmView.slideToConfirm`; irreversible deletions now all end in it:
  emptying the library trash, emptying the pin-board trash, deleting a branch,
  deleting a node with its later history, deleting history nodes that carry
  manual intent (its second confirmation), and cancelling a publication. The
  two hand-written dialogs in the authenticity module were folded back into the
  shared `ConfirmDialog`, so the module no longer repeats the dialog markup;
  deleting a certification record keeps a plain click confirmation. The stronger
  red styling for the highest-risk tier now lives with the shared dialog, so it
  applies to every module that asks for it instead of only inside the history
  workspace.

### Changed

- The application version is incremented to `0.2.0-rc.3`. The bump changes only
  the version string and does not by itself tag or release anything; tagging
  `v0.2.0-rc.3` and publishing it stays a separate, later maintainer decision,
  so this cut is a transitional, untagged version.
- Marquee moves in the pin board that leave the selected set unchanged now skip
  the canvas redraw and state broadcast. Dragging the marquee across empty space
  or over already-selected images no longer re-renders and re-sorts the whole
  board on every pointer event; the marquee rectangle itself is a separate
  overlay and still follows the pointer.
- Removing intermediate history nodes is now labelled as what it does. The
  compact toolbar button reads "删除节点" and uses the shared outlined danger
  button instead of the accent-coloured "执行精简", and the confirmation dialog
  matches. When the selection contains manual commits or nodes whose title was
  changed by hand, a second, stronger confirmation follows; plain automatic
  backups still need only one. Whether a title was changed by hand is derived by
  comparing it with the title generated at commit time, so no schema or DTO
  field was added.

### Fixed

- The confirmation dialog no longer offers a cancel button that cancels
  nothing. While the operation it started is still running, the footer button
  now reads "取消操作" and aborts that operation, and the dialog can no longer
  be dismissed through the backdrop or the header close button, so the one
  remaining exit is the one that works; the previously working cancel lived on
  the header progress bar, which the modal backdrop put out of reach. Callers
  opt in with the new `ConfirmDialog.onCancelOperation`, and only operations
  with real interruption points pass it. The two authenticity dialogs whose
  commands have no cancellation checkpoints (cancelling a publication, deleting
  a certification record) now close as soon as they are confirmed instead of
  sitting in a running state behind a cancel button that only closes the
  window.
- Removing history nodes now really stops when the user cancels it. Compaction
  calls the per-node command in a loop, while the shared cancel flag is cleared
  whenever a command takes the run lock and again when it returns, so a cancel
  could only ever affect the one command that was executing: a request landing
  after its last checkpoint, or in the gap between two commands, was silently
  dropped and the remaining nodes were deleted anyway. The controller now
  records the request itself, the loop checks it before starting the next node,
  and a node command that fails while a cancel is pending is folded into the
  same batch result, so the outcome is always reported as "精简已取消，剩余 N 个
  节点未被删除。" with the compact selection kept; the bare native "操作已取消"
  no longer reaches the user as the whole story. A request that arrives after
  the operation has already finished is reported as such instead of being
  swallowed. The shared cancellation error is neutral now ("操作已取消"): it
  read "恢复操作已取消" for compaction, checkpoints and subtree deletion, which
  all reuse that check.
- Minimizing the window before quitting no longer wipes the settings file. The
  exit capture read `GetWindowRect` while the window was minimized, which
  returns the taskbar-icon rect (about 237x39 at -32000,-32000); that size was
  stored, failed the window-bounds check on the next launch, and made
  `load_settings` fall back to the whole default document, silently dropping the
  repository path and every other preference. Capture now skips minimized and
  maximized windows and rejects implausible rects, and loading repairs an
  out-of-range field in place (window sizes are clamped to the allowed range)
  instead of discarding the rest of the file.
- The main window is centered when the saved position no longer overlaps any
  connected monitor, instead of being restored off-screen. A monitor that was
  present when the position was saved (a drawing tablet, a projector) may be gone
  by the next launch, and the window used to open outside the desktop, looking
  like the application had not started.
- Creating a pin board no longer leaves a stray empty directory in the
  process working directory. The board directory was built from an empty
  root, so the relative path was resolved by `create_dir_all` against the
  working directory: the install directory for an installed build, or
  `src-tauri/` when running the test suite, which accumulated hundreds
  of empty `artworks/<artwork-id>/boards/<board-id>` trees. The directory
  is now created on demand by `persist_dds_file` when the first DDS is
  written, which is the only place that needs it.
- The framing loupe now samples a fixed ratio of the source instead of a fixed
  72 source pixels. The lens keeps a fixed 3x magnification over the main view,
  so the sampling span is `lens size / (3 x main-view scale)`: a larger canvas is
  covered by a proportionally larger span and is no longer magnified far past
  what the main view shows. The tile that covers the window grows with the span
  (at least 512 source pixels, 1.25x the span when larger) while its output edge
  is sized to what the lens actually shows (the cell's on-screen size times the
  device pixel ratio, never above the source resolution or 1024 px), so a large
  canvas transfers display-resolution pixels instead of source pixels that are
  never drawn; tiles below source resolution are drawn smoothly instead of
  pixelated, and the tile cache key now includes the output edge because it
  depends on the device pixel ratio.
- `cargo test` no longer launches the application. The eight stress-test targets
  in `src-tauri/tests/` are now gated with `#![cfg(feature = "headless")]`,
  which is what the policy that they are a publish-time-only suite always
  implied: without the feature they compile to empty targets, so a plain
  `cargo test` — listed as an allowed daily command — runs the library unit
  tests and nothing else. Every file under `tests/` is discovered and run
  automatically, so the suite was in fact part of bare `cargo test` all along;
  without the feature `--headless` is stripped from the binary, every case
  started the real application against the `devUrl` that only exists while the
  dev server runs, and the single-instance plugin kept re-showing the window
  until the run was interrupted. The check that the target really is headless
  now also covers the default debug binary instead of only a `LILITH_STRESS_BIN`
  override, so a mismatched build fails immediately with a clear message rather
  than by opening windows. The two `unused_imports` warnings that appeared only
  in `cargo test` are gone as well: the library re-exports used solely by the
  headless entry point are gated on the feature alone, not `any(test, feature =
  "headless")`.

## 0.2.0-rc.2 - 2026-10-09

### Added

- A reusable logging and diagnostics layer. The log plugin no longer fixes its
  level at build time: `tauri-plugin-log` filters through fern's static
  dispatch level, so a level chosen at build time could not be raised later.
  The level is now owned solely by `app::diagnostics::apply_level`
  (`log::set_max_level`), which makes diagnostics mode switchable at runtime
  without a restart.   The baseline comes from `LILITH_LOG_LEVEL`, defaulting to
  `debug` in debug builds and `info` in release builds; the effective level is
  the baseline, raised to at least `debug` while diagnostics mode is on, so the
  variable can lower the level as well as raise it. Diagnostics mode
  is a process-scoped flag that is never written to the settings file; it
  defaults to the baseline, and a new "详细日志" switch on a dedicated "调试"
  settings page toggles it live through `set_diagnostics_enabled`, which
  broadcasts `diagnostics_mode_changed`. Because the WebView has no path into
  the log file, a new `log_frontend_diagnostics` command forwards frontend
  events into the same log. The remaining frontend records are all
  event-driven and low frequency (lifecycle, save, finalize, switch, GPU device
  health), so `info`, `warn`, and `error` are all forwarded unconditionally;
  threshold warnings (slow steps and long tasks) are always on as well because
  they stay silent until a threshold is crossed. In diagnostics mode the
  frontend keeps a main-thread heartbeat that refreshes the forensics channel
  and logs only when it runs late, plus a 500 ms event-loop lag probe. A native
  watchdog thread pings the webview every two seconds and expects an immediate
  pong, so a frozen main thread is reported as `[watchdog] webview
  unresponsive` from the Rust side, which is the only place that can still
  write during a freeze and
  survives the 15-second force-exit fallback. A separate forensics worker on
  its own thread records the last heartbeat, last operation, and silence
  duration into IndexedDB, so the evidence survives a frozen main thread and is
  replayed as `[forensic] previous freeze` on the next launch. Global
  `error` / `unhandledrejection` capture and WebGPU `device.lost` /
  `onuncapturederror` reporting make frontend and GPU faults visible. Renderer
  create and destroy, GPU device sequence and live-renderer count, board save
  and finalize, texture drain batches, the shutdown handshake, and
  repository-switch settlement are logged as well, while repository lock
  wait/hold, slow reads, slow texture batches, and long tasks are reported only
  past a threshold so the normal tier stays quiet. Every line is indexed by a
  per-run id, level, and target, and probe lines carry a stable bracket tag
  (`[hb]`, `[lag]`, `[task]`, `[slow]`, `[forensic]`, `[watchdog]`, `[gpu]`,
  `[error]`) so signal can be filtered out of the noise.
  `get_diagnostics_status` reports the mode, level, and log directory, and the
  tier policy, tags, and the rule for adding new logs are documented in
  `docs/guides/logging.md`.
- Drag-and-drop image import for the pin board. The canvas area had no drop
  handling while the window cancelled the webview's file-drop defaults, so
  dropping an image onto a board did nothing. The stage now accepts dropped
  image files, reads their bytes, and imports them through the existing
  `import_pin_board_clipboard_image` command. Multiple files are placed to the
  right of the previous one, the placement follows the drop point, and a small
  pill plus a gradient frame mark the drop target without covering the canvas.
  Because the window runs with `dragDropEnabled: false` (required for HTML5
  drag and drop on Windows), the webview exposes no filesystem path, so only
  byte-sniffable bitmaps (PNG, JPEG, WebP, BMP, GIF) are accepted; DDS and TGA
  still import through the file picker.
- Select-all for the pin board. `Ctrl`/`Cmd`+`A` now selects every non-deleted
  image on the board instead of only the viewport, through a new
  `renderer.selectAll()`. Selection is view state, so it neither reorders images
  nor marks the board dirty, and it is ignored while the board is locked.
- A seamless framing loupe for TrustMark region selection. Hovering the image in
  the publish or identify editor shows a source-resolution magnifier next to the
  pointer, with the lens tracking the pointer, flipping at the stage edges, and a
  crosshair plus a pixel box marking the exact location. The lens is drawn on a
  canvas, so moving the pointer only repaints and never issues a request: pixels
  come from the source-resolution tile covering the current window and fall back
  to the already-loaded full-image preview, so the lens is painted immediately and
  never shows a loading state. Tiles are 512 source pixels on a grid whose step is
  one sampling window smaller than the tile (so the window is always fully
  covered): after a 120 ms pause the resting cell is fetched, after 400 ms more
  its four neighbours are prefetched, and up to 12 cells are kept in an LRU cache
  that is cleared when the source changes. Both editors reuse
  `preview_authenticity_tile`, which now also accepts an `external` source: the
  publish editor samples the branch final artifact by branch id, while the
  identify editor samples the pending image by absolute path. That path goes
  through the same `ensure_dialog_authorized` check as `decode_authenticity` and
  is additionally constrained to stay outside the repository and inside the
  source rectangle. The overlay is pointer-transparent, so hit testing and
  normalized coordinates are unchanged.

### Fixed

- Jumping to a different artwork from an identification result no longer
  misreports "此 Artwork 尚无分支". The branch list used to be filled only by
  the history page, so a navigation that lands directly on the publish view
  never populated it and an empty list was read as "no branches". The workspace
  now also loads an artwork summary (title plus branch list) whenever the active
  view is not the history page and the branches are not loaded yet, through a
  new `useArtworkSummary` hook that consumes the same `history/api.ts` as the
  history controller. The publish page shows "读取分支历史…" while loading,
  disables the branch selector, and offers an inline retry on failure; the "no
  branches" message now appears only after loading finished with a genuinely
  empty list. The branch id carried by the navigation is preserved once the
  summary arrives, and falls back to the first branch only when it no longer
  exists.

- Pin-board board and artwork switching no longer freezes the app. Every
  switch used to tear down the renderer and its WebGPU device and create a new
  one, and that create/destroy churn was confirmed to hang the WebView2 render
  process (frozen UI, tray-only force quit after a 15 s timeout). The renderer's
  GPU setup (`create`) is now separated from board loading (`loadBoard`): one
  device is created per module lifetime and reused, so switching a board or an
  artwork only swaps data. `loadBoard` settles the previous board (save plus
  `finalize_pin_board`, aborting the switch on failure) and releases its
  textures before loading the next board, and is idempotent per `boardId`. The
  canvas stays mounted instead of unmounting on a null view, the creation effect
  is idempotent so StrictMode no longer doubles the device count, and the
  workspace `key` moved from `ArtworkWorkspace` to the history/publish/identify
  panes so the pin-board pane survives artwork switches. A stress session with
  8 artwork switches and 27 board switches now reports exactly one GPU device
  created and zero renderer destroys, with no unresponsive-webview warning and
  an instant shutdown handshake.

- Shift rotation now snaps the absolute angle instead of the drag increment.
  Snapping the increment meant an image already tilted by 7° could only ever
  land on `7° + n×15°` and could never return to 0°. The renderer now derives
  each image's target absolute angle from its pre-drag points, snaps it to the
  nearest 15° multiple, and converts that back into an increment
  (`geometry.snapRotationDelta`), so a tilted image rotates exactly back to 0°.

- Texture reads no longer touch a released WebGPU device. Switching boards
  destroyed the renderer and its device while a texture IPC read could still be
  in flight; its result then called `createTexture`/`writeTexture` on the
  destroyed device, a dangling call on the WebView2 GPU path. The device now
  carries a `released` flag and the read result is discarded once the device is
  released, only the canvas's current GPU owner may unconfigure the context,
  and the destroy log records the in-flight texture count.

### Changed

- Pin-board device loss now recovers automatically within a bounded budget.
  Reusing one device for the whole session means a lost device no longer heals
  itself on the next switch, so `device.lost` now rebuilds the device through
  the same `makeGpu` path used at creation, re-arms the new device's `lost`
  handler, and reloads the current board's textures. Recovery is throttled to at
  most two rebuilds per renderer lifetime with a 600 ms × attempt backoff, and
  drawing and texture loading are suspended while a rebuild is in flight; beyond
  the limit or on rebuild failure the renderer stays suspended and only reports
  the error, so repeated losses cannot turn into a device rebuild storm. This
  replaces the previous report-only behavior; reusing the device also releases
  the open board's textures on every switch instead of keeping them resident.

- Changelog and versioning now follow a lagging model, and user-facing documents
  are separated from developer documents. The five version fields keep the last
  cut version until the maintainer cuts the next one; every change since the cut
  accumulates under a single `## Unreleased` section, which a version cut renames
  to a dated heading; tagging (the actual release) stays a separate, later
  decision, so an untagged version is transitional. `docs/guides/release-policy.md`
  states the model and `AGENTS.md` enforces it. The `0.2.0-rc.1` section is frozen:
  the seven entries committed after the `v0.2.0-rc.1` tag moved to `Unreleased`.
  Stale version references are corrected (`README.md` names `v0.2.0-rc.1` as the
  latest tag, `SECURITY.md` lists `0.2.x` as the supported line,
  `docs/modules/pin-board.md` notes the pin-board tables arrived at repository
  schema v2). `README.md`, `SECURITY.md`, `THIRD_PARTY_NOTICES.md`, and
  `docs/user/stress-test-report.md` drop self-description, marketing phrasing,
  and imperative requirements.

## 0.2.0-rc.1 - 2026-10-08

### Added

- High-resolution region loading for the publication quality preview. A new
  `preview_authenticity_tile` command crops a rectangle out of the source at
  full resolution and returns it bounded by a 64–4096 px edge, reusing the
  shared image resource budget and validating the crop rectangle and decoded
  image size. Its `source` field selects the cached unsigned preview JPEG
  (validated against the cache token and metadata) or the branch final
  artifact, and a single-entry decoded-source cache keeps panning cheap.
  Tiles are encoded as lossless PNG: the preview exists to reveal JPEG and
  TrustMark losses, so writing the crop back as lossy JPEG would add
  compression artifacts that the published output does not have. The preview
  dialog requests debounced viewport crops only while the thumbnail is
  enlarged, sizes the requested output edge to the tile's on-screen footprint
  so a source pixel lands on exactly one screen pixel, and snaps each crop
  rect to a "visible span × margin" grid so small pans reuse the tile they
  already have. Tiles overlay the base image at their source-rect position
  through an inline style and are cached client-side per source, rect, and
  resolution. Only a tile whose key matches the current rect is rendered, so
  panning or zooming never leaves a misaligned tile behind. The 2400 px
  thumbnail no longer caps how sharp the enlarged export preview and
  "显示原图" comparison can get.
- Numeric-only zoom for the publication quality preview, measured in source
  pixels. The dialog no longer has a separate "fit" mode: zoom is always a
  number and 100% means one source pixel per on-screen CSS pixel, so canvas
  dragging works from the moment the preview opens and the zoom readout
  matches what an image viewer would show. "适应窗口" resets the numeric zoom
  to the current fit ratio, and the wheel handler is attached as a
  non-passive native listener so zooming no longer scrolls the outer
  container.
- Drag-and-drop import for the identify page. Dropping an image file onto
  the left preview panel reads its bytes with `FileReader` and hands them to
  a new `stage_authenticity_input` command, which writes the file into a
  session directory under the system temp dir, adds it to the filesystem
  scope, and returns the path for the existing external-image preview and
  identify flow. Only PNG, JPEG, WebP, and TIFF are accepted, failures
  surface through the shared operation notice, and a drop overlay is shown
  while dragging. The app also cancels window-level `dragover`/`drop`
  defaults so dropping a file never navigates the webview.
- Per-record deletion for export certification records. A new
  `delete_certification_record` command removes a single
  `certification_records` row inside a transaction that first enqueues a
  hash-checked repository-file cleanup intent for the record's stored JPG
  copy, then runs the cleanup and reports leftovers through the existing
  retry banner. The command resolves the owning branch for run-lock scoping
  and executes under the exclusive backup run lock. The read-only record
  view gains a "more actions" menu next to "退出查看" with a red
  "删除本记录" entry that opens an in-app confirmation dialog modeled on
  the publication deletion dialog; the first-exported JPG always stays at
  its original path. Deleting a record exits the view and refreshes the
  branch record list.

### Fixed

- Pin-board rename no longer breaks the open board's saves. Renaming a
  board used to advance its stored `revision` while the open renderer kept
  the revision it had loaded, so every later save failed the revision
  check: manual saves and autosaves never recovered, paste and import were
  gated by the same failing save, switching boards blocked on the failing
  finalize, and closing the app or leaving the artwork silently dropped
  unsaved edits. Only an app restart reloaded a fresh revision. Rename now
  updates `name` and `updated_ms` without touching `revision`, matching
  the reorder precedent that list metadata is not board content; the
  rename repository test asserts the revision stays unchanged.

- Switching the repository now settles the open pin board before releasing the
  previous repository. Leaving the workspace only called the renderer's
  fire-and-forget `destroy()`, so the last edit could land on an already
  released repository and be lost. The settings save now awaits
  `preparePinBoardRuntimeChange()` before saving a changed repository path; a
  failed settlement aborts the switch, keeps the old repository, and surfaces
  the failure. The shutdown handshake path is unchanged, and switching artwork
  within one repository still leaves the repository open.

### Changed

- The application version is incremented to `0.2.0-rc.1`. The bump changes only
  the version string and does not by itself tag or release anything. Tagging and
  publishing `v0.2.0-rc.1` is a separate, later maintainer step;
  `docs/guides/release-policy.md` states that an untagged `rc.N` carries no
  gate, while a tagged `vX.Y.Z-rc.N` is a public candidate that must pass every
  automatic and manual gate.

- The documentation set is split by audience. User-facing documents move to
  `docs/user/` (`stress-test-report.md`); `architecture/`, `modules/`,
  `guides/`, and `planning/` stay developer- and agent-facing. The 2026-09-12
  to 2026-10-08 handoff log is archived under `docs/planning/archive/`, the
  current handoff and todo list are condensed, and `README.md` and
  `docs/README.md` now state the audience split.

- A `.gitattributes` excludes the generated third-party license bundle
  (`licenses/THIRD_PARTY_LICENSES.html`) from GitHub language statistics and
  marks the bundled ONNX models as binary. Without it the ~4.6 MB generated
  HTML dominated the repository's reported language share.

- The `vitest` devDependency moves from 3.x to 4.1.11 to clear the advisories
  that had started failing the Windows CI dependency-audit job: tinypool
  prototype pollution leading to RCE (GHSA-5gmw-xhrv-c9v3,
  GHSA-85c8-ppgw-ccpr, critical), source-map-js event-loop denial of service
  (GHSA-68fv-2mgg-jv7q, high), and the @vitest/mocker redirect-mock path
  traversal (GHSA-82fw-gwwq-j7x9, moderate). vitest 4.1.11 ships a patched
  `@vitest/mocker` and no longer depends on tinypool, and `postcss` now
  resolves `source-map-js` to 1.2.2. This is test tooling only and does not
  change the shipped bundle; the repository's third-party license closure is
  unchanged because it covers runtime dependencies only. Risk is limited to
  the test runner, and the full suite, type check, production build, and
  release-metadata check pass unchanged.

- The log level policy now follows the build profile and is switchable at
  runtime. Debug builds used to log at `info` unless `LILITH_LOG_LEVEL` was
  set, so newly added `debug!` instrumentation silently disappeared during
  development; debug builds now default to `debug` and release builds to
  `info`, and the settings switch can raise either to `debug` without a
  restart. A `debug`/`trace` level is not compiled out of release builds
  either, because no dependency enables `log`'s `release_max_level_*`
  features. `docs/architecture/overview.md` no longer claims a 1 MiB cap with a
  single retained file and documents the level policy, diagnostics mode, and
  frontend bridge.

## 0.2.0-alpha.4 - 2026-10-04

### Added

- Unreferenced-file scanning for the repository. A new scan command walks
  `artworks/*/snapshots`, `artworks/*/deltas`, and `artworks/*/boards/*` and
  reports the files that match the known snapshot/delta/pin-board-DDS naming
  patterns, are older than a 30-minute grace period, and are not referenced by
  any database column (including `pin_board_images`). It only reports, never
  deletes. Crash orphans published before `history::commit` (which were never
  enqueued) and pin-board DDS files left without a record can finally be
  discovered this way. A companion `cleanup_repository_unreferenced` command
  re-registers each confirmed candidate with its current SHA-256, enqueues it in
  `pending_file_cleanup`, and replays once, so a repeated confirmation is
  idempotent and a candidate that becomes referenced again stays queued for
  retry. Both commands run under the shared run lock and the repository
  operation lock and are cancellable; neither is exposed as a headless
  subcommand. The settings page exposes both the scan and the confirmed
  cleanup.
- The repository settings page gains a "文件清理" section that surfaces the
  cleanup ledger and its discovery mechanism. It lists every pending entry
  (path, reason, last error, last attempt time) with per-entry and full-queue
  retries through `retry_pending_file_cleanup`, runs the unreferenced-file scan
  under the shared cancellable run state, and shows the reported candidates
  with their size and reason before purging them behind an in-app
  confirmation. The list is read-only through a new `list_pending_file_cleanup`
  command that takes the shared repository read lease instead of the run lock,
  so the queue still answers while a backup, restore, or compaction runs and
  the listing never writes `last_attempt_ms` or `last_error`.
- Repository integrity checking now covers pin-board DDS files in both
  directions. A third segment of `scrub_repository_integrity` walks every
  `pin_board_images` record and checks that its DDS exists under its board
  directory, is owned by the right path, carries a valid DDS/DX10/BC7 header
  whose declared dimensions match the record, has the declared payload length,
  and decodes as BC7; it also counts DDS files that no record references. The
  counts are returned in the scrub report
  (`pinBoardImages` / `pinBoardMissingDds` / `pinBoardCorruptDds` /
  `pinBoardOrphanDds`) and shown in the settings page, which now warns when a
  missing, corrupt, or orphan DDS is found. The scan only reports and never
  repairs; a missing DDS is a history-migration concern. There is no SHA-256
  comparison because `pin_board_images` stores no digest and the schema is not
  migrated in this batch.
- A pre-release stress-test suite and the headless command-line entry point it
  uses. A new `--headless <command>` mode (gated behind the `headless` Cargo
  feature, absent from release builds) runs the real executable as a separate
  process with no window, tray, webview, or IPC; it is a 1:1 adapter over the
  existing domain functions and adds no business logic. The suite under
  `src-tauri/tests/` drives that binary across a process boundary and asserts
  only on returned JSON and on-disk facts. It covers cancel boundaries,
  cross-process kills (including inside the SQLite commit transaction), 4 GiB
  file round trips, scale and disaster recovery, parameter boundaries, crash
  orphan reclamation, pin-board DDS integrity, authenticity publish memory and
  read-back, and damaged snapshot/delta detection and repair. It runs only
  before a release, is not part of CI, and changes no shipped command's
  behavior. See `docs/guides/validation.md` and
  `docs/user/stress-test-report.md`.

### Fixed

- Finalizing a pin board no longer deletes the DDS files of deleted images
  before the SQLite commit. The deletion is now recorded in the
  `pending_file_cleanup` queue inside the transaction (reason
  `pin_board_finalize`) and replayed once after the commit succeeds, so a
  failed commit can no longer leave a database record pointing at a DDS that
  is already gone. A deletion failure keeps the entry in the queue for a later
  single-pass retry (via the next finalize, trash operation, or the existing
  retry command) instead of failing the finalize; retries never loop and never
  block the board.
- History cleanup now goes through the same `pending_file_cleanup` ledger.
  Deleting a branch, deleting a history subtree, undoing a checkpoint,
  committing (which releases the parent snapshot), repairing a head snapshot,
  and compacting an intermediate node no longer delete repository files
  directly after the commit. The released paths are reference-checked and
  enqueued inside the same SQLite transaction (reasons
  `history_branch_deletion`, `history_subtree_deletion`,
  `history_checkpoint_release`, `history_commit_release`,
  `history_snapshot_replaced`, `history_compaction`) and replayed once after
  the commit succeeds. A failed deletion now keeps the entry in the queue with
  its error and retry state instead of being silently dropped, and a crash
  between the commit and the replay is recovered by the next replay. Rollback
  deletions (a failed publish, commit, or snapshot registration) still remove
  their own temporary files directly, because those files are never referenced
  by the database.
- Residual backup staging directories are no longer left behind. Before a
  repository backup starts, the destination directory's top level is scanned
  for leftover unpublished staging directories (`.lilith-artworks-<32 hex>.tmp`).
  A matching directory that is a real directory (symlinks are not followed) and
  is older than a 30-minute grace period is removed one by one; the reclaimed
  and failed counts are returned in the backup report
  (`reclaimedStagingDirectories` / `failedStagingDirectories`). The grace period
  keeps a concurrently running backup's staging directory safe, the sweep only
  touches the top level and never recurses, and a failed removal only counts as
  a failure instead of aborting the backup. The application does not persist
  previous destinations, so no standalone sweep entry point is provided.

### Changed

- The application version is incremented to `0.2.0-alpha.4`. The bump itself
  carries no behavior change and follows the alpha-stage lagging-version
  policy (no tag, no release, matching the alpha.1 and alpha.2 precedent); the
  unified cleanup system and the pre-release stress-test suite recorded above
  landed under the same version. The schema v4 and scheduler batches recorded
  under `0.2.0-alpha.3` below remain part of that section.
- Pre-release verification is finalized in the docs. `docs/guides/validation.md`
  and `docs/guides/release-policy.md` state that power-loss durability is not
  automated (a killed process does not touch OS/disk caches or directory-entry
  persistence; the guarantee rests on the OS, the disk, and SQLite
  `synchronous = FULL`). `docs/modules/history-and-backup.md` and
  `docs/modules/authenticity.md` gain a "reliability invariants and coverage"
  table mapping each guarantee to the scenario that proves it. The stress-test
  plan is archived under `docs/planning/archive/`.

## 0.2.0-alpha.3 - 2026-10-03

### Added

- Quick automatic backups. Each branch has a "快速检查" switch that forces the
  quick mode even when the global default is a full check, and the settings
  dialog gains a default check mode (quick by default). The quick mode compares
  only the working file's size and modification time against the baseline
  recorded after the last successful full check or commit; a complete match is
  reported as unchanged without reading the file, and any mismatch, missing
  baseline, or read failure falls back to the full check-and-backup flow.
  Manual commits always run a full check.
- Manual commits now take priority over automatic backups. A pending manual
  commit defers the branch's automatic backup at candidate selection and again
  after the exclusive run lock is acquired (without counting as a failure), and
  an automatic backup of the same branch that is already running is requested
  to cancel so the manual commit runs first.
- An "打开所在文件夹" action beside the working file actions in the branch
  settings, revealing the working file in the system file manager (selects the
  file on Windows via the new `reveal_path_in_folder` command).
- Pin-board edits can now autosave: when the new "自动保存" pin-board setting
  (off by default) is enabled, the board persists itself 1.5 seconds after the
  last model change (drag, scale, rotate, layer/order, delete, undo/redo), so
  a crash or forced kill only loses the last few seconds of work. A companion
  "关闭时保存" setting (on by default) controls whether the exit handshake
  finalizes the open board before quitting.
- Idle history chain verification. After due automatic backups, and only while
  no foreground command waits and no manual commit is pending, the scheduler
  verifies one branch head per idle slot once that head has been quiet for ten
  minutes. A derived queue lists branches whose head differs from the last
  verified head (including those never verified), so there is no stored queue
  state to corrupt and the queue rebuilds after any restart; the outcome is
  cached in the new branch verification columns.
- The shared runtime status now reports the running task kind (automatic
  backup, idle verification, or user operation). Foreground long commands
  request background tasks to cancel and register a foreground-waiting count
  before taking the run lock, so the scheduler yields to them instead of
  pre-empting and swallowing their cancellation.
- The branch status row surfaces a chain verification failure independently of
  a backup failure, with an expandable detail, a copy action, and a
  "重新校验此分支" button that re-queues the branch through the new
  `reverify_branch_history` command.

### Fixed

- Pin-board arrangements no longer regress after closing the app. Exit (window
  close with close-to-tray disabled, or tray "退出") used to call `app.exit(0)`
  immediately, dropping any in-flight save. The native side now requests a
  shutdown handshake: it emits `app_shutdown_requested`, the webview finalizes
  the open board (persist and truncate the step history) and answers via the
  new `confirm_app_shutdown` command, and a 15-second fallback force-exit keeps
  the window closable if the webview hangs or crashes.

### Changed

- The repository schema moves to v4 through append-only migrations: v2 → v3
  adds the quick-check columns (`backup_quick_enabled`, `last_source_size`,
  `last_source_modified_ms`), and v3 → v4 adds the verification-state columns
  (`verified_history_id`, `verified_ms`, `verify_error`). Every step only
  appends columns and leaves existing data untouched, and a repository above
  v4 is still refused on open. `tools/release/verify-metadata.mjs` now asserts
  schema v4.

## 0.2.0-alpha.2 - 2026-09-18

### Added

- Configurable pin-board lock and fullscreen shortcuts (defaults `Ctrl+R` /
  `F11`) on a paginated settings dialog with a Client-style pin-board page. The
  app-level webview guard cancels the F5 / `Ctrl+R` page reload without
  swallowing the key, so `Ctrl+R` still triggers the lock action.
- Drag-and-drop reordering of pin boards in the board sidebar. Reordering only
  rewrites list order, so an open board's editing session is not invalidated.
- Artworks can be created without a working file. Branches without a working
  file are excluded from automatic backup scheduling and manual commits are
  disabled for them with an explanatory hint; pin boards and other repository
  features keep working.
- A "clear working file path" action beside the edit action in the branch
  settings, and a prominent "选择文件" button replaces the small icon action
  while a branch has no working file.
- `docs/planning/todo.md` as the single list of unfinished, unverified, and
  decision-pending work. Completed batches move to `docs/planning/archive/`.

### Fixed

- Automatic backup can no longer be left switched on for a branch without a
  working file. The switch is disabled and forced off in the branch settings,
  the interval input is disabled with it, and the repository now enforces the
  same rule so a stored branch can never report an enabled scheduler that has
  no source file to read.
- Pin-board fullscreen now works: the window capability allows
  `set_fullscreen` and `is_fullscreen`.
- Trashing the selected board no longer triggers a doomed finalize save
  against the deleted board.
- Clipboard/file image imports create the board directory when it is missing
  (fixes `os error 3` after repository data migration).
- The pin-board canvas shows a "no board selected" placeholder instead of an
  endless spinner when no board is selected.
- The pin-board tab is now listed before the version history tab in the
  Artwork workspace.
- Pin boards no longer open with a degenerate viewport: the workspace pane
  keeps its layout size while inactive, the renderer fits the undeleted-image
  bounds on first draw, and sessions captured from a zero-size canvas are not
  persisted. This fixes images that appeared too small and the missing
  bounding-box fit when reopening a board.
- The settings dialog now uses a roomy single-column layout per page instead
  of the cramped two-column arrangement.

### Changed

- User-facing text and documentation use "分支 / 创建分支 / 分支起点" instead
  of the English "fork". Command names, request DTOs, and SQLite constraint
  names are unchanged.
- The project is positioned as a resource, version, and publishing tool for
  personal 2D art projects rather than a version-control and release tool
  only; plugin and architecture documents were corrected to describe the
  repository operation lock used by pin-board writes, the actual coverage of
  whole-repository backups over `boards/`, and the pin-board command layer.
- Pin-board planning, migration, and comparison records moved to
  `docs/planning/archive/`; module and architecture documents keep only the
  currently valid contracts.

### Compatibility

- Application settings version moves to 2. Existing settings.json files load
  through an append-only migration: a persisted pin-board lock shortcut that
  still holds the previous default `CommandOrControl+Shift+K` is upgraded to
  `CommandOrControl+R`, while any other custom shortcut is preserved.
- Application settings gain optional `lockShortcut` and `fullscreenShortcut`
  fields in the `pinBoard` section; existing settings.json files load with the
  new defaults.
- Branches may now store an empty `source_path`; the database schema is
  unchanged and no migration is required.
- A branch whose `source_path` is empty always persists `backup_enabled = 0`.
  Because `branches.source_path_key` is a non-null column in a unique index
  over `(artwork_id, source_path_key)`, only one branch per Artwork may omit
  its working file; clearing the path of a second branch is rejected and the
  transaction rolls back.

## 0.2.0-alpha.1 - 2026-09-12

### Added

- Pin-board (素材板) module migrated from Lilith Client as the fifth domain
  module: flat per-Artwork boards, BC7 DDS image storage, WebGPU renderer,
  import/export with progress channels, text overlays, and a persistent
  step-based undo/redo history stored in SQLite.
- Board trash with soft delete, restore, permanent delete, and empty-trash
  flows; DDS directories are removed through the shared pending file cleanup
  queue with retry on the next launch.
- Settings for the pin-board texture cache level (low/medium/high) and the
  arrangement gap in CSS pixels.

### Compatibility

- Repository schema is now v2. Opening a v1 repository performs an append-only
  migration that adds the `pin_boards`, `pin_board_images`, and
  `pin_board_history` tables; existing v1 data is unchanged and there is no
  downgrade path back to v1.
- Application settings gain a versioned `pinBoard` section; existing
  settings.json files load with default pin-board values.

## 0.1.0 - 2026-09-12

### Compatibility

- Repository schema v1 is the initial supported format. Repositories created by
  earlier pre-release builds are not compatible and must be recreated; the
  application refuses to open any repository that does not report schema v1.
- Application versioning restarts at 0.1.0. The project does not declare
  support for migrating data from previous pre-release builds.
