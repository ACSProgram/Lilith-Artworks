export interface CleanupFailure {
  id: string;
  path: string;
  error: string;
}

export interface CleanupReport {
  cleanedCount: number;
  pendingCount: number;
  failures: CleanupFailure[];
}

/** 待清理队列中的一条条目；与后端 `cleanup::PendingCleanupEntry` 对齐。 */
export interface PendingCleanupEntry {
  id: string;
  path: string;
  pathKind: string;
  reason: string;
  createdMs: number;
  /** 上次删除尝试时间；尚未尝试过时为空。 */
  lastAttemptMs: number | null;
  /** 上次删除失败原因；尚未尝试或上次成功时为空。 */
  lastError: string | null;
  /** 入队时记录的期望 SHA-256；外部文件条目必填，删除前据此比对内容。 */
  expectedSha256: string | null;
  /** 外部文件条目所属作品标题；按导出路径回查不到记录时为空。 */
  artworkTitle: string | null;
}

/**
 * 外部文件条目（`path_kind = 'external_file'`）保存仓库之外的绝对路径，删除它
 * 等于一条任意路径删除，因此**不参与自动重放**：应用启动与「全部重试」都只处理
 * 仓库内条目，外部条目必须在设置页逐条确认后凭显式 id 执行。
 */
export function isExternalCleanupEntry(entry: PendingCleanupEntry): boolean {
  return entry.pathKind === "external_file";
}

/** 未引用文件扫描候选；与后端 `cleanup::ScanCandidate` 对齐，只报告不删除。 */
export interface UnreferencedScanCandidate {
  path: string;
  byteSize: number;
  reason: string;
}

/**
 * 入队原因的展示文案。后端原因字符串是稳定的内部标识，这里只做展示层映射；
 * 未登记的原因直接回退为原字符串，避免新增入队点时静默丢失信息。
 */
const CLEANUP_REASON_LABELS: Record<string, string> = {
  pin_board_finalize: "画板结算",
  pin_board_permanent_deletion: "永久删除画板",
  pin_board_trash_emptied: "清空画板回收站",
  history_branch_deletion: "删除分支",
  history_subtree_deletion: "删除历史子树",
  history_checkpoint_release: "取消检查点",
  history_commit_release: "提交释放旧快照",
  history_snapshot_replaced: "修复快照替换",
  history_compaction: "精简历史",
  permanent_artwork_deletion: "永久删除作品",
  cancel_branch_publication: "取消发布",
  publish_certification: "认证导出",
  unreferenced_scan: "未引用文件扫描",
};

export function cleanupReasonLabel(reason: string): string {
  return CLEANUP_REASON_LABELS[reason] ?? reason;
}
