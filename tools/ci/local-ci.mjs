#!/usr/bin/env node
// 本地 CI 预检：只在准备发布或推送候选版本前运行。
//
// 它只覆盖 Windows CI 中「日常检查不会跑」的门槛：两条 npm 审计、RustSec 审计、
// 发布元数据、平台分支检查、许可证生成与一致性、headless 测试目标编译，以及
// gate_guard 和九组便宜压力测试。`npm test`、`cargo test --lib`、`cargo fmt
// --check`、`npm run build`、`tsc` 属于日常分层验证，不在这里重复。
//
// 纪律（见 docs/guides/validation.md 的「本地 CI 预检」）：
// - 所有产物写入 <target>/ci/，默认 src-tauri/target/ci，跟随 CARGO_TARGET_DIR；
//   该目录与 *.log 都已被 .gitignore 覆盖，运行不会在仓库里留下未跟踪文件。
// - 独立门槛并行启动，失败也继续跑完其余门槛，最后一次性汇总。
// - cargo 命令共用同一个 target 目录，必须串行；压力组之间也串行，因为
//   stress_support 的 report.jsonl / ledger.jsonl 只有进程内锁。

import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const root = process.cwd();
const isWindows = process.platform === "win32";
const targetDirectory = path.resolve(
  process.env.CARGO_TARGET_DIR || path.join(root, "src-tauri", "target"),
);
const logDirectory = path.join(targetDirectory, "ci");
const workflowDirectory = path.join(root, ".github", "workflows");
const licensePath = path.join(root, "licenses", "THIRD_PARTY_LICENSES.html");
const children = new Set();
const results = [];

const NPM = isWindows ? process.env.ComSpec || "cmd.exe" : "npm";
const npmArgs = (args) =>
  isWindows ? ["/d", "/s", "/c", "npm", ...args] : args;

const STRESS_GROUPS = [
  "stress_cancel",
  "stress_crash",
  "stress_cleanup",
  "stress_damage",
  "stress_format",
  "stress_pin_board",
  "stress_authenticity",
  "stress_concurrency",
  "stress_input",
];
const TEST_TARGETS = ["gate_guard", ...STRESS_GROUPS];
const REGISTRY = "https://registry.npmjs.org";

function formatDuration(milliseconds) {
  return `${(milliseconds / 1000).toFixed(1)}s`;
}

function emit(line) {
  console.log(line);
}

function tailOf(text, lines = 30) {
  return text.split(/\r?\n/).slice(-lines).join("\n");
}

function record(result) {
  results.push(result);
  const marker = result.status.toUpperCase().padEnd(4);
  const detail =
    result.status === "skip"
      ? ` — ${result.reason}`
      : ` ${formatDuration(result.durationMs)}`;
  const suffix =
    result.status === "fail" && result.logPath ? ` — ${result.logPath}` : "";
  emit(`[${marker}] ${result.title}${detail}${suffix}`);
  if (result.status === "fail" && result.tail) {
    emit(tailOf(result.tail).replace(/^/gm, "       "));
  }
  return result;
}

function runCommand(name, title, command, args, options = {}) {
  const startedAt = Date.now();
  const logPath = path.join(logDirectory, `${name}.log`);
  const stream = fs.createWriteStream(logPath);
  let tail = "";
  const collect = (chunk) => {
    const text = chunk.toString();
    stream.write(text);
    tail = (tail + text).slice(-65536);
  };

  const child = spawn(command, args, {
    cwd: root,
    windowsHide: true,
    stdio: ["ignore", "pipe", "pipe"],
    env: {
      ...process.env,
      ...options.env,
      CARGO_TERM_COLOR: "never",
      CARGO_TERM_PROGRESS_WHEN: "never",
      NO_COLOR: "1",
      npm_config_color: "false",
    },
  });
  children.add(child);

  return new Promise((resolve) => {
    let settled = false;
    const finish = (code, error) => {
      if (settled) return;
      settled = true;
      children.delete(child);
      stream.end();
      resolve(
        record({
          name,
          title,
          status: code === 0 ? "pass" : "fail",
          durationMs: Date.now() - startedAt,
          startedAt,
          logPath,
          tail,
          error,
        }),
      );
    };
    child.on("error", (error) => finish(-1, error));
    child.on("close", (code) => finish(code, null));
    child.stdout.on("data", collect);
    child.stderr.on("data", collect);
  });
}

function runCheck(name, title, check) {
  const startedAt = Date.now();
  const logPath = path.join(logDirectory, `${name}.log`);
  try {
    const detail = check();
    fs.writeFileSync(logPath, detail || "ok\n");
    return record({
      name,
      title,
      status: "pass",
      durationMs: Date.now() - startedAt,
      startedAt,
      logPath,
      tail: "",
    });
  } catch (error) {
    fs.writeFileSync(logPath, `${error.stack || error}\n`);
    return record({
      name,
      title,
      status: "fail",
      durationMs: Date.now() - startedAt,
      startedAt,
      logPath,
      tail: String(error.message ?? error),
      error,
    });
  }
}

function skip(name, title, reason) {
  return Promise.resolve(
    record({
      name,
      title,
      status: "skip",
      durationMs: 0,
      startedAt: Date.now(),
      logPath: null,
      reason,
    }),
  );
}

function readWorkflowIgnoreList(file) {
  const text = fs.readFileSync(path.join(workflowDirectory, file), "utf8");
  const matches = [...text.matchAll(/^\s*ignore:\s*(\S+)\s*$/gm)];
  if (matches.length !== 1) {
    throw new Error(`${file}: 期望恰好一条 ignore 行，实际 ${matches.length} 条`);
  }
  const ids = matches[0][1].split(",");
  for (const id of ids) {
    if (!/^RUSTSEC-\d{4}-\d{4}$/.test(id)) {
      throw new Error(`${file}: ignore 列表里有非法 ID ${id}`);
    }
  }
  return ids;
}

function checkWorkflowAuditParity() {
  const ci = readWorkflowIgnoreList("windows-ci.yml");
  const release = readWorkflowIgnoreList("release.yml");
  if (ci.join(",") !== release.join(",")) {
    throw new Error(
      "windows-ci.yml 与 release.yml 的 ignore 列表不一致；本地审计必须与 CI 用同一份列表。",
    );
  }
  return ci.join(",");
}

function checkPlatformBranches() {
  const sourceRoot = path.join(root, "src-tauri", "src");
  const pattern = /cfg(_attr)?!?\((?:target_os|windows|unix)/;
  const violations = [];

  const walk = (directory) => {
    for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
      const fullPath = path.join(directory, entry.name);
      if (entry.isDirectory()) {
        walk(fullPath);
        continue;
      }
      if (!entry.name.endsWith(".rs")) continue;
      const relative = path.relative(root, fullPath).replaceAll("\\", "/");
      if (
        relative.startsWith("src-tauri/src/platform/") ||
        relative === "src-tauri/src/repository_lock.rs"
      ) {
        continue;
      }
      const lines = fs.readFileSync(fullPath, "utf8").split(/\r?\n/);
      for (const [index, line] of lines.entries()) {
        const trimmed = line.trimStart();
        if (trimmed.startsWith("//") || trimmed.startsWith("/*")) continue;
        if (trimmed.startsWith("*")) continue;
        if (pattern.test(line)) violations.push(`${relative}:${index + 1}:${line.trim()}`);
      }
    }
  };
  walk(sourceRoot);

  if (violations.length > 0) {
    throw new Error(
      `平台分支出现在 platform/ 之外（见 docs/guides/platform.md 规则 1）：\n${violations.join("\n")}`,
    );
  }
  return "ok\n";
}

function licenseVersionLines(text) {
  return text
    .split(/\r?\n/)
    .filter((line) => line.trimStart().startsWith("<h2>"));
}

function compareLicenseFreshness(beforePath, afterPath) {
  const normalize = (text) => text.replace(/\r\n/g, "\n");
  const before = normalize(fs.readFileSync(beforePath, "utf8"));
  const after = normalize(fs.readFileSync(afterPath, "utf8"));
  if (before === after) return "许可证清单已是最新。\n";

  const beforeLines = licenseVersionLines(before);
  const afterLines = licenseVersionLines(after);
  const removed = beforeLines.filter((line) => !afterLines.includes(line));
  const added = afterLines.filter((line) => !beforeLines.includes(line));
  const details = [
    "许可证清单在本次生成后发生了变化，请审查并提交 licenses/THIRD_PARTY_LICENSES.html。",
    ...removed.map((line) => `- ${line.trim()}`),
    ...added.map((line) => `+ ${line.trim()}`),
  ];
  if (removed.length === 0 && added.length === 0) {
    details.push("（差异不只在版本行，请直接查看文件 diff。）");
  }
  throw new Error(details.join("\n"));
}

function preflight() {
  if (
    !fs.existsSync(path.join(root, "src-tauri", "Cargo.toml")) ||
    !fs.existsSync(workflowDirectory)
  ) {
    console.error("请在仓库根目录运行（npm run ci:local）。");
    process.exit(2);
  }
  const nodeMajor = Number(process.versions.node.split(".")[0]);
  if (nodeMajor !== 24) {
    console.warn(`警告：Windows CI 使用 Node 24，本机为 ${process.version}。`);
  }
  const status = spawnSync("git", ["status", "--porcelain"], { encoding: "utf8" });
  if (status.status === 0 && status.stdout.trim()) {
    console.warn(
      "警告：工作区不干净；本地 CI 只对已落盘的内容负责，请确认这些改动就是要验证的内容。",
    );
  }
  fs.mkdirSync(logDirectory, { recursive: true });
}

async function main() {
  preflight();
  const startedAt = Date.now();
  emit(`本地 CI 预检 — ${new Date().toISOString()}`);
  emit(`仓库：${root}`);
  emit(`产物：${logDirectory}`);
  emit("");

  let ignoreIds = null;
  runCheck("workflow-audit-parity", "workflow 审计 ignore 列表一致", () => {
    const value = checkWorkflowAuditParity();
    ignoreIds = value.split(",");
    return value;
  });
  runCheck("platform-branches", "平台分支只在 platform/ 内", checkPlatformBranches);

  const npmInstall = runCommand("npm-ci", "npm ci", NPM, npmArgs(["ci"]));

  const rustBuild = runCommand("rust-build", "Rust 测试目标预编译", "cargo", [
    "test",
    "--manifest-path",
    "src-tauri/Cargo.toml",
    "--features",
    "headless",
    "--no-run",
    ...TEST_TARGETS.flatMap((target) => ["--test", target]),
  ]);

  const npmAuditProduction = runCommand(
    "npm-audit-prod",
    "npm 生产依赖审计",
    NPM,
    npmArgs(["audit", "--omit=dev", "--audit-level=high", `--registry=${REGISTRY}`]),
  );
  const npmAuditAll = runCommand(
    "npm-audit-full",
    "npm 完整依赖审计",
    NPM,
    npmArgs(["audit", "--audit-level=high", `--registry=${REGISTRY}`]),
  );
  const metadata = runCommand(
    "release-metadata",
    "发布元数据",
    NPM,
    npmArgs(["run", "verify:release-metadata"]),
  );

  const cargoAudit = (async () => {
    if (!ignoreIds) {
      return skip("cargo-audit", "RustSec 审计", "workflow ignore 列表不可用");
    }
    const probe = spawnSync("cargo", ["audit", "--version"], { encoding: "utf8" });
    if (probe.status !== 0) {
      const installed = await runCommand(
        "cargo-audit-install",
        "安装 cargo-audit",
        "cargo",
        ["install", "--locked", "cargo-audit"],
      );
      if (installed.status !== "pass") {
        return skip("cargo-audit", "RustSec 审计", "cargo-audit 安装失败");
      }
    }
    return runCommand("cargo-audit", "RustSec 审计", "cargo", [
      "audit",
      ...ignoreIds.flatMap((id) => ["--ignore", id]),
      "--file",
      "src-tauri/Cargo.lock",
    ]);
  })();

  const licenseChain = (async () => {
    const installed = await npmInstall;
    if (installed.status !== "pass") {
      await skip("legal", "许可证生成", "npm ci 失败");
      await skip("license-diff", "许可证一致性", "npm ci 失败");
      return;
    }
    if (!fs.existsSync(licensePath)) {
      await skip("license-diff", "许可证一致性", "缺少 licenses/THIRD_PARTY_LICENSES.html");
      return;
    }
    const snapshot = path.join(logDirectory, "THIRD_PARTY_LICENSES.before.html");
    fs.copyFileSync(licensePath, snapshot);
    const legal = await runCommand("legal", "许可证生成", NPM, npmArgs(["run", "legal"]));
    if (legal.status !== "pass") {
      await skip("license-diff", "许可证一致性", "许可证生成失败");
      return;
    }
    // 以运行前的文件为基准，而不是 git HEAD：本地预检常在工作区尚未提交时运行。
    runCheck("license-diff", "许可证一致性", () =>
      compareLicenseFreshness(snapshot, licensePath),
    );
  })();

  const testChain = (async () => {
    const built = await rustBuild;
    if (built.status !== "pass") {
      for (const target of TEST_TARGETS) {
        await skip(`test-${target}`, `测试 ${target}`, "Rust 预编译失败");
      }
      return;
    }
    const binary = await runCommand(
      "headless-bin",
      "headless 二进制",
      "cargo",
      [
        "build",
        "--manifest-path",
        "src-tauri/Cargo.toml",
        "--features",
        "headless",
        "--bin",
        "lilith-artworks",
      ],
    );
    if (binary.status !== "pass") {
      for (const target of TEST_TARGETS) {
        await skip(`test-${target}`, `测试 ${target}`, "headless 二进制构建失败");
      }
      return;
    }
    // 串行执行：stress_support 的报告与账本只有进程内锁，跨进程并行会交错。
    for (const target of TEST_TARGETS) {
      await runCommand(
        `test-${target}`,
        `测试 ${target}`,
        "cargo",
        [
          "test",
          "--manifest-path",
          "src-tauri/Cargo.toml",
          "--features",
          "headless",
          "--test",
          target,
        ],
      );
    }
  })();

  await Promise.all([
    npmInstall,
    rustBuild,
    npmAuditProduction,
    npmAuditAll,
    metadata,
    cargoAudit,
    licenseChain,
    testChain,
  ]);

  const totalMs = Date.now() - startedAt;
  const ordered = [...results].sort((left, right) => (left.startedAt ?? 0) - (right.startedAt ?? 0));
  const summary = [
    `本地 CI 汇总 — ${new Date().toISOString()}`,
    `总耗时 ${formatDuration(totalMs)}`,
    "",
    ...ordered.map((result) => {
      const status = result.status.toUpperCase().padEnd(4);
      const duration = result.status === "skip" ? "    -" : formatDuration(result.durationMs);
      const reason = result.status === "skip" ? `  ${result.reason}` : "";
      return `${status} ${duration.padStart(7)}  ${result.title}${reason}`;
    }),
    "",
  ].join("\n");
  emit(summary);
  fs.writeFileSync(path.join(logDirectory, "summary.txt"), `${summary}\n`);

  if (ordered.some((result) => result.status === "fail")) {
    process.exitCode = 1;
  }
}

process.on("SIGINT", () => {
  console.error("\n中断：正在结束子进程...");
  for (const child of children) {
    if (child.killed || !child.pid) continue;
    if (isWindows) {
      spawnSync("taskkill", ["/pid", String(child.pid), "/t", "/f"]);
    } else {
      child.kill("SIGTERM");
    }
  }
  process.exit(130);
});

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
