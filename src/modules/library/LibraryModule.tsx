import {
  FileImage,
  FolderPlus,
  LoaderCircle,
  ChevronsUp,
  Pencil,
  Plus,
  Search,
  Settings,
  Trash2,
  X,
} from "lucide-react";
import {
  Fragment,
  useEffect,
  useMemo,
  useState,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
} from "react";
import type { CleanupReport } from "../../shared/fileCleanup";
import { flattenTree, selectionForClick, visibleTree } from "./tree";
import { NodeEditor, TrashDialog } from "./LibraryDialogs";
import type { EditorState } from "./LibraryDialogs";
import { LibraryTreeView } from "./LibraryTreeView";
import { CommandMenu, NodeOverview, WorkspaceEmpty } from "./LibraryViews";
import type { LibraryNode, LibrarySearchResult, MoveLibraryNodesRequest } from "./types";
import { useLibraryController } from "./useLibraryController";

interface LibraryModuleProps {
  repositoryReady: boolean;
  onConfigure: () => void;
  onError: (message: string | null) => void;
  onRetryFileCleanup: (ids: string[]) => Promise<CleanupReport>;
  renderArtworkWorkspace: (props: LibraryArtworkWorkspaceProps) => ReactNode;
}

export interface ArtworkTraceTarget {
  artworkId: string;
  branchId: string;
  recordId: string;
}

export interface LibraryArtworkWorkspaceProps {
  artworkId: string;
  initialView: "history" | "publish";
  initialBranchId: string | null;
  initialRecordId: string | null;
  onNavigateRecord: (target: ArtworkTraceTarget) => void;
}

interface ContextState {
  node: LibraryNode | null;
  x: number;
  y: number;
}

export function LibraryModule({ repositoryReady, onConfigure, onError, onRetryFileCleanup, renderArtworkWorkspace }: LibraryModuleProps) {
  const controller = useLibraryController({ repositoryReady, onError, onRetryFileCleanup });
  const {
    tree, loading, operationBusy, expandedIds, setExpandedIds, selectedIds, setSelectedIds,
    anchorId, setAnchorId, activeId, setActiveId, query, setQuery, searchResults,
    setSearchResults, searching, trashEntries, cleanupFailures, retryCleanup, loadTrash,
    createGroup, createArtwork, renameNode, trashNodes, moveNodes, restoreTrash,
    permanentlyDeleteTrash, emptyTrash,
  } = controller;
  const [newMenuOpen, setNewMenuOpen] = useState(false);
  const [context, setContext] = useState<ContextState | null>(null);
  const [editor, setEditor] = useState<EditorState | null>(null);
  const [trashOpen, setTrashOpen] = useState(false);
  const [traceTarget, setTraceTarget] = useState<{ artworkId: string; branchId: string; recordId: string } | null>(null);

  const allNodes = useMemo(() => flattenTree(tree.nodes), [tree.nodes]);

  const nodeById = useMemo(
    () => new Map(allNodes.map((node) => [node.id, node])),
    [allNodes],
  );
  const visibleIds = useMemo(
    () => visibleTree(tree.nodes, expandedIds).map(({ node }) => node.id),
    [expandedIds, tree.nodes],
  );
  const activeNode = activeId ? nodeById.get(activeId) ?? null : null;

  useEffect(() => {
    if (!context && !newMenuOpen) return;
    const close = () => {
      setContext(null);
      setNewMenuOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") close();
    };
    window.addEventListener("pointerdown", close);
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("pointerdown", close);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [context, newMenuOpen]);

  const creationParent = (node: LibraryNode | null): string | null => {
    if (!node) return null;
    return node.kind === "group" ? node.id : node.parentId;
  };

  const selectedForNode = (node: LibraryNode): string[] =>
    selectedIds.has(node.id) ? [...selectedIds] : [node.id];

  const openContext = (node: LibraryNode | null, event: ReactMouseEvent) => {
    event.preventDefault();
    event.stopPropagation();
    if (node && !selectedIds.has(node.id)) {
      setSelectedIds(new Set([node.id]));
      setAnchorId(node.id);
      setActiveId(node.id);
    }
    setNewMenuOpen(false);
    setContext({
      node,
      x: Math.min(event.clientX, window.innerWidth - 210),
      y: Math.min(event.clientY, window.innerHeight - 240),
    });
  };

  const deleteSelection = async (node: LibraryNode) => {
    const ids = selectedForNode(node);
    const label = ids.length === 1 ? `“${node.title}”` : `选中的 ${ids.length} 个节点`;
    if (!window.confirm(`将${label}及其全部内容移到回收站？`)) return;
    setContext(null);
    try {
      await trashNodes(ids);
      setSelectedIds(new Set());
      setActiveId(null);
    } catch {
      // Error is already surfaced by runMutation.
    }
  };

  const openTrash = async () => {
    try {
      await loadTrash();
      setTrashOpen(true);
    } catch { /* Error is surfaced by the controller. */ }
  };

  const selectSearchResult = (result: LibrarySearchResult) => {
    setExpandedIds((current) => new Set([...current, ...result.ancestorIds]));
    setSelectedIds(new Set([result.id]));
    setAnchorId(result.id);
    setActiveId(result.id);
    setTraceTarget(null);
    setQuery("");
    setSearchResults([]);
  };

  return (
    <Fragment>
      <aside className="library-sidebar">
        <div className="sidebar-heading">
          <div>
            <span>作品库</span>
            <small>{tree.artworkCount}</small>
          </div>
          <div className="sidebar-actions">
            <div className="menu-anchor">
              <button
                className="icon-button"
                type="button"
                title="新建"
                disabled={!repositoryReady || operationBusy}
                onPointerDown={(event) => event.stopPropagation()}
                onClick={() => setNewMenuOpen((current) => !current)}
              >
                <Plus aria-hidden="true" size={18} />
              </button>
              {newMenuOpen && (
                <CommandMenu
                  className="new-command-menu"
                  onPointerDown={(event) => event.stopPropagation()}
                  onGroup={() => {
                    setEditor({ mode: "group", parentId: creationParent(activeNode), node: null });
                    setNewMenuOpen(false);
                  }}
                  onArtwork={() => {
                    setEditor({ mode: "artwork", parentId: creationParent(activeNode), node: null });
                    setNewMenuOpen(false);
                  }}
                />
              )}
            </div>
            <button className="icon-button" type="button" title="折叠所有分组" disabled={!repositoryReady} onClick={() => setExpandedIds(new Set())}>
              <ChevronsUp aria-hidden="true" size={18} />
            </button>
          </div>
        </div>

        <div className="search-area">
          <label className="search-box">
            {searching ? <LoaderCircle className="spin" aria-hidden="true" size={16} /> : <Search aria-hidden="true" size={16} />}
            <input
              type="search"
              placeholder="搜索标题或工作文件"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              disabled={!repositoryReady}
            />
            {query && (
              <button type="button" title="清除搜索" onClick={() => setQuery("")}>
                <X aria-hidden="true" size={14} />
              </button>
            )}
          </label>
          {query.trim() && (
            <div className="search-results">
              {!searching && searchResults.length === 0 && <div>没有匹配项</div>}
              {searchResults.map((result) => (
                <button key={result.id} type="button" onClick={() => selectSearchResult(result)}>
                  {result.kind === "group" ? <FolderPlus aria-hidden="true" size={16} /> : <FileImage aria-hidden="true" size={16} />}
                  <span>
                    <strong>{result.title}</strong>
                    <small>{result.breadcrumb}</small>
                  </span>
                </button>
              ))}
            </div>
          )}
        </div>

        <div className="tree-scroll">
          {loading ? (
            <div className="tree-loading"><LoaderCircle className="spin" aria-hidden="true" size={16} />读取作品树</div>
          ) : (
            <LibraryTreeView
              nodes={tree.nodes}
              selectedIds={selectedIds}
              disabled={!repositoryReady || operationBusy}
              isExpanded={(id) => expandedIds.has(id)}
              onToggleExpanded={(id) => setExpandedIds((current) => {
                const next = new Set(current);
                if (next.has(id)) next.delete(id);
                else next.add(id);
                return next;
              })}
              onSelect={(node, event) => {
                const next = selectionForClick(
                  visibleIds,
                  selectedIds,
                  anchorId,
                  node.id,
                  event.shiftKey,
                  event.ctrlKey || event.metaKey,
                );
                setSelectedIds(next.ids);
                setAnchorId(next.anchorId);
                setActiveId(node.id);
                setTraceTarget(null);
              }}
              onMove={(request: MoveLibraryNodesRequest) => {
                void moveNodes(request).catch(() => undefined);
              }}
              onContextMenu={openContext}
            />
          )}
        </div>

        <footer className="sidebar-footer">
          <span>{tree.groupCount} 个分组</span>
          {selectedIds.size > 1 && <strong>已选择 {selectedIds.size} 项</strong>}
          <button type="button" title="回收站" disabled={!repositoryReady || operationBusy} onClick={() => void openTrash()}>
            <Trash2 aria-hidden="true" size={16} />
          </button>
        </footer>
      </aside>

      <section className="workspace">
        {!repositoryReady ? (
          <WorkspaceEmpty
            icon={<Settings aria-hidden="true" size={28} />}
            title="配置作品仓库"
            description="选择一个空目录保存作品树、分支历史与认证记录。"
            action={<button className="primary-button" type="button" onClick={onConfigure}><Settings aria-hidden="true" size={18} />设置仓库</button>}
          />
        ) : activeNode?.kind === "artwork" && selectedIds.size === 1 ? (
          renderArtworkWorkspace({
            artworkId: activeNode.id,
            initialView: traceTarget?.artworkId === activeNode.id ? "publish" : "history",
            initialBranchId: traceTarget?.artworkId === activeNode.id ? traceTarget.branchId : null,
            initialRecordId: traceTarget?.artworkId === activeNode.id ? traceTarget.recordId : null,
            onNavigateRecord: (target) => {
              const node = nodeById.get(target.artworkId);
              if (!node) {
                onError("匹配记录所属 Artwork 当前不在作品树中。");
                return;
              }
              setExpandedIds((current) => {
                const ancestors: string[] = [];
                let parentId = node.parentId;
                while (parentId) {
                  ancestors.push(parentId);
                  parentId = nodeById.get(parentId)?.parentId ?? null;
                }
                return new Set([...current, ...ancestors]);
              });
              setSelectedIds(new Set([target.artworkId]));
              setAnchorId(target.artworkId);
              setActiveId(target.artworkId);
              setTraceTarget(target);
            },
          })
        ) : activeNode ? (
          <NodeOverview node={activeNode} selectedCount={selectedIds.size} />
        ) : (
          <WorkspaceEmpty
            icon={<FileImage aria-hidden="true" size={28} />}
            title={tree.artworkCount ? "选择一个 Artwork" : "创建第一个 Artwork"}
            description={tree.artworkCount ? "从左侧作品树中选择项目。" : "Artwork 是作品树的叶节点，并从一个独立工作文件开始。"}
            action={!tree.artworkCount ? <button className="primary-button" type="button" onClick={() => setEditor({ mode: "artwork", parentId: null, node: null })}><Plus aria-hidden="true" size={18} />新建 Artwork</button> : undefined}
          />
        )}
        {operationBusy && <div className="operation-indicator"><LoaderCircle className="spin" aria-hidden="true" size={16} />正在更新作品树</div>}
      </section>

      {context && (
        <div
          className="context-menu"
          style={{ left: context.x, top: context.y }}
          onPointerDown={(event) => event.stopPropagation()}
        >
          {(!context.node || context.node.kind === "group") && (
            <>
              <button type="button" onClick={() => {
                setEditor({ mode: "group", parentId: creationParent(context.node), node: null });
                setContext(null);
              }}><FolderPlus aria-hidden="true" size={16} />新建分组</button>
              <button type="button" onClick={() => {
                setEditor({ mode: "artwork", parentId: creationParent(context.node), node: null });
                setContext(null);
              }}><FileImage aria-hidden="true" size={16} />新建 Artwork</button>
            </>
          )}
          {context.node && (
            <>
              <div className="context-separator" />
              <button type="button" onClick={() => {
                setEditor({ mode: "rename", parentId: context.node?.parentId ?? null, node: context.node });
                setContext(null);
              }}><Pencil aria-hidden="true" size={16} />重命名</button>
              <button className="danger" type="button" onClick={() => void deleteSelection(context.node!)}><Trash2 aria-hidden="true" size={16} />移到回收站{selectedIds.size > 1 ? ` ${selectedIds.size} 项` : ""}</button>
            </>
          )}
        </div>
      )}

      {editor && (
        <NodeEditor
          state={editor}
          busy={operationBusy}
          onClose={() => setEditor(null)}
          onSubmit={async (values) => {
            try {
              if (editor.mode === "group") {
                await createGroup(editor.parentId, values.title);
              } else if (editor.mode === "artwork") {
                await createArtwork({
                  parentId: editor.parentId,
                  title: values.title,
                  branchTitle: values.branchTitle,
                  sourcePath: values.sourcePath,
                });
              } else if (editor.node) {
                await renameNode(editor.node!.id, values.title);
              }
              setEditor(null);
              if (editor.parentId) setExpandedIds((current) => new Set([...current, editor.parentId!]));
            } catch {
              // Error is already surfaced by runMutation.
            }
          }}
        />
      )}

      {trashOpen && (
        <TrashDialog
          entries={trashEntries}
          busy={operationBusy}
          cleanupFailures={cleanupFailures}
          onClose={() => setTrashOpen(false)}
          onRestore={async (entry) => {
            await restoreTrash(entry.id);
            setExpandedIds((current) => new Set(current));
          }}
          onDelete={async (entry) => {
            if (!window.confirm(`永久删除“${entry.title}”及其全部内容？此操作无法撤销。`)) return;
            await permanentlyDeleteTrash([entry.id]);
          }}
          onEmpty={async () => {
            if (!trashEntries.length || !window.confirm("永久删除回收站中的全部项目？此操作无法撤销。")) return;
            await emptyTrash();
          }}
          onRetryCleanup={retryCleanup}
        />
      )}
    </Fragment>
  );
}
