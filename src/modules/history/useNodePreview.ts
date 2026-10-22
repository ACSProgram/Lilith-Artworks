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
  value: NodePreview | null;
  loading: boolean;
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

/**
 * 节点格式、缩略图与总览悬停浮层的前端状态。
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
  /** 缩略图按节点缓存在内存里：同一节点重复悬停与选中不再重复请求。 */
  const previewCache = useRef(new Map<string, NodePreview>());
  const hoverRequest = useRef(0);
  const hoverCloseTimer = useRef<number | null>(null);

  const clearHoverTimer = () => {
    if (hoverCloseTimer.current === null) return;
    window.clearTimeout(hoverCloseTimer.current);
    hoverCloseTimer.current = null;
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
    previewCache.current.clear();
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

  // 浮层不在卡片内部，指针从卡片移到浮层会先触发一次 leave：留一点宽限时间，
  // 由浮层自己的 enter 取消关闭。
  const scheduleHoverClose = () => {
    clearHoverTimer();
    hoverCloseTimer.current = window.setTimeout(() => setHover(null), 140);
  };

  /**
   * 悬停预览只在总览提供：当前分支视图的「节点信息」栏本来就会显示选中节点的
   * 缩略图，再弹一层浮层是重复的。格式不支持缩略图时不显示任何浮层，与悬停前
   * 完全一致。
   */
  const showHoverPreview = (event: MouseEvent<HTMLButtonElement>, node: HistoryNode) => {
    if (view !== "overview") return;
    clearHoverTimer();
    const rect = event.currentTarget.getBoundingClientRect();
    const anchor = { top: rect.top, left: rect.left, right: rect.right };
    const cached = previewCache.current.get(node.id);
    if (cached) {
      setHover({ nodeId: node.id, anchor, value: cached, loading: false });
      return;
    }
    const request = ++hoverRequest.current;
    setHover({ nodeId: node.id, anchor, value: null, loading: true });
    void readNodeThumbnail(node.id).then((value) => {
      previewCache.current.set(node.id, value);
      // 结果到达时指针可能已经移到别的节点或离开卡片，只更新仍然指向它的浮层。
      if (hoverRequest.current !== request) return;
      setHover((current) => current?.nodeId === node.id
        ? { nodeId: node.id, anchor: current.anchor, value, loading: false }
        : current);
    });
  };

  useEffect(() => {
    if (!hover) return;
    const close = () => setHover(null);
    window.addEventListener("scroll", close, true);
    window.addEventListener("resize", close);
    return () => {
      window.removeEventListener("scroll", close, true);
      window.removeEventListener("resize", close);
    };
  }, [hover?.nodeId]);

  // 切到当前分支视图时收起总览留下的浮层。
  useEffect(() => {
    if (view !== "overview") setHover(null);
  }, [view]);

  // 切换作品时旧作品的悬停浮层与缩略图缓存都不再成立。
  useEffect(() => {
    setHover(null);
    clearHoverTimer();
  }, [artworkId]);

  useEffect(() => () => clearHoverTimer(), []);

  const refreshThumbnail = (node: HistoryNode) => {
    const previous = previewCache.current.get(node.id);
    setNodeThumbnail((current) => current?.nodeId === node.id ? { ...current, loading: true } : current);
    setHover((current) => current?.nodeId === node.id ? { ...current, loading: true } : current);
    void refreshNodeThumbnail(node.id).then((dataUrl) => {
      // 抽取不到内容的刷新不能抹掉仍然存着的预览：后端只在抽取成功时才替换该行。
      // 选中已经切到别的节点时状态归读取 effect 管理，迟到的结果不得覆盖它。
      const value: NodePreview = dataUrl
        ? { dataUrl, supported: previous?.supported ?? true }
        : { dataUrl: previous?.dataUrl ?? null, supported: previous?.supported ?? true };
      if (dataUrl) previewCache.current.set(node.id, value);
      setNodeThumbnail((current) => current?.nodeId === node.id ? { ...current, value, loading: false } : current);
      setHover((current) => current?.nodeId === node.id ? { ...current, value, loading: false } : current);
    });
  };

  return {
    nodeFormat,
    nodeThumbnail,
    hover,
    showHoverPreview,
    scheduleHoverClose,
    clearHoverTimer,
    refreshThumbnail,
  };
}
