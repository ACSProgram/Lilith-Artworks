import {
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { AlertCircle, X } from "lucide-react";
import { NoticeContext, type NoticeContextValue } from "./noticeContext";

const NOTICE_DURATION_MS = 7_000;

/**
 * 应用级提示的唯一出口。控制器只调用 `notify`，不各自维护 message state；
 * 提示条本身固定渲染一次，避免多个页面复制同一段 DOM。
 */
export function NoticeCenter({ children }: { children: ReactNode }) {
  const [message, setMessage] = useState<string | null>(null);

  useEffect(() => {
    if (!message) return undefined;
    const timer = window.setTimeout(() => setMessage(null), NOTICE_DURATION_MS);
    return () => window.clearTimeout(timer);
  }, [message]);

  const value = useMemo<NoticeContextValue>(() => ({ notify: setMessage }), []);

  return (
    <NoticeContext.Provider value={value}>
      {children}
      {message && (
        <div className="notice" role="alert">
          <AlertCircle aria-hidden="true" size={20} />
          <div><strong>操作提示</strong><span>{message}</span></div>
          <button type="button" title="关闭提示" onClick={() => setMessage(null)}>
            <X aria-hidden="true" size={16} />
          </button>
          <i aria-hidden="true" />
        </div>
      )}
    </NoticeContext.Provider>
  );
}
