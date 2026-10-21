import { describe, expect, it } from "vitest";
import type { ArtworkBranch, HistoryNode } from "./types";
import {
  buildBranchLine, canCompact, checkModeLabel, describeCheckRun, formatCheckElapsed, formatClockTime,
  isRenamedNode, mergeNodeVerifyStates, needsCompactSecondConfirm, suggestedRestorePath,
} from "./historyModel";

const node = (
  id: string,
  parentId: string | null,
  createdMs: number,
  overrides: Partial<HistoryNode> = {},
): HistoryNode => ({
  id,
  createdOnBranchId: "branch",
  parentId,
  title: id,
  note: "",
  commitKind: "manual",
  isCheckpoint: false,
  createdMs,
  logicalSize: 1,
  chunkFileSize: 1,
  sha256: id.padEnd(64, "0"),
  chunkCount: 1,
  verifiedMs: null,
  verifyError: null,
  verifyIssue: null,
  ...overrides,
});

const branch: ArtworkBranch = {
  id: "branch",
  title: "Main",
  sourcePath: "C:/art/source.psd",
  headHistoryId: "child",
  createdFromHistoryId: null,
  backupEnabled: true,
  backupIntervalMinutes: 5,
  backupQuickEnabled: false,
  lastCheckMs: null,
  lastSuccessMs: null,
  lastCheckElapsedMs: null,
  lastCheckMode: null,
  lastError: null,
  consecutiveBackupFailures: 0,
  backupRetryAtMs: null,
  backupDisableNoticePending: false,
  finalArtifactLocked: false,
  publishedCount: 0,
  verifyError: null,
  verifyFailureCount: 0,
  verifyUnverifiableCount: 0,
  verifyIssue: null,
  verifiedMs: null,
};

describe("history model", () => {
  const nodes = [node("root", null, 1), node("middle", "root", 2), node("child", "middle", 3)];

  it("builds a branch line from root to head", () => {
    expect(buildBranchLine(nodes, "child").map((item) => item.id)).toEqual(["root", "middle", "child"]);
  });

  it("allows only an unreferenced linear middle node to compact", () => {
    const history = { artworkId: "art", artworkTitle: "Art", branches: [branch], nodes };
    expect(canCompact(nodes[1], history)).toBe(true);
    expect(canCompact(nodes[2], history)).toBe(false);
  });

  it("preserves the working file extension in restore suggestions", () => {
    expect(suggestedRestorePath(nodes[2], branch)).toBe("C:/art/source_restored.psd");
  });

  it("describes the last automatic check with its mode and elapsed time", () => {
    expect(describeCheckRun(branch)).toBeNull();
    expect(describeCheckRun({ ...branch, lastCheckMode: "quick", lastCheckElapsedMs: 12 }))
      .toBe("快速检查命中 12 ms");
    expect(describeCheckRun({ ...branch, lastCheckMode: "fallback", lastCheckElapsedMs: 65_000 }))
      .toBe("快速检查退回全量 1 分 5 秒");
    expect(describeCheckRun({ ...branch, lastCheckMode: "full", lastCheckElapsedMs: 1_500 }))
      .toBe("全量检查 1.5 秒");
    expect(describeCheckRun({ ...branch, lastCheckMode: "full" })).toBe("全量检查");
  });

  it("keeps status times short: clock today, month and day across days", () => {
    const now = new Date(2026, 9, 21, 18, 0, 0).getTime();
    expect(formatClockTime(new Date(2026, 9, 21, 16, 48, 51).getTime(), now)).toBe("16:48");
    expect(formatClockTime(new Date(2026, 9, 20, 9, 5, 0).getTime(), now)).toBe("10/20 09:05");
    expect(formatClockTime(new Date(2025, 11, 31, 23, 59, 0).getTime(), now))
      .toBe("2025/12/31 23:59");
  });

  it("formats elapsed times at human scales", () => {
    expect(formatCheckElapsed(0)).toBe("0 ms");
    expect(formatCheckElapsed(999)).toBe("999 ms");
    expect(formatCheckElapsed(1_000)).toBe("1.0 秒");
    expect(formatCheckElapsed(59_940)).toBe("59.9 秒");
    expect(formatCheckElapsed(59_960)).toBe("1 分");
    expect(formatCheckElapsed(60_000)).toBe("1 分");
    expect(formatCheckElapsed(125_000)).toBe("2 分 5 秒");
    expect(checkModeLabel("quick")).toBe("快速检查命中");
    expect(checkModeLabel("full")).toBe("全量检查");
    expect(checkModeLabel("fallback")).toBe("快速检查退回全量");
  });

  it("reports a node as renamed only when its title left the generated default", () => {
    const automatic = node("auto", "root", 2, { title: "自动备份", commitKind: "automatic" });
    expect(isRenamedNode(automatic)).toBe(false);
    expect(isRenamedNode({ ...automatic, title: "发布前定稿" })).toBe(true);

    const manual = node("manual", "root", 3, { title: "主动提交" });
    expect(isRenamedNode(manual)).toBe(false);
    expect(isRenamedNode({ ...manual, note: "改肤色", title: "改肤色" })).toBe(false);
    expect(isRenamedNode({ ...manual, note: "改肤色", title: "改肤色 v2" })).toBe(true);
  });

  it("keeps the node array identity when a background check changed nothing", () => {
    const current = [node("a", null, 1, { verifiedMs: 100 }), node("b", "a", 2)];
    // 后台检查通过：只有带时间戳的字段变了，其余字段沿用旧对象。
    const next = [node("a", null, 1, { verifiedMs: 200 }), node("b", "a", 2)];
    const merged = mergeNodeVerifyStates(current, next);
    expect(merged).not.toBe(current);
    expect(merged[0]).toBe(next[0]);
    expect(merged[1]).toBe(current[1]);

    // 完全没变时必须原样返回，调用方据此保住 mindmap 与缩略图缓存。
    expect(mergeNodeVerifyStates(current, current.map((item) => ({ ...item })))).toBe(current);
  });

  it("takes the whole array when the node set changed", () => {
    const current = [node("a", null, 1)];
    const next = [node("a", null, 1), node("b", "a", 2)];
    expect(mergeNodeVerifyStates(current, next)).toBe(next);
  });

  it("asks for a stronger confirmation for manual or renamed nodes", () => {
    const automatic = node("auto", "root", 2, { title: "自动备份", commitKind: "automatic" });
    expect(needsCompactSecondConfirm(automatic)).toBe(false);
    expect(needsCompactSecondConfirm({ ...automatic, title: "自动备份 v2" })).toBe(true);
    expect(needsCompactSecondConfirm(node("manual", "root", 3, { title: "主动提交" }))).toBe(true);
  });
});
