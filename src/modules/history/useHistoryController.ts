import { useCallback, useEffect, useRef, useState } from "react";
import { errorMessage } from "../../shared/errors";
import { useAsyncOperation } from "../../shared/useAsyncOperation";
import { useRequestEpoch } from "../../shared/useRequestEpoch";
import { useRuntimeSubscription } from "../../shared/useRuntimeSubscription";
import { historyApi } from "./api";
import { mergeNodeVerifyStates } from "./historyModel";
import type {
  ArtworkHistory, BackupCommitResult, BackupRuntimeStatus, ForkBranchRequest,
  HistoryNode, NodePreview, RenameHistoryNodeRequest, UpdateBranchBackupRequest,
} from "./types";

/** 后台任务完成事件里与历史刷新决策有关的字段。 */
interface BackgroundCompletion {
  artworkId: string | null;
  structureChanged: boolean;
}

type CompletionRefresh = "reload" | "status" | "skip";

/** 完成事件只为「两次轮询之间」服务，保留最近一段足够覆盖正常节奏。 */
const MAX_TRACKED_COMPLETIONS = 64;

/**
 * 两次轮询之间可能完成多个后台任务。只有每个完成事件都在、且都不改变结构时
 * 才走状态行刷新；事件丢失或任意一次是结构变化时退回整图读取。
 */
function planCompletionRefresh(
  previous: number,
  next: number,
  completions: ReadonlyMap<number, BackgroundCompletion>,
  artworkId: string,
): CompletionRefresh {
  if (next <= previous) return "skip";
  if (next - previous > MAX_TRACKED_COMPLETIONS) return "reload";
  let refreshStatus = false;
  for (let revision = previous + 1; revision <= next; revision += 1) {
    const completion = completions.get(revision);
    if (!completion) return "reload";
    // 别的作品的完成不影响当前历史图。
    if (completion.artworkId !== null && completion.artworkId !== artworkId) continue;
    if (completion.structureChanged) return "reload";
    refreshStatus = true;
  }
  return refreshStatus ? "status" : "skip";
}

/** 丢掉已经消费掉的完成记录：它们只服务于自己那一段轮询间隔。 */
function forgetCompletionsThrough(
  tracked: Map<number, BackgroundCompletion>,
  through: number,
): void {
  for (const revision of [...tracked.keys()]) {
    if (revision <= through) tracked.delete(revision);
  }
}

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

export function useHistoryController({
  artworkId,
  refreshVersion,
  onHistoryChanged,
  onError,
}: HistoryControllerOptions) {
  const [history, setHistory] = useState<ArtworkHistory | null>(null);
  const historyRef = useRef<ArtworkHistory | null>(null);
  historyRef.current = history;
  const [loading, setLoading] = useState(true);
  const { busy, run: runBusy } = useAsyncOperation();
  const [runtime, setRuntime] = useState(IDLE_RUNTIME);
  const [localOperation, setLocalOperation] = useState<{ operation: string; label: string } | null>(null);
  const artworkIdRef = useRef(artworkId);
  const onHistoryChangedRef = useRef(onHistoryChanged);
  const onErrorRef = useRef(onError);
  const { begin: beginRead, invalidate: invalidateReads, isCurrent: isCurrentRead } = useRequestEpoch();
  const lastRefresh = useRef({ artworkId, refreshVersion });
  const lastCompletionRevision = useRef<number | null>(null);
  /** 已收到的后台完成事件，键为 `completionRevision`。 */
  const backgroundCompletions = useRef(new Map<number, BackgroundCompletion>());
  const runtimeRequest = useRef<Promise<void> | null>(null);
  /** 在途读取期间到达的刷新请求：当前请求结束后补读一次。 */
  const runtimeRefreshQueued = useRef(false);
  const mountedRef = useRef(true);
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

  const replaceHistory = useCallback((next: ArtworkHistory): boolean => {
    if (next.artworkId !== artworkIdRef.current) return false;
    invalidateReads();
    setHistory(next);
    onHistoryChangedRef.current(next);
    return true;
  }, [invalidateReads]);

  const reload = useCallback(async (): Promise<ArtworkHistory | null> => {
    const requestedArtworkId = artworkId;
    const requestId = beginRead();
    try {
      const next = await historyApi.get(requestedArtworkId);
      if (!isCurrentRead(requestId)
        || artworkIdRef.current !== requestedArtworkId
        || next.artworkId !== requestedArtworkId) {
        return null;
      }
      setHistory(next);
      onHistoryChangedRef.current(next);
      return next;
    } catch (error) {
      if (!isCurrentRead(requestId) || artworkIdRef.current !== requestedArtworkId) {
        return null;
      }
      throw error;
    } finally {
      if (isCurrentRead(requestId)) setLoading(false);
    }
  }, [artworkId, beginRead, isCurrentRead]);

  /**
   * 非结构变化的完成只刷新分支状态行与节点校验状态：仍读取同一份历史 DTO，
   * 但只替换 `branches` 和节点的校验字段。节点没有变化时保留 `nodes` 数组引用，
   * mindmap 不重建，缩略图缓存也不作废。这是后台检查的尽力刷新，失败不进入
   * 全局错误提示。
   */
  const mergeBranches = useCallback(async (): Promise<void> => {
    const requestedArtworkId = artworkIdRef.current;
    const requestId = beginRead();
    try {
      const next = await historyApi.get(requestedArtworkId);
      if (!isCurrentRead(requestId)
        || artworkIdRef.current !== requestedArtworkId
        || next.artworkId !== requestedArtworkId) {
        return;
      }
      const current = historyRef.current;
      const merged = current && current.artworkId === requestedArtworkId
        ? {
          ...current,
          branches: next.branches,
          nodes: mergeNodeVerifyStates(current.nodes, next.nodes),
        }
        : next;
      setHistory(merged);
      onHistoryChangedRef.current(merged);
    } catch {
      // 状态行刷新失败时保留现有 DTO；下一次操作或轮询会补上。
    } finally {
      if (isCurrentRead(requestId)) setLoading(false);
    }
  }, [beginRead, isCurrentRead]);

  const mergeBranchesRef = useRef(mergeBranches);
  mergeBranchesRef.current = mergeBranches;

  /** 详情面板的格式版本按选中节点读取；文件不可读时返回 null，不进入全局错误提示。 */
  const readNodeFormat = useCallback(async (historyId: string): Promise<string | null> => {
    try {
      return await historyApi.nodeFormat(historyId);
    } catch {
      return null;
    }
  }, []);

  /**
   * 节点缩略图是派生数据；读取失败按「暂无预览」处理，不影响历史读取。返回的
   * `supported` 决定「无可用预览」占位与刷新入口是否出现。
   */
  const readNodeThumbnail = useCallback(async (historyId: string): Promise<NodePreview> => {
    try {
      return await historyApi.nodeThumbnail(historyId);
    } catch {
      return { dataUrl: null, supported: true };
    }
  }, []);

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

  const reloadRef = useRef(reload);
  reloadRef.current = reload;

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  /**
   * 读取共享运行状态，并在 `completionRevision` 推进时刷新历史。
   * 事件只负责即时唤醒；3 秒兜底轮询保证丢事件或后台调度任务的状态最终一致。
   * 在途读取只合并并发调用，但会为期间到达的新请求补读一次：否则事件恰好落在
   * 上一次快照发出之后、返回之前时，这次唤醒就丢了，只能等 3 秒后的兜底。
   */
  const refreshRuntime = useCallback((): Promise<void> => {
    if (runtimeRequest.current) {
      runtimeRefreshQueued.current = true;
      return runtimeRequest.current;
    }
    const request = (async () => {
      try {
        const next = await historyApi.runtime();
        if (!mountedRef.current) return;
        const previousRevision = lastCompletionRevision.current;
        const operationCompleted = previousRevision !== null
          && previousRevision !== next.completionRevision;
        lastCompletionRevision.current = next.completionRevision;
        setRuntime(next);
        if (operationCompleted) {
          const previous = previousRevision ?? next.completionRevision;
          const refresh = planCompletionRefresh(
            previous,
            next.completionRevision,
            backgroundCompletions.current,
            artworkIdRef.current,
          );
          forgetCompletionsThrough(backgroundCompletions.current, next.completionRevision);
          if (refresh === "reload") await reloadRef.current();
          else if (refresh === "status") await mergeBranchesRef.current();
        }
      } catch {
        // Commands surface repository errors; the snapshot is best-effort.
      }
    })().finally(() => {
      runtimeRequest.current = null;
      if (!runtimeRefreshQueued.current) return;
      runtimeRefreshQueued.current = false;
      void refreshRuntimeRef.current();
    });
    runtimeRequest.current = request;
    return request;
  }, []);

  const refreshRuntimeRef = useRef(refreshRuntime);
  refreshRuntimeRef.current = refreshRuntime;

  useEffect(() => {
    void refreshRuntime();
    const timer = window.setInterval(() => void refreshRuntime(), 3_000);
    return () => window.clearInterval(timer);
  }, [refreshRuntime]);

  useRuntimeSubscription((event) => {
    if (event.type === "operationFinished"
      && event.domain === "backup"
      && event.structureChanged !== undefined) {
      // 后台自动备份/空闲校验会声明本次是否改变历史图结构。先记下再刷新
      // 运行状态，让随后的轮询能对同一 revision 做出「只合并分支」的决定。
      backgroundCompletions.current.set(event.completionRevision, {
        artworkId: event.artworkId ?? null,
        structureChanged: event.structureChanged,
      });
      while (backgroundCompletions.current.size > MAX_TRACKED_COMPLETIONS) {
        const oldest = backgroundCompletions.current.keys().next().value;
        if (oldest === undefined) break;
        backgroundCompletions.current.delete(oldest);
      }
    }
    if (event.type !== "operationFinished" && event.type !== "repositoryChanged") return;
    if (event.domain === "history" || event.domain === "backup" || event.domain === "authenticity") {
      void refreshRuntime();
    }
  });

  const runOperation = useCallback(async (
    operation: string,
    label: string,
    action: () => Promise<void>,
  ): Promise<boolean> => {
    invalidateReads();
    // 每次新操作都从「未请求取消」开始：上一次留下的请求只属于上一次操作。
    cancelRequested.current = false;
    setLocalOperation({ operation, label });
    onErrorRef.current(null);
    let completed = false;
    try {
      await runBusy(action, { onError: reportError });
      completed = true;
      return true;
    } catch {
      return false;
    } finally {
      setLocalOperation(null);
      void refreshRuntime();
      // 用户请求了取消但操作仍走完成功：请求落在最后一个取消检查点之后（或两条
      // 批量命令之间的空档里）。不说明的话，用户会以为取消无效。
      if (completed && cancelRequested.current) {
        onErrorRef.current("取消请求未被接受，操作已在取消到达前完成。");
      }
    }
  }, [invalidateReads, refreshRuntime, reportError, runBusy]);

  const refreshNodeThumbnail = useCallback(async (historyId: string): Promise<string | null> => {
    let value: string | null = null;
    const succeeded = await runOperation("thumbnail", "正在重新提取缩略图", async () => {
      value = await historyApi.refreshNodeThumbnail(historyId);
    });
    return succeeded ? value : null;
  }, [runOperation]);

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
    runOperation("verify", "正在重新校验分支历史", async () => {
      // 前台立即执行：逐节点物化并验哈希，同时写回节点级校验状态。
      replaceHistory(await historyApi.reverifyBranch(branchId));
    }), [replaceHistory, runOperation]);

  const deleteBranch = useCallback((branchId: string) =>
    runOperation("delete-branch", "正在删除分支", async () => {
      replaceHistory(await historyApi.deleteBranch(branchId));
    }), [replaceHistory, runOperation]);

  const resetArtworkHistory = useCallback(() =>
    runOperation("reset-artwork", "正在重置作品历史", async () => {
      replaceHistory(await historyApi.resetArtworkHistory(artworkId));
    }), [artworkId, replaceHistory, runOperation]);

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
    resetArtworkHistory,
    reverifyBranch,
    deleteSubtree,
    setCheckpoint,
    forkBranch,
    renameNode,
    readNodeFormat,
    readNodeThumbnail,
    refreshNodeThumbnail,
    cancelOperation,
  };
}
