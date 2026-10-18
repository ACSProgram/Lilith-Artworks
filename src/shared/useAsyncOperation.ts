import { useCallback, useState } from "react";

export interface AsyncOperationOptions {
  /** 在错误继续抛给调用方之前调用；控制器用它把错误接到统一提示通道。 */
  onError?: (error: unknown) => void;
}

export interface AsyncOperation {
  /** 是否有操作在执行；并发调用按计数合并。 */
  busy: boolean;
  /**
   * 执行一个异步操作并维护 busy。错误在可选 `onError` 之后原样抛出，
   * 调用方仍可用 try/catch 决定提交成功与否，语义与手写 finally 一致。
   */
  run: <T>(action: () => Promise<T>, options?: AsyncOperationOptions) => Promise<T>;
}

/**
 * 控制器统一的异步操作包装：忙状态计数 + finally 复位 + 可选错误上报。
 * 它不吞错误，避免「失败也像成功」地关闭对话框或清空选择。
 */
export function useAsyncOperation(): AsyncOperation {
  const [active, setActive] = useState(0);

  const run = useCallback(async <T,>(
    action: () => Promise<T>,
    options?: AsyncOperationOptions,
  ): Promise<T> => {
    setActive((current) => current + 1);
    try {
      return await action();
    } catch (error) {
      options?.onError?.(error);
      throw error;
    } finally {
      setActive((current) => Math.max(0, current - 1));
    }
  }, []);

  return { busy: active > 0, run };
}
