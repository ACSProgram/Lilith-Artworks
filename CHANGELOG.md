# Changelog

All notable changes are recorded here. The project uses semantic versioning
before and after the first stable release.

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
