import { createContext, useContext } from "react";

export interface NoticeContextValue {
  /** `null` 关闭提示；同一时刻只保留一条，与 0.2 的行为一致。 */
  notify: (message: string | null) => void;
}

/**
 * 提示通道的独立上下文模块，理由同 `appRuntimeContext.ts`：与 Provider 同文件会在
 * 热更新时换掉 context 对象身份，让消费者 `useContext` 拿到 `null` 而抛错。
 */
export const NoticeContext = createContext<NoticeContextValue | null>(null);

export function useNotice(): NoticeContextValue {
  const context = useContext(NoticeContext);
  if (!context) {
    throw new Error("useNotice 必须在 NoticeCenter 内使用");
  }
  return context;
}
