import { useCallback, useEffect, useRef, useState } from "react";
import type { CleanupFailure, CleanupReport } from "../../shared/fileCleanup";
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

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

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
  const [operationBusy, setOperationBusy] = useState(false);
  const [expandedIds, setExpandedIds] = useState<Set<string>>(loadExpandedIds);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [anchorId, setAnchorId] = useState<string | null>(null);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [searchResults, setSearchResults] = useState<LibrarySearchResult[]>([]);
  const [searching, setSearching] = useState(false);
  const [trashEntries, setTrashEntries] = useState<LibraryTrashEntry[]>([]);
  const [cleanupFailures, setCleanupFailures] = useState<CleanupFailure[]>([]);
  const repositoryRequest = useRef(0);
  const searchRequest = useRef(0);

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
    const requestId = ++repositoryRequest.current;
    searchRequest.current += 1;
    setOperationBusy(false);
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
      return;
    }
    setLoading(true);
    libraryApi.listTree()
      .then((next) => {
        if (requestId === repositoryRequest.current) applyTree(next);
      })
      .catch((error) => {
        if (requestId === repositoryRequest.current) onError(errorMessage(error));
      })
      .finally(() => {
        if (requestId === repositoryRequest.current) setLoading(false);
      });
    return () => {
      if (requestId === repositoryRequest.current) repositoryRequest.current += 1;
    };
  }, [applyTree, onError, repositoryReady]);

  useEffect(() => {
    if (!repositoryReady) return;
    const timer = window.setInterval(() => {
      const repositoryId = repositoryRequest.current;
      libraryApi.listTree()
        .then((next) => {
          if (repositoryId === repositoryRequest.current) applyTree(next);
        })
        .catch(() => {});
    }, 30_000);
    return () => window.clearInterval(timer);
  }, [applyTree, repositoryReady]);

  useEffect(() => {
    const trimmed = query.trim();
    const repositoryId = repositoryRequest.current;
    const requestId = ++searchRequest.current;
    if (!repositoryReady || !trimmed) {
      setSearchResults([]);
      setSearching(false);
      return;
    }
    setSearching(true);
    const timeout = window.setTimeout(() => {
      libraryApi.search(trimmed)
        .then((results) => {
          if (requestId === searchRequest.current && repositoryId === repositoryRequest.current) {
            setSearchResults(results);
          }
        })
        .catch((error) => {
          if (requestId === searchRequest.current && repositoryId === repositoryRequest.current) {
            onError(errorMessage(error));
          }
        })
        .finally(() => {
          if (requestId === searchRequest.current && repositoryId === repositoryRequest.current) {
            setSearching(false);
          }
        });
    }, 180);
    return () => window.clearTimeout(timeout);
  }, [onError, query, repositoryReady]);

  const runMutation = useCallback(async (operation: () => Promise<LibraryTree>) => {
    const repositoryId = repositoryRequest.current;
    setOperationBusy(true);
    onError(null);
    try {
      const next = await operation();
      if (repositoryId === repositoryRequest.current) applyTree(next);
    } catch (error) {
      if (repositoryId === repositoryRequest.current) onError(errorMessage(error));
      throw error;
    } finally {
      if (repositoryId === repositoryRequest.current) setOperationBusy(false);
    }
  }, [applyTree, onError]);

  const applyCleanupReport = useCallback((report: CleanupReport) => {
    setCleanupFailures(report.failures);
    if (report.failures.length > 0) {
      onError(`元数据已删除，但有 ${report.failures.length} 个文件清理失败；可在回收站中重试。`);
    }
  }, [onError]);

  const retryCleanup = useCallback(async () => {
    if (cleanupFailures.length === 0) return;
    setOperationBusy(true);
    onError(null);
    try {
      const report = await onRetryFileCleanup(cleanupFailures.map((failure) => failure.id));
      applyCleanupReport(report);
      if (report.failures.length === 0) onError(null);
    } catch (error) {
      onError(errorMessage(error));
    } finally {
      setOperationBusy(false);
    }
  }, [applyCleanupReport, cleanupFailures, onError, onRetryFileCleanup]);

  const acknowledgeBackupDisableNotices = useCallback(async (artworkIds: string[]) => {
    if (artworkIds.length === 0) return;
    const repositoryId = repositoryRequest.current;
    setOperationBusy(true);
    onError(null);
    try {
      await onAcknowledgeBackupDisableNotices(artworkIds);
      const next = await libraryApi.listTree();
      if (repositoryId === repositoryRequest.current) applyTree(next);
    } catch (error) {
      if (repositoryId === repositoryRequest.current) onError(errorMessage(error));
    } finally {
      if (repositoryId === repositoryRequest.current) setOperationBusy(false);
    }
  }, [applyTree, onAcknowledgeBackupDisableNotices, onError]);

  const loadTrash = useCallback(async () => {
    const repositoryId = repositoryRequest.current;
    setOperationBusy(true);
    onError(null);
    try {
      const entries = await libraryApi.listTrash();
      if (repositoryId === repositoryRequest.current) setTrashEntries(entries);
    } catch (error) {
      if (repositoryId === repositoryRequest.current) onError(errorMessage(error));
      throw error;
    } finally {
      if (repositoryId === repositoryRequest.current) setOperationBusy(false);
    }
  }, [onError]);

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

  const restoreTrash = useCallback(async (id: string) => {
    const repositoryId = repositoryRequest.current;
    setOperationBusy(true);
    onError(null);
    try {
      const next = await libraryApi.restoreTrash(id);
      if (repositoryId !== repositoryRequest.current) return;
      applyTree(next, id);
      await loadTrash();
    } catch (error) {
      if (repositoryId === repositoryRequest.current) onError(errorMessage(error));
    } finally {
      if (repositoryId === repositoryRequest.current) setOperationBusy(false);
    }
  }, [applyTree, loadTrash, onError]);

  const permanentlyDeleteTrash = useCallback(async (ids: string[]) => {
    const repositoryId = repositoryRequest.current;
    setOperationBusy(true);
    onError(null);
    try {
      const report = await libraryApi.permanentlyDeleteTrash(ids);
      if (repositoryId !== repositoryRequest.current) return;
      applyCleanupReport(report);
      await loadTrash();
    } catch (error) {
      if (repositoryId === repositoryRequest.current) onError(errorMessage(error));
    } finally {
      if (repositoryId === repositoryRequest.current) setOperationBusy(false);
    }
  }, [applyCleanupReport, loadTrash, onError]);

  const emptyTrash = useCallback(async () => {
    const repositoryId = repositoryRequest.current;
    setOperationBusy(true);
    onError(null);
    try {
      const report = await libraryApi.emptyTrash();
      if (repositoryId !== repositoryRequest.current) return;
      applyCleanupReport(report);
      setTrashEntries([]);
    } catch (error) {
      if (repositoryId === repositoryRequest.current) onError(errorMessage(error));
    } finally {
      if (repositoryId === repositoryRequest.current) setOperationBusy(false);
    }
  }, [applyCleanupReport, onError]);

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
