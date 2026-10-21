import {
  normalizeLayerVisibility,
  type PinBoardLayerVisibility,
} from "./layers";

export interface PinBoardViewSession {
  centerX: number;
  centerY: number;
  worldUnitsPerCssPixel: number;
  locked: boolean;
  selectedImageIds: number[];
  /** 图层可见性：纯视图状态，随会话保留，不进 schema、不写库。 */
  visibleLayers: PinBoardLayerVisibility;
}

/**
 * 进程内画板会话：跨工作区视图切换保留，不承诺跨应用重启恢复。
 * 仓库/作品以 id 为键，切换作品时模块随工作区整体卸载。
 *
 * 会话只按需增长、不做淘汰：每条只有几个标量，规模受「本次运行访问过的画板数」
 * 约束，因此不需要（也没有）按作品清理的入口。
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
    visibleLayers: normalizeLayerVisibility(session.visibleLayers),
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
    visibleLayers: normalizeLayerVisibility(session.visibleLayers),
  });
}
