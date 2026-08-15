import { describe, expect, it } from "vitest";
import type { LibraryNode } from "./types";
import { orderedSelection, selectionForClick, visibleTree } from "./tree";

const node = (id: string, kind: LibraryNode["kind"], children: LibraryNode[] = []): LibraryNode => ({
  id,
  parentId: null,
  kind,
  title: id,
  position: 0,
  updatedMs: 0,
  children,
  artwork: null,
});

describe("library tree", () => {
  const nodes = [node("group", "group", [node("a", "artwork"), node("b", "artwork")])];

  it("only exposes children of expanded groups", () => {
    expect(visibleTree(nodes, new Set()).map(({ node: item }) => item.id)).toEqual(["group"]);
    expect(visibleTree(nodes, new Set(["group"])).map(({ node: item }) => item.id)).toEqual(["group", "a", "b"]);
  });

  it("keeps range and command selections in tree order", () => {
    const selected = selectionForClick(["group", "a", "b"], new Set(), "group", "b", true, false);
    expect(orderedSelection(nodes, selected.ids)).toEqual(["group", "a", "b"]);
  });
});
