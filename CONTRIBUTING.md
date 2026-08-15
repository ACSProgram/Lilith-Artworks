# Contributing to Lilith Artworks

## Scope

Keep changes within the module boundaries documented in
[`docs/architecture/ai-reading-guide.md`](docs/architecture/ai-reading-guide.md).
Read [`docs/planning/current-handoff.md`](docs/planning/current-handoff.md) before
starting work so completed architecture work is not reopened accidentally.

## Development setup

The supported development environment is Windows with:

- Node.js 20 and npm;
- the stable Rust toolchain with `rustfmt`;
- the Microsoft C++ build tools required by Tauri;
- WebView2;
- the model files listed in `src-tauri/resources/models/README.md`.

Install locked frontend dependencies with `npm ci`. Run the app with
`npm run tauri -- dev` only when an interactive desktop check is required.

## Change requirements

- Preserve the existing History, Backup, Library, Authenticity, and application
  workflow boundaries.
- Keep database and file mutations transactional and test them with temporary
  repositories rather than user data.
- Update the owning module document when a command, DTO, schema, cleanup rule,
  or user workflow changes.
- Record unfinished work and manual acceptance results only in
  `docs/planning/current-handoff.md`.
- Do not commit signing keys, certificates, application state, generated build
  output, or model files without confirming their redistribution terms.

## Validation

Use [`docs/guides/validation.md`](docs/guides/validation.md) to select checks.
The normal minimum for TypeScript or Rust changes is:

```powershell
npm run build
cargo fmt --check --manifest-path src-tauri/Cargo.toml
git diff --check
```

Run focused Rust tests for the changed database or pure-logic contract. Full
Rust library tests run in Windows CI. Desktop GUI flows, real C2PA verification,
and TrustMark image checks are manual release gates.

## Pull requests

Describe the behavior change, affected module boundaries, schema impact, and
the exact validation performed. Keep unrelated edits out of the change. Do not
include generated `dist/`, `target/`, runtime repositories, or credentials.
