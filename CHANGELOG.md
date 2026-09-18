# Changelog

All notable changes are recorded here. The project uses semantic versioning
before and after the first stable release.

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
