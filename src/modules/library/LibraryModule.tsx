import { open } from "@tauri-apps/plugin-dialog";
import {
  FileImage,
  FolderPlus,
  GitBranch,
  LoaderCircle,
  ChevronsUp,
  Pencil,
  Plus,
  Search,
  Settings,
  Trash2,
  RotateCcw,
  X,
} from "lucide-react";
import {
  Fragment,
  useEffect,
  useMemo,
  useState,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { libraryApi } from "./api";
import { ArtworkWorkspace } from "../../app/ArtworkWorkspace";
import type { CertificationRecord } from "../authenticity/types";
import { flattenTree, selectionForClick, visibleTree } from "./tree";
import { LibraryTreeView } from "./LibraryTreeView";
import type {
  LibraryNode,
  LibrarySearchResult,
  LibraryTrashEntry,
  LibraryTree,
  MoveLibraryNodesRequest,
} from "./types";

interface LibraryModuleProps {
  repositoryReady: boolean;
  onConfigure: () => void;
  onError: (message: string | null) => void;
}

type EditorMode = "group" | "artwork" | "rename";

interface EditorState {
  mode: EditorMode;
  parentId: string | null;
  node: LibraryNode | null;
}

interface ContextState {
  node: LibraryNode | null;
  x: number;
  y: number;
}

const EMPTY_TREE: LibraryTree = { nodes: [], groupCount: 0, artworkCount: 0 };

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function LibraryModule({ repositoryReady, onConfigure, onError }: LibraryModuleProps) {
  const [tree, setTree] = useState(EMPTY_TREE);
  const [loading, setLoading] = useState(false);
  const [operationBusy, setOperationBusy] = useState(false);
  const expandedStorageKey = "lilith-artworks:library:expanded";
  const [expandedIds, setExpandedIds] = useState<Set<string>>(() => {
    try {
      const value = window.localStorage.getItem(expandedStorageKey);
      const ids = value ? JSON.parse(value) : [];
      return new Set(Array.isArray(ids) ? ids.filter((id): id is string => typeof id === "string") : []);
    } catch {
      return new Set();
    }
  });
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [anchorId, setAnchorId] = useState<string | null>(null);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [searchResults, setSearchResults] = useState<LibrarySearchResult[]>([]);
  const [searching, setSearching] = useState(false);
  const [newMenuOpen, setNewMenuOpen] = useState(false);
  const [context, setContext] = useState<ContextState | null>(null);
  const [editor, setEditor] = useState<EditorState | null>(null);
  const [trashOpen, setTrashOpen] = useState(false);
  const [trashEntries, setTrashEntries] = useState<LibraryTrashEntry[]>([]);
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
    window.localStorage.setItem(expandedStorageKey, JSON.stringify([...expandedIds]));
  }, [expandedIds]);

  useEffect(() => {
    if (!repositoryReady) {
      setTree(EMPTY_TREE);
      setSelectedIds(new Set());
      setActiveId(null);
      return;
    }
    let cancelled = false;
    setLoading(true);
    libraryApi
      .listTree()
      .then((next) => {
        if (!cancelled) {
          setTree(next);
          setExpandedIds((current) => {
            const groups = new Set(flattenTree(next.nodes).filter((node) => node.kind === "group").map((node) => node.id));
            return new Set([...current].filter((id) => groups.has(id)));
          });
        }
      })
      .catch((error) => {
        if (!cancelled) onError(errorMessage(error));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [onError, repositoryReady]);

  useEffect(() => {
    const trimmed = query.trim();
    if (!repositoryReady || !trimmed) {
      setSearchResults([]);
      setSearching(false);
      return;
    }
    let cancelled = false;
    setSearching(true);
    const timeout = window.setTimeout(() => {
      libraryApi
        .search(trimmed)
        .then((results) => {
          if (!cancelled) setSearchResults(results);
        })
        .catch((error) => {
          if (!cancelled) onError(errorMessage(error));
        })
        .finally(() => {
          if (!cancelled) setSearching(false);
        });
    }, 180);
    return () => {
      cancelled = true;
      window.clearTimeout(timeout);
    };
  }, [onError, query, repositoryReady]);

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

  const applyTree = (next: LibraryTree, preferredActiveId?: string | null) => {
    const valid = new Set(flattenTree(next.nodes).map((node) => node.id));
    setTree(next);
    setSelectedIds((current) => new Set([...current].filter((id) => valid.has(id))));
    setActiveId((current) => {
      const candidate = preferredActiveId === undefined ? current : preferredActiveId;
      return candidate && valid.has(candidate) ? candidate : null;
    });
  };

  const runMutation = async (operation: () => Promise<LibraryTree>) => {
    setOperationBusy(true);
    onError(null);
    try {
      applyTree(await operation());
    } catch (error) {
      onError(errorMessage(error));
      throw error;
    } finally {
      setOperationBusy(false);
    }
  };

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
      await runMutation(() => libraryApi.trashNodes(ids));
      setSelectedIds(new Set());
      setActiveId(null);
    } catch {
      // Error is already surfaced by runMutation.
    }
  };

  const openTrash = async () => {
    setOperationBusy(true);
    onError(null);
    try {
      setTrashEntries(await libraryApi.listTrash());
      setTrashOpen(true);
    } catch (error) {
      onError(errorMessage(error));
    } finally {
      setOperationBusy(false);
    }
  };

  const reloadTrash = async () => setTrashEntries(await libraryApi.listTrash());

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
                void runMutation(() => libraryApi.moveNodes(request)).catch(() => undefined);
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
          <ArtworkWorkspace
            key={`${activeNode.id}:${traceTarget?.branchId ?? "default"}`}
            artworkId={activeNode.id}
            initialView={traceTarget?.artworkId === activeNode.id ? "publish" : "history"}
            initialBranchId={traceTarget?.artworkId === activeNode.id ? traceTarget.branchId : null}
            initialRecordId={traceTarget?.artworkId === activeNode.id ? traceTarget.recordId : null}
            onError={onError}
            onNavigateRecord={(record: CertificationRecord) => {
              const node = nodeById.get(record.artworkId);
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
              setSelectedIds(new Set([record.artworkId]));
              setAnchorId(record.artworkId);
              setActiveId(record.artworkId);
              setTraceTarget({ artworkId: record.artworkId, branchId: record.branchId, recordId: record.id });
            }}
          />
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
                await runMutation(() => libraryApi.createGroup(editor.parentId, values.title));
              } else if (editor.mode === "artwork") {
                await runMutation(() => libraryApi.createArtwork({
                  parentId: editor.parentId,
                  title: values.title,
                  branchTitle: values.branchTitle,
                  sourcePath: values.sourcePath,
                }));
              } else if (editor.node) {
                await runMutation(() => libraryApi.renameNode(editor.node!.id, values.title));
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
          onClose={() => setTrashOpen(false)}
          onRestore={async (entry) => {
            setOperationBusy(true);
            onError(null);
            try {
              applyTree(await libraryApi.restoreTrash(entry.id), entry.id);
              await reloadTrash();
              setExpandedIds((current) => new Set(current));
            } catch (error) {
              onError(errorMessage(error));
            } finally {
              setOperationBusy(false);
            }
          }}
          onDelete={async (entry) => {
            if (!window.confirm(`永久删除“${entry.title}”及其全部内容？此操作无法撤销。`)) return;
            setOperationBusy(true);
            onError(null);
            try {
              await libraryApi.permanentlyDeleteTrash([entry.id]);
              await reloadTrash();
            } catch (error) {
              onError(errorMessage(error));
            } finally {
              setOperationBusy(false);
            }
          }}
          onEmpty={async () => {
            if (!trashEntries.length || !window.confirm("永久删除回收站中的全部项目？此操作无法撤销。")) return;
            setOperationBusy(true);
            onError(null);
            try {
              await libraryApi.emptyTrash();
              setTrashEntries([]);
            } catch (error) {
              onError(errorMessage(error));
            } finally {
              setOperationBusy(false);
            }
          }}
        />
      )}
    </Fragment>
  );
}

function CommandMenu({ className, onGroup, onArtwork, onPointerDown }: {
  className: string;
  onGroup: () => void;
  onArtwork: () => void;
  onPointerDown: (event: ReactPointerEvent) => void;
}) {
  return (
    <div className={`command-menu ${className}`} onPointerDown={onPointerDown}>
      <button type="button" onClick={onGroup}><FolderPlus aria-hidden="true" size={16} />新建分组</button>
      <button type="button" onClick={onArtwork}><FileImage aria-hidden="true" size={16} />新建 Artwork</button>
    </div>
  );
}

function WorkspaceEmpty({ icon, title, description, action }: {
  icon: React.ReactNode;
  title: string;
  description: string;
  action?: React.ReactNode;
}) {
  return (
    <div className="empty-workspace">
      <div className="empty-icon">{icon}</div>
      <h1>{title}</h1>
      <p>{description}</p>
      {action}
    </div>
  );
}

function NodeOverview({ node, selectedCount }: { node: LibraryNode; selectedCount: number }) {
  return (
    <div className="node-overview">
      <header>
        <div className={`overview-icon ${node.kind}`}>
          {node.kind === "group" ? <FolderPlus aria-hidden="true" size={22} /> : <FileImage aria-hidden="true" size={22} />}
        </div>
        <div>
          <span>{node.kind === "group" ? "分组" : "Artwork"}</span>
          <h1>{node.title}</h1>
        </div>
      </header>
      {selectedCount > 1 ? (
        <div className="selection-summary"><strong>{selectedCount}</strong><span>个节点已选择，可一起拖动或移到回收站。</span></div>
      ) : node.kind === "group" ? (
        <dl className="overview-facts">
          <div><dt>直接子节点</dt><dd>{node.children.length}</dd></div>
          <div><dt>最后更新</dt><dd>{new Date(node.updatedMs).toLocaleString()}</dd></div>
        </dl>
      ) : (
        <>
          <dl className="overview-facts">
            <div><dt>分支数量</dt><dd>{node.artwork?.branchCount ?? 0}</dd></div>
            <div><dt>主分支</dt><dd>{node.artwork?.primaryBranch?.title ?? "未创建"}</dd></div>
          </dl>
          <section className="source-file-band">
            <GitBranch aria-hidden="true" size={18} />
            <div><span>工作文件</span><strong>{node.artwork?.primaryBranch?.sourcePath ?? "-"}</strong></div>
          </section>
        </>
      )}
    </div>
  );
}

function TrashDialog({ entries, busy, onClose, onRestore, onDelete, onEmpty }: {
  entries: LibraryTrashEntry[];
  busy: boolean;
  onClose: () => void;
  onRestore: (entry: LibraryTrashEntry) => Promise<void>;
  onDelete: (entry: LibraryTrashEntry) => Promise<void>;
  onEmpty: () => Promise<void>;
}) {
  return (
    <div className="dialog-backdrop" role="presentation" onMouseDown={onClose}>
      <section className="trash-dialog" role="dialog" aria-modal="true" aria-labelledby="trash-title" onMouseDown={(event) => event.stopPropagation()}>
        <header>
          <div><h2 id="trash-title">回收站</h2><small>{entries.length} 个项目</small></div>
          <button className="icon-button" type="button" title="关闭" onClick={onClose}><X aria-hidden="true" size={18} /></button>
        </header>
        <div className="trash-list">
          {!entries.length && <div className="trash-empty"><Trash2 aria-hidden="true" size={24} /><span>回收站为空</span></div>}
          {entries.map((entry) => (
            <article key={entry.id} className="trash-row">
              <div className="trash-kind">{entry.kind === "group" ? <FolderPlus aria-hidden="true" size={18} /> : <FileImage aria-hidden="true" size={18} />}</div>
              <div className="trash-copy">
                <strong>{entry.title}</strong>
                <span>{new Date(entry.deletedMs).toLocaleString()} · {entry.artworkCount} 个 Artwork{entry.originalParentTitle ? ` · 原位置：${entry.originalParentTitle}` : " · 原位置：根目录"}</span>
              </div>
              <button className="icon-button" type="button" title="恢复" disabled={busy} onClick={() => void onRestore(entry)}><RotateCcw aria-hidden="true" size={16} /></button>
              <button className="icon-button danger-icon" type="button" title="永久删除" disabled={busy} onClick={() => void onDelete(entry)}><Trash2 aria-hidden="true" size={16} /></button>
            </article>
          ))}
        </div>
        <footer>
          <span>永久删除不会进入其他回收站。</span>
          <button className="danger-button" type="button" disabled={busy || !entries.length} onClick={() => void onEmpty()}><Trash2 aria-hidden="true" size={16} />清空回收站</button>
        </footer>
      </section>
    </div>
  );
}

interface EditorValues {
  title: string;
  branchTitle: string;
  sourcePath: string;
}

function NodeEditor({ state, busy, onClose, onSubmit }: {
  state: EditorState;
  busy: boolean;
  onClose: () => void;
  onSubmit: (values: EditorValues) => Promise<void>;
}) {
  const [title, setTitle] = useState(state.node?.title ?? "");
  const [branchTitle, setBranchTitle] = useState("主分支");
  const [sourcePath, setSourcePath] = useState("");
  const isArtwork = state.mode === "artwork";
  const heading = state.mode === "rename" ? "重命名" : isArtwork ? "新建 Artwork" : "新建分组";

  const chooseSource = async () => {
    const selected = await open({ directory: false, multiple: false });
    if (typeof selected === "string") setSourcePath(selected);
  };

  const valid = title.trim() && (!isArtwork || (branchTitle.trim() && sourcePath.trim()));
  return (
    <div className="dialog-backdrop" role="presentation" onMouseDown={onClose}>
      <form
        className="node-editor"
        role="dialog"
        aria-modal="true"
        aria-labelledby="node-editor-title"
        onMouseDown={(event) => event.stopPropagation()}
        onSubmit={(event) => {
          event.preventDefault();
          if (valid && !busy) void onSubmit({ title, branchTitle, sourcePath });
        }}
      >
        <header>
          <h2 id="node-editor-title">{heading}</h2>
          <button className="icon-button" type="button" title="关闭" onClick={onClose}><X aria-hidden="true" size={18} /></button>
        </header>
        <div className="editor-fields">
          <label><span>标题</span><input autoFocus value={title} maxLength={160} onChange={(event) => setTitle(event.target.value)} /></label>
          {isArtwork && (
            <>
              <label><span>初始分支标题</span><input value={branchTitle} maxLength={160} onChange={(event) => setBranchTitle(event.target.value)} /></label>
              <label>
                <span>工作文件</span>
                <div className="path-control"><input value={sourcePath} onChange={(event) => setSourcePath(event.target.value)} placeholder="选择现有作品文件" /><button className="secondary-button" type="button" onClick={chooseSource}>浏览</button></div>
              </label>
            </>
          )}
        </div>
        <footer>
          <button className="text-button" type="button" onClick={onClose}>取消</button>
          <button className="primary-button" type="submit" disabled={!valid || busy}>{busy && <LoaderCircle className="spin" aria-hidden="true" size={16} />}{state.mode === "rename" ? "保存" : "创建"}</button>
        </footer>
      </form>
    </div>
  );
}
