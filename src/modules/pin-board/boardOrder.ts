export type BoardDropPosition = "before" | "after";

/**
 * 计算拖放后的画板 id 顺序：把 `draggedId` 插到 `targetId` 的前或后。
 * 返回 `null` 表示顺序没有变化（同位置、id 不在列表中或拖到自己身上）。
 */
export function reorderBoardIds(
  boardIds: number[],
  draggedId: number,
  targetId: number,
  position: BoardDropPosition,
): number[] | null {
  if (draggedId === targetId || !boardIds.includes(draggedId)) return null;
  const withoutDragged = boardIds.filter((boardId) => boardId !== draggedId);
  const targetIndex = withoutDragged.indexOf(targetId);
  if (targetIndex < 0) return null;
  const insertAt = position === "before" ? targetIndex : targetIndex + 1;
  const next = [
    ...withoutDragged.slice(0, insertAt),
    draggedId,
    ...withoutDragged.slice(insertAt),
  ];
  return next.every((boardId, index) => boardId === boardIds[index]) ? null : next;
}
