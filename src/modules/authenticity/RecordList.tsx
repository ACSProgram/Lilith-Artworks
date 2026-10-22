import { BadgeCheck } from "lucide-react";
import { formatBytes } from "../../shared/format";
import type { CertificationRecord } from "./types";

interface RecordListProps {
  records: CertificationRecord[];
  onNavigate: (record: CertificationRecord) => void;
  /** 紧凑列表用于识别结果与记录搜索，不显示列表标题。 */
  compact?: boolean;
  selectedId?: string | null;
  /** 识别结果里该记录的候选证据来源；发布页的记录列表不传。 */
  evidence?: Record<string, string[]>;
}

export function RecordList({ records, onNavigate, compact = false, selectedId = null, evidence = {} }: RecordListProps) {
  return <section className={`record-list${compact ? " compact" : ""}`}>
    {!compact && <header><strong>分支导出记录</strong><span>{records.length} 条</span></header>}
    {records.length === 0 ? <div className="records-empty">没有匹配的导出记录。</div> : records.map((record) => <button type="button" data-record-id={record.id} className={`record-row${selectedId === record.id ? " selected" : ""}`} key={record.id} onClick={() => onNavigate(record)}>
      <BadgeCheck size={16} />
      <span><strong>{record.title}</strong><small>{record.creator || "作者未声明"} · {record.artworkTitle} / {record.branchTitle} · {new Date(record.createdMs).toLocaleString()}</small><small>{record.outputPath} · {formatBytes(record.outputBytes)}</small>{evidence[record.id] && <small>候选证据：{evidence[record.id].map((source) => source === "c2pa" ? "C2PA" : "TrustMark").join(" + ")}</small>}<code>{record.watermarkId ?? "无 TrustMark"}</code></span>
      <i>{record.trustmarkEnabled ? "C2PA + TrustMark" : "C2PA"}</i>
    </button>)}
  </section>;
}
