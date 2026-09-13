import { useCallback, useEffect, useState } from "react";
import { Fingerprint, GitCommitVertical, Images, ShieldCheck } from "lucide-react";
import { AuthenticityModule } from "../modules/authenticity/AuthenticityModule";
import type { CertificationRecord } from "../modules/authenticity/types";
import { HistoryModule } from "../modules/history/HistoryModule";
import type { ArtworkBranch, ArtworkHistory } from "../modules/history/types";
import { PinBoardModule } from "../modules/pin-board/PinBoardModule";
import type { PinBoardModuleSettings } from "../modules/pin-board/PinBoardModule";
import type { CleanupReport } from "../shared/fileCleanup";

type WorkspaceView = "history" | "publish" | "identify" | "pin-board";

interface ArtworkWorkspaceProps {
  artworkId: string;
  initialView?: WorkspaceView;
  initialBranchId?: string | null;
  initialRecordId?: string | null;
  navigationKey?: number;
  /** 素材板显示与缓存设置（来自应用设置）。 */
  pinBoardSettings: PinBoardModuleSettings;
  onError: (message: string | null) => void;
  onNavigateRecord: (record: CertificationRecord) => void;
  onRetryFileCleanup: (ids: string[]) => Promise<CleanupReport>;
}

export function ArtworkWorkspace({
  artworkId,
  initialView = "history",
  initialBranchId = null,
  initialRecordId = null,
  navigationKey = 0,
  pinBoardSettings,
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
  }, [artworkId]);

  useEffect(() => {
    setView(initialView);
    if (navigationKey) setBranchId(initialBranchId);
  }, [initialBranchId, initialView, navigationKey]);

  return <div className="artwork-workspace">
    <nav className="artwork-tabs" aria-label="Artwork 工作区">
      <button className={view === "pin-board" ? "active" : ""} type="button" onClick={() => setView("pin-board")}><Images size={16} />素材板</button>
      <button className={view === "history" ? "active" : ""} type="button" onClick={() => setView("history")}><GitCommitVertical size={16} />版本历史</button>
      <button className={view === "publish" ? "active" : ""} type="button" onClick={() => setView("publish")}><ShieldCheck size={16} />发布与认证</button>
      <button className={view === "identify" ? "active" : ""} type="button" onClick={() => setView("identify")}><Fingerprint size={16} />识别与溯源</button>
    </nav>
    <div className="artwork-view">
      {view === "history" && <div className="workspace-view-pane"><HistoryModule artworkId={artworkId} selectedBranchId={branchId} refreshVersion={historyRefreshVersion} onSelectBranch={setBranchId} onHistoryChanged={applyWorkspaceHistory} onError={onError} /></div>}
      {view === "publish" && <div className="workspace-view-pane"><AuthenticityModule mode="publish" artworkTitle={title} branches={branches} selectedBranchId={branchId} selectedRecordId={initialRecordId} recordNavigationKey={navigationKey} onSelectBranch={setBranchId} onError={onError} onNavigateRecord={onNavigateRecord} onRetryFileCleanup={onRetryFileCleanup} onPublicationChanged={refreshAfterPublication} /></div>}
      {view === "identify" && <div className="workspace-view-pane"><AuthenticityModule mode="identify" artworkTitle={title} branches={branches} selectedBranchId={branchId} selectedRecordId={initialRecordId} onSelectBranch={setBranchId} onError={onError} onNavigateRecord={onNavigateRecord} onRetryFileCleanup={onRetryFileCleanup} /></div>}
      {/* 素材板保持挂载：切换视图只暂停全局键盘交互与在途纹理任务，不释放 GPU 资源。
          与 Client 的 keep-alive 一致，非活跃时用 visibility 隐藏而不是 display:none，
          这样画布始终保有布局尺寸，渲染器初始化即可按最小包围框完成视图适配。 */}
      <div
        className={`workspace-view-pane pin-board-view-pane${view === "pin-board" ? " active" : ""}`}
        aria-hidden={view !== "pin-board"}
        inert={view !== "pin-board"}
      >
        <PinBoardModule
          artworkId={artworkId}
          active={view === "pin-board"}
          settings={pinBoardSettings}
        />
      </div>
    </div>
  </div>;
}
