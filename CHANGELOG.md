# Changelog

All notable changes are recorded here. The project uses semantic versioning
before and after the first stable release.

## Unreleased

### Fixed

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

### Added

- Configurable pin-board lock and fullscreen shortcuts (defaults
  `Ctrl+Shift+K` / `F11`) on a new paginated settings dialog with a
  Client-style pin-board page; `Ctrl+R` page reload is suppressed while the
  pin board is active.
- Artworks can be created without a working file. Branches without a working
  file are excluded from automatic backup scheduling and manual commits are
  disabled for them with an explanatory hint; pin boards and other repository
  features keep working.

### Compatibility

- Application settings gain optional `lockShortcut` and `fullscreenShortcut`
  fields in the `pinBoard` section; existing settings.json files load with the
  new defaults.
- Branches may now store an empty `source_path`; the database schema is
  unchanged and no migration is required.

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
