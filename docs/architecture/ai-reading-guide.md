# AI Reading Guide

This is the first entry point for code tasks. Choose one route below and read only its contract and implementation files. Read adjacent modules only when a cross-module call or database schema contract requires it.

## Routes

| Problem | Start with | Implementation |
| --- | --- | --- |
| Artwork tree, search, drag/drop, trash | `docs/modules/library.md` | `src/modules/library/`, `src-tauri/src/library/` |
| Branches, commits, fork, restore, compaction, checkpoints | `docs/modules/history-and-backup.md` | `src/modules/history/`, `src-tauri/src/history/`, `src-tauri/src/backup/` |
| Publication, artifacts, C2PA, TrustMark, recognition | `docs/modules/authenticity.md` | `src/modules/authenticity/`, `src-tauri/src/authenticity/` |
| Settings, tray, window lifecycle | `docs/architecture/overview.md` | `src-tauri/src/app/`, `src-tauri/src/lib.rs` |
| Build, format, static checks | `docs/guides/validation.md` | Run only checks matching the change |

## Read order

1. Read the selected module document and `docs/planning/current-handoff.md`.
2. Read frontend `types.ts`/`api.ts` or Rust `model.rs`/`commands.rs` to confirm DTO and command contracts.
3. Read the implementation. Database work enters the domain repository; file-format work enters `backup/chunk_file.rs` only when required.

## Boundaries

- Frontend modules do not import each other; cross-module flows are composed in `src/app/`.
- `src/modules/<module>/api.ts` is the only frontend entry for that domain's Tauri commands.
- `src-tauri/src/storage.rs` owns connection configuration, paths, IDs, time, and basic validation, not domain workflows.
- `history` owns SQLite graph metadata and does not read ChunkFile; `backup` owns materialization and scheduling.
- `authenticity` uses public history/backup capabilities for checkpoints and does not import library internals.
- Destructive history operations enter through `src-tauri/src/history/deletion_repository.rs`; ordinary graph operations enter through `repository.rs`.
- Publication state and `final_artifacts` binding enter through `src-tauri/src/authenticity/publication_repository.rs`; certification config and record queries enter through `repository.rs`.

## Change rules

Reuse shared infrastructure and existing DTOs. Do not duplicate database connection setup, path normalization, ID generation, or error formatting in domain repositories. Compose cross-module flows at the application command layer. Update architecture/module docs when an entry point or contract changes; record unfinished work only in `docs/planning/current-handoff.md`.
