import { invoke } from "@tauri-apps/api/core";

/** 结构化命令错误的可选字段。v1 的 Tauri 命令仍以字符串错误拒绝；字段只在
 * 已知的结构化错误形态出现时挂到 Error 上，字符串错误保持原样，避免用户文案
 * 多出 `Error:` 前缀。 */
export interface CommandErrorFields {
  code?: string;
  kind?: string;
  retryable?: boolean;
  operationId?: string;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function normalizeCommandError(error: unknown): unknown {
  if (!isRecord(error) || typeof error.message !== "string") return error;
  const normalized = new Error(error.message) as Error & CommandErrorFields;
  if (typeof error.code === "string") normalized.code = error.code;
  if (typeof error.kind === "string") normalized.kind = error.kind;
  if (typeof error.retryable === "boolean") normalized.retryable = error.retryable;
  if (typeof error.operationId === "string") normalized.operationId = error.operationId;
  return normalized;
}

export function invokeCommand<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  return invoke<T>(command, args).catch((error: unknown) => {
    throw normalizeCommandError(error);
  });
}

/**
 * 以原始字节作为请求体调用命令（Tauri v2 的 `InvokeBody::Raw`）。
 *
 * 大块二进制（素材板整画布导出的分块 RGBA）走这条路：JSON 序列化会把每个字节变成
 * 一个数字，几十 MB 的块光是编码就要几百 MB 内存。短参数（块的坐标与尺寸）走 header，
 * 值必须是 ASCII 短字符串。错误归一化与 `invokeCommand` 保持一致。
 */
export function invokeCommandWithBody<T>(
  command: string,
  body: Uint8Array | ArrayBuffer,
  headers: Record<string, string>,
): Promise<T> {
  return invoke<T>(command, body, { headers }).catch((error: unknown) => {
    throw normalizeCommandError(error);
  });
}
