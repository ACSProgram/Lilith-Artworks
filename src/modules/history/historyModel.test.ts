import { describe, expect, it } from "vitest";
import type { ArtworkBranch, HistoryNode } from "./types";
import { buildBranchLine, canCompact, suggestedRestorePath } from "./historyModel";

const node = (id: string, parentId: string | null, createdMs: number): HistoryNode => ({
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
});

const branch: ArtworkBranch = {
  id: "branch",
  title: "Main",
  sourcePath: "C:/art/source.psd",
  headHistoryId: "child",
  createdFromHistoryId: null,
  backupEnabled: true,
  backupIntervalMinutes: 5,
  lastCheckMs: null,
  lastSuccessMs: null,
  lastError: null,
  finalArtifactLocked: false,
  publishedCount: 0,
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
});
