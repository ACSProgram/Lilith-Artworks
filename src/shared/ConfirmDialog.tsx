import { LoaderCircle, X } from "lucide-react";
import type { ReactNode } from "react";
import { SlideToConfirm } from "./SlideToConfirm";

export interface ConfirmView {
  icon: ReactNode;
  eyebrow: string;
  title: string;
  subject: string;
  description: string;
  detail?: string | null;
  /** 页脚主操作（或滑动确认控件）上的文字。 */
  action: string;
  danger?: boolean;
  /** 最危险的一档视觉：在 danger 基础上进一步加强红色警示。 */
  emphasis?: boolean;
  /**
   * 最高风险的确认：页脚不提供可点击的确认按钮，改为 `SlideToConfirm`
   * 滑动控件，必须把滑块拖到底才会触发，避免误点误删。文案仍取自 `action`。
   *
   * 最危险的一档由三者共同组成（`danger` + `emphasis` + `slideToConfirm`），
   * 三者各自独立只是为了普通危险操作仍能只用其中一两个。
   */
  slideToConfirm?: boolean;
}

/**
 * In-app confirmation dialog shared by every module. Native `window.confirm`
 * is not reliable inside the Tauri webview, so irreversible actions must use
 * this service-owned dialog instead.
 *
 * 操作已经在运行时（`busy`），取消按钮的含义由调用方决定：只有本操作底层真的存在
 * 可中断点时才传 `onCancelOperation`，此时页脚取消按钮变成「取消操作」、并禁止关闭
 * 弹窗（背景点击与头部关闭按钮一并失效），运行期间只有一个出口。不传时保持原有
 * 语义（取消按钮只关闭弹窗），因此不可中断的操作不要把弹窗留在运行态——应在确认后
 * 立即关窗，而不是提供一个点了不生效的取消入口。契约见
 * `docs/architecture/overview.md` 的「前端共享控件」。
 */
export function ConfirmDialog({ view, busy, onCancelOperation, onCancel, onConfirm }: {
  view: ConfirmView;
  busy: boolean;
  /** 运行期间的「取消操作」入口；缺省表示当前操作不可中断。 */
  onCancelOperation?: () => void;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const cancellable = busy && Boolean(onCancelOperation);
  return <div className="dialog-backdrop" onMouseDown={cancellable ? undefined : onCancel}>
    <section
      className={`confirm-dialog${view.danger ? " danger" : ""}${view.emphasis ? " emphasis" : ""}`}
      role="alertdialog"
      aria-modal="true"
      aria-labelledby="confirm-title"
      onMouseDown={(event) => event.stopPropagation()}
    >
      <header>
        <span className="confirm-icon">{view.icon}</span>
        <div><small>{view.eyebrow}</small><h2 id="confirm-title">{view.title}</h2></div>
        <button
          className="icon-button"
          type="button"
          aria-label="关闭"
          title={cancellable ? "操作进行中，取消操作后才能关闭" : "关闭"}
          disabled={cancellable}
          onClick={onCancel}
        ><X size={18} /></button>
      </header>
      <div className="confirm-body">
        <strong>{view.subject}</strong>
        <p>{view.description}</p>
        {view.detail && <div className="confirm-detail">{view.detail}</div>}
      </div>
      <footer className={view.slideToConfirm ? "with-slide" : undefined}>
        {cancellable
          ? <button className="text-button" type="button" title="中止正在进行的操作" onClick={onCancelOperation}>取消操作</button>
          : <button className="text-button" type="button" onClick={onCancel}>取消</button>}
        {view.slideToConfirm
          ? <SlideToConfirm label={view.action} disabled={busy} onConfirm={onConfirm} />
          : <button className={view.danger ? "danger-button solid" : "primary-button"} type="button" disabled={busy} onClick={onConfirm}>
            {busy && <LoaderCircle className="spin" size={15} />}{view.action}
          </button>}
      </footer>
    </section>
  </div>;
}
