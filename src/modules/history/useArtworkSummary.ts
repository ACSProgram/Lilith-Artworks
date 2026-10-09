import { useCallback, useEffect, useRef, useState } from "react";
import { historyApi } from "./api";
import type { ArtworkHistory } from "./types";

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export interface ArtworkSummaryState {
  history: ArtworkHistory | null;
  loading: boolean;
  error: string | null;
  retry: () => void;
}

/**
 * 工作区摘要：只读取作品标题与分支列表。
 *
 * 发布页与识别页此前只能依赖历史页回调填充分支数据，因此「直接进入发布页」的跳转
 * 会在分支被填充前误判为「此 Artwork 尚无分支」。这里提供一条不依赖历史页挂载的
 * 兜底读取路径：`historyApi.get` 只有本 hook 消费，仍满足「api.ts 只由控制器消费」。
 *
 * `enabled` 为 false 时立即释放当前作品的数据且不发请求，供历史页自己负责的路径
 * 跳过重复读取。请求带代次，作品变化后旧响应不会回填。
 */
export function useArtworkSummary(artworkId: string, enabled: boolean): ArtworkSummaryState {
  const [history, setHistory] = useState<ArtworkHistory | null>(null);
  const [loading, setLoading] = useState(enabled);
  const [error, setError] = useState<string | null>(null);
  const [revision, setRevision] = useState(0);
  const request = useRef(0);

  useEffect(() => {
    if (!enabled) {
      request.current += 1;
      setHistory(null);
      setLoading(false);
      setError(null);
      return;
    }
    const requestId = ++request.current;
    setLoading(true);
    setError(null);
    historyApi.get(artworkId)
      .then((next) => {
        if (requestId !== request.current || next.artworkId !== artworkId) return;
        setHistory(next);
        setLoading(false);
      })
      .catch((failure) => {
        if (requestId !== request.current) return;
        setError(message(failure));
        setLoading(false);
      });
    return () => { request.current += 1; };
  }, [artworkId, enabled, revision]);

  const retry = useCallback(() => setRevision((current) => current + 1), []);

  return {
    // 只暴露当前作品的数据：跨作品重挂载前若旧响应仍在途也不会回填。
    history: history?.artworkId === artworkId ? history : null,
    loading,
    error,
    retry,
  };
}
