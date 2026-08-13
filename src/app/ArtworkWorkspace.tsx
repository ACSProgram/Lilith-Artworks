import { useEffect, useState } from "react";
import { Fingerprint, GitCommitVertical, ShieldCheck } from "lucide-react";
import { AuthenticityModule } from "../modules/authenticity/AuthenticityModule";
import type { CertificationRecord } from "../modules/authenticity/types";
import { HistoryModule } from "../modules/history/HistoryModule";
import { historyApi } from "../modules/history/api";
import type { ArtworkBranch } from "../modules/history/types";

type WorkspaceView = "history" | "publish" | "identify";

interface ArtworkWorkspaceProps {
  artworkId: string;
  initialView?: WorkspaceView;
  initialBranchId?: string | null;
  initialRecordId?: string | null;
  onError: (message: string | null) => void;
  onNavigateRecord: (record: CertificationRecord) => void;
}

export function ArtworkWorkspace({
  artworkId,
  initialView = "history",
  initialBranchId = null,
  initialRecordId = null,
  onError,
  onNavigateRecord,
}: ArtworkWorkspaceProps) {
  const [view, setView] = useState<WorkspaceView>(initialView);
  const [title, setTitle] = useState("");
  const [branches, setBranches] = useState<ArtworkBranch[]>([]);
  const [branchId, setBranchId] = useState<string | null>(initialBranchId);

  useEffect(() => {
    setView(initialView);
    historyApi.get(artworkId).then((history) => {
      setTitle(history.artworkTitle);
      setBranches(history.branches);
      setBranchId((current) => {
        const preferred = initialBranchId ?? current;
        return preferred && history.branches.some((branch) => branch.id === preferred)
          ? preferred
          : history.branches[0]?.id ?? null;
      });
    }).catch((error) => onError(error instanceof Error ? error.message : String(error)));
  }, [artworkId, initialBranchId, initialView, onError]);

  return <div className="artwork-workspace">
    <nav className="artwork-tabs" aria-label="Artwork 工作区">
      <button className={view === "history" ? "active" : ""} type="button" onClick={() => setView("history")}><GitCommitVertical size={16} />版本历史</button>
      <button className={view === "publish" ? "active" : ""} type="button" onClick={() => setView("publish")}><ShieldCheck size={16} />发布与认证</button>
      <button className={view === "identify" ? "active" : ""} type="button" onClick={() => setView("identify")}><Fingerprint size={16} />识别与溯源</button>
    </nav>
    <div className="artwork-view">
      {view === "history" ? <HistoryModule artworkId={artworkId} onError={onError} /> :
        <AuthenticityModule
          mode={view}
          artworkTitle={title}
          branches={branches}
          selectedBranchId={branchId}
          selectedRecordId={initialRecordId}
          onSelectBranch={setBranchId}
          onError={onError}
          onNavigateRecord={onNavigateRecord}
        />}
    </div>
  </div>;
}
