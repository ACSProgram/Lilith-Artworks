/**
 * 画板撤销/重做栈。
 *
 * 只保存快照对与游标，不认识渲染器里的条目；合并窗口与条数上限是这里的策略，
 * 撤销后的模型恢复仍由调用方完成。
 */

import type { ImageSnapshot } from "./boardSnapshot";

const HISTORY_LIMIT = 100;
const HISTORY_MERGE_MS = 350;

/** 一次可撤销操作的前后快照；`mergeKey` 相同且落在窗口内的连续操作会合并成一条。 */
export type HistoryEntry = {
  before: ImageSnapshot[];
  after: ImageSnapshot[];
  mergeKey?: string;
  timestamp: number;
};

export class HistoryStack {
  private entries: HistoryEntry[] = [];
  private cursor = 0;

  constructor(
    private readonly limit = HISTORY_LIMIT,
    private readonly mergeWindowMs = HISTORY_MERGE_MS,
    private readonly clock: () => number = () => performance.now(),
  ) {}

  get length(): number {
    return this.entries.length;
  }

  get position(): number {
    return this.cursor;
  }

  get canUndo(): boolean {
    return this.cursor > 0;
  }

  get canRedo(): boolean {
    return this.cursor < this.entries.length;
  }

  get items(): readonly HistoryEntry[] {
    return this.entries;
  }

  clear(): void {
    this.entries = [];
    this.cursor = 0;
  }

  push(before: ImageSnapshot[], after: ImageSnapshot[], mergeKey?: string): void {
    if (JSON.stringify(before) === JSON.stringify(after)) return;
    const timestamp = this.clock();
    const previous = this.entries[this.cursor - 1];
    if (mergeKey
      && this.cursor === this.entries.length
      && previous?.mergeKey === mergeKey
      && timestamp - previous.timestamp <= this.mergeWindowMs) {
      previous.after = after;
      previous.timestamp = timestamp;
      return;
    }
    this.entries.splice(this.cursor);
    this.entries.push({ before, after, mergeKey, timestamp });
    if (this.entries.length > this.limit) this.entries.shift();
    this.cursor = this.entries.length;
  }

  undo(): HistoryEntry | null {
    if (!this.canUndo) return null;
    this.cursor -= 1;
    return this.entries[this.cursor];
  }

  redo(): HistoryEntry | null {
    if (!this.canRedo) return null;
    const entry = this.entries[this.cursor];
    this.cursor += 1;
    return entry;
  }
}
