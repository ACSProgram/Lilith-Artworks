# Changelog

All notable changes are recorded here. The project uses semantic versioning
before and after the first stable release.

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
