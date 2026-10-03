import fs from "node:fs";
import path from "node:path";

const root = process.cwd();
const outputPath = process.argv[2];
if (!outputPath) {
  throw new Error("Usage: node tools/release/write-release-notes.mjs <output-path>");
}

const packageJson = JSON.parse(fs.readFileSync(path.join(root, "package.json"), "utf8"));
const version = packageJson.version;
const schemaSource = fs.readFileSync(
  path.join(root, "src-tauri", "src", "library", "schema.rs"),
  "utf8",
);
const schemaVersion = schemaSource.match(/SCHEMA_VERSION:\s*i64\s*=\s*(\d+)\s*;/)?.[1];
if (!schemaVersion) {
  throw new Error("Could not read SCHEMA_VERSION from src-tauri/src/library/schema.rs");
}
const changelog = fs.readFileSync(path.join(root, "CHANGELOG.md"), "utf8");
const escapedVersion = version.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
const sectionPattern = new RegExp(
  `^## ${escapedVersion} - [^\\r\\n]+\\r?\\n([\\s\\S]*?)(?=^## |$(?![\\s\\S]))`,
  "m",
);
const section = changelog.match(sectionPattern)?.[1]?.trim();
if (!section) {
  throw new Error(`CHANGELOG.md has no dated section for ${version}`);
}

const signature = process.env.LILITH_RELEASE_SIGNATURE?.trim();
if (!signature) {
  throw new Error("LILITH_RELEASE_SIGNATURE must describe the signing status");
}

const notes = `# Lilith Artworks ${version}

${signature}

Repository schema: v${schemaVersion}. Opening a repository written by an earlier schema performs an append-only migration up to v${schemaVersion}; migration is one-way with no downgrade path, and a repository below v1 or above v${schemaVersion} is refused on open. Application settings migrate through their own append-only path.

${section}

Automated build evidence includes SHA-256 checksums, a CycloneDX SBOM, the Windows target license closure, and GitHub artifact attestations. Publish this draft only after the manual release checklist is complete.
`;

fs.writeFileSync(outputPath, notes, "utf8");
console.log(`Release notes generated for ${version}: ${outputPath}`);
