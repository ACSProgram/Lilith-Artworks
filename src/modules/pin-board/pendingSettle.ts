import { diagnosticsLog } from "../../shared/diagnostics";
import { errorMessage } from "../../shared/errors";
import { pinBoardApi } from "./api";
import type { PinBoardEditableItem, PinBoardItem } from "./types";

/**
 * 后台结算队列：把「切换画板/作品时还没写回仓库的改动」从渲染器里摘出来。
 *
 * 素材板窗格跨作品保持挂载，渲染器释放一块画板就必须先把它的改动写回仓库。原先这一步
 * 是同步等待（`loadBoard` 等 `save_pin_board` + `finalize_pin_board` 成功才装载新画板），
 * 于是切换作品被仓库写锁绑架：维护扫描、恢复、精简这类长任务持锁几十秒到几分钟时，
 * 一次点击会排队 10 秒后失败，界面停在新作品、画布停在旧画板。
 *
 * 现在顺序反过来：切换时**同步**导出快照给本队列，渲染器立刻释放旧画板并装载新画板，
 * 写回交给这里的串行 worker。切换本身因此不取仓库写锁，后台任务再长也不影响切换。
 *
 * 两条硬约定：
 *
 * - **同一画板只有一个写者**：渲染器再次装载该画板时用 `takeBoardSettle` 取回快照内容，
 *   队列就不再写它（快照回到渲染器手里，随它的下一次保存落库）。因此队列里的快照不会
 *   和渲染器的编辑互相覆盖。
 * - **revision 只认最新**：队列写成功会把新 revision 记在 `recordBoardRevision` 里，
 *   渲染器装载画板与保存时经 `resolveBoardRevision` 取值，避免切换期间队列已经推进了
 *   revision、渲染器还拿着旧值去撞「画板已被修改」。
 *
 * 快照只在内存里：进程被强杀会丢掉队列里还没写回的改动。这是刻意接受的取舍——换取
 * 「切换作品永不被写锁阻塞」。退出与切仓库会经 `flushPendingSettle` 尽量写完。
 */

/** 渲染器导出的待结算快照。`items` 是完整条目（含位图/文字各自的字段），供再次装载时还原画布。 */
export interface PendingBoardSettle {
  boardId: number;
  /** 导出时的 revision；写回前会被队列记录的最新值覆盖（见 `resolveBoardRevision`）。 */
  revision: string;
  items: PinBoardItem[];
}

/** 展示用状态。「正在后台保存」与「保存失败」共用一条状态行，见 `usePendingSettle`。 */
export interface PendingSettleState {
  /** 仍待写回的画板数，含已经失败、等用户处理的那些。 */
  count: number;
  /** 最近一条被判为不可自动恢复的失败。 */
  failure: { boardId: number; message: string } | null;
}

/** 失败重试的起始间隔与上限；指数退避，避免长期占用仓库写锁。 */
const RETRY_BASE_DELAY_MS = 500;
const RETRY_MAX_DELAY_MS = 15_000;
/** 结算后的清理（`finalize_pin_board`）失败重试间隔：只影响已删除条目的回收。 */
const PRUNE_RETRY_DELAYS_MS = [2_000, 8_000];

interface Entry {
  snapshot: PendingBoardSettle;
  /** 导出快照时该画板已在飞行的那次保存；写回前要等它落地，否则会撞 revision。 */
  after: Promise<unknown> | null;
  /** 该快照正在写库。期间被 `takeBoardSettle` 取回时，本次写仍会完成并推进 revision。 */
  running: boolean;
  /** 已尝试次数，用于退避。 */
  attempts: number;
  /** 退避到期时刻；非空且未到期时本轮写回跳过它。 */
  retryAt: number | null;
  /** 不可自动恢复的失败原因；非空时停止自动重试，等用户处理。 */
  failure: string | null;
}

const entries = new Map<number, Entry>();
/** 每个画板已知的最新 revision，是队列与渲染器之间唯一的共享账本。 */
const revisions = new Map<number, string>();
const listeners = new Set<() => void>();

let retryTimer: number | null = null;
let drainPromise: Promise<void> | null = null;

function notify() {
  for (const listener of listeners) listener();
}

export function pendingSettleState(): PendingSettleState {
  let failure: PendingSettleState["failure"] = null;
  for (const entry of entries.values()) {
    if (entry.failure) failure = { boardId: entry.snapshot.boardId, message: entry.failure };
  }
  return { count: entries.size, failure };
}

export function subscribePendingSettle(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** 画板的最新 revision：队列已经写过就用队列的值，否则用调用方手上的（通常是查询结果）。 */
export function resolveBoardRevision(boardId: number, fallback: string): string {
  return revisions.get(boardId) ?? fallback;
}

/** 记录某画板已推进到哪个 revision（渲染器自己的保存与队列的写回都要记）。 */
export function recordBoardRevision(boardId: number, revision: string) {
  if (revision) revisions.set(boardId, revision);
}

/**
 * 暂存一块画板的待结算快照并启动写回。
 *
 * 同一画板重复暂存时以后者为准（用户来回切换会产生多次），但正在飞行的那次写仍会完成，
 * 其 revision 通过 `recordBoardRevision` 供后续写使用。`after` 是导出快照时该画板已在
 * 飞行的那次保存——写回前先等它落地，避免拿着旧 revision 去撞「画板已被修改」。
 */
export function stageBoardSettle(
  snapshot: PendingBoardSettle,
  after?: Promise<unknown>,
): void {
  const existing = entries.get(snapshot.boardId);
  entries.set(snapshot.boardId, {
    snapshot,
    after: after ?? existing?.after ?? null,
    running: existing?.running ?? false,
    attempts: 0,
    retryAt: null,
    failure: null,
  });
  diagnosticsLog(
    "info",
    `pin-board settle staged: boardId=${snapshot.boardId}, items=${snapshot.items.length}, wasRunning=${existing?.running ?? false}`,
  );
  notify();
  void drain();
}

/**
 * 取回某画板的待结算快照（渲染器重新装载它时调用）。
 *
 * 返回后队列不再负责这块画板：内容是渲染器的了，随它下一次保存落库。若快照正被写回，
 * 那次写仍会完成——它写的是同一份内容，只会额外推进 revision，不会覆盖后续编辑。
 */
export function takeBoardSettle(boardId: number): PendingBoardSettle | null {
  const entry = entries.get(boardId);
  if (!entry) return null;
  entries.delete(boardId);
  diagnosticsLog(
    "info",
    `pin-board settle taken back: boardId=${boardId}, running=${entry.running}, attempts=${entry.attempts}`,
  );
  notify();
  return entry.snapshot;
}

/** 用户点「重试」：清掉失败标记与退避，让队列按原快照重新排一次。 */
export function retryPendingSettle(): void {
  let resumed = false;
  for (const entry of entries.values()) {
    if (!entry.failure) continue;
    entry.failure = null;
    entry.attempts = 0;
    entry.retryAt = null;
    resumed = true;
  }
  if (!resumed) return;
  diagnosticsLog("info", `pin-board settle retry requested: pending=${entries.size}`);
  notify();
  void drain();
}

/**
 * 在预算内尽量写完队列（退出与切仓库前调用）。
 *
 * `budgetMs` 只是等待上限：单次写仍可能在写入闸门里排到 10 秒上限，因此到点就返回
 * `false`，由调用方决定是记日志（退出）还是中止（切仓库）。
 */
export async function flushPendingSettle(budgetMs = 5_000): Promise<boolean> {
  if (entries.size === 0) return true;
  cancelScheduledRetry();
  // 退出前值得立刻再试一次：退避只是为了避免长时间占锁，不是拒绝写入。
  for (const entry of entries.values()) {
    entry.retryAt = null;
    if (entry.failure) entry.failure = null;
  }
  let timer: number | null = null;
  const expired = new Promise<"expired">((resolve) => {
    timer = window.setTimeout(() => resolve("expired"), budgetMs);
  });
  const outcome = await Promise.race([drain().then(() => "drained" as const), expired]);
  if (timer !== null) window.clearTimeout(timer);
  const clear = entries.size === 0;
  diagnosticsLog(
    "info",
    `pin-board settle flush finished: outcome=${outcome}, remaining=${entries.size}, budgetMs=${budgetMs}`,
  );
  return clear;
}

function cancelScheduledRetry() {
  if (retryTimer === null) return;
  window.clearTimeout(retryTimer);
  retryTimer = null;
}

/** 有快照在等退避时按最近一条到期时间排一次唤醒。 */
function armRetryTimer() {
  if (retryTimer !== null) return;
  const now = Date.now();
  let earliest: number | null = null;
  for (const entry of entries.values()) {
    if (entry.running || entry.failure || entry.retryAt === null) continue;
    const delay = Math.max(0, entry.retryAt - now);
    earliest = earliest === null ? delay : Math.min(earliest, delay);
  }
  if (earliest === null) return;
  retryTimer = window.setTimeout(() => {
    retryTimer = null;
    void drain();
  }, earliest);
}

function nextEntry(now: number): Entry | null {
  for (const entry of entries.values()) {
    if (entry.running || entry.failure) continue;
    if (entry.retryAt !== null && entry.retryAt > now) continue;
    return entry;
  }
  return null;
}

/** 串行写回：一次只写一块画板，写完再取下一块。并发调用共用同一次遍历。 */
function drain(): Promise<void> {
  if (drainPromise) return drainPromise;
  const run = (async () => {
    for (let entry = nextEntry(Date.now()); entry; entry = nextEntry(Date.now())) {
      await writeEntry(entry);
    }
  })();
  drainPromise = run
    .catch(() => undefined)
    .then(() => {
      drainPromise = null;
      if (nextEntry(Date.now())) {
        void drain();
        return;
      }
      armRetryTimer();
    });
  return drainPromise;
}

async function writeEntry(entry: Entry): Promise<void> {
  const { boardId, items } = entry.snapshot;
  entry.running = true;
  // 导出快照时该画板可能还有一次保存正在飞行：先让它落地，否则这里会拿着旧 revision
  // 被后端判成「画板已被其他程序修改」。它落地后会把新 revision 记进账本。
  if (entry.after) {
    const inFlight = entry.after;
    entry.after = null;
    await inFlight.catch(() => undefined);
  }
  // 等的那段时间里用户可能已经切回这块画板（`takeBoardSettle`）：内容已经交回渲染器，
  // 这里不要再写一遍，避免用旧内容覆盖它。
  if (entries.get(boardId) !== entry) return;
  const revision = resolveBoardRevision(boardId, entry.snapshot.revision);
  const started = performance.now();
  try {
    const result = await pinBoardApi.savePinBoard(boardId, editableItems(items), revision);
    recordBoardRevision(boardId, result.revision);
    // 写回期间用户可能已经切回这块画板（`takeBoardSettle` 取走了快照）：那就不要再动它，
    // 内容已经交回渲染器。
    if (entries.get(boardId) === entry) {
      entries.delete(boardId);
      notify();
    }
    diagnosticsLog(
      "info",
      `pin-board settle saved: boardId=${boardId}, items=${items.length}, elapsedMs=${Math.round(performance.now() - started)}`,
    );
    // 保存已经落库，剩下的只是回收软删除条目；失败不影响数据，因此只记日志。
    if (items.some((item) => item.deleted)) void pruneBoard(boardId, result.revision, 0);
  } catch (error) {
    if (entries.get(boardId) !== entry) return;
    if (isRetryableSettleError(error)) {
      entry.attempts += 1;
      const delay = Math.min(RETRY_BASE_DELAY_MS * 2 ** (entry.attempts - 1), RETRY_MAX_DELAY_MS);
      entry.retryAt = Date.now() + delay;
      diagnosticsLog(
        "info",
        `pin-board settle retry scheduled: boardId=${boardId}, attempt=${entry.attempts}, delayMs=${delay}, error=${errorMessage(error)}`,
      );
      notify();
      return;
    }
    entry.failure = errorMessage(error);
    diagnosticsLog(
      "warn",
      `pin-board settle failed: boardId=${boardId}, attempts=${entry.attempts + 1}, error=${entry.failure}`,
    );
    notify();
  } finally {
    entry.running = false;
  }
}

/**
 * 回收一块画板的软删除条目（`finalize_pin_board`）。
 *
 * 切作品时不再先结算，回收因此挪到这里：保存成功后按 `PRUNE_RETRY_DELAYS_MS` 补做，
 * 全部失败只写日志——数据已经落库，残留的条目记录与 DDS 会由下一次保存、退出结算或
 * 设置页的清理扫描收走。
 */
async function pruneBoard(boardId: number, revision: string, attempt: number): Promise<void> {
  try {
    const result = await pinBoardApi.finalizePinBoard(
      boardId,
      resolveBoardRevision(boardId, revision),
    );
    recordBoardRevision(boardId, result.revision);
    diagnosticsLog("info", `pin-board settle pruned: boardId=${boardId}`);
  } catch (error) {
    const delay = PRUNE_RETRY_DELAYS_MS[attempt];
    if (delay !== undefined) {
      window.setTimeout(() => void pruneBoard(boardId, revision, attempt + 1), delay);
      return;
    }
    diagnosticsLog(
      "warn",
      `pin-board settle prune failed: boardId=${boardId}, error=${errorMessage(error)}`,
    );
  }
}

/**
 * 失败是否值得重试。
 *
 * 后端已把「可重试」标在结构化错误上（仓库写入排队超时、IO 抖动为 true；画板被其他
 * 程序修改、仓库只读这类为 false），优先用它；老命令只给字符串文案，按文案兜底。
 */
function isRetryableSettleError(error: unknown): boolean {
  const retryable = (error as { retryable?: unknown } | null)?.retryable;
  if (typeof retryable === "boolean") return retryable;
  if ((error as { code?: unknown } | null)?.code === "repository_busy") return true;
  const message = errorMessage(error);
  return message.includes("仓库正在写入") || message.includes("另一个进程写入");
}

/**
 * 画板保存只需要位置/层级/删除态与两个四边形；文字条目的载荷随快照一并带上，
 * 否则切换画板时正在编辑的文字会停留在旧内容。非文字条目不携带该字段。
 */
function editableItems(items: PinBoardItem[]): PinBoardEditableItem[] {
  return items.map((item) => ({
    itemId: item.itemId,
    order: item.order,
    layer: item.layer,
    deleted: item.deleted,
    points: item.points.map((point) => [...point]) as PinBoardEditableItem["points"],
    uv: item.uv.map((point) => [...point]) as PinBoardEditableItem["uv"],
    ...(item.kind === "text"
      ? {
        text: {
          text: item.text,
          fontSize: item.fontSize,
          color: item.color,
          align: item.align,
          measuredWidth: item.width,
          measuredHeight: item.height,
        },
      }
      : {}),
  }));
}

/** 测试用：清空队列与账本，避免用例之间互相污染。 */
export function resetPendingSettleForTests() {
  cancelScheduledRetry();
  entries.clear();
  revisions.clear();
  drainPromise = null;
  listeners.clear();
}
