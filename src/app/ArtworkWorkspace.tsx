import { useCallback, useEffect, useState } from "react";
import { Fingerprint, GitCommitVertical, ShieldCheck } from "lucide-react";
import { AuthenticityModule } from "../modules/authenticity/AuthenticityModule";
import type { CertificationRecord } from "../modules/authenticity/types";
import { HistoryModule } from "../modules/history/HistoryModule";
import type { ArtworkBranch, ArtworkHistory } from "../modules/history/types";
import type { CleanupReport } from "../shared/fileCleanup";

type WorkspaceView = "history" | "publish" | "identify";

interface ArtworkWorkspaceProps {
  artworkId: string;
  initialView?: WorkspaceView;
  initialBranchId?: string | null;
  initialRecordId?: string | null;
  onError: (message: string | null) => void;
  onNavigateRecord: (record: CertificationRecord) => void;
  onRetryFileCleanup: (ids: string[]) => Promise<CleanupReport>;
}

export function ArtworkWorkspace({
  artworkId,
  initialView = "history",
  initialBranchId = null,
  initialRecordId = null,
  onError,
  onNavigateRecord,
  onRetryFileCleanup,
}: ArtworkWorkspaceProps) {
  const [view, setView] = useState<WorkspaceView>(initialView);
  const [title, setTitle] = useState("");
  const [branches, setBranches] = useState<ArtworkBranch[]>([]);
  const [branchId, setBranchId] = useState<string | null>(initialBranchId);
  const [historyRefreshVersion, setHistoryRefreshVersion] = useState(0);

  const applyWorkspaceHistory = useCallback((history: ArtworkHistory) => {
    setTitle(history.artworkTitle);
    setBranches(history.branches);
    setBranchId((current) =>
      current && history.branches.some((branch) => branch.id === current)
        ? current
        : history.branches[0]?.id ?? null
    );
  }, []);

  const refreshAfterPublication = useCallback(async () => {
    setHistoryRefreshVersion((current) => current + 1);
  }, []);

  useEffect(() => {
    setView(initialView);
    setTitle("");
    setBranches([]);
    setBranchId(initialBranchId);
  }, [artworkId, initialBranchId, initialView]);

  return <div className="artwork-workspace">
    <nav className="artwork-tabs" aria-label="Artwork 工作区">
      <button className={view === "history" ? "active" : ""} type="button" onClick={() => setView("history")}><GitCommitVertical size={16} />版本历史</button>
      <button className={view === "publish" ? "active" : ""} type="button" onClick={() => setView("publish")}><ShieldCheck size={16} />发布与认证</button>
      <button className={view === "identify" ? "active" : ""} type="button" onClick={() => setView("identify")}><Fingerprint size={16} />识别与溯源</button>
    </nav>
    <div className="artwork-view">
      <div hidden={view !== "history"} className="workspace-view-pane"><HistoryModule artworkId={artworkId} selectedBranchId={branchId} refreshVersion={historyRefreshVersion} onSelectBranch={setBranchId} onHistoryChanged={applyWorkspaceHistory} onError={onError} /></div>
      <div hidden={view !== "publish"} className="workspace-view-pane"><AuthenticityModule mode="publish" artworkTitle={title} branches={branches} selectedBranchId={branchId} selectedRecordId={initialRecordId} onSelectBranch={setBranchId} onError={onError} onNavigateRecord={onNavigateRecord} onRetryFileCleanup={onRetryFileCleanup} onPublicationChanged={refreshAfterPublication} /></div>
      <div hidden={view !== "identify"} className="workspace-view-pane"><AuthenticityModule mode="identify" artworkTitle={title} branches={branches} selectedBranchId={branchId} selectedRecordId={initialRecordId} onSelectBranch={setBranchId} onError={onError} onNavigateRecord={onNavigateRecord} onRetryFileCleanup={onRetryFileCleanup} /></div>
    </div>
  </div>;
}
