import { LoaderCircle, X } from "lucide-react";
import type { ReactNode } from "react";

export interface ConfirmView {
  icon: ReactNode;
  eyebrow: string;
  title: string;
  subject: string;
  description: string;
  detail?: string | null;
  action: string;
  danger?: boolean;
}

/**
 * In-app confirmation dialog shared by every module. Native `window.confirm`
 * is not reliable inside the Tauri webview, so irreversible actions must use
 * this service-owned dialog instead.
 */
export function ConfirmDialog({ view, busy, onCancel, onConfirm }: {
  view: ConfirmView;
  busy: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  return <div className="dialog-backdrop" onMouseDown={onCancel}>
    <section
      className={`confirm-dialog${view.danger ? " danger" : ""}`}
      role="alertdialog"
      aria-modal="true"
      aria-labelledby="confirm-title"
      onMouseDown={(event) => event.stopPropagation()}
    >
      <header>
        <span className="confirm-icon">{view.icon}</span>
        <div><small>{view.eyebrow}</small><h2 id="confirm-title">{view.title}</h2></div>
        <button className="icon-button" type="button" title="关闭" onClick={onCancel}><X size={18} /></button>
      </header>
      <div className="confirm-body">
        <strong>{view.subject}</strong>
        <p>{view.description}</p>
        {view.detail && <div className="confirm-detail">{view.detail}</div>}
      </div>
      <footer>
        <button className="text-button" type="button" onClick={onCancel}>取消</button>
        <button className={view.danger ? "danger-button solid" : "primary-button"} type="button" disabled={busy} onClick={onConfirm}>
          {busy && <LoaderCircle className="spin" size={15} />}{view.action}
        </button>
      </footer>
    </section>
  </div>;
}
