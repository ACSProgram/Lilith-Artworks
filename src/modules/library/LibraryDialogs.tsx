import { open } from "@tauri-apps/plugin-dialog";
import { FileImage, FolderPlus, LoaderCircle, RotateCcw, Trash2, X } from "lucide-react";
import { useState } from "react";
import { ConfirmDialog } from "../../shared/ConfirmDialog";
import type { ConfirmView } from "../../shared/ConfirmDialog";
import type { CleanupFailure } from "../../shared/fileCleanup";
import type { LibraryNode, LibraryTrashEntry } from "./types";

export type EditorMode = "group" | "artwork" | "rename";

export interface EditorState {
  mode: EditorMode;
  parentId: string | null;
  node: LibraryNode | null;
}

interface EditorValues {
  title: string;
  branchTitle: string;
  sourcePath: string;
}

export function NodeEditor({ state, busy, onClose, onSubmit }: {
  state: EditorState;
  busy: boolean;
  onClose: () => void;
  onSubmit: (values: EditorValues) => Promise<void>;
}) {
  const [title, setTitle] = useState(state.node?.title ?? "");
  const [branchTitle, setBranchTitle] = useState("主分支");
  const [sourcePath, setSourcePath] = useState("");
  const isArtwork = state.mode === "artwork";
  const heading = state.mode === "rename" ? "重命名" : isArtwork ? "新建 Artwork" : "新建分组";

  const chooseSource = async () => {
    const selected = await open({ directory: false, multiple: false });
    if (typeof selected === "string") setSourcePath(selected);
  };

  const valid = title.trim() && (!isArtwork || branchTitle.trim());
  return (
    <div className="dialog-backdrop" role="presentation" onMouseDown={onClose}>
      <form
        className="node-editor"
        role="dialog"
        aria-modal="true"
        aria-labelledby="node-editor-title"
        onMouseDown={(event) => event.stopPropagation()}
        onSubmit={(event) => {
          event.preventDefault();
          if (valid && !busy) void onSubmit({ title, branchTitle, sourcePath });
        }}
      >
        <header>
          <h2 id="node-editor-title">{heading}</h2>
          <button className="icon-button" type="button" title="关闭" onClick={onClose}><X aria-hidden="true" size={18} /></button>
        </header>
        <div className="editor-fields">
          <label><span>标题</span><input autoFocus value={title} maxLength={160} onChange={(event) => setTitle(event.target.value)} /></label>
          {isArtwork && (
            <>
              <label><span>初始分支标题</span><input value={branchTitle} maxLength={160} onChange={(event) => setBranchTitle(event.target.value)} /></label>
              <label>
                <span>工作文件（可选）</span>
                <div className="path-control"><input value={sourcePath} onChange={(event) => setSourcePath(event.target.value)} placeholder="选择现有作品文件" /><button className="secondary-button" type="button" onClick={chooseSource}>浏览</button></div>
              </label>
              {!sourcePath.trim() && <p className="editor-hint">未选择工作文件时，该分支的自动备份与主动提交不可用，但仍可使用素材板。</p>}
            </>
          )}
        </div>
        <footer>
          <button className="text-button" type="button" onClick={onClose}>取消</button>
          <button className="primary-button" type="submit" disabled={!valid || busy}>{busy && <LoaderCircle className="spin" aria-hidden="true" size={16} />}{state.mode === "rename" ? "保存" : "创建"}</button>
        </footer>
      </form>
    </div>
  );
}

export function TrashDialog({ entries, busy, cleanupFailures, onClose, onRestore, onDelete, onEmpty, onRetryCleanup }: {
  entries: LibraryTrashEntry[];
  busy: boolean;
  cleanupFailures: CleanupFailure[];
  onClose: () => void;
  onRestore: (entry: LibraryTrashEntry) => Promise<void>;
  onDelete: (entry: LibraryTrashEntry) => Promise<void>;
  onEmpty: () => Promise<void>;
  onRetryCleanup: () => Promise<void>;
}) {
  const [pending, setPending] = useState<TrashConfirmTarget | null>(null);
  const confirmView = pending ? trashConfirmView(pending) : null;
  const runPending = async () => {
    const target = pending;
    setPending(null);
    if (!target) return;
    if (target.kind === "empty") await onEmpty();
    else await onDelete(target.entry);
  };
  return (<>
    <div className="dialog-backdrop" role="presentation" onMouseDown={onClose}>
      <section className="trash-dialog" role="dialog" aria-modal="true" aria-labelledby="trash-title" onMouseDown={(event) => event.stopPropagation()}>
        <header>
          <div><h2 id="trash-title">回收站</h2><small>{entries.length} 个项目</small></div>
          <button className="icon-button" type="button" title="关闭" onClick={onClose}><X aria-hidden="true" size={18} /></button>
        </header>
        <div className="trash-list">
          {!entries.length && <div className="trash-empty"><Trash2 aria-hidden="true" size={24} /><span>回收站为空</span></div>}
          {entries.map((entry) => (
            <article key={entry.id} className="trash-row">
              <div className="trash-kind">{entry.kind === "group" ? <FolderPlus aria-hidden="true" size={18} /> : <FileImage aria-hidden="true" size={18} />}</div>
              <div className="trash-copy">
                <strong>{entry.title}</strong>
                <span>{new Date(entry.deletedMs).toLocaleString()} · {entry.artworkCount} 个 Artwork{entry.originalParentTitle ? ` · 原位置：${entry.originalParentTitle}` : " · 原位置：根目录"}</span>
              </div>
              <button className="icon-button" type="button" title="恢复" disabled={busy} onClick={() => void onRestore(entry)}><RotateCcw aria-hidden="true" size={16} /></button>
              <button className="icon-button danger-icon" type="button" title="永久删除" disabled={busy} onClick={() => setPending({ kind: "entry", entry })}><Trash2 aria-hidden="true" size={16} /></button>
            </article>
          ))}
        </div>
        {cleanupFailures.length > 0 && <div className="cleanup-failure-banner" role="status"><span><strong>{cleanupFailures.length} 个文件尚未清理</strong><small>{cleanupFailures[0].path}</small></span><button className="secondary-button" type="button" disabled={busy} onClick={() => void onRetryCleanup()}><RotateCcw aria-hidden="true" size={15} />重试清理</button></div>}
        <footer>
          <span>永久删除不会进入其他回收站。</span>
          <button className="danger-button" type="button" disabled={busy || !entries.length} onClick={() => setPending({ kind: "empty", count: entries.length })}><Trash2 aria-hidden="true" size={16} />清空回收站</button>
        </footer>
      </section>
    </div>
    {confirmView && (
      <ConfirmDialog view={confirmView} busy={busy} onCancel={() => setPending(null)} onConfirm={() => void runPending()} />
    )}
  </>);
}

type TrashConfirmTarget =
  | { kind: "empty"; count: number }
  | { kind: "entry"; entry: LibraryTrashEntry };

function trashConfirmView(target: TrashConfirmTarget): ConfirmView {
  if (target.kind === "empty") {
    return {
      icon: <Trash2 size={20} />,
      eyebrow: "不可撤销",
      title: "清空回收站",
      subject: `回收站中的 ${target.count} 个项目`,
      description: "这些分组、Artwork、分支历史与 Chunk 文件将被永久删除，无法恢复。",
      detail: "如果还需要其中的作品，请先取消并按需恢复项目。",
      action: `永久删除 ${target.count} 个项目`,
      danger: true,
    };
  }
  return {
    icon: <Trash2 size={20} />,
    eyebrow: "不可撤销",
    title: "永久删除",
    subject: target.entry.title,
    description: `将永久删除该项目及其中 ${target.entry.artworkCount} 个 Artwork 的全部历史与 Chunk 文件。`,
    detail: "此操作不进入其他回收站，也无法通过恢复历史节点找回。",
    action: "确认永久删除",
    danger: true,
  };
}
