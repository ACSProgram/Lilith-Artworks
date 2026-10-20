import type { ArtworkBranch, ArtworkHistory, CheckMode, HistoryNode, NodeVerifyIssue } from "./types";

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

export function checkModeLabel(mode: CheckMode): string {
  switch (mode) {
    case "quick":
      return "快速检查命中";
    case "fallback":
      return "快速检查退回全量";
    case "full":
      return "全量检查";
  }
}

export function formatCheckElapsed(elapsedMs: number): string {
  const clamped = Math.max(0, elapsedMs);
  if (clamped < 1_000) return `${Math.round(clamped)} ms`;
  const tenths = Math.round(clamped / 100);
  // 按一位小数的显示值判断是否满一分钟，避免 59.96 秒显示成「60.0 秒」。
  if (tenths < 600) return `${(tenths / 10).toFixed(1)} 秒`;
  const totalSeconds = Math.round(clamped / 1_000);
  const minutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  return seconds ? `${minutes} 分 ${seconds} 秒` : `${minutes} 分`;
}

/**
 * 分支状态行里的「上次自动检查」说明。只有调度器成功完成过一次检查时才有值；
 * 手动提交不写检查方式与耗时，因此不会把人工操作说成自动检查。
 */
export function describeLastCheck(branch: ArtworkBranch): string | null {
  if (!branch.lastCheckMode) return null;
  const label = checkModeLabel(branch.lastCheckMode);
  const detail = branch.lastCheckElapsedMs == null
    ? label
    : `${label}（${formatCheckElapsed(branch.lastCheckElapsedMs)}）`;
  return `上次自动检查：${detail}`;
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

/**
 * 后台检查只改节点的校验状态，不改历史图结构。把新状态并回现有节点，且在没有
 * 变化时原样返回旧数组：上游据此保留 `nodes` 引用，mindmap 不重建、缩略图缓存
 * 也不作废。结构变化另有整图读取的路径，不走这里。
 */
export function mergeNodeVerifyStates(
  current: HistoryNode[],
  next: HistoryNode[],
): HistoryNode[] {
  if (current.length !== next.length) return next;
  const previous = new Map(current.map((node) => [node.id, node]));
  let changed = false;
  const merged = next.map((node) => {
    const before = previous.get(node.id);
    if (!before) {
      changed = true;
      return node;
    }
    if (before.verifiedMs === node.verifiedMs
      && before.verifyError === node.verifyError
      && before.verifyIssue === node.verifyIssue) {
      return before;
    }
    changed = true;
    return node;
  });
  return changed ? merged : current;
}

/**
 * 校验状态的标签用词。卡片、时间轴、节点信息栏与分支详情共用一套，避免同一个
 * 状态在不同地方有两种说法。
 */
export function verifyIssueLabel(issue: NodeVerifyIssue): string {
  switch (issue) {
    case "missing": return "文件丢失";
    case "damaged": return "文件损坏";
    case "unverifiable": return "无法检查";
  }
}

/**
 * 悬停与详情里的一句话说明。两个字段都只说后果，不用「校验」「摘要」这类术语。
 *
 * 「无法检查」的原因是**后续版本**的文件坏了：一个节点是由更新版本的快照一路
 * 回溯还原出来的，往回走的路断了就无从判断，而不是这个节点自己一定有问题。
 */
export function verifyIssueNotice(issue: NodeVerifyIssue): string {
  switch (issue) {
    case "missing": return "该节点文件丢失，这个历史版本可能无法恢复";
    case "damaged": return "该节点文件损坏，这个历史版本可能无法恢复";
    case "unverifiable": return "后续版本的文件已丢失或损坏，无法确认这个历史版本是否完好";
  }
}
