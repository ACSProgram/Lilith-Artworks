/**
 * 前端诊断层与取证 Worker 之间的消息协议。
 *
 * 两侧共用同一份类型定义，避免消息结构在两处各写一遍后逐渐漂移。本模块只含类型，
 * 编译后不产生运行时代码，因此主线程与 Worker 不会因此产生运行时耦合。
 */

/** 主线程 → Worker 的消息。 */
export type ForensicsRequest =
  | { type: "start" }
  | { type: "stop" }
  | { type: "beat"; at: number; seq: number; op: string };

/** 一次冻结的取证记录，由 Worker 写入 IndexedDB 并在下次启动时回读。 */
export interface FreezeRecord {
  /** Worker 判定冻结的时刻（`Date.now()`）。 */
  at: number;
  /** 判定时的静默时长。 */
  silenceMs: number;
  /** 最后一次心跳的墙上时刻与 JS 侧序号。 */
  lastBeatAt: number;
  lastBeatSeq: number;
  lastOp: string;
  /** 主线程若在冻结后恢复，补记恢复时刻；缺失表示直到退出都未恢复。 */
  recoveredAt?: number;
}

/** Worker → 主线程的消息。 */
export type ForensicsEvent =
  | { type: "frozen"; silenceMs: number; lastBeatSeq: number; lastOp: string }
  | { type: "recovered"; silenceMs: number; recoveredAt: number }
  | { type: "previous-freeze"; record: FreezeRecord };
