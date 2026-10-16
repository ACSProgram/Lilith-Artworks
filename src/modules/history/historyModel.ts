import type { ArtworkBranch, ArtworkHistory, HistoryNode } from "./types";

export interface HistoryTreeNode {
  node: HistoryNode;
  children: HistoryTreeNode[];
}

export function buildHistoryTree(nodes: HistoryNode[]): HistoryTreeNode[] {
  const byParent = new Map<string | null, HistoryNode[]>();
  for (const node of nodes) {
    byParent.set(node.parentId, [...(byParent.get(node.parentId) ?? []), node]);
  }
  const visit = (parentId: string | null): HistoryTreeNode[] =>
    (byParent.get(parentId) ?? [])
      .sort((left, right) => left.createdMs - right.createdMs)
      .map((node) => ({ node, children: visit(node.id) }));
  return visit(null);
}

export function buildBranchLine(nodes: HistoryNode[], headId: string | null): HistoryNode[] {
  const byId = new Map(nodes.map((node) => [node.id, node]));
  const result: HistoryNode[] = [];
  let cursor = headId;
  while (cursor) {
    const node = byId.get(cursor);
    if (!node) break;
    result.push(node);
    cursor = node.parentId;
  }
  return result.reverse();
}

export function branchesContainingNode(history: ArtworkHistory, nodeId: string): ArtworkBranch[] {
  const byId = new Map(history.nodes.map((node) => [node.id, node]));
  return history.branches.filter((branch) => {
    let cursor = branch.headHistoryId;
    while (cursor) {
      if (cursor === nodeId) return true;
      cursor = byId.get(cursor)?.parentId ?? null;
    }
    return false;
  });
}

const AUTOMATIC_TITLE = "自动备份";
const MANUAL_TITLE = "主动提交";
/** 与 `backup::worker::run_backup` 的 `MAX_TITLE_CHARS` 保持一致。 */
const MAX_TITLE_CHARS = 160;

/**
 * 判断节点标题是否已被人工改名。
 *
 * 节点标题只有两个写入点：提交时按 `note`/`commitKind` 生成默认标题
 * （`backup::worker::run_backup`），以及 `rename_history_node` 直接改写
 * `title`。把 `title` 与推导出的默认标题比对即可得出结论，不需要为此
 * 在 schema 或 DTO 里增加标记字段。
 *
 * 调度器写入自动备份时固定使用空备注，因此自动备份节点的默认标题恒为
 * 「自动备份」，改过名才可能不同。
 */
export function isRenamedNode(node: HistoryNode): boolean {
  const note = node.note.trim();
  const defaultTitle = note
    ? Array.from(note).slice(0, MAX_TITLE_CHARS).join("")
    : node.commitKind === "automatic" ? AUTOMATIC_TITLE : MANUAL_TITLE;
  return node.title !== defaultTitle;
}

/**
 * 精简会永久移除节点。自动备份产生的节点只是后台记录，而主动提交的节点、
 * 以及已被人工改名的节点承载了明确的人工意图，删除前需要更强的二次确认。
 */
export function needsCompactSecondConfirm(node: HistoryNode): boolean {
  return node.commitKind !== "automatic" || isRenamedNode(node);
}

export function canCompact(node: HistoryNode, history: ArtworkHistory): boolean {
  const children = history.nodes.filter((item) => item.parentId === node.id);
  return Boolean(
    node.parentId
      && children.length === 1
      && !node.isCheckpoint
      && !history.branches.some((branch) =>
        branch.headHistoryId === node.id || branch.createdFromHistoryId === node.id),
  );
}

export function isForcedCheckpoint(node: HistoryNode, history: ArtworkHistory): boolean {
  return history.nodes.filter((item) => item.parentId === node.id).length > 1
    || history.branches.some((branch) =>
      branch.headHistoryId === node.id || branch.createdFromHistoryId === node.id);
}

export function suggestedRestorePath(node: HistoryNode, branch: ArtworkBranch | null): string {
  const source = branch?.sourcePath ?? "";
  const match = source.match(/^(.*[\\/])?([^\\/]*)$/);
  const filename = match?.[2] || node.title;
  const extension = filename.match(/\.[^.]+$/)?.[0] ?? "";
  const stem = extension ? filename.slice(0, -extension.length) : filename;
  return `${match?.[1] ?? ""}${stem}_restored${extension}`;
}
