import { describe, expect, it } from "vitest";
import type { ArtworkBranch, HistoryNode } from "./types";
import {
  buildBranchLine, canCompact, isRenamedNode, needsCompactSecondConfirm, suggestedRestorePath,
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
  lastError: null,
  consecutiveBackupFailures: 0,
  backupRetryAtMs: null,
  backupDisableNoticePending: false,
  finalArtifactLocked: false,
  publishedCount: 0,
  verifyError: null,
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

  it("reports a node as renamed only when its title left the generated default", () => {
    const automatic = node("auto", "root", 2, { title: "自动备份", commitKind: "automatic" });
    expect(isRenamedNode(automatic)).toBe(false);
    expect(isRenamedNode({ ...automatic, title: "发布前定稿" })).toBe(true);

    const manual = node("manual", "root", 3, { title: "主动提交" });
    expect(isRenamedNode(manual)).toBe(false);
    expect(isRenamedNode({ ...manual, note: "改肤色", title: "改肤色" })).toBe(false);
    expect(isRenamedNode({ ...manual, note: "改肤色", title: "改肤色 v2" })).toBe(true);
  });

  it("asks for a stronger confirmation for manual or renamed nodes", () => {
    const automatic = node("auto", "root", 2, { title: "自动备份", commitKind: "automatic" });
    expect(needsCompactSecondConfirm(automatic)).toBe(false);
    expect(needsCompactSecondConfirm({ ...automatic, title: "自动备份 v2" })).toBe(true);
    expect(needsCompactSecondConfirm(node("manual", "root", 3, { title: "主动提交" }))).toBe(true);
  });
});
