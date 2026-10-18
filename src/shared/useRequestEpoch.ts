import { useCallback, useRef } from "react";

/**
 * 请求代次守卫：每次读取前 `begin()` 取得本次编号，异步结果回来时用
 * `isCurrent(id)` 判断是否仍属于当前代次；`invalidate()` 用于切换仓库、
 * 切换作品或发起 mutation 时让全部在途读取失效。
 *
 * 返回的四个函数身份稳定，可直接放进 `useCallback`/`useEffect` 依赖。
 */
export interface RequestEpoch {
  begin: () => number;
  invalidate: () => void;
  current: () => number;
  isCurrent: (id: number) => boolean;
}

export function useRequestEpoch(): RequestEpoch {
  const sequence = useRef(0);
  const begin = useCallback(() => {
    sequence.current += 1;
    return sequence.current;
  }, []);
  const invalidate = useCallback(() => {
    sequence.current += 1;
  }, []);
  const current = useCallback(() => sequence.current, []);
  const isCurrent = useCallback((id: number) => id === sequence.current, []);
  return { begin, invalidate, current, isCurrent };
}
