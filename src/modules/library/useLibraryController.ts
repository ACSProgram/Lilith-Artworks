import { useCallback, useEffect, useRef, useState } from "react";
import { errorMessage } from "../../shared/errors";
import type { CleanupFailure, CleanupReport } from "../../shared/fileCleanup";
import { useAsyncOperation } from "../../shared/useAsyncOperation";
import { useRequestEpoch } from "../../shared/useRequestEpoch";
import { useRuntimeSubscription } from "../../shared/useRuntimeSubscription";
import { libraryApi } from "./api";
import { flattenTree } from "./tree";
import type {
  CreateArtworkRequest,
  LibrarySearchResult,
  LibraryTrashEntry,
  LibraryTree,
  MoveLibraryNodesRequest,
} from "./types";

const EMPTY_TREE: LibraryTree = { nodes: [], groupCount: 0, artworkCount: 0 };
const EXPANDED_STORAGE_KEY = "lilith-artworks:library:expanded";

/**
 * 写操作会改动作品树的域。素材板、设置与维护操作的写库不改作品树结构，
 * 因此不为它们重读整棵树（它们仍由 30 秒兜底刷新对齐）。
 */
const TREE_CHANGING_DOMAINS = new Set<string>([
  "library",
  "history",
  "authenticity",
  "backup",
  "cleanup",
  "repository",
]);

function loadExpandedIds(): Set<string> {
  try {
    const value = window.localStorage.getItem(EXPANDED_STORAGE_KEY);
    const ids = value ? JSON.parse(value) : [];
    return new Set(Array.isArray(ids) ? ids.filter((id): id is string => typeof id === "string") : []);
  } catch {
    return new Set();
  }
}

interface LibraryControllerOptions {
  repositoryReady: boolean;
  onError: (message: string | null) => void;
  onRetryFileCleanup: (ids: string[]) => Promise<CleanupReport>;
  onAcknowledgeBackupDisableNotices: (artworkIds: string[]) => Promise<void>;
}

export function useLibraryController({
  repositoryReady,
  onError,
  onRetryFileCleanup,
  onAcknowledgeBackupDisableNotices,
}: LibraryControllerOptions) {
  const [tree, setTree] = useState(EMPTY_TREE);
  const [loading, setLoading] = useState(false);
  const { busy: operationBusy, run: runBusy } = useAsyncOperation();
  const [expandedIds, setExpandedIds] = useState<Set<string>>(loadExpandedIds);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [anchorId, setAnchorId] = useState<string | null>(null);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [searchResults, setSearchResults] = useState<LibrarySearchResult[]>([]);
  const [searching, setSearching] = useState(false);
  const [trashEntries, setTrashEntries] = useState<LibraryTrashEntry[]>([]);
  const [cleanupFailures, setCleanupFailures] = useState<CleanupFailure[]>([]);
  const {
    begin: beginTreeRead,
    invalidate: invalidateTreeReads,
    isCurrent: isCurrentTreeRead,
  } = useRequestEpoch();
  const {
    begin: beginSearchRead,
    invalidate: invalidateSearchReads,
    isCurrent: isCurrentSearchRead,
  } = useRequestEpoch();
  /** 仓库身份（切换仓库时递增）；与读取代次分开，后台刷新不使 mutation 失效。 */
  const repositoryGeneration = useRef(0);
  const currentRepositoryGeneration = useCallback(() => repositoryGeneration.current, []);
  const isCurrentRepository = useCallback(
    (id: number) => id === repositoryGeneration.current,
    [],
  );
  const treeReloadTimer = useRef<number | null>(null);

  const applyTree = useCallback((next: LibraryTree, preferredActiveId?: string | null) => {
    const valid = new Set(flattenTree(next.nodes).map((node) => node.id));
    setTree(next);
    setSelectedIds((current) => new Set([...current].filter((id) => valid.has(id))));
    setActiveId((current) => {
      const candidate = preferredActiveId === undefined ? current : preferredActiveId;
      return candidate && valid.has(candidate) ? candidate : null;
    });
    setExpandedIds((current) => {
      const groups = new Set(flattenTree(next.nodes)
        .filter((node) => node.kind === "group")
        .map((node) => node.id));
      return new Set([...current].filter((id) => groups.has(id)));
    });
  }, []);

  useEffect(() => {
    window.localStorage.setItem(EXPANDED_STORAGE_KEY, JSON.stringify([...expandedIds]));
  }, [expandedIds]);

  useEffect(() => {
    const repositoryId = ++repositoryGeneration.current;
    const requestId = beginTreeRead();
    invalidateSearchReads();
    if (!repositoryReady) {
      setTree(EMPTY_TREE);
      setSelectedIds(new Set());
      setAnchorId(null);
      setActiveId(null);
      setQuery("");
      setSearchResults([]);
      setTrashEntries([]);
      setLoading(false);
      setSearching(false);
      return undefined;
    }
    setLoading(true);
    libraryApi.listTree()
      .then((next) => {
        if (isCurrentTreeRead(requestId)) applyTree(next);
      })
      .catch((error) => {
        if (isCurrentTreeRead(requestId)) onError(errorMessage(error));
      })
      .finally(() => {
        if (isCurrentTreeRead(requestId)) setLoading(false);
      });
    return () => {
      if (isCurrentTreeRead(requestId)) invalidateTreeReads();
      if (isCurrentRepository(repositoryId)) repositoryGeneration.current += 1;
    };
  }, [
    applyTree,
    beginTreeRead,
    invalidateSearchReads,
    invalidateTreeReads,
    isCurrentRepository,
    isCurrentTreeRead,
    onError,
    repositoryReady,
  ]);

  const reloadTree = useCallback(async () => {
    const requestId = beginTreeRead();
    try {
      const next = await libraryApi.listTree();
      if (isCurrentTreeRead(requestId)) applyTree(next);
    } catch {
      // 后台刷新是尽力而为；下一次操作或事件会再次尝试。
    }
  }, [applyTree, beginTreeRead, isCurrentTreeRead]);

  useEffect(() => {
    if (!repositoryReady) return;
    const timer = window.setInterval(() => void reloadTree(), 30_000);
    return () => window.clearInterval(timer);
  }, [reloadTree, repositoryReady]);

  const scheduleTreeReload = useCallback(() => {
    if (treeReloadTimer.current !== null) return;
    treeReloadTimer.current = window.setTimeout(() => {
      treeReloadTimer.current = null;
      void reloadTree();
    }, 100);
  }, [reloadTree]);

  useEffect(() => () => {
    if (treeReloadTimer.current !== null) window.clearTimeout(treeReloadTimer.current);
  }, []);

  // 仓库写操作成功后的即时刷新；30 秒轮询仍是后台调度写库（没有事件）的兜底。
  useRuntimeSubscription((event) => {
    if (event.type !== "repositoryChanged") return;
    if (!event.domain || !TREE_CHANGING_DOMAINS.has(event.domain)) return;
    scheduleTreeReload();
  }, { enabled: repositoryReady });

  useEffect(() => {
    const trimmed = query.trim();
    const repositoryId = currentRepositoryGeneration();
    const requestId = beginSearchRead();
    if (!repositoryReady || !trimmed) {
      setSearchResults([]);
      setSearching(false);
      return;
    }
    setSearching(true);
    const timeout = window.setTimeout(() => {
      libraryApi.search(trimmed)
        .then((results) => {
          if (isCurrentSearchRead(requestId) && isCurrentRepository(repositoryId)) {
            setSearchResults(results);
          }
        })
        .catch((error) => {
          if (isCurrentSearchRead(requestId) && isCurrentRepository(repositoryId)) {
            onError(errorMessage(error));
          }
        })
        .finally(() => {
          if (isCurrentSearchRead(requestId) && isCurrentRepository(repositoryId)) {
            setSearching(false);
          }
        });
    }, 180);
    return () => window.clearTimeout(timeout);
  }, [
    beginSearchRead,
    currentRepositoryGeneration,
    isCurrentRepository,
    isCurrentSearchRead,
    onError,
    query,
    repositoryReady,
  ]);

  const runMutation = useCallback(async (operation: () => Promise<LibraryTree>) => {
    const repositoryId = currentRepositoryGeneration();
    onError(null);
    const next = await runBusy(operation, {
      onError: (error) => {
        if (isCurrentRepository(repositoryId)) onError(errorMessage(error));
      },
    });
    if (isCurrentRepository(repositoryId)) applyTree(next);
  }, [applyTree, currentRepositoryGeneration, isCurrentRepository, onError, runBusy]);

  /**
   * 在仓库代次内执行一次写操作：先清掉旧提示，`busy` 由共享
   * `useAsyncOperation` 按计数维护，错误接到统一提示通道后不再向调用方抛出
   * （调用方只关心是否继续执行下一步）。切换仓库后旧仓库的结果与提示一律丢弃。
   */
  const runScopedOperation = useCallback(async (
    operation: (repositoryId: number) => Promise<void>,
  ): Promise<void> => {
    const repositoryId = currentRepositoryGeneration();
    onError(null);
    try {
      await runBusy(
        () => operation(repositoryId),
        {
          onError: (error) => {
            if (isCurrentRepository(repositoryId)) onError(errorMessage(error));
          },
        },
      );
    } catch {
      // 错误已由 onError 提示。
    }
  }, [currentRepositoryGeneration, isCurrentRepository, onError, runBusy]);

  const applyCleanupReport = useCallback((report: CleanupReport) => {
    setCleanupFailures(report.failures);
    if (report.failures.length > 0) {
      onError(`元数据已删除，但有 ${report.failures.length} 个文件清理失败；可在回收站中重试。`);
    }
  }, [onError]);

  const retryCleanup = useCallback(() => {
    if (cleanupFailures.length === 0) return Promise.resolve();
    const ids = cleanupFailures.map((failure) => failure.id);
    return runScopedOperation(async (repositoryId) => {
      const report = await onRetryFileCleanup(ids);
      if (isCurrentRepository(repositoryId)) applyCleanupReport(report);
    });
  }, [applyCleanupReport, cleanupFailures, isCurrentRepository, onRetryFileCleanup, runScopedOperation]);

  const acknowledgeBackupDisableNotices = useCallback((artworkIds: string[]) => {
    if (artworkIds.length === 0) return Promise.resolve();
    return runScopedOperation(async (repositoryId) => {
      await onAcknowledgeBackupDisableNotices(artworkIds);
      const next = await libraryApi.listTree();
      if (isCurrentRepository(repositoryId)) applyTree(next);
    });
  }, [applyTree, isCurrentRepository, onAcknowledgeBackupDisableNotices, runScopedOperation]);

  /** 读取回收站条目；不接管 busy 与提示，供写操作内部与公开入口共用。 */
  const readTrash = useCallback(async () => {
    setTrashEntries(await libraryApi.listTrash());
  }, []);

  const loadTrash = useCallback(async () => {
    const repositoryId = currentRepositoryGeneration();
    onError(null);
    // 打开回收站前必须先读成功，因此这里保留向调用方抛出的语义。
    await runBusy(readTrash, {
      onError: (error) => {
        if (isCurrentRepository(repositoryId)) onError(errorMessage(error));
      },
    });
  }, [currentRepositoryGeneration, isCurrentRepository, onError, readTrash, runBusy]);

  const createGroup = useCallback((parentId: string | null, title: string) =>
    runMutation(() => libraryApi.createGroup(parentId, title)), [runMutation]);

  const createArtwork = useCallback((request: CreateArtworkRequest) =>
    runMutation(() => libraryApi.createArtwork(request)), [runMutation]);

  const renameNode = useCallback((id: string, title: string) =>
    runMutation(() => libraryApi.renameNode(id, title)), [runMutation]);

  const trashNodes = useCallback((ids: string[]) =>
    runMutation(() => libraryApi.trashNodes(ids)), [runMutation]);

  const moveNodes = useCallback((request: MoveLibraryNodesRequest) =>
    runMutation(() => libraryApi.moveNodes(request)), [runMutation]);

  const restoreTrash = useCallback((id: string) => runScopedOperation(async (repositoryId) => {
    const next = await libraryApi.restoreTrash(id);
    if (!isCurrentRepository(repositoryId)) return;
    applyTree(next, id);
    await readTrash();
  }), [applyTree, isCurrentRepository, readTrash, runScopedOperation]);

  const permanentlyDeleteTrash = useCallback((ids: string[]) => runScopedOperation(async (repositoryId) => {
    const report = await libraryApi.permanentlyDeleteTrash(ids);
    if (!isCurrentRepository(repositoryId)) return;
    applyCleanupReport(report);
    await readTrash();
  }), [applyCleanupReport, isCurrentRepository, readTrash, runScopedOperation]);

  const emptyTrash = useCallback(() => runScopedOperation(async (repositoryId) => {
    const report = await libraryApi.emptyTrash();
    if (!isCurrentRepository(repositoryId)) return;
    applyCleanupReport(report);
    setTrashEntries([]);
  }), [applyCleanupReport, isCurrentRepository, runScopedOperation]);

  return {
    tree,
    loading,
    operationBusy,
    expandedIds,
    setExpandedIds,
    selectedIds,
    setSelectedIds,
    anchorId,
    setAnchorId,
    activeId,
    setActiveId,
    query,
    setQuery,
    searchResults,
    setSearchResults,
    searching,
    trashEntries,
    cleanupFailures,
    retryCleanup,
    acknowledgeBackupDisableNotices,
    loadTrash,
    createGroup,
    createArtwork,
    renameNode,
    trashNodes,
    moveNodes,
    restoreTrash,
    permanentlyDeleteTrash,
    emptyTrash,
  };
}
