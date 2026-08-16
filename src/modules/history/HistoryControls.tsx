import { open, save as saveDialog } from "@tauri-apps/plugin-dialog";
import {
  AlertTriangle, Ban, Check, ChevronDown, CircleDot, Copy, FileOutput, FilePenLine,
  GitBranch, GitFork, LoaderCircle, Trash2, X,
} from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import type { ArtworkBranch, HistoryNode, UpdateBranchBackupRequest } from "./types";

type SaveState = "saved" | "dirty" | "saving" | "error";

export function BranchScheduleStatus({ branch }: { branch: ArtworkBranch }) {
  const [copyState, setCopyState] = useState<"idle" | "copied" | "failed">("idle");

  useEffect(() => setCopyState("idle"), [branch.id, branch.lastError]);

  const copyError = async () => {
    if (!branch.lastError) return;
    try {
      await navigator.clipboard.writeText(branch.lastError);
      setCopyState("copied");
    } catch {
      setCopyState("failed");
    }
  };

  const status = branch.finalArtifactLocked
    ? `成品已锁定 · 已发布 ${branch.publishedCount} 条`
    : branch.lastError && !branch.backupEnabled
      ? `自动备份已关闭（连续 ${branch.consecutiveBackupFailures} 次失败）`
      : branch.lastError
        ? "备份失败，将按策略重试"
        : branch.backupEnabled
          ? `每 ${branch.backupIntervalMinutes} 分钟自动备份 · ${branch.lastSuccessMs ? `最近成功 ${new Date(branch.lastSuccessMs).toLocaleString()}` : "等待首次检查"}`
          : "自动备份已关闭";

  return <div className={`branch-schedule${branch.lastError ? " error" : ""}`}>
    {branch.finalArtifactLocked
      ? <Ban aria-hidden="true" size={16} />
      : branch.lastError
        ? <AlertTriangle aria-hidden="true" size={16} />
        : branch.backupEnabled
          ? <Check aria-hidden="true" size={16} />
          : null}
    <span className="branch-schedule-summary">{status}</span>
    {branch.lastError && <details className="branch-error-details">
      <summary className="icon-button" role="button" aria-label="查看备份失败详情" title="查看备份失败详情">
        <ChevronDown aria-hidden="true" size={14} />
      </summary>
      <div className="branch-error-popover">
        <header>
          <strong>备份失败详情</strong>
          <button
            className="icon-button"
            type="button"
            aria-label="复制备份失败详情"
            title={copyState === "copied" ? "已复制" : copyState === "failed" ? "复制失败，请手动选择错误文本" : "复制完整错误"}
            onClick={() => void copyError()}
          >
            {copyState === "copied" ? <Check aria-hidden="true" size={14} /> : <Copy aria-hidden="true" size={14} />}
          </button>
        </header>
        <pre>{branch.lastError}</pre>
        {copyState === "failed" && <small role="status">复制失败，请手动选择错误文本。</small>}
      </div>
    </details>}
  </div>;
}

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
  const [sourcePath, setSourcePath] = useState(branch.sourcePath);
  const [saveState, setSaveState] = useState<SaveState>("saved");
  const requestVersion = useRef(0);
  const persisted = useRef({
    id: branch.id,
    title: branch.title,
    enabled: branch.backupEnabled,
    interval: branch.backupIntervalMinutes,
    sourcePath: branch.sourcePath,
  });

  useEffect(() => {
    const previous = persisted.current;
    const branchChanged = previous.id !== branch.id;
    setTitle((current) => branchChanged || current === previous.title ? branch.title : current);
    setEnabled((current) => branchChanged || current === previous.enabled
      ? branch.backupEnabled
      : current);
    setInterval((current) => branchChanged || current === previous.interval
      ? branch.backupIntervalMinutes
      : current);
    setSourcePath((current) => branchChanged || current === previous.sourcePath
      ? branch.sourcePath
      : current);
    persisted.current = {
      id: branch.id,
      title: branch.title,
      enabled: branch.backupEnabled,
      interval: branch.backupIntervalMinutes,
      sourcePath: branch.sourcePath,
    };
    requestVersion.current += 1;
    if (branchChanged) setSaveState("saved");
  }, [
    branch.backupEnabled,
    branch.backupIntervalMinutes,
    branch.id,
    branch.sourcePath,
    branch.title,
  ]);

  const valid = title.trim().length > 0 && interval >= 1 && interval <= 10_080;
  const dirty = title.trim() !== branch.title
    || enabled !== branch.backupEnabled
    || interval !== branch.backupIntervalMinutes
    || sourcePath !== branch.sourcePath;

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
        expectedBackupEnabled: branch.backupEnabled,
        backupEnabled: enabled,
        backupIntervalMinutes: interval,
        ...(sourcePath !== branch.sourcePath ? { sourcePath } : {}),
      }).then(() => {
        if (requestVersion.current === version) setSaveState("saved");
      }).catch(() => {
        setSourcePath(branch.sourcePath);
        if (requestVersion.current === version) setSaveState("error");
      });
    }, 650);
    return () => window.clearTimeout(timer);
  }, [branch.id, branch.sourcePath, dirty, disabled, enabled, interval, onSave, sourcePath, title, valid]);

  return <div className="branch-settings">
    <div className="branch-primary">
      <GitBranch aria-hidden="true" size={18} />
      <div className="branch-summary">
        <div className="branch-title-row">
          <label className="branch-title-field">
            <span>当前分支</span>
            <input
              aria-label="名称"
              value={title}
              maxLength={160}
              disabled={disabled}
              onChange={(event) => setTitle(event.target.value)}
            />
          </label>
          <SaveIndicator state={saveState} />
        </div>
        <div className="branch-source-file">
          <span>工作文件</span>
          <strong aria-label="工作文件" title={sourcePath}>{sourcePath}</strong>
          <button className="icon-button" type="button" title="修改工作文件" disabled={disabled || branch.finalArtifactLocked} onClick={async () => {
            const value = await open({ directory: false, multiple: false, title: "选择分支工作文件" });
            if (typeof value === "string") setSourcePath(value);
          }}><FilePenLine aria-hidden="true" size={15} /></button>
        </div>
      </div>
    </div>
    <div className="branch-backup-controls">
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
    </div>
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
