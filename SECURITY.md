# Security Policy

## Supported versions

Until the first stable release, security fixes are made only on the latest
development line and the newest published `0.1.x` build, if one exists.

## Reporting a vulnerability

Do not open a public issue for a suspected vulnerability. Use the repository's
private GitHub security advisory channel and include:

- the affected version or commit;
- reproduction steps and required files;
- the expected and observed security boundary;
- whether credentials, repository files, exported images, or signatures are at
  risk.

Do not include real signing keys, private artwork, or a user's repository in the
report. Use a minimal temporary repository and disposable credentials.

The maintainers should acknowledge a complete report within seven days. Fix
timing depends on severity and whether coordinated disclosure is required.

## Security-sensitive areas

Changes to path validation, cleanup queues, SQLite migrations, C2PA signing,
private-key handling, bundled model files, or Tauri permissions require focused
tests and explicit review. A successful build alone is not sufficient evidence
for these boundaries.
