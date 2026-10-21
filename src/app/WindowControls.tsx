import { Minus, Square, X } from "lucide-react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { errorMessage } from "../shared/errors";

interface WindowControlsProps {
  /** 窗口命令失败时的上报出口。根兜底里 provider 已经卸载，省略即静默忽略。 */
  onError?: (message: string) => void;
}

/**
 * 无边框窗口的最小化 / 最大化 / 关闭三连。标题栏与根错误兜底共用同一份标记，
 * 免得两处各写一遍之后图标尺寸、按钮顺序或关闭按钮的红色悬停悄悄跑偏。
 *
 * 窗口对象每次点击才取：`getCurrentWindow()` 在非 Tauri 宿主（浏览器预览、jsdom）
 * 会直接抛错，取早了会让整块界面连渲染都过不去。同步抛错与 Promise 拒绝走同一个出口。
 */
export function WindowControls({ onError }: WindowControlsProps) {
  const run = (command: (win: ReturnType<typeof getCurrentWindow>) => Promise<void>) => {
    try {
      void command(getCurrentWindow()).catch((error: unknown) => onError?.(errorMessage(error)));
    } catch (error) {
      onError?.(errorMessage(error));
    }
  };

  return (
    <div className="window-controls" aria-label="窗口控制">
      <button className="window-control" type="button" title="最小化" onClick={() => run((win) => win.minimize())}>
        <Minus aria-hidden="true" size={16} />
      </button>
      <button className="window-control" type="button" title="最大化或还原" onClick={() => run((win) => win.toggleMaximize())}>
        <Square aria-hidden="true" size={13} />
      </button>
      <button className="window-control close" type="button" title="关闭" onClick={() => run((win) => win.close())}>
        <X aria-hidden="true" size={17} />
      </button>
    </div>
  );
}
