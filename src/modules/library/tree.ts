import type { LibraryNode } from "./types";

export interface VisibleLibraryNode {
  node: LibraryNode;
  depth: number;
}

export function flattenTree(nodes: LibraryNode[]): LibraryNode[] {
  return nodes.flatMap((node) => [node, ...flattenTree(node.children)]);
}

export function visibleTree(
  nodes: LibraryNode[],
  expandedIds: ReadonlySet<string>,
  depth = 0,
): VisibleLibraryNode[] {
  return nodes.flatMap((node) => [
    { node, depth },
    ...(node.kind === "group" && expandedIds.has(node.id)
      ? visibleTree(node.children, expandedIds, depth + 1)
      : []),
  ]);
}

export function selectionForClick(
  visibleIds: string[],
  currentIds: ReadonlySet<string>,
  anchorId: string | null,
  clickedId: string,
  extend: boolean,
  toggle: boolean,
): { ids: Set<string>; anchorId: string } {
  if (extend && anchorId) {
    const anchorIndex = visibleIds.indexOf(anchorId);
    const clickedIndex = visibleIds.indexOf(clickedId);
    if (anchorIndex >= 0 && clickedIndex >= 0) {
      const start = Math.min(anchorIndex, clickedIndex);
      const end = Math.max(anchorIndex, clickedIndex);
      return { ids: new Set(visibleIds.slice(start, end + 1)), anchorId };
    }
  }
  if (toggle) {
    const ids = new Set(currentIds);
    if (ids.has(clickedId)) ids.delete(clickedId);
    else ids.add(clickedId);
    return { ids, anchorId: clickedId };
  }
  return { ids: new Set([clickedId]), anchorId: clickedId };
}

export function orderedSelection(
  nodes: LibraryNode[],
  selectedIds: ReadonlySet<string>,
): string[] {
  return flattenTree(nodes)
    .map((node) => node.id)
    .filter((id) => selectedIds.has(id));
}

export function containsNode(node: LibraryNode, id: string): boolean {
  return node.id === id || node.children.some((child) => containsNode(child, id));
}

