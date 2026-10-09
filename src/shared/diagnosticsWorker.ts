/**
 * 诊断取证 Worker：唯一能扛过主线程冻结的取证通道。
 *
 * 主线程冻结时它写不出任何日志，经 IPC 转发的心跳也一并停摆——**日志本身走的就是
 * 那条已经死掉的通道**。本 Worker 运行在**独立线程**上，能在主线程停摆期间继续计时，
 * 并把「最后一次心跳时刻 / 最后操作 / 静默时长」写入 IndexedDB（由浏览器进程落盘，
 * 不经过主线程）。下次启动时读回并写进应用日志，即可拿到冻结瞬间的第一手证据。
 *
 * 两点为什么必须是 IndexedDB：
 *
 * - `localStorage` 在 Worker 里不可用；
 * - `localStorage` 即便可用也挂在主线程上，冻结时同样写不进去。
 *
 * 主线程与 Worker 之间只有「心跳上报」一条单向通道；判定静默完全由 Worker 自己完成，
 * 不依赖主线程回话，否则判定逻辑会随主线程一起死掉。
 */

import type { ForensicsEvent, ForensicsRequest, FreezeRecord } from "./diagnosticsProtocol";

/** 本文件运行在 Worker 线程。用最小接口描述作用域，避免与 DOM 库的类型定义冲突。 */
interface WorkerScope {
  onmessage: ((event: MessageEvent) => void) | null;
  postMessage(message: ForensicsEvent): void;
  setInterval(handler: () => void, timeout: number): number;
  clearInterval(id: number): void;
}

const scope = self as unknown as WorkerScope;

/** IndexedDB 库与对象仓库；与主线程共享同一份 WebView2 profile 存储。 */
const DB_NAME = "lilith.diagnostics";
const DB_VERSION = 1;
const STORE = "state";
/** 冻结记录用固定键，避免记录无限增长。 */
const FREEZE_KEY = "freeze";

/**
 * 判定冻结的静默阈值。必须明显大于心跳间隔（2 秒）与节流后的上报间隔，
 * 否则正常的 GC 停顿或一次长绘制就会被误判。
 */
const SILENCE_MS = 4_000;
/** 静默检测轮询间隔。决定冻结时刻的取证精度。 */
const POLL_MS = 250;

let dbPromise: Promise<IDBDatabase> | null = null;

function openDb(): Promise<IDBDatabase> {
  if (dbPromise) return dbPromise;
  dbPromise = new Promise<IDBDatabase>((resolve, reject) => {
    const request = indexedDB.open(DB_NAME, DB_VERSION);
    request.onupgradeneeded = () => {
      const db = request.result;
      if (!db.objectStoreNames.contains(STORE)) db.createObjectStore(STORE);
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
  return dbPromise;
}

async function putRecord(key: string, value: unknown): Promise<void> {
  try {
    const db = await openDb();
    await new Promise<void>((resolve, reject) => {
      const tx = db.transaction(STORE, "readwrite");
      tx.objectStore(STORE).put(value, key);
      tx.oncomplete = () => resolve();
      tx.onerror = () => reject(tx.error);
      tx.onabort = () => reject(tx.error);
    });
  } catch {
    // 存储不可用时静默降级；取证失败不能反过来影响业务。
  }
}

async function getRecord<T>(key: string): Promise<T | null> {
  try {
    const db = await openDb();
    return await new Promise<T | null>((resolve, reject) => {
      const tx = db.transaction(STORE, "readonly");
      const request = tx.objectStore(STORE).get(key);
      request.onsuccess = () => resolve((request.result as T | undefined) ?? null);
      request.onerror = () => reject(request.error);
    });
  } catch {
    return null;
  }
}

/** 是否处于「上报心跳」状态；`stop` 后停止检测，但保留库与历史记录。 */
let armed = false;
let poller: number | null = null;
/** 最近一次心跳的墙上时刻；为 0 表示尚未收到心跳或刚判定完冻结。 */
let lastBeatAt = 0;
let lastBeatSeq = 0;
let lastOp = "";
/** 是否已判定冻结但尚未恢复。 */
let frozen = false;
/** 判定冻结前最后一次心跳的墙上时刻，用于在恢复时反推停顿总时长。 */
let frozenLastBeatAt = 0;
/** 冻结记录的写盘句柄；恢复时的补记必须排在它之后，避免读改写竞态。 */
let freezeWrite: Promise<void> = Promise.resolve();
/** 是否已把历史冻结记录回读过（每次 `start` 只回读一次）。 */
let reportedHistory = false;

function startPoll(): void {
  if (poller !== null) return;
  poller = scope.setInterval(() => {
    if (!armed || lastBeatAt === 0) return;
    const silence = Date.now() - lastBeatAt;
    if (silence < SILENCE_MS) return;
    const record: FreezeRecord = {
      at: Date.now(),
      silenceMs: silence,
      lastBeatAt,
      lastBeatSeq,
      lastOp,
    };
    // 置 0 使本轮只记一次；主线程若恢复会重新上报心跳并再次武装检测。
    frozenLastBeatAt = lastBeatAt;
    lastBeatAt = 0;
    frozen = true;
    freezeWrite = putRecord(FREEZE_KEY, record);
    scope.postMessage({ type: "frozen", silenceMs: silence, lastBeatSeq, lastOp });
  }, POLL_MS);
}

function stopPoll(): void {
  if (poller === null) return;
  scope.clearInterval(poller);
  poller = null;
}

/** 回读上一次会话留下的冻结记录，交给主线程写进日志。 */
async function reportHistory(): Promise<void> {
  if (reportedHistory) return;
  reportedHistory = true;
  const record = await getRecord<FreezeRecord>(FREEZE_KEY);
  if (record) scope.postMessage({ type: "previous-freeze", record });
}

/** 主线程恢复后补记恢复时刻，用于区分「永久挂起」与「长时间停顿后恢复」。 */
async function markRecovered(recoveredAt: number): Promise<void> {
  await freezeWrite;
  const record = await getRecord<FreezeRecord>(FREEZE_KEY);
  if (!record) return;
  record.recoveredAt = recoveredAt;
  await putRecord(FREEZE_KEY, record);
}

scope.onmessage = (event: MessageEvent) => {
  const message = event.data as ForensicsRequest | null;
  if (!message || typeof message !== "object") return;
  switch (message.type) {
    case "start": {
      armed = true;
      void reportHistory();
      startPoll();
      break;
    }
    case "stop": {
      armed = false;
      stopPoll();
      break;
    }
    case "beat": {
      if (frozen) {
        // 主线程在判定冻结之后又上报了心跳：这是「长停顿后恢复」，不是永久挂起。
        const silenceMs = message.at - frozenLastBeatAt;
        frozen = false;
        void markRecovered(message.at);
        scope.postMessage({ type: "recovered", silenceMs, recoveredAt: message.at });
      }
      lastBeatAt = message.at;
      lastBeatSeq = message.seq;
      lastOp = message.op;
      break;
    }
    default:
      break;
  }
};

// 显式标记为模块：避免本文件被当成全局脚本，使上面的接口污染全局作用域。
export {};
