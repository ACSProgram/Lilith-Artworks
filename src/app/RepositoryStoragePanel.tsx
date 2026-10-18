import { useEffect, useMemo, useState } from "react";
import {
  ChartPie,
  Database,
  HardDrive,
  LoaderCircle,
  RefreshCw,
  ScanLine,
} from "lucide-react";
import { formatBytes } from "../shared/format";
import { SettingsOperationProgress } from "./SettingsOperationProgress";
import type {
  BackupRuntimeStatus,
  RepositoryStorageDiskScan,
  RepositoryStorageOverview,
  StorageCategoryKind,
} from "./types";

const CATEGORY_LABELS: Record<StorageCategoryKind, string> = {
  snapshot: "历史快照",
  delta: "历史增量",
  finalArtifact: "最终成品",
  certification: "认证输出",
  boardDds: "画板图片（DDS）",
  other: "其它",
};

const CATEGORY_ORDER: StorageCategoryKind[] = [
  "snapshot",
  "delta",
  "finalArtifact",
  "certification",
  "boardDds",
  "other",
];

function percentOf(bytes: number, total: number): number {
  if (total <= 0) return 0;
  return Math.min(100, Math.max(0, (bytes / total) * 100));
}

function formatRatio(ratio: number | null): string {
  if (ratio == null) return "—";
  const digits = ratio >= 100 ? 0 : ratio >= 10 ? 1 : 2;
  return `${ratio.toFixed(digits)}×`;
}

interface RepositoryStoragePanelProps {
  ready: boolean;
  overview: RepositoryStorageOverview | null;
  scan: RepositoryStorageDiskScan | null;
  loading: boolean;
  error: string | null;
  scanning: boolean;
  cancelPending: boolean;
  runtime: BackupRuntimeStatus | null;
  onRefresh: () => void;
  onScan: () => void;
  onCancelScan: () => void;
}

export function RepositoryStoragePanel({
  ready,
  overview,
  scan,
  loading,
  error,
  scanning,
  cancelPending,
  runtime,
  onRefresh,
  onScan,
  onCancelScan,
}: RepositoryStoragePanelProps) {
  const [mode, setMode] = useState<"recorded" | "disk">("recorded");
  const scannedAt = scan?.scannedAtMs;

  useEffect(() => {
    if (scannedAt) setMode("disk");
  }, [scannedAt]);

  const effectiveMode = mode === "disk" && scan ? "disk" : "recorded";
  const total = effectiveMode === "disk" && scan
    ? scan.totalBytes
    : overview?.databaseRecordedBytes ?? 0;
  const categories = (effectiveMode === "disk" && scan
    ? scan.categories
    : overview?.categories ?? [])
    .slice()
    .sort((left, right) => CATEGORY_ORDER.indexOf(left.kind) - CATEGORY_ORDER.indexOf(right.kind));
  const diskByArtwork = useMemo(
    () => new Map((scan?.artworkBytes ?? []).map((entry) => [entry.artworkId, entry.bytes])),
    [scan],
  );
  const ddsByBoard = useMemo(
    () => new Map((scan?.ddsByBoard ?? []).map((entry) => [entry.boardId, entry.bytes])),
    [scan],
  );
  /** 作品在当前口径下的字节数：磁盘口径取扫描结果，记录口径取数据库聚合。 */
  const artworkBytesById = useMemo(() => {
    if (effectiveMode === "disk") return diskByArtwork;
    return new Map(
      (overview?.artworks ?? []).map((artwork) => [artwork.artworkId, artwork.recordedBytes]),
    );
  }, [diskByArtwork, effectiveMode, overview]);
  const sortedArtworks = useMemo(
    () => [...(overview?.artworks ?? [])].sort(
      (left, right) => (artworkBytesById.get(right.artworkId) ?? 0)
        - (artworkBytesById.get(left.artworkId) ?? 0),
    ),
    [artworkBytesById, overview],
  );
  const unassignedRecordedBytes = overview?.unassigned.reduce(
    (sum, entry) => sum + entry.bytes,
    0,
  ) ?? 0;
  const unassignedDiskBytes = scan
    ? Math.max(
      0,
      scan.totalBytes - scan.artworkBytes.reduce((sum, entry) => sum + entry.bytes, 0),
    )
    : 0;
  const unassignedBytes = effectiveMode === "disk" ? unassignedDiskBytes : unassignedRecordedBytes;

  if (!ready) {
    return (
      <div className="storage-empty" role="status">
        <HardDrive aria-hidden="true" size={20} />
        <strong>仓库不可用</strong>
        <small>配置并打开作品仓库后，这里会显示仓储空间状态。</small>
      </div>
    );
  }

  if (!overview && loading) {
    return (
      <div className="storage-empty" role="status">
        <LoaderCircle className="spin" aria-hidden="true" size={20} />
        <strong>正在读取仓储空间…</strong>
      </div>
    );
  }

  if (!overview && error) {
    return (
      <div className="storage-empty is-error" role="alert">
        <strong>无法读取仓储空间</strong>
        <small>{error}</small>
        <button className="secondary-button" type="button" onClick={onRefresh}>重试</button>
      </div>
    );
  }

  return (
    <div className="storage-overview">
      <div className="storage-toolbar">
        <div className="storage-toolbar-copy">
          <strong>仓储空间</strong>
          <small>只读统计；数据库记录与仓库目录的磁盘实际分开呈现，不做静默合并。</small>
        </div>
        <div className="storage-toolbar-actions">
          <button className="secondary-button" type="button" onClick={onRefresh} disabled={loading || scanning}>
            {loading ? <LoaderCircle className="spin" aria-hidden="true" size={15} /> : <RefreshCw aria-hidden="true" size={15} />}
            刷新
          </button>
          <button className="secondary-button" type="button" onClick={onScan} disabled={scanning || loading}>
            {scanning ? <LoaderCircle className="spin" aria-hidden="true" size={15} /> : <ScanLine aria-hidden="true" size={15} />}
            {scan ? "重新扫描磁盘" : "扫描磁盘占用"}
          </button>
        </div>
      </div>

      {scanning && runtime && (
        <SettingsOperationProgress
          runtime={runtime}
          cancelPending={cancelPending}
          onCancel={onCancelScan}
        />
      )}

      {scan && (
        <small className="storage-scan-note">
          磁盘扫描于 {new Date(scan.scannedAtMs).toLocaleString()} · {scan.fileCount} 个文件
        </small>
      )}

      {error && overview && (
        <p className="storage-warning storage-refresh-warning">
          刷新失败，以下仍是上一次读取的结果：{error}
        </p>
      )}

      <div className="segmented-control storage-mode-control" role="radiogroup" aria-label="统计口径">
        <button
          type="button"
          role="radio"
          aria-checked={effectiveMode === "recorded"}
          className={effectiveMode === "recorded" ? "active" : ""}
          onClick={() => setMode("recorded")}
        >
          数据库记录
        </button>
        <button
          type="button"
          role="radio"
          aria-checked={effectiveMode === "disk"}
          className={effectiveMode === "disk" ? "active" : ""}
          disabled={!scan}
          onClick={() => setMode("disk")}
        >
          磁盘实际
        </button>
      </div>

      <div className="storage-total-grid">
        <div>
          <small>数据库记录</small>
          <strong>{formatBytes(overview?.databaseRecordedBytes ?? 0)}</strong>
          <span>历史 + 成品 + 认证输出</span>
        </div>
        <div>
          <small>磁盘实际</small>
          <strong>{scan ? formatBytes(scan.totalBytes) : "未扫描"}</strong>
          <span>仅仓库目录，不含外部导出文件</span>
        </div>
        <div>
          <small>快照压缩比</small>
          <strong>{formatRatio(overview?.compression.snapshot.ratio ?? null)}</strong>
          <span>逻辑大小 / 快照文件</span>
        </div>
        <div>
          <small>增量还原比</small>
          <strong>{formatRatio(overview?.compression.delta.ratio ?? null)}</strong>
          <span>父节点逻辑 / 增量文件</span>
        </div>
      </div>

      <div className="settings-section storage-section">
        <div className="settings-section-title"><ChartPie aria-hidden="true" size={17} /><h3>分类占比</h3></div>
        {total === 0 ? (
          <p className="storage-muted">仓库暂无数据。</p>
        ) : (
          <div className="storage-category-list">
            {categories.map((category) => {
              const percent = percentOf(category.bytes, total);
              return (
                <div className="storage-category-row" key={category.kind}>
                  <div className="storage-category-copy">
                    <strong>{CATEGORY_LABELS[category.kind]}</strong>
                    <small>{category.fileCount} 个文件</small>
                  </div>
                  <span>{formatBytes(category.bytes)} · {percent.toFixed(1)}%</span>
                  <div className="storage-category-bar" aria-hidden="true">
                    <span style={{ width: `${percent}%` }} />
                  </div>
                </div>
              );
            })}
          </div>
        )}
        {effectiveMode === "disk" && scan && (
          <div className="storage-other-breakdown">
            <span>SQLite / WAL：{formatBytes(scan.otherBreakdown.sqliteBytes)}</span>
            <span>仓库 temp：{formatBytes(scan.otherBreakdown.tempBytes)}</span>
            <span>未识别文件：{formatBytes(scan.otherBreakdown.unrecognizedBytes)}</span>
          </div>
        )}
        {scan && scan.missingFiles.length > 0 && (
          <p className="storage-warning">
            数据库记录了但磁盘上未找到：
            {scan.missingFiles
              .map((entry) => `${CATEGORY_LABELS[entry.kind]} ${entry.count}`)
              .join("、")}
          </p>
        )}
      </div>

      <div className="settings-section storage-section">
        <div className="settings-section-title"><Database aria-hidden="true" size={17} /><h3>作品占比与明细</h3></div>
        {sortedArtworks.length === 0 && unassignedBytes === 0 ? (
          <p className="storage-muted">还没有作品数据。</p>
        ) : (
          <div className="storage-artwork-list">
            {sortedArtworks.map((artwork) => {
              const bytes = artworkBytesById.get(artwork.artworkId) ?? 0;
              return (
                <details className="storage-artwork" key={artwork.artworkId}>
                  <summary>
                    <div className="storage-artwork-copy">
                      <strong>{artwork.title}{artwork.trashed ? " · 回收站" : ""}</strong>
                      <small>
                        {artwork.groupPath.length > 0 ? artwork.groupPath.join(" / ") : "根级"}
                        {" · "}{artwork.branchCount} 个分支 · {artwork.nodeCount} 个节点
                      </small>
                    </div>
                    <div className="storage-artwork-size">
                      <strong>{formatBytes(bytes)}</strong>
                      <small>{percentOf(bytes, total).toFixed(1)}%</small>
                    </div>
                  </summary>
                  <div className="storage-artwork-body">
                    {artwork.branches.map((branch) => (
                      <details className="storage-branch" key={branch.branchId}>
                        <summary>
                          <span>{branch.title}</span>
                          <small>{effectiveMode === "disk" ? "记录 " : ""}{formatBytes(branch.recordedBytes)}</small>
                        </summary>
                        <div className="storage-branch-body">
                          <p className="storage-branch-summary">
                            快照 {formatBytes(branch.snapshotBytes)} ·
                            增量 {formatBytes(branch.deltaBytes)} ·
                            成品 {formatBytes(branch.finalArtifactBytes)} ·
                            认证 {formatBytes(branch.certificationBytes)}
                          </p>
                          {branch.finalArtifact && (
                            <div className="storage-record-row">
                              <span>最终成品</span>
                              <small title={branch.finalArtifact.sourcePath}>{branch.finalArtifact.sourcePath}</small>
                              <strong>{formatBytes(branch.finalArtifact.byteSize)}</strong>
                            </div>
                          )}
                          {branch.certificationRecords.map((record) => (
                            <div className="storage-record-row" key={record.id}>
                              <span>{record.title || "认证记录"}</span>
                              <small title={record.outputPath}>{record.outputPath}</small>
                              <strong>{formatBytes(record.outputBytes)}</strong>
                            </div>
                          ))}
                          {branch.nodes.map((node) => (
                            <div className="storage-node-row" key={node.id}>
                              <div className="storage-node-copy">
                                <strong>{node.title}</strong>
                                <small>
                                  {new Date(node.createdMs).toLocaleString()} ·
                                  {node.isCheckpoint ? " 检查点" : " 普通节点"} ·
                                  {node.hasSnapshot ? " 快照" : " 增量"}
                                </small>
                              </div>
                              <div className="storage-node-size">
                                <span>逻辑 {formatBytes(node.logicalSize)}</span>
                                <span>{node.hasSnapshot ? "快照文件" : "重建读取"} {formatBytes(node.chunkFileSize)}</span>
                                <span>{node.chunkCount} 块</span>
                                <span>{formatRatio(node.compressionRatio)}</span>
                              </div>
                            </div>
                          ))}
                        </div>
                      </details>
                    ))}
                    {artwork.boards.length > 0 && (
                      <div className="storage-board-list">
                        {artwork.boards.map((board) => {
                          const ddsBytes = ddsByBoard.get(board.boardId);
                          return (
                            <div className="storage-board-row" key={board.boardId}>
                              <span>{board.name}{board.trashed ? " · 回收站" : ""}</span>
                              <small>
                                {board.imageCount} 张
                                {ddsBytes != null ? ` · ${formatBytes(ddsBytes)}` : " · 未扫描磁盘"}
                              </small>
                            </div>
                          );
                        })}
                      </div>
                    )}
                  </div>
                </details>
              );
            })}
            {unassignedBytes > 0 && (
              <div className="storage-artwork is-unassigned">
                <div className="storage-artwork-copy">
                  <strong>未归属</strong>
                  <small>
                    {effectiveMode === "disk"
                      ? "SQLite / WAL、仓库 temp 与未识别文件"
                      : overview?.unassigned.map((entry) => CATEGORY_LABELS[entry.kind]).join("、")}
                  </small>
                </div>
                <div className="storage-artwork-size">
                  <strong>{formatBytes(unassignedBytes)}</strong>
                  <small>{percentOf(unassignedBytes, total).toFixed(1)}%</small>
                </div>
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
