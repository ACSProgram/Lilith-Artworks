import { useCallback, useEffect, useRef, useState } from "react";
import { historyApi } from "./api";
import type {
  ArtworkHistory, BackupCommitResult, BackupRuntimeStatus, ForkBranchRequest,
  HistoryNode, RenameHistoryNodeRequest, UpdateBranchBackupRequest,
} from "./types";

const IDLE_RUNTIME: BackupRuntimeStatus = {
  busy: false,
  activeBranchId: null,
  operation: null,
  progressLabel: null,
  progressCurrent: 0,
  progressTotal: 0,
  automaticScheduling: true,
  completionRevision: 0,
};

interface HistoryControllerOptions {
  artworkId: string;
  refreshVersion: number;
  onHistoryChanged: (history: ArtworkHistory) => void;
  onError: (message: string | null) => void;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function useHistoryController({
  artworkId,
  refreshVersion,
  onHistoryChanged,
  onError,
}: HistoryControllerOptions) {
  const [history, setHistory] = useState<ArtworkHistory | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [runtime, setRuntime] = useState(IDLE_RUNTIME);
  const [localOperation, setLocalOperation] = useState<{ operation: string; label: string } | null>(null);
  const artworkIdRef = useRef(artworkId);
  const onHistoryChangedRef = useRef(onHistoryChanged);
  const onErrorRef = useRef(onError);
  const requestSequence = useRef(0);
  const lastRefresh = useRef({ artworkId, refreshVersion });
  const lastCompletionRevision = useRef<number | null>(null);

  artworkIdRef.current = artworkId;
  onHistoryChangedRef.current = onHistoryChanged;
  onErrorRef.current = onError;

  const reportError = useCallback((error: unknown) => {
    onErrorRef.current(errorMessage(error));
  }, []);

  const invalidateReads = useCallback(() => {
    requestSequence.current += 1;
  }, []);

  const replaceHistory = useCallback((next: ArtworkHistory): boolean => {
    if (next.artworkId !== artworkIdRef.current) return false;
    invalidateReads();
    setHistory(next);
    onHistoryChangedRef.current(next);
    return true;
  }, [invalidateReads]);

  const reload = useCallback(async (): Promise<ArtworkHistory | null> => {
    const requestedArtworkId = artworkId;
    const requestId = ++requestSequence.current;
    try {
      const next = await historyApi.get(requestedArtworkId);
      if (requestId !== requestSequence.current
        || artworkIdRef.current !== requestedArtworkId
        || next.artworkId !== requestedArtworkId) {
        return null;
      }
      setHistory(next);
      onHistoryChangedRef.current(next);
      return next;
    } catch (error) {
      if (requestId !== requestSequence.current || artworkIdRef.current !== requestedArtworkId) {
        return null;
      }
      throw error;
    } finally {
      if (requestId === requestSequence.current) setLoading(false);
    }
  }, [artworkId]);

  useEffect(() => {
    setHistory(null);
    setLoading(true);
    void reload().catch(reportError);
    return invalidateReads;
  }, [artworkId, invalidateReads, reload, reportError]);

  useEffect(() => {
    const previous = lastRefresh.current;
    lastRefresh.current = { artworkId, refreshVersion };
    if (previous.artworkId !== artworkId || previous.refreshVersion === refreshVersion) return;
    void reload().catch(reportError);
  }, [artworkId, refreshVersion, reload, reportError]);

  useEffect(() => {
    let cancelled = false;
    let timer: number | undefined;
    const poll = async () => {
      let delay = busy ? 350 : 3000;
      try {
        const next = await historyApi.runtime();
        if (cancelled) return;
        const previousRevision = lastCompletionRevision.current;
        const operationCompleted = previousRevision !== null
          && previousRevision !== next.completionRevision;
        lastCompletionRevision.current = next.completionRevision;
        setRuntime(next);
        delay = busy || next.busy ? 350 : 3000;
        if (operationCompleted) await reload();
      } catch {
        // Commands surface repository errors; polling remains best-effort.
      }
      if (!cancelled) timer = window.setTimeout(poll, delay);
    };
    void poll();
    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [busy, reload]);

  const runOperation = useCallback(async (
    operation: string,
    label: string,
    action: () => Promise<void>,
  ): Promise<boolean> => {
    invalidateReads();
    setBusy(true);
    setLocalOperation({ operation, label });
    onErrorRef.current(null);
    try {
      await action();
      return true;
    } catch (error) {
      reportError(error);
      return false;
    } finally {
      setBusy(false);
      setLocalOperation(null);
      try { setRuntime(await historyApi.runtime()); } catch { /* best-effort */ }
    }
  }, [invalidateReads, reportError]);

  const saveBranch = useCallback(async (request: UpdateBranchBackupRequest) => {
    invalidateReads();
    onErrorRef.current(null);
    try {
      replaceHistory(await historyApi.updateBranch(request));
    } catch (error) {
      reportError(error);
      throw error;
    }
  }, [invalidateReads, replaceHistory, reportError]);

  const commitBranch = useCallback(async (branchId: string, note: string): Promise<BackupCommitResult | null> => {
    let result: BackupCommitResult | null = null;
    const succeeded = await runOperation("commit", "正在提交工作文件", async () => {
      result = await historyApi.commit(branchId, note);
      try {
        await reload();
      } catch (error) {
        onErrorRef.current(`提交已完成，但历史列表刷新失败：${errorMessage(error)}`);
      }
    });
    return succeeded ? result : null;
  }, [reload, runOperation]);

  const restoreNode = useCallback((historyId: string, outputPath: string) =>
    runOperation("restore", "正在准备恢复历史节点", async () => {
      await historyApi.restore(historyId, outputPath);
    }), [runOperation]);

  const compactNodes = useCallback((nodes: readonly HistoryNode[]) =>
    runOperation("compact", "正在重新整理历史链", async () => {
      for (const [index, node] of nodes.entries()) {
        setLocalOperation({ operation: "compact", label: `正在精简节点 ${index + 1}/${nodes.length}` });
        await historyApi.compact(node.id);
      }
      await reload();
    }), [reload, runOperation]);

  const deleteBranch = useCallback((branchId: string) =>
    runOperation("delete-branch", "正在删除分支", async () => {
      replaceHistory(await historyApi.deleteBranch(branchId));
    }), [replaceHistory, runOperation]);

  const deleteSubtree = useCallback((historyId: string, branchId: string) =>
    runOperation("delete", "正在删除节点与后续历史", async () => {
      await historyApi.deleteSubtree(historyId, branchId);
      await reload();
    }), [reload, runOperation]);

  const setCheckpoint = useCallback((historyId: string, enabled: boolean) =>
    runOperation("checkpoint", enabled ? "正在生成检查点" : "正在恢复增量存储", async () => {
      await historyApi.checkpoint(historyId, enabled);
      await reload();
    }), [reload, runOperation]);

  const forkBranch = useCallback((request: ForkBranchRequest) =>
    runOperation("fork", "正在创建分支", async () => {
      replaceHistory(await historyApi.fork(request));
    }), [replaceHistory, runOperation]);

  const renameNode = useCallback((request: RenameHistoryNodeRequest) =>
    runOperation("rename", "正在保存节点名称", async () => {
      replaceHistory(await historyApi.renameNode(request));
    }), [replaceHistory, runOperation]);

  const cancelOperation = useCallback(() => {
    void historyApi.cancel().catch(reportError);
  }, [reportError]);

  const visibleRuntime: BackupRuntimeStatus = runtime.busy ? runtime : localOperation ? {
    ...IDLE_RUNTIME,
    busy: true,
    operation: localOperation.operation,
    progressLabel: localOperation.label,
    progressTotal: 1,
  } : runtime;

  return {
    history: history?.artworkId === artworkId ? history : null,
    loading,
    busy,
    runtime,
    visibleRuntime,
    saveBranch,
    commitBranch,
    restoreNode,
    compactNodes,
    deleteBranch,
    deleteSubtree,
    setCheckpoint,
    forkBranch,
    renameNode,
    cancelOperation,
  };
}
