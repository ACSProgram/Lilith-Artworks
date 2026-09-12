export interface PinBoardViewSession {
  centerX: number;
  centerY: number;
  worldUnitsPerCssPixel: number;
  locked: boolean;
  selectedImageIds: number[];
}

/**
 * 进程内画板会话：跨工作区视图切换保留，不承诺跨应用重启恢复。
 * 仓库/作品以 id 为键，切换作品时模块随工作区整体卸载。
 */
const boardSessions = new Map<string, PinBoardViewSession>();
const selectedBoardIds = new Map<string, number>();
let sidebarVisible = true;

function boardKey(artworkId: string, boardId: number): string {
  return `${artworkId}\u0000${boardId}`;
}

export function getSelectedBoardId(artworkId: string): number | null {
  return selectedBoardIds.get(artworkId) ?? null;
}

export function setSelectedBoardId(artworkId: string, boardId: number) {
  selectedBoardIds.set(artworkId, boardId);
}

export function getSidebarVisible(): boolean {
  return sidebarVisible;
}

export function setSidebarVisible(visible: boolean) {
  sidebarVisible = visible;
}

export function getBoardSession(artworkId: string, boardId: number): PinBoardViewSession | null {
  const session = boardSessions.get(boardKey(artworkId, boardId));
  return session ? {
    centerX: session.centerX,
    centerY: session.centerY,
    worldUnitsPerCssPixel: session.worldUnitsPerCssPixel,
    locked: session.locked,
    selectedImageIds: [...session.selectedImageIds],
  } : null;
}

export function setBoardSession(
  artworkId: string,
  boardId: number,
  session: PinBoardViewSession,
) {
  boardSessions.set(boardKey(artworkId, boardId), {
    centerX: session.centerX,
    centerY: session.centerY,
    worldUnitsPerCssPixel: session.worldUnitsPerCssPixel,
    locked: session.locked,
    selectedImageIds: [...session.selectedImageIds],
  });
}

export function dropArtworkSessions(artworkId: string) {
  boardSessions.delete(artworkId);
  selectedBoardIds.delete(artworkId);
}
