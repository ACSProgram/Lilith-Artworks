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
