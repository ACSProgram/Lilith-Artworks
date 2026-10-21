import { useEffect, useState } from "react";
import {
  Images,
  LoaderCircle,
  Pencil,
  Plus,
  Trash2,
  Undo2,
} from "lucide-react";
import { ContextMenu } from "../../shared/ui/ContextMenu";
import type { BoardDropPosition } from "./boardOrder";
import type { PinBoardSummary } from "./types";

/** 侧栏画板拖放排序使用的自定义 dataTransfer 类型。 */
const PIN_BOARD_DRAG_TYPE = "application/x-lilith-pin-board-board";

export interface PinBoardSidebarProps {
  /** 工作区 keep-mounted：离开时收起列表菜单，不在后台留着浮层。 */
  active: boolean;
  boards: PinBoardSummary[] | null;
  trash: PinBoardSummary[];
  selected: number | null;
  /** 整画布导出期间：画板行与拖放一并停用。 */
  exporting: boolean;
  /** 传输进行中：新建与拖放排序都要等它结束。 */
  transferBusy: boolean;
  /** 打开侧栏菜单前先关掉画布菜单，避免两个菜单叠在一起。 */
  onDismissCanvasMenu: () => void;
  onCreate: () => void;
  onSelect: (board: PinBoardSummary) => void;
  onMoveBoard: (draggedId: number, targetId: number, position: BoardDropPosition) => void;
  onRename: (board: PinBoardSummary) => void;
  onTrash: (board: PinBoardSummary) => void;
  onRestore: (boardId: number) => void;
  onPermanentDelete: (board: PinBoardSummary) => void;
  onEmptyTrash: () => void;
}

/** 画板列表、回收站与拖放排序；列表右键菜单也归这里管理。 */
export function PinBoardSidebar({
  active,
  boards,
  trash,
  selected,
  exporting,
  transferBusy,
  onDismissCanvasMenu,
  onCreate,
  onSelect,
  onMoveBoard,
  onRename,
  onTrash,
  onRestore,
  onPermanentDelete,
  onEmptyTrash,
}: PinBoardSidebarProps) {
  const [listContextMenu, setListContextMenu] = useState<{
    board: PinBoardSummary | null;
    trash: boolean;
    x: number;
    y: number;
  } | null>(null);
  /** 正在拖动的画板与当前落点。 */
  const [draggingBoardId, setDraggingBoardId] = useState<number | null>(null);
  const [boardDropTarget, setBoardDropTarget] = useState<
    { boardId: number; position: BoardDropPosition } | null
  >(null);

  useEffect(() => {
    if (!active) setListContextMenu(null);
  }, [active]);

  useEffect(() => {
    if (!listContextMenu) return undefined;
    const close = () => setListContextMenu(null);
    const keyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") close();
    };
    window.addEventListener("pointerdown", close);
    window.addEventListener("blur", close);
    window.addEventListener("keydown", keyDown);
    return () => {
      window.removeEventListener("pointerdown", close);
      window.removeEventListener("blur", close);
      window.removeEventListener("keydown", keyDown);
    };
  }, [listContextMenu]);

  const openMenu = (menu: {
    board: PinBoardSummary | null;
    trash: boolean;
    x: number;
    y: number;
  }) => {
    onDismissCanvasMenu();
    setListContextMenu(menu);
  };

  return (
    <aside className="pin-board-sidebar">
      <header>
        <div>
          <strong>素材板</strong>
          <small>
            {boards
              ? `${boards.length} 个画板`
              : "正在载入"}
          </small>
        </div>
        <button
          className="icon-button"
          type="button"
          title="新建画板"
          aria-label="新建画板"
          disabled={transferBusy}
          onClick={onCreate}
        >
          <Plus size={16} />
        </button>
      </header>
      <div className="pin-board-tree-scroll">
        {boards
          ? <div className="pin-board-board-list">
            {boards.map((board) => {
              const dragging = draggingBoardId === board.boardId;
              const dropPosition = boardDropTarget?.boardId === board.boardId
                ? boardDropTarget.position
                : null;
              return (
                <button
                  type="button"
                  key={board.boardId}
                  className={`pin-board-tree-row${board.boardId === selected ? " active" : ""}${dragging ? " dragging" : ""}${dropPosition ? ` drop-${dropPosition}` : ""}`}
                  draggable={boards.length > 1 && !transferBusy && !exporting}
                  disabled={exporting}
                  onClick={() => {
                    onDismissCanvasMenu();
                    onSelect(board);
                  }}
                  onContextMenu={(event) => {
                    event.preventDefault();
                    openMenu({ board, trash: false, x: event.clientX, y: event.clientY });
                  }}
                  onDragStart={(event) => {
                    onDismissCanvasMenu();
                    setListContextMenu(null);
                    setDraggingBoardId(board.boardId);
                    setBoardDropTarget(null);
                    event.dataTransfer.effectAllowed = "move";
                    event.dataTransfer.setData(PIN_BOARD_DRAG_TYPE, String(board.boardId));
                    event.dataTransfer.setData("text/plain", board.name);
                  }}
                  onDragOver={(event) => {
                    if (draggingBoardId === null || draggingBoardId === board.boardId) return;
                    event.preventDefault();
                    event.dataTransfer.dropEffect = "move";
                    const rect = event.currentTarget.getBoundingClientRect();
                    const position: BoardDropPosition =
                      event.clientY < rect.top + rect.height / 2 ? "before" : "after";
                    setBoardDropTarget((current) => (
                      current?.boardId === board.boardId && current.position === position
                        ? current
                        : { boardId: board.boardId, position }
                    ));
                  }}
                  onDrop={(event) => {
                    event.preventDefault();
                    const draggedId =
                      Number(event.dataTransfer.getData(PIN_BOARD_DRAG_TYPE)) || draggingBoardId;
                    const position: BoardDropPosition = boardDropTarget?.boardId === board.boardId
                      ? boardDropTarget.position
                      : "after";
                    setDraggingBoardId(null);
                    setBoardDropTarget(null);
                    if (draggedId === null || draggedId === board.boardId) return;
                    onMoveBoard(draggedId, board.boardId, position);
                  }}
                  onDragEnd={() => {
                    setDraggingBoardId(null);
                    setBoardDropTarget(null);
                  }}
                  title={boards.length > 1 ? `拖动可调整顺序：${board.name}` : board.name}
                >
                  <Images size={16} />
                  <span>{board.name}</span>
                </button>
              );
            })}
            {boards.length === 0 && (
              <div className="pin-board-tree-empty">尚无画板，点击右上角“新建画板”</div>
            )}
          </div>
          : <LoaderCircle className="spin" size={20} />}
      </div>
      <div className="pin-board-trash-section">
        <div className="pin-board-trash-divider" />
        <div
          className="pin-board-trash-heading"
          onContextMenu={(event) => {
            event.preventDefault();
            openMenu({ board: null, trash: true, x: event.clientX, y: event.clientY });
          }}
        >
          <Trash2 size={16} />
          <span>回收站</span>
          <small>{trash.length}</small>
          {trash.length > 0 && (
            <button
              type="button"
              className="icon-button"
              title="清空回收站"
              aria-label="清空回收站"
              onClick={onEmptyTrash}
            >
              <Trash2 size={14} />
            </button>
          )}
        </div>
        <div className="pin-board-trash-list">
          {trash.map((board) => (
            <button
              type="button"
              className="pin-board-tree-row trash"
              key={board.boardId}
              onContextMenu={(event) => {
                event.preventDefault();
                event.stopPropagation();
                openMenu({ board, trash: true, x: event.clientX, y: event.clientY });
              }}
              title={`来自作品“${board.artworkTitle}”；右键恢复或永久删除`}
            >
              <Images size={16} />
              <span>{board.name}</span>
            </button>
          ))}
          {trash.length === 0 && <div className="pin-board-trash-empty">回收站为空</div>}
        </div>
      </div>
      {listContextMenu && (
        <ContextMenu
          x={listContextMenu.x}
          y={listContextMenu.y}
          items={[
            {
              key: "heading",
              heading: true,
              label: listContextMenu.trash
                ? "画板回收站"
                : listContextMenu.board?.name ?? "素材板",
              icon: listContextMenu.trash ? <Trash2 size={15} /> : <Images size={15} />,
            },
            ...(listContextMenu.trash
              ? listContextMenu.board
                ? [
                    { key: "restore", label: "恢复到原作品", icon: <Undo2 size={15} />, onClick: () => {
                      const board = listContextMenu.board!;
                      setListContextMenu(null);
                      onRestore(board.boardId);
                    } },
                    { key: "permanent-delete", label: "永久删除", icon: <Trash2 size={15} />, danger: true, onClick: () => {
                      onPermanentDelete(listContextMenu.board!);
                      setListContextMenu(null);
                    } },
                  ]
                : [
                    { key: "empty-trash", label: "清空回收站", icon: <Trash2 size={15} />, disabled: trash.length === 0, onClick: () => {
                      onEmptyTrash();
                      setListContextMenu(null);
                    } },
                  ]
              : listContextMenu.board
                ? [
                    { key: "rename", label: "重命名", icon: <Pencil size={15} />, onClick: () => {
                      onRename(listContextMenu.board!);
                      setListContextMenu(null);
                    } },
                    { key: "trash", label: "移入回收站", icon: <Trash2 size={15} />, danger: true, onClick: () => {
                      onTrash(listContextMenu.board!);
                      setListContextMenu(null);
                    } },
                  ]
                : [
                    { key: "create", label: "新建画板", icon: <Plus size={15} />, onClick: () => {
                      onCreate();
                      setListContextMenu(null);
                    } },
                  ]),
          ]}
          onClose={() => setListContextMenu(null)}
        />
      )}
    </aside>
  );
}
