# Changelog

All notable changes are recorded here. The project uses semantic versioning
before and after the first stable release.

## Unreleased

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
  `docs/guides/stress-test-report.md`.

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
