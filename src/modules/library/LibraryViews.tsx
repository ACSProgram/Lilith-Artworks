import { FileImage, FolderPlus, GitBranch } from "lucide-react";
import type { PointerEvent as ReactPointerEvent, ReactNode } from "react";
import type { LibraryNode } from "./types";

export function CommandMenu({ className, onGroup, onArtwork, onPointerDown }: {
  className: string;
  onGroup: () => void;
  onArtwork: () => void;
  onPointerDown: (event: ReactPointerEvent) => void;
}) {
  return (
    <div className={`command-menu ${className}`} onPointerDown={onPointerDown}>
      <button type="button" onClick={onGroup}><FolderPlus aria-hidden="true" size={16} />新建分组</button>
      <button type="button" onClick={onArtwork}><FileImage aria-hidden="true" size={16} />新建 Artwork</button>
    </div>
  );
}

export function WorkspaceEmpty({ icon, title, description, action }: {
  icon: ReactNode;
  title: string;
  description: string;
  action?: ReactNode;
}) {
  return (
    <div className="empty-workspace">
      <div className="empty-icon">{icon}</div>
      <h1>{title}</h1>
      <p>{description}</p>
      {action}
    </div>
  );
}

export function NodeOverview({ node, selectedCount }: { node: LibraryNode; selectedCount: number }) {
  return (
    <div className="node-overview">
      <header>
        <div className={`overview-icon ${node.kind}`}>
          {node.kind === "group" ? <FolderPlus aria-hidden="true" size={22} /> : <FileImage aria-hidden="true" size={22} />}
        </div>
        <div>
          <span>{node.kind === "group" ? "分组" : "Artwork"}</span>
          <h1>{node.title}</h1>
        </div>
      </header>
      {selectedCount > 1 ? (
        <div className="selection-summary"><strong>{selectedCount}</strong><span>个节点已选择，可一起拖动或移到回收站。</span></div>
      ) : node.kind === "group" ? (
        <dl className="overview-facts">
          <div><dt>直接子节点</dt><dd>{node.children.length}</dd></div>
          <div><dt>最后更新</dt><dd>{new Date(node.updatedMs).toLocaleString()}</dd></div>
        </dl>
      ) : (
        <>
          <dl className="overview-facts">
            <div><dt>分支数量</dt><dd>{node.artwork?.branchCount ?? 0}</dd></div>
            <div><dt>主分支</dt><dd>{node.artwork?.primaryBranch?.title ?? "未创建"}</dd></div>
          </dl>
          <section className="source-file-band">
            <GitBranch aria-hidden="true" size={18} />
            <div><span>工作文件</span><strong>{node.artwork?.primaryBranch?.sourcePath ?? "-"}</strong></div>
          </section>
        </>
      )}
    </div>
  );
}
