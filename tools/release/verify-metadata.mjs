import fs from "node:fs";
import path from "node:path";

const root = process.cwd();
const packageJson = JSON.parse(fs.readFileSync(path.join(root, "package.json"), "utf8"));
const packageLock = JSON.parse(fs.readFileSync(path.join(root, "package-lock.json"), "utf8"));
const cargoToml = fs.readFileSync(path.join(root, "src-tauri", "Cargo.toml"), "utf8");
const tauriConfig = JSON.parse(fs.readFileSync(path.join(root, "src-tauri", "tauri.conf.json"), "utf8"));
const npmConfig = fs.readFileSync(path.join(root, ".npmrc"), "utf8");

const cargoVersion = cargoToml.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
const versions = new Map([
  ["package.json", packageJson.version],
  ["package-lock.json", packageLock.version],
  ["package-lock root package", packageLock.packages?.[""]?.version],
  ["Cargo.toml", cargoVersion],
  ["tauri.conf.json", tauriConfig.version],
]);
const expected = packageJson.version;
const mismatches = [...versions].filter(([, version]) => version !== expected);
if (mismatches.length > 0) {
  throw new Error(`Release version mismatch: ${mismatches.map(([file, version]) => `${file}=${version}`).join(", ")}; expected ${expected}`);
}
if (tauriConfig.identifier !== "com.lilith.artworks") {
  throw new Error(`Unexpected Tauri identifier: ${tauriConfig.identifier}`);
}

if (!/^registry=https:\/\/registry\.npmjs\.org\/$/m.test(npmConfig)) {
  throw new Error(".npmrc must use the official npm registry");
}
if (!/^replace-registry-host=always$/m.test(npmConfig)) {
  throw new Error(".npmrc must replace lockfile registry hosts during installs");
}
const nonOfficialPackages = Object.entries(packageLock.packages ?? {})
  .filter(([, metadata]) => metadata?.resolved?.startsWith("https://"))
  .filter(([, metadata]) => new URL(metadata.resolved).hostname !== "registry.npmjs.org")
  .map(([name, metadata]) => `${name || "<root>"}=${metadata.resolved}`);
if (nonOfficialPackages.length > 0) {
  throw new Error(`Package lock uses non-official registries: ${nonOfficialPackages.join(", ")}`);
}

const schema = fs.readFileSync(path.join(root, "src-tauri", "src", "library", "schema.rs"), "utf8");
if (!/SCHEMA_VERSION:\s*i64\s*=\s*9\s*;/.test(schema)) {
  throw new Error("Release candidate must use repository schema v9");
}

const changelog = fs.readFileSync(path.join(root, "CHANGELOG.md"), "utf8");
const escapedVersion = expected.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
if (!new RegExp(`^## ${escapedVersion} - \\d{4}-\\d{2}-\\d{2}$`, "m").test(changelog)) {
  throw new Error(`CHANGELOG.md has no dated section for ${expected}`);
}

const tag = process.env.GITHUB_REF_TYPE === "tag" ? process.env.GITHUB_REF_NAME : "";
if (tag && tag !== `v${expected}`) {
  throw new Error(`Tag ${tag} does not match version v${expected}`);
}

console.log(`Release metadata verified: v${expected}, com.lilith.artworks, schema v9`);
