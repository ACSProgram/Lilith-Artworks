import { ChevronRight, FileImage, Folder, FolderOpen, Library, TriangleAlert } from "lucide-react";
import {
  useMemo,
  useRef,
  useState,
  type DragEvent as ReactDragEvent,
  type MouseEvent as ReactMouseEvent,
  type MutableRefObject,
} from "react";
import { flattenTree, orderedSelection } from "./tree";
import type { LibraryNode, MoveLibraryNodesRequest } from "./types";

type DropPosition = "before" | "inside" | "after";

interface DropTarget {
  id: string | null;
  position: DropPosition;
}

interface LibraryTreeViewProps {
  nodes: LibraryNode[];
  selectedIds: ReadonlySet<string>;
  disabled?: boolean;
  onSelect: (node: LibraryNode, event: ReactMouseEvent) => void;
  onMove: (request: MoveLibraryNodesRequest) => void;
  onContextMenu: (node: LibraryNode | null, event: ReactMouseEvent) => void;
  isExpanded: (id: string) => boolean;
  onToggleExpanded: (id: string) => void;
}

interface BranchProps extends LibraryTreeViewProps {
  rootNodes: LibraryNode[];
  depth: number;
  draggingIds: string[];
  draggingRef: MutableRefObject<string[]>;
  dropTarget: DropTarget | null;
  nodeById: Map<string, LibraryNode>;
  setDraggingIds: (ids: string[]) => void;
  setDropTarget: (target: DropTarget | null) => void;
}

function contains(node: LibraryNode, id: string): boolean {
  return node.id === id || node.children.some((child) => contains(child, id));
}

function positionFor(event: ReactDragEvent, node: LibraryNode): DropPosition {
  const bounds = event.currentTarget.getBoundingClientRect();
  const ratio = (event.clientY - bounds.top) / Math.max(bounds.height, 1);
  if (node.kind === "group") {
    if (ratio < 0.25) return "before";
    if (ratio > 0.75) return "after";
    return "inside";
  }
  return ratio < 0.5 ? "before" : "after";
}

function TreeBranch(props: BranchProps) {
  return (
    <div className="library-tree-branch" role="group">
      {props.nodes.map((node) => (
        <TreeRow key={node.id} {...props} node={node} siblings={props.nodes} />
      ))}
    </div>
  );
}

function TreeRow({
  node,
  siblings,
  depth,
  selectedIds,
  disabled,
  onSelect,
  onMove,
  onContextMenu,
  isExpanded,
  onToggleExpanded,
  draggingIds,
  draggingRef,
  dropTarget,
  nodeById,
  setDraggingIds,
  setDropTarget,
  ...branchProps
}: Omit<BranchProps, "nodes"> & { node: LibraryNode; siblings: LibraryNode[] }) {
  const expanded = node.kind === "group" && isExpanded(node.id);
  const selected = selectedIds.has(node.id);
  const targetPosition = dropTarget?.id === node.id ? dropTarget.position : null;

  const finishDrag = () => {
    draggingRef.current = [];
    setDraggingIds([]);
    setDropTarget(null);
  };

  const canDrop = () => draggingRef.current.length > 0 && draggingRef.current.every((sourceId) => {
    const source = nodeById.get(sourceId);
    return Boolean(source && !contains(source, node.id));
  });

  const moveForDrop = (event: ReactDragEvent) => {
    if (disabled || !canDrop()) return;
    event.preventDefault();
    event.stopPropagation();
    const position = positionFor(event, node);
    const dragged = new Set(draggingRef.current);
    if (position === "inside" && node.kind === "group") {
      onMove({
        ids: draggingRef.current,
        parentId: node.id,
        index: node.children.filter((child) => !dragged.has(child.id)).length,
      });
    } else {
      const ordered = siblings.filter((sibling) => !dragged.has(sibling.id));
      const targetIndex = ordered.findIndex((sibling) => sibling.id === node.id);
      onMove({
        ids: draggingRef.current,
        parentId: node.parentId,
        index: Math.max(0, targetIndex + (position === "after" ? 1 : 0)),
      });
    }
    finishDrag();
  };

  return (
    <div className="library-tree-node">
      <button
        className={`library-tree-row${selected ? " selected" : ""}${draggingIds.includes(node.id) ? " dragging" : ""}${targetPosition ? ` drop-${targetPosition}` : ""}`}
        style={{ paddingLeft: 9 + depth * 16 }}
        draggable={!disabled}
        data-library-node-id={node.id}
        role="treeitem"
        aria-selected={selected}
        aria-expanded={node.kind === "group" ? expanded : undefined}
        title={node.artwork?.primaryBranch?.sourcePath ?? node.title}
        onClick={(event) => {
          onSelect(node, event);
          if (node.kind === "group") onToggleExpanded(node.id);
        }}
        onContextMenu={(event) => {
          event.preventDefault();
          event.stopPropagation();
          onContextMenu(node, event);
        }}
        onDragStart={(event) => {
          if (disabled) return event.preventDefault();
          const ids = selectedIds.has(node.id) ? orderedSelection(branchProps.rootNodes, selectedIds) : [node.id];
          draggingRef.current = ids;
          setDraggingIds(ids);
          event.dataTransfer.effectAllowed = "move";
          event.dataTransfer.setData("application/x-lilith-artworks-nodes", ids.join("\n"));
          event.dataTransfer.setData("text/plain", node.title);
        }}
        onDragEnd={finishDrag}
        onDragOver={(event) => {
          if (disabled || !canDrop()) return;
          event.preventDefault();
          event.stopPropagation();
          event.dataTransfer.dropEffect = "move";
          setDropTarget({ id: node.id, position: positionFor(event, node) });
        }}
        onDrop={moveForDrop}
      >
        {node.kind === "group" ? (
          <>
            <span
              className={`tree-chevron${expanded ? " open" : ""}`}
              role="button"
              tabIndex={-1}
              onClick={(event) => {
                event.stopPropagation();
                onToggleExpanded(node.id);
              }}
            >
              <ChevronRight aria-hidden="true" size={14} />
            </span>
            {expanded ? <FolderOpen aria-hidden="true" size={16} /> : <Folder aria-hidden="true" size={16} />}
          </>
        ) : (
          <>
            <span className="tree-chevron-spacer" />
            <FileImage aria-hidden="true" size={16} />
          </>
        )}
        <span className="tree-row-title">{node.title}</span>
        {node.kind === "artwork" && <small className={(node.artwork?.backupDisableNoticeCount ?? 0) > 0 ? "backup-warning" : ""}>
          {(node.artwork?.backupDisableNoticeCount ?? 0) > 0 && <TriangleAlert aria-hidden="true" size={12} />}
          {node.artwork?.branchCount ?? 0}
        </small>}
      </button>
      {expanded && node.children.length > 0 && (
        <TreeBranch
          {...branchProps}
          nodes={node.children}
          depth={depth + 1}
          selectedIds={selectedIds}
          disabled={disabled}
          onSelect={onSelect}
          onMove={onMove}
          onContextMenu={onContextMenu}
          isExpanded={isExpanded}
          onToggleExpanded={onToggleExpanded}
          draggingIds={draggingIds}
          draggingRef={draggingRef}
          dropTarget={dropTarget}
          nodeById={nodeById}
          setDraggingIds={setDraggingIds}
          setDropTarget={setDropTarget}
        />
      )}
    </div>
  );
}

export function LibraryTreeView(props: LibraryTreeViewProps) {
  const [draggingIds, setDraggingIds] = useState<string[]>([]);
  const [dropTarget, setDropTarget] = useState<DropTarget | null>(null);
  const draggingRef = useRef<string[]>([]);
  const flattened = useMemo(() => flattenTree(props.nodes), [props.nodes]);
  const nodeById = useMemo(() => new Map(flattened.map((node) => [node.id, node])), [flattened]);

  return (
    <div className="library-tree" role="tree" aria-multiselectable="true">
      <div
        className={`library-tree-root${dropTarget?.id === null ? " drop-inside" : ""}`}
        role="treeitem"
        title="右键新建分组或 Artwork；拖到这里移动到根级"
        onDragOver={(event) => {
          if (!draggingRef.current.length || props.disabled) return;
          event.preventDefault();
          event.dataTransfer.dropEffect = "move";
          setDropTarget({ id: null, position: "inside" });
        }}
        onDrop={(event) => {
          if (!draggingRef.current.length || props.disabled) return;
          event.preventDefault();
          const dragged = new Set(draggingRef.current);
          props.onMove({
            ids: draggingRef.current,
            parentId: null,
            index: props.nodes.filter((node) => !dragged.has(node.id)).length,
          });
          draggingRef.current = [];
          setDraggingIds([]);
          setDropTarget(null);
        }}
        onContextMenu={(event) => props.onContextMenu(null, event)}
      >
        <Library aria-hidden="true" size={16} />
        <span>全部作品</span>
      </div>
      <TreeBranch
        {...props}
        rootNodes={props.nodes}
        depth={0}
        draggingIds={draggingIds}
        draggingRef={draggingRef}
        dropTarget={dropTarget}
        nodeById={nodeById}
        setDraggingIds={setDraggingIds}
        setDropTarget={setDropTarget}
      />
      {props.nodes.length === 0 && <div className="tree-empty">右键或使用新建菜单添加作品</div>}
    </div>
  );
}
