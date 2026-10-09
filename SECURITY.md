# Security Policy

## Supported versions

Until the first stable release, security fixes are made only on the latest
development line and the newest published `0.2.x` build, if one exists.

## Reporting a vulnerability

Prefer the repository's GitHub **Report a vulnerability** form when it is
available. The direct form is:
https://github.com/ACSProgram/Lilith-Artworks/security/advisories/new

If that private form is unavailable, vulnerability details are exchanged
through a private channel first. Open a minimal public issue titled
`[Security contact request]` containing only the affected version and a way for
the maintainer to contact you; the maintainer arranges the private channel
before reproduction details or files are shared.

A complete private report covers:

- the affected version or commit;
- reproduction steps and required files;
- the expected and observed security boundary;
- whether credentials, repository files, exported images, or signatures are at
  risk.

A report needs no real signing keys, private artwork, or a user's repository; a
minimal temporary repository and disposable credentials are enough.

The maintainer aims to acknowledge a complete report within seven days. Fix
timing depends on severity and whether coordinated disclosure is required.

## Security-sensitive areas

Path validation, cleanup queues, SQLite migrations, C2PA signing, private-key
handling, bundled model files, and Tauri permissions are treated as
security-sensitive. Changes there are reviewed against focused tests rather
than a successful build alone.
