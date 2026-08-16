# Security Policy

## Supported versions

Until the first stable release, security fixes are made only on the latest
development line and the newest published `0.1.x` build, if one exists.

## Reporting a vulnerability

Prefer the repository's GitHub **Report a vulnerability** form when it is
available. The direct form is:
https://github.com/ACSProgram/Lilith-Artworks/security/advisories/new

If that private form is unavailable, do not publish vulnerability details.
Open a minimal public issue titled `[Security contact request]` containing only
the affected version and a way for the maintainer to contact you. The
maintainer will arrange a private channel before reproduction details or files
are exchanged.

In the private report, include:

- the affected version or commit;
- reproduction steps and required files;
- the expected and observed security boundary;
- whether credentials, repository files, exported images, or signatures are at
  risk.

Do not include real signing keys, private artwork, or a user's repository in the
report. Use a minimal temporary repository and disposable credentials.

The maintainers aim to acknowledge a complete report within seven days. Fix
timing depends on severity and whether coordinated disclosure is required.

Before a formal release, maintainers must enable and test GitHub private
vulnerability reporting or replace the fallback above with another verified
private channel.

## Security-sensitive areas

Changes to path validation, cleanup queues, SQLite migrations, C2PA signing,
private-key handling, bundled model files, or Tauri permissions require focused
tests and explicit review. A successful build alone is not sufficient evidence
for these boundaries.
