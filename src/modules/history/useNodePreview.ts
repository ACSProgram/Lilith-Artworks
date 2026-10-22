import { useEffect, useRef, useState } from "react";
import type { MouseEvent } from "react";
import type { HistoryViewMode } from "./historyModel";
import type { PreviewAnchor } from "./NodePreview";
import type { ArtworkHistory, HistoryNode, NodePreview } from "./types";

export interface NodeFormatState {
  nodeId: string;
  value: string | null;
  loading: boolean;
}

export interface NodeThumbnailState {
  nodeId: string;
  value: NodePreview | null;
  loading: boolean;
}

interface HoverState {
  nodeId: string;
  anchor: PreviewAnchor;
  value: NodePreview;
}

interface NodePreviewInput {
  artworkId: string;
  view: HistoryViewMode;
  history: ArtworkHistory | null;
  selectedNode: HistoryNode | null;
  readNodeFormat: (historyId: string) => Promise<string | null>;
  readNodeThumbnail: (historyId: string) => Promise<NodePreview>;
  refreshNodeThumbnail: (historyId: string) => Promise<string | null>;
}

/** 首次显示的静止阈值。 */
const INITIAL_HOVER_DELAY_MS = 300;
/** 已有预览时的会话内切换阈值：立即切换。 */
const SESSION_HOVER_DELAY_MS = 0;
/** 指针离开所有卡片后的关闭宽限。 */
const HOVER_CLOSE_DELAY_MS = 500;
/** 滚动结束后多久内不触发新的预览。 */
const SCROLL_GUARD_MS = 250;
/** 小于这个位移的指针抖动不重置静止计时。 */
const MOVE_TOLERANCE_PX = 2;

/**
 * 节点格式、缩略图与总览悬停浮层的前端状态。
 *
 * 悬停采用“停住才看”的粘性模型：
 * - 指针在卡片内静止到阈值才显示；移动中不触发，也不读取缩略图。
 * - 首次显示等得久一点，本次比对会话内切换立即生效。
 * - 显示后不因离开卡片立即关闭；切换节点时旧图保持，新图就绪后才替换。
 *
 * 缓存按节点保存，历史刷新时整份作废（提交、检查点与手动刷新都可能改写缩略图）；
 * 选中节点的读取与悬停读取共用同一份缓存。这里只管读取与展示状态，不进入历史图
 * 轮询，也不触发恢复或工作文件读取。
 */
export function useNodePreview({
  artworkId,
  view,
  history,
  selectedNode,
  readNodeFormat,
  readNodeThumbnail,
  refreshNodeThumbnail,
}: NodePreviewInput) {
  const [nodeFormat, setNodeFormat] = useState<NodeFormatState | null>(null);
  const [nodeThumbnail, setNodeThumbnail] = useState<NodeThumbnailState | null>(null);
  const [hover, setHover] = useState<HoverState | null>(null);
  /** 正在重新抽取缩略图的节点；只影响悬停浮层上的刷新按钮。 */
  const [hoverRefreshingNodeId, setHoverRefreshingNodeId] = useState<string | null>(null);
  /** 缩略图按节点缓存在内存里：同一节点重复悬停与选中不再重复请求。 */
  const previewCache = useRef(new Map<string, NodePreview>());
  /** 同一节点的在途请求，避免重复进入时并发读取。 */
  const inFlight = useRef(new Map<string, Promise<NodePreview>>());
  const cacheGeneration = useRef(0);
  /** 悬停状态的同步镜像，供定时器与异步回调用。 */
  const hoverRef = useRef<HoverState | null>(null);
  /** 指针当前所在的卡片；移开后迟到的读取不再显示。 */
  const activeNodeId = useRef<string | null>(null);
  const lastPointer = useRef<{ nodeId: string; x: number; y: number } | null>(null);
  const pendingNodeId = useRef<string | null>(null);
  const pendingElement = useRef<HTMLButtonElement | null>(null);
  const dwellTimer = useRef<number | null>(null);
  const closeTimer = useRef<number | null>(null);
  const scrollGuardUntil = useRef(0);

  const setHoverState = (next: HoverState | null) => {
    hoverRef.current = next;
    setHover(next);
  };

  const clearDwell = () => {
    if (dwellTimer.current !== null) {
      window.clearTimeout(dwellTimer.current);
      dwellTimer.current = null;
    }
    pendingNodeId.current = null;
    pendingElement.current = null;
  };

  const clearClose = () => {
    if (closeTimer.current === null) return;
    window.clearTimeout(closeTimer.current);
    closeTimer.current = null;
  };

  const closeHover = () => {
    clearDwell();
    clearClose();
    activeNodeId.current = null;
    setHoverState(null);
  };

  const loadThumbnail = (nodeId: string): Promise<NodePreview> => {
    const existing = inFlight.current.get(nodeId);
    if (existing) return existing;
    const generation = cacheGeneration.current;
    const request = readNodeThumbnail(nodeId).then((value) => {
      inFlight.current.delete(nodeId);
      if (cacheGeneration.current === generation) previewCache.current.set(nodeId, value);
      return value;
    });
    inFlight.current.set(nodeId, request);
    return request;
  };

  /** 格式不支持时保持既有约定：不出现浮层。 */
  const applyHover = (nodeId: string, anchor: PreviewAnchor, value: NodePreview) => {
    if (!value.dataUrl && !value.supported) {
      setHoverState(null);
      return;
    }
    setHoverState({ nodeId, anchor, value });
  };

  const openPreview = (nodeId: string, anchor: PreviewAnchor) => {
    const cached = previewCache.current.get(nodeId);
    if (cached) {
      if (activeNodeId.current === nodeId) applyHover(nodeId, anchor, cached);
      return;
    }
    // 同一节点已在读取：等那份结果回来时再判断指针是否还在它上面。
    if (inFlight.current.has(nodeId)) return;
    void loadThumbnail(nodeId).then((value) => {
      // 指针已经移开或换到别的卡片时只写缓存，不把迟到的结果弹出来。
      if (activeNodeId.current !== nodeId) return;
      applyHover(nodeId, anchor, value);
    });
  };

  const openPending = () => {
    const target = pendingNodeId.current;
    const element = pendingElement.current;
    pendingNodeId.current = null;
    pendingElement.current = null;
    if (!target || !element || !element.isConnected || activeNodeId.current !== target) return;
    // 位置在真正显示时再量：移动过程中不读布局，滚动后也不会用旧矩形。
    const rect = element.getBoundingClientRect();
    openPreview(target, { top: rect.top, left: rect.left, right: rect.right });
  };

  /** 在卡片内重新开始静止计时；会话内阈值为 0 时立即切换。 */
  const startDwell = (event: MouseEvent<HTMLButtonElement>, node: HistoryNode) => {
    clearDwell();
    activeNodeId.current = node.id;
    lastPointer.current = { nodeId: node.id, x: event.clientX, y: event.clientY };
    pendingNodeId.current = node.id;
    pendingElement.current = event.currentTarget;
    const guard = Math.max(0, scrollGuardUntil.current - Date.now());
    const delay = (hoverRef.current ? SESSION_HOVER_DELAY_MS : INITIAL_HOVER_DELAY_MS) + guard;
    if (delay <= 0) {
      openPending();
      return;
    }
    dwellTimer.current = window.setTimeout(() => {
      dwellTimer.current = null;
      openPending();
    }, delay);
  };

  /**
   * 悬停预览只在总览提供：当前分支视图的「节点信息」栏本来就会显示选中节点的
   * 缩略图，再弹一层浮层是重复的。格式不支持缩略图时不显示任何浮层。
   */
  const handleNodeEnter = (event: MouseEvent<HTMLButtonElement>, node: HistoryNode) => {
    if (view !== "overview") return;
    clearClose();
    if (hoverRef.current?.nodeId === node.id) {
      // 已经显示的就是它：粘住，不重开也不重读。
      activeNodeId.current = node.id;
      clearDwell();
      return;
    }
    startDwell(event, node);
  };

  const handleNodeMove = (event: MouseEvent<HTMLButtonElement>, node: HistoryNode) => {
    if (view !== "overview") return;
    if (hoverRef.current?.nodeId === node.id) return;
    // 会话内阈值为 0 时每次移动都会走到这里；正在读取的节点等结果回来再切。
    if (inFlight.current.has(node.id)) return;
    const last = lastPointer.current;
    if (last && last.nodeId === node.id
      && Math.abs(event.clientX - last.x) < MOVE_TOLERANCE_PX
      && Math.abs(event.clientY - last.y) < MOVE_TOLERANCE_PX) {
      return;
    }
    startDwell(event, node);
  };

  /**
   * 指针离开卡片不关闭浮层：节点间移动时常会短暂离开，关闭会让画面闪。
   * 只有离开所有卡片超过宽限时间才收起；重新进入卡片会取消这个计时。
   */
  const scheduleHoverClose = () => {
    if (view !== "overview") return;
    activeNodeId.current = null;
    clearDwell();
    clearClose();
    closeTimer.current = window.setTimeout(() => {
      closeTimer.current = null;
      if (activeNodeId.current !== null) return;
      setHoverState(null);
    }, HOVER_CLOSE_DELAY_MS);
  };

  /** 指针进入浮层里的刷新按钮时，取消卡片离开排定的关闭。 */
  const cancelHoverClose = () => {
    clearClose();
  };

  // 详情面板的格式版本只按选中的节点读取文件头；历史刷新后节点对象会更新，
  // 因此检查点替换快照等操作也会重新读取。
  useEffect(() => {
    if (!selectedNode || view !== "branch") {
      setNodeFormat(null);
      return;
    }
    let cancelled = false;
    setNodeFormat({ nodeId: selectedNode.id, value: null, loading: true });
    void readNodeFormat(selectedNode.id).then((value) => {
      if (!cancelled) setNodeFormat({ nodeId: selectedNode.id, value, loading: false });
    });
    return () => {
      cancelled = true;
    };
  }, [readNodeFormat, selectedNode, view]);

  // 缩略图随节点行保存，选中时单独读取；历史刷新后重新拉取，不进入图轮询。
  // 只有节点集合真的换了才作废缓存：后台检查的刷新会替换节点的校验字段，那
  // 不该让已取到的缩略图重读一次。
  const nodeIdKey = history?.nodes.map((node) => node.id).join("|");
  useEffect(() => {
    cacheGeneration.current += 1;
    previewCache.current.clear();
    inFlight.current.clear();
    closeHover();
  }, [artworkId, nodeIdKey]);

  useEffect(() => {
    if (!selectedNode || view !== "branch") {
      setNodeThumbnail(null);
      return;
    }
    const cached = previewCache.current.get(selectedNode.id);
    setNodeThumbnail({ nodeId: selectedNode.id, value: cached ?? null, loading: !cached });
    if (cached) return;
    let cancelled = false;
    void readNodeThumbnail(selectedNode.id).then((value) => {
      if (cancelled) return;
      previewCache.current.set(selectedNode.id, value);
      setNodeThumbnail({ nodeId: selectedNode.id, value, loading: false });
    });
    return () => {
      cancelled = true;
    };
  }, [readNodeThumbnail, selectedNode, view]);

  // 滚动与缩放会同时让锚点失效：收起浮层，并在一小段时间内不再触发新的预览，
  // 避免滚轮扫过一排卡片时反复弹出。
  useEffect(() => {
    const handleViewportChange = () => {
      scrollGuardUntil.current = Date.now() + SCROLL_GUARD_MS;
      closeHover();
    };
    window.addEventListener("scroll", handleViewportChange, true);
    window.addEventListener("resize", handleViewportChange);
    return () => {
      window.removeEventListener("scroll", handleViewportChange, true);
      window.removeEventListener("resize", handleViewportChange);
    };
  }, []);

  useEffect(() => {
    if (!hover) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") closeHover();
    };
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target as HTMLElement | null;
      if (target?.closest(".history-node-card") || target?.closest(".history-hover-preview")) return;
      closeHover();
    };
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("pointerdown", onPointerDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("pointerdown", onPointerDown);
    };
  }, [hover?.nodeId]);

  // 切到当前分支视图时收起总览留下的浮层。
  useEffect(() => {
    if (view !== "overview") closeHover();
  }, [view]);

  useEffect(() => () => {
    clearDwell();
    clearClose();
  }, []);

  const refreshThumbnail = (node: HistoryNode) => {
    const previous = previewCache.current.get(node.id);
    setNodeThumbnail((current) => current?.nodeId === node.id ? { ...current, loading: true } : current);
    if (hoverRef.current?.nodeId === node.id) setHoverRefreshingNodeId(node.id);
    void refreshNodeThumbnail(node.id).then((dataUrl) => {
      setHoverRefreshingNodeId((current) => current === node.id ? null : current);
      // 抽取不到内容的刷新不能抹掉仍然存着的预览：后端只在抽取成功时才替换该行。
      // 选中已经切到别的节点时状态归读取 effect 管理，迟到的结果不得覆盖它。
      const value: NodePreview = dataUrl
        ? { dataUrl, supported: previous?.supported ?? true }
        : { dataUrl: previous?.dataUrl ?? null, supported: previous?.supported ?? true };
      if (dataUrl) {
        previewCache.current.set(node.id, value);
        if (hoverRef.current?.nodeId === node.id) {
          setHoverState({ ...hoverRef.current, value });
        }
      }
      setNodeThumbnail((current) => current?.nodeId === node.id ? { ...current, value, loading: false } : current);
    });
  };

  return {
    nodeFormat,
    nodeThumbnail,
    hover,
    hoverRefreshingNodeId,
    handleNodeEnter,
    handleNodeMove,
    scheduleHoverClose,
    cancelHoverClose,
    closeHover,
    refreshThumbnail,
  };
}
