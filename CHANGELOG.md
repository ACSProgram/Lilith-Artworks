# Changelog

All notable changes are recorded here. The project uses semantic versioning
before and after the first stable release.

## Unreleased

### Changed

- Bumped all release metadata to `0.1.0-rc.2`, upgraded Vitest to 3.2.6,
  moved CI to Node 24, and disabled PS256 pending resolution of its Rust crypto advisory.
- Added a Windows dependency license closure, an About/Legal entry, and a
  tag-only draft release workflow with signing inputs, package inspection,
  checksums, CycloneDX SBOM, and provenance attestations.

- Changed the application identifier and C2PA label from
  `art.lilith.artworks` to `com.lilith.artworks`; the next published build must
  use a new release-candidate version rather than reuse `0.1.0-rc.1`.
- Added persistent automatic-backup retry state and branch notices under
  repository schema v9.
- Added Tauri single-instance handling and native settings/log-folder actions.
- Corrected the public preview status, security-reporting fallback, and
  third-party licensing language. The bundle configuration now includes the
  project license, third-party notices, and the unmodified Adobe TrustMark
  model license.

### Fixed

- Publication preview source switching now resets the fitted view and avoids
  unnecessary re-encoding for repository-owned JPEG, PNG, and WebP files.

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
