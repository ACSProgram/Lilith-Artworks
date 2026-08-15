# Changelog

All notable changes are recorded here. The project uses semantic versioning
before and after the first stable release.

## 0.1.0-rc.1 - 2026-08-15

### Added

- Source code is released under GPL-3.0; added root `LICENSE` and
  `THIRD_PARTY_NOTICES.md`. Adobe TrustMark models keep their independent MIT
  license under `src-tauri/resources/models/LICENSE`.
- Added frontend tests for tree/history helpers, shared formatting, publication
  preview invalidation, navigator geometry, and latest-request-wins controllers.
- Added a draggable navigator thumbnail for zoomed publication quality previews.
- Added rotating native application logs for startup, shutdown, backup, and
  window lifecycle diagnostics.

### Changed

- Cancellation of branch publication now removes only repository-owned final
  artifacts, certification copies, records, and saved configuration. The first
  exported JPG remains at its publication path.
- Certification records now always require a repository-owned JPG copy under
  repository schema v8; the legacy `contentStored` API and UI field were removed.
- Authenticity trace navigation now uses an explicit navigation generation so
  same-Artwork and repeated-record jumps select the requested branch reliably.
- Added a Windows CI gate, contribution guidance, a security policy, and a
  release policy for public release candidates.
- Repository-owned artifact previews and size estimates now resolve paths from
  branch IDs in the backend; publication and re-export targets reject repository
  storage paths. Native commands also require transient file-dialog scope for
  external images, certificates, final artifacts, and output destinations.

### Fixed

- Fixed empty branch selectors and missing targets after navigating from
  identification or trace results to publication records.
- Quality preview generation now rejects incomplete signing parameters before
  encoding, including an empty private key.
- Quality preview zoom now starts from the actual fitted scale, responds to
  wheel increments continuously, preserves the pointer anchor, and uses
  drag/navigation positioning without visible scrollbars.
- History deletion and certification record queries now propagate malformed
  database rows instead of silently skipping or replacing them.

## 0.1.0 - Unreleased

Initial local-first Artwork library, branch history, backup, publication,
C2PA/TrustMark, identification, and recovery implementation.
