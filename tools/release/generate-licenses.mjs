import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import spdxLicenses from "spdx-license-list/full.js";

const root = process.cwd();
const target = "x86_64-pc-windows-msvc";
const output = path.join(root, "licenses", "THIRD_PARTY_LICENSES.html");
const commonLicenseName = /^(license|licence|copying|notice)(\.[^.]+)?$/i;

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, "utf8"));
}

function escapeHtml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function licenseTexts(directory, explicitFile) {
  const files = new Set();
  if (explicitFile && fs.existsSync(explicitFile)) files.add(path.resolve(explicitFile));
  if (fs.existsSync(directory)) {
    for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
      if (entry.isFile() && commonLicenseName.test(entry.name)) {
        files.add(path.join(directory, entry.name));
      }
    }
  }
  return [...files]
    .sort((left, right) => left.localeCompare(right))
    .map((file) => ({ name: path.basename(file), text: fs.readFileSync(file, "utf8").trim() }))
    .filter(({ text }) => text.length > 0);
}

function spdxLicenseTexts(expression) {
  const identifiers = String(expression ?? "")
    .match(/[A-Za-z0-9][A-Za-z0-9.+-]*/g)
    ?.filter((value) => !["AND", "OR", "WITH"].includes(value)) ?? [];
  return [...new Set(identifiers)]
    .filter((identifier) => spdxLicenses[identifier]?.licenseText)
    .map((identifier) => ({
      name: `${identifier}.txt (SPDX canonical text)`,
      text: spdxLicenses[identifier].licenseText.trim(),
    }));
}

function resolveNpmDependency(packages, fromKey, name) {
  let scope = fromKey;
  while (true) {
    const candidate = scope ? `${scope}/node_modules/${name}` : `node_modules/${name}`;
    if (packages[candidate]) return candidate;
    const marker = scope.lastIndexOf("/node_modules/");
    if (marker < 0) {
      if (!scope) return null;
      scope = "";
    } else {
      scope = scope.slice(0, marker);
    }
  }
}

function npmComponents() {
  const lock = readJson(path.join(root, "package-lock.json"));
  const packages = lock.packages;
  const pending = Object.keys(packages[""]?.dependencies ?? {})
    .map((name) => resolveNpmDependency(packages, "", name));
  const visited = new Set();
  const components = [];
  while (pending.length > 0) {
    const key = pending.pop();
    if (!key || visited.has(key)) continue;
    visited.add(key);
    const locked = packages[key];
    if (locked.dev) continue;
    const directory = path.join(root, ...key.split("/"));
    if (!fs.existsSync(directory)) {
      if (locked.optional) continue;
      throw new Error(`npm runtime package is not installed: ${key}`);
    }
    const manifest = readJson(path.join(directory, "package.json"));
    const license = manifest.license ?? locked.license ?? "UNKNOWN";
    const texts = licenseTexts(directory, manifest.licenseFile && path.join(directory, manifest.licenseFile));
    if (texts.length === 0) texts.push(...spdxLicenseTexts(license));
    if (texts.length === 0) throw new Error(`Missing npm license body: ${manifest.name}@${manifest.version}`);
    components.push({
      ecosystem: "npm",
      name: manifest.name,
      version: manifest.version,
      license,
      authors: manifest.author?.name ?? manifest.author ?? manifest.contributors?.map((item) => item.name ?? item).join(", ") ?? "See license text",
      source: manifest.repository?.url ?? manifest.homepage ?? locked.resolved ?? "",
      texts,
    });
    for (const name of Object.keys({ ...locked.dependencies, ...locked.optionalDependencies })) {
      pending.push(resolveNpmDependency(packages, key, name));
    }
  }
  return components;
}

function cargoComponents() {
  const raw = execFileSync("cargo", [
    "metadata", "--locked", "--format-version", "1", "--filter-platform", target,
    "--manifest-path", path.join(root, "src-tauri", "Cargo.toml"),
  ], {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
    stdio: ["ignore", "pipe", "inherit"],
  });
  const metadata = JSON.parse(raw);
  const packages = new Map(metadata.packages.map((item) => [item.id, item]));
  const nodes = new Map(metadata.resolve.nodes.map((item) => [item.id, item]));
  const rootId = metadata.resolve.root;
  const pending = [...(nodes.get(rootId)?.dependencies ?? [])];
  const visited = new Set();
  const components = [];
  while (pending.length > 0) {
    const id = pending.pop();
    if (visited.has(id)) continue;
    visited.add(id);
    const item = packages.get(id);
    if (!item) throw new Error(`Cargo metadata package is missing: ${id}`);
    const directory = path.dirname(item.manifest_path);
    const explicit = item.license_file
      ? path.resolve(directory, item.license_file)
      : null;
    const texts = licenseTexts(directory, explicit);
    if (texts.length === 0) texts.push(...spdxLicenseTexts(item.license));
    if (texts.length === 0) throw new Error(`Missing Cargo license body: ${item.name}@${item.version}`);
    components.push({
      ecosystem: "Cargo",
      name: item.name,
      version: item.version,
      license: item.license ?? "UNKNOWN",
      authors: item.authors?.join(", ") || "See license text",
      source: item.repository ?? item.homepage ?? item.source ?? "",
      texts,
    });
    pending.push(...(nodes.get(id)?.dependencies ?? []));
  }
  return components;
}

const packageJson = readJson(path.join(root, "package.json"));
const components = [...npmComponents(), ...cargoComponents()]
  .sort((left, right) => left.ecosystem.localeCompare(right.ecosystem)
    || left.name.localeCompare(right.name)
    || left.version.localeCompare(right.version));
const duplicateKeys = components
  .map((item) => `${item.ecosystem}:${item.name}@${item.version}`)
  .filter((key, index, all) => all.indexOf(key) !== index);
if (duplicateKeys.length > 0) throw new Error(`Duplicate components: ${duplicateKeys.join(", ")}`);

const sections = components.map((item) => `
<section>
  <h2>${escapeHtml(item.name)} <small>${escapeHtml(item.version)} · ${escapeHtml(item.ecosystem)}</small></h2>
  <dl><dt>License</dt><dd>${escapeHtml(item.license)}</dd><dt>Copyright / authors</dt><dd>${escapeHtml(item.authors)}</dd><dt>Source</dt><dd>${escapeHtml(item.source)}</dd></dl>
  ${item.texts.map((license) => `<h3>${escapeHtml(license.name)}</h3><pre>${escapeHtml(license.text)}</pre>`).join("\n  ")}
</section>`).join("\n");
const html = `<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Lilith Artworks ${escapeHtml(packageJson.version)} third-party licenses</title>
<style>body{max-width:980px;margin:40px auto;padding:0 24px;color:#202420;background:#fff;font:14px/1.5 system-ui,sans-serif}h1{font-size:24px}h2{margin-top:32px;border-top:1px solid #ccd2cc;padding-top:20px;font-size:18px}h2 small{color:#596259;font-size:12px;font-weight:400}dl{display:grid;grid-template-columns:150px 1fr;gap:4px 12px}dt{font-weight:700}dd{margin:0;overflow-wrap:anywhere}pre{overflow:auto;padding:14px;border:1px solid #d9ddd9;background:#f6f8f6;font:12px/1.45 ui-monospace,monospace;white-space:pre-wrap}</style>
</head><body><h1>Lilith Artworks ${escapeHtml(packageJson.version)} third-party licenses</h1>
<p>Copyright 2026 ACSProgram. Application code is licensed under GPL-3.0-only. This Windows ${escapeHtml(target)} dependency closure contains ${components.length} runtime and linked components; each component retains its own copyright and license.</p>
${sections}
</body></html>
`;
fs.mkdirSync(path.dirname(output), { recursive: true });
fs.writeFileSync(output, html, "utf8");
console.log(`Generated ${path.relative(root, output)} with ${components.length} components`);
