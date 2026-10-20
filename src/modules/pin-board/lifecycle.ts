import { flushPendingSettle } from "./pendingSettle";

export interface PinBoardLifecycleParticipant {
  finishActiveInteraction: () => void;
  finalize: () => Promise<boolean>;
}

/** 退出与切仓库时为后台结算队列留的预算：到点就放弃，退出另有原生端兜底强退。 */
const PENDING_SETTLE_FLUSH_BUDGET_MS = 5_000;

let participant: PinBoardLifecycleParticipant | null = null;

export function registerPinBoardLifecycleParticipant(
  next: PinBoardLifecycleParticipant,
): () => void {
  participant = next;
  return () => {
    if (participant === next) participant = null;
  };
}

/**
 * 运行时变更（退出、切换仓库）前的结算：当前画板真实写入仓库，再把切换作品时留下、
 * 还在后台队列里的快照一并写完。
 *
 * 切换作品本身不等写入（见 `pendingSettle`），所以队列里可能还压着别的画板的改动。
 * 退出与切仓库之后这些快照就再也写不回去了——要么换掉了仓库，要么进程没了——因此这里
 * 必须收口。失败抛错：切仓库会因此中止并保留旧仓库；退出路径只记录结果，仍继续确认
 * 退出（原生端 15 秒兜底强退保证窗口一定关得掉）。
 */
export async function preparePinBoardRuntimeChange(): Promise<void> {
  const current = participant;
  if (current) {
    current.finishActiveInteraction();
    if (!await current.finalize()) {
      throw new Error("素材板未能在运行时变更前完成结算");
    }
  }
  if (!await flushPendingSettle(PENDING_SETTLE_FLUSH_BUDGET_MS)) {
    throw new Error("素材板仍有改动未能写回仓库（后台结算队列未清空）");
  }
}
