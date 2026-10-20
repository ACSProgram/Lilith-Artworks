import { useEffect, useState } from "react";
import {
  pendingSettleState,
  subscribePendingSettle,
  type PendingSettleState,
} from "./pendingSettle";

/**
 * 订阅后台结算队列的展示状态。
 *
 * 「正在后台保存」延迟 `delayMs` 才出现：队列通常几十毫秒就写完，立刻显示会让一次正常
 * 切换闪一下提示。失败则立即出现并保持——那需要用户处理。
 */
export function usePendingSettle(delayMs = 800): PendingSettleState | null {
  const [state, setState] = useState<PendingSettleState>(pendingSettleState);
  const [visible, setVisible] = useState(false);

  useEffect(() => {
    setState(pendingSettleState());
    return subscribePendingSettle(() => setState(pendingSettleState()));
  }, []);

  useEffect(() => {
    if (state.failure) {
      setVisible(true);
      return undefined;
    }
    if (state.count === 0) {
      setVisible(false);
      return undefined;
    }
    const timer = window.setTimeout(() => setVisible(true), delayMs);
    return () => window.clearTimeout(timer);
  }, [state.count, state.failure, delayMs]);

  return visible && state.count > 0 ? state : null;
}
