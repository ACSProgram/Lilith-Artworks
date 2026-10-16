import { useCallback, useEffect, useRef, useState } from "react";
import { historyApi } from "./api";
import type {
  ArtworkHistory, BackupCommitResult, BackupRuntimeStatus, ForkBranchRequest,
  HistoryNode, RenameHistoryNodeRequest, UpdateBranchBackupRequest,
} from "./types";

const IDLE_RUNTIME: BackupRuntimeStatus = {
  busy: false,
  activeBranchId: null,
  taskKind: null,
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
  /**
   * 批级取消标志：原生命令的取消检查点只覆盖「正在执行的那一个节点」，
   * 精简这类逐节点循环需要自己记住用户已经取消，否则后面的节点会继续删。
   */
  const cancelRequested = useRef(false);

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
    // 每次新操作都从「未请求取消」开始：上一次留下的请求只属于上一次操作。
    cancelRequested.current = false;
    setBusy(true);
    setLocalOperation({ operation, label });
    onErrorRef.current(null);
    let completed = false;
    try {
      await action();
      completed = true;
      return true;
    } catch (error) {
      reportError(error);
      return false;
    } finally {
      setBusy(false);
      setLocalOperation(null);
      try { setRuntime(await historyApi.runtime()); } catch { /* best-effort */ }
      // 用户请求了取消但操作仍走完成功：请求落在最后一个取消检查点之后（或两条
      // 批量命令之间的空档里）。不说明的话，用户会以为取消无效。
      if (completed && cancelRequested.current) {
        onErrorRef.current("取消请求未被接受，操作已在取消到达前完成。");
      }
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

  /**
   * 精简在取消后停下：先刷新历史，让已删除的节点立刻从列表消失，再以「永不返回」的
   * 异常收场——取消仍走既有错误通道，由 `runOperation` 统一报给用户。剩余节点保留在
   * 精简选择里，由用户决定是否再处理。
   */
  const stopCompact = useCallback(async (remaining: number): Promise<never> => {
    await reload();
    throw new Error(`精简已取消，剩余 ${remaining} 个节点未被删除。`);
  }, [reload]);

  const compactNodes = useCallback((nodes: readonly HistoryNode[]) =>
    runOperation("compact", "正在重新整理历史链", async () => {
      for (const [index, node] of nodes.entries()) {
        // 从这一个节点起都还没被删除：命令被取消时它没有被提交，命令正常结束时它
        // 已经删掉了，因此两个分支都用同一个剩余数量。
        const remaining = nodes.length - index;
        // 取消可能落在原生命令已经越过最后一个检查点之后（甚至落在两条命令之间的
        // 空档里），此时命令不会报取消错误，只有这里的批级标志能拦住后面的节点。
        if (cancelRequested.current) await stopCompact(remaining);
        setLocalOperation({ operation: "compact", label: `正在精简节点 ${index + 1}/${nodes.length}` });
        try {
          await historyApi.compact(node.id);
        } catch (error) {
          // 取消不比对错误文案——原生侧有多份取消文案，判定一律以取消闸门自身为准
          // （`src-tauri/src/headless.rs` 的闸门说明）；前端同理，只看用户是否请求过
          // 取消。请求过就折算成批量结果：命令自己以「操作已取消」退出时，用户只知道
          // 取消了、不知道还剩多少。其余错误原样抛出。
          if (cancelRequested.current) await stopCompact(remaining);
          throw error;
        }
      }
      await reload();
    }), [reload, runOperation, stopCompact]);

  const reverifyBranch = useCallback((branchId: string) =>
    runOperation("verify", "正在重新校验分支链路", async () => {
      // 命令只清空失败摘要并让分支重新入队，真正的校验由调度器在空闲时执行。
      replaceHistory(await historyApi.reverifyBranch(branchId));
    }), [replaceHistory, runOperation]);

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

  const cancelOperation = useCallback(async () => {
    // 批级取消：置位后逐节点循环不会再启动下一个节点。原生命令的取消检查点只覆盖
    // 正在执行的那一个，且共享取消标志会在下一条命令取得运行锁时被清零。
    cancelRequested.current = true;
    try {
      await historyApi.cancel();
    } catch (error) {
      reportError(error);
    }
  }, [reportError]);

  const visibleRuntime: BackupRuntimeStatus = runtime.busy ? runtime : localOperation ? {
    ...IDLE_RUNTIME,
    busy: true,
    taskKind: "userOperation",
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
    reverifyBranch,
    deleteSubtree,
    setCheckpoint,
    forkBranch,
    renameNode,
    cancelOperation,
  };
}
