export interface ArtworkHistory {
  artworkId: string;
  artworkTitle: string;
  branches: ArtworkBranch[];
  nodes: HistoryNode[];
}

export interface ArtworkBranch {
  id: string;
  title: string;
  sourcePath: string;
  headHistoryId: string | null;
  createdFromHistoryId: string | null;
  backupEnabled: boolean;
  backupIntervalMinutes: number;
  backupQuickEnabled: boolean;
  lastCheckMs: number | null;
  lastSuccessMs: number | null;
  lastError: string | null;
  consecutiveBackupFailures: number;
  backupRetryAtMs: number | null;
  backupDisableNoticePending: boolean;
  finalArtifactLocked: boolean;
  publishedCount: number;
  /** 历史链上任一节点校验失败的摘要；与备份失败 `lastError` 相互独立。 */
  verifyError: string | null;
  /** 链上确认出问题的节点数（文件丢失或损坏）；不含「无法检查」。 */
  verifyFailureCount: number;
  /** 链上连检查都做不到的节点数：文件未必坏，只是恢复链被更深的坏节点挡住。 */
  verifyUnverifiableCount: number;
  /** 链上节点里最严重的一类：丢失 > 损坏 > 无法检查。 */
  verifyIssue: NodeVerifyIssue | null;
  /** 整条链全部通过校验时取最旧的通过时间；任一节点未验证为 null。 */
  verifiedMs: number | null;
}

/**
 * 节点校验状态的归类。前两类是「确认出问题」：恢复所需的仓库文件不在了
 * （`missing`），或文件还在但内容读不出来、与记录不符（`damaged`）。
 *
 * `unverifiable` 是**没有结论**：这个节点自己的文件可能完好，但它的还原起点在更
 * 深的节点上，那段链已经断了，谁也无法判断它还能不能恢复。既不能说它坏了，也
 * 不能像正常节点那样打勾。
 *
 * 后端从完整文案派生，前端只按它上色与选文案，不再解析错误文本。
 */
export type NodeVerifyIssue = "missing" | "damaged" | "unverifiable";

export interface HistoryNode {
  id: string;
  createdOnBranchId: string;
  parentId: string | null;
  title: string;
  note: string;
  commitKind: "manual" | "automatic";
  isCheckpoint: boolean;
  createdMs: number;
  logicalSize: number;
  chunkFileSize: number;
  sha256: string;
  chunkCount: number;
  /** 最近一次通过校验的时间；失败或尚未校验为 null。 */
  verifiedMs: number | null;
  /** 校验失败的完整文案，只在详情里展示；通过或尚未校验为 null。 */
  verifyError: string | null;
  /** 校验失败归类；通过或尚未校验为 null。 */
  verifyIssue: NodeVerifyIssue | null;
}

/**
 * 节点缩略图读取结果。`supported` 说明该节点工作文件的格式有没有抽取器：无缩略图
 * 时前端据此决定「无可用预览」占位与刷新入口是否出现，格式不支持时连占位都不显示。
 */
export interface NodePreview {
  dataUrl: string | null;
  supported: boolean;
}

export interface ForkBranchRequest {
  artworkId: string;
  fromHistoryId: string;
  title: string;
  sourcePath: string;
}

export interface UpdateBranchBackupRequest {
  branchId: string;
  title: string;
  expectedBackupEnabled: boolean;
  backupEnabled: boolean;
  backupIntervalMinutes: number;
  backupQuickEnabled: boolean;
  sourcePath?: string;
}

export interface RenameHistoryNodeRequest {
  historyId: string;
  title: string;
}

export interface BackupCommitResult {
  created: boolean;
  unchanged: boolean;
  historyId: string | null;
}

/** 当前占用共享运行锁的任务类型；与后端 `BackupTaskKind` 的序列化值一致。 */
export type BackupTaskKind = "automaticBackup" | "idleVerify" | "userOperation";

export interface BackupRuntimeStatus {
  busy: boolean;
  activeBranchId: string | null;
  taskKind: BackupTaskKind | null;
  operation: string | null;
  progressLabel: string | null;
  progressCurrent: number;
  progressTotal: number;
  automaticScheduling: boolean;
  completionRevision: number;
}
