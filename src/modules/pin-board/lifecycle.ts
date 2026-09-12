export interface PinBoardLifecycleParticipant {
  finishActiveInteraction: () => void;
  finalize: () => Promise<boolean>;
}

let participant: PinBoardLifecycleParticipant | null = null;

export function registerPinBoardLifecycleParticipant(
  next: PinBoardLifecycleParticipant,
): () => void {
  participant = next;
  return () => {
    if (participant === next) participant = null;
  };
}

export async function preparePinBoardRuntimeChange(): Promise<void> {
  const current = participant;
  if (!current) return;
  current.finishActiveInteraction();
  if (!await current.finalize()) {
    throw new Error("素材板未能在运行时变更前完成结算");
  }
}
