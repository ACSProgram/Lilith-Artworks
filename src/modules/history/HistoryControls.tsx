import { open, save as saveDialog } from "@tauri-apps/plugin-dialog";
import {
  AlertTriangle, Check, CircleDot, FileOutput, GitFork, LoaderCircle, Trash2, X,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import type { ArtworkBranch, HistoryNode, UpdateBranchBackupRequest } from "./types";

type SaveState = "saved" | "dirty" | "saving" | "error";

export function BranchSettings({
  branch,
  disabled,
  onSave,
}: {
  branch: ArtworkBranch;
  disabled: boolean;
  onSave: (request: UpdateBranchBackupRequest) => Promise<void>;
}) {
  const [title, setTitle] = useState(branch.title);
  const [enabled, setEnabled] = useState(branch.backupEnabled);
  const [interval, setInterval] = useState(branch.backupIntervalMinutes);
  const [saveState, setSaveState] = useState<SaveState>("saved");
  const requestVersion = useRef(0);

  useEffect(() => {
    setTitle(branch.title);
    setEnabled(branch.backupEnabled);
    setInterval(branch.backupIntervalMinutes);
    setSaveState("saved");
    requestVersion.current += 1;
  }, [branch.id]);

  const valid = title.trim().length > 0 && interval >= 1 && interval <= 10_080;
  const dirty = title.trim() !== branch.title
    || enabled !== branch.backupEnabled
    || interval !== branch.backupIntervalMinutes;

  useEffect(() => {
    if (!dirty || !valid || disabled) {
      if (!dirty) setSaveState("saved");
      return;
    }
    setSaveState("dirty");
    const version = ++requestVersion.current;
    const timer = window.setTimeout(() => {
      setSaveState("saving");
      void onSave({
        branchId: branch.id,
        title: title.trim(),
        backupEnabled: enabled,
        backupIntervalMinutes: interval,
      }).then(() => {
        if (requestVersion.current === version) setSaveState("saved");
      }).catch(() => {
        if (requestVersion.current === version) setSaveState("error");
      });
    }, 650);
    return () => window.clearTimeout(timer);
  }, [branch.id, dirty, disabled, enabled, interval, onSave, title, valid]);

  return <div className="branch-settings">
    <div className="branch-settings-heading">
      <strong>分支设置</strong>
      <SaveIndicator state={saveState} />
    </div>
    <label className="branch-title-field">
      <span>名称</span>
      <input
        value={title}
        maxLength={160}
        disabled={disabled}
        onChange={(event) => setTitle(event.target.value)}
      />
    </label>
    <label className="switch-field">
      <span className="switch-copy"><strong>自动备份</strong><small>{enabled ? "按间隔运行" : "当前已关闭"}</small></span>
      <input
        className="switch-input"
        type="checkbox"
        checked={enabled}
        disabled={disabled || branch.finalArtifactLocked}
        onChange={(event) => setEnabled(event.target.checked)}
      />
    </label>
    <label className="interval-field">
      <span>间隔</span>
      <input
        type="number"
        min={1}
        max={10_080}
        value={interval}
        disabled={disabled || !enabled || branch.finalArtifactLocked}
        onChange={(event) => setInterval(Number(event.target.value))}
      />
      <span>分钟</span>
    </label>
  </div>;
}

function SaveIndicator({ state }: { state: SaveState }) {
  return <span className={`save-state ${state}`} role="status">
    {state === "saving" && <LoaderCircle className="spin" size={13} />}
    {state === "saved" && <Check size={13} />}
    {state === "error" && <AlertTriangle size={13} />}
    {state === "saving" ? "保存中" : state === "dirty" ? "未保存" : state === "error" ? "保存失败" : "已保存"}
  </span>;
}

export function ForkDialog({
  node,
  busy,
  onClose,
  onSubmit,
}: {
  node: HistoryNode;
  busy: boolean;
  onClose: () => void;
  onSubmit: (title: string, sourcePath: string) => Promise<void>;
}) {
  const [title, setTitle] = useState(`${node.title} 分支`);
  const [sourcePath, setSourcePath] = useState("");
  const choose = async () => {
    const value = await open({ directory: false, multiple: false, title: "选择分支工作文件" });
    if (typeof value === "string") setSourcePath(value);
  };
  return <div className="dialog-backdrop" onMouseDown={onClose}>
    <form className="node-editor" role="dialog" aria-modal="true" onMouseDown={(event) => event.stopPropagation()} onSubmit={(event) => {
      event.preventDefault();
      if (title.trim() && sourcePath && !busy) void onSubmit(title.trim(), sourcePath);
    }}>
      <header><div><small>创建独立工作线</small><h2>从“{node.title}”创建分支</h2></div><button className="icon-button" type="button" title="关闭" onClick={onClose}><X size={18} /></button></header>
      <div className="editor-fields">
        <label><span>分支名称</span><input autoFocus maxLength={160} value={title} onChange={(event) => setTitle(event.target.value)} /></label>
        <label><span>工作文件</span><div className="path-control"><input value={sourcePath} readOnly /><button className="secondary-button" type="button" onClick={() => void choose()}>浏览</button></div></label>
      </div>
      <footer><button className="text-button" type="button" onClick={onClose}>取消</button><button className="primary-button" type="submit" disabled={!title.trim() || !sourcePath || busy}><GitFork size={16} />创建分支</button></footer>
    </form>
  </div>;
}

export function EditNodeDialog({
  initialValue,
  busy,
  onClose,
  onSubmit,
}: {
  initialValue: string;
  busy: boolean;
  onClose: () => void;
  onSubmit: (value: string) => Promise<void>;
}) {
  const [value, setValue] = useState(initialValue);
  return <div className="dialog-backdrop" onMouseDown={onClose}>
    <form className="node-editor compact-dialog" role="dialog" aria-modal="true" onMouseDown={(event) => event.stopPropagation()} onSubmit={(event) => {
      event.preventDefault();
      if (value.trim() && !busy) void onSubmit(value.trim());
    }}>
      <header><div><small>历史节点</small><h2>编辑节点名称</h2></div><button className="icon-button" type="button" title="关闭" onClick={onClose}><X size={18} /></button></header>
      <div className="editor-fields"><label><span>名称</span><input autoFocus maxLength={160} value={value} onChange={(event) => setValue(event.target.value)} /></label></div>
      <footer><button className="text-button" type="button" onClick={onClose}>取消</button><button className="primary-button" type="submit" disabled={!value.trim() || busy}>保存</button></footer>
    </form>
  </div>;
}

export type ConfirmRequest =
  | { kind: "delete-nodes"; node: HistoryNode; branch: ArtworkBranch; descendantCount: number }
  | { kind: "delete-branch"; node: HistoryNode; branch: ArtworkBranch }
  | { kind: "compact"; node: HistoryNode; selectedCount: number }
  | { kind: "checkpoint"; node: HistoryNode; enable: boolean };

export function ConfirmDialog({
  request,
  busy,
  onClose,
  onConfirm,
}: {
  request: ConfirmRequest;
  busy: boolean;
  onClose: () => void;
  onConfirm: () => void;
}) {
  const copy = useMemo(() => confirmCopy(request), [request]);
  return <div className="dialog-backdrop" onMouseDown={onClose}>
    <section className={`confirm-dialog ${copy.danger ? "danger" : ""}`} role="alertdialog" aria-modal="true" aria-labelledby="confirm-title" onMouseDown={(event) => event.stopPropagation()}>
      <header><span className="confirm-icon">{copy.icon}</span><div><small>{copy.eyebrow}</small><h2 id="confirm-title">{copy.title}</h2></div><button className="icon-button" type="button" title="关闭" onClick={onClose}><X size={18} /></button></header>
      <div className="confirm-body"><strong>{copy.subject}</strong><p>{copy.description}</p>{copy.detail && <div className="confirm-detail">{copy.detail}</div>}</div>
      <footer><button className="text-button" type="button" onClick={onClose}>取消</button><button className={copy.danger ? "danger-button solid" : "primary-button"} type="button" disabled={busy} onClick={onConfirm}>{busy && <LoaderCircle className="spin" size={15} />}{copy.action}</button></footer>
    </section>
  </div>;
}

function confirmCopy(request: ConfirmRequest) {
  if (request.kind === "delete-branch") return {
    danger: true,
    icon: <Trash2 size={20} />,
    eyebrow: "不可撤销",
    title: "删除分支",
    subject: request.branch.title,
    description: "这个分支独有的历史链与 Chunk 文件将被永久删除；被其他分支共享的祖先节点会保留。",
    detail: "如分支已经发布成品，必须先移除成品。",
    action: "确认删除分支",
  };
  if (request.kind === "delete-nodes") return {
    danger: true,
    icon: <Trash2 size={20} />,
    eyebrow: "不可撤销",
    title: "删除节点与后续历史",
    subject: request.node.title,
    description: `将从“${request.branch.title}”删除此节点及后续 ${request.descendantCount} 个节点，并自动把分支回退到前一个保留节点。`,
    detail: "如果后续历史属于其他完整分支，操作会被拒绝，请先删除对应分支。",
    action: "确认永久删除",
  };
  if (request.kind === "compact") return {
    danger: false,
    icon: <CircleDot size={20} />,
    eyebrow: "重新整理历史链",
    title: "执行精简",
    subject: `已选择 ${request.selectedCount} 个中间节点`,
    description: "程序会逐个重建相邻节点的反向增量并重新连接历史链，所选节点随后被永久移除。",
    detail: "分支 head、fork 起点、分叉点和检查点不会被选中。",
    action: "开始精简",
  };
  return {
    danger: false,
    icon: <FileOutput size={20} />,
    eyebrow: request.enable ? "生成完整快照" : "恢复增量存储",
    title: request.enable ? "设为检查点" : "取消检查点",
    subject: request.node.title,
    description: request.enable
      ? "程序需要沿历史链回溯并生成完整 snapshot，期间会显示进度。"
      : "程序会释放完整 snapshot，并恢复为用于回溯的差值 Chunk 统计。",
    detail: request.enable ? "耗时取决于历史链长度和原文件大小。" : "关键分支节点的检查点不能取消。",
    action: request.enable ? "确认并生成" : "确认取消",
  };
}

export async function chooseRestoreOutput(defaultPath: string): Promise<string | null> {
  const selected = await saveDialog({ title: "恢复历史节点到文件", defaultPath });
  return typeof selected === "string" ? selected : null;
}
