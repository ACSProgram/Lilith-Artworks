export function errorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  if (error && typeof error === "object") {
    const candidate = error as { message?: unknown; error?: unknown; reason?: unknown };
    for (const value of [candidate.message, candidate.error, candidate.reason]) {
      if (typeof value === "string" && value.trim()) return value;
    }
  }
  return "发生了未知错误";
}
