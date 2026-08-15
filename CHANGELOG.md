# Changelog

All notable changes are recorded here. The project uses semantic versioning
before and after the first stable release.

## Unreleased

### Added

- Source code is released under GPL-3.0; added root `LICENSE` and
  `THIRD_PARTY_NOTICES.md`. Adobe TrustMark models keep their independent MIT
  license under `src-tauri/resources/models/LICENSE`.

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

### Fixed

- Fixed empty branch selectors and missing targets after navigating from
  identification or trace results to publication records.

## 0.1.0 - Unreleased

Initial local-first Artwork library, branch history, backup, publication,
C2PA/TrustMark, identification, and recovery implementation.
