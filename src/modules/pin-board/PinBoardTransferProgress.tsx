import { LoaderCircle } from "lucide-react";
import type { PinBoardTransferProgress as PinBoardTransferProgressState } from "./types";

/**
 * 传输进度条：导入图片、导出图片与整画布导出共用同一条。
 *
 * 一行放下四件事——在做什么、走了多少、百分比、取消。取消和进度条同排，是因为用户
 * 点它的时机正是「盯着这条进度不想再等了」；不可取消的传输（加文字这类瞬时动作）
 * 不渲染按钮，避免点了没反应的假入口。
 */
export function PinBoardTransferProgress({
  progress,
  onCancel,
}: {
  progress: PinBoardTransferProgressState;
  onCancel: () => void;
}) {
  const total = Math.max(progress.total, 1);
  const percent = Math.min(100, Math.max(0, Math.round((progress.current / total) * 100)));
  return (
    <div className="pin-board-transfer" role="status">
      <LoaderCircle className="spin" size={14} aria-hidden="true" />
      <span className="pin-board-transfer-label">{progress.label}</span>
      <span
        className="pin-board-transfer-track"
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent}
        aria-label={progress.label}
      >
        <span className="pin-board-transfer-fill" style={{ width: `${percent}%` }} />
      </span>
      <span className="pin-board-transfer-percent">{percent}%</span>
      {progress.cancellable && (
        <button type="button" className="pin-board-transfer-cancel" onClick={onCancel}>
          取消
        </button>
      )}
    </div>
  );
}
