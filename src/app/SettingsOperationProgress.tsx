import { LoaderCircle, X } from "lucide-react";
import type { BackupRuntimeStatus } from "./types";

export function SettingsOperationProgress({
  runtime,
  cancelPending,
  onCancel,
}: {
  runtime: BackupRuntimeStatus;
  cancelPending: boolean;
  onCancel: () => void;
}) {
  const hasTotal = runtime.progressTotal > 0;
  const percent = hasTotal
    ? Math.round((runtime.progressCurrent / runtime.progressTotal) * 100)
    : 0;
  return (
    <div className="settings-operation-progress" role="status" aria-live="polite">
      <div>
        <LoaderCircle className="spin" aria-hidden="true" size={15} />
        <span>{runtime.progressLabel ?? "正在处理"}</span>
        <strong>{hasTotal ? `${percent}%` : "准备中"}</strong>
      </div>
      <progress value={runtime.progressCurrent} max={Math.max(runtime.progressTotal, 1)} />
      <button
        className="icon-button"
        type="button"
        title="取消当前操作"
        aria-label="取消当前操作"
        disabled={cancelPending}
        onClick={onCancel}
      >
        {cancelPending ? <LoaderCircle className="spin" aria-hidden="true" size={15} /> : <X aria-hidden="true" size={15} />}
      </button>
    </div>
  );
}
