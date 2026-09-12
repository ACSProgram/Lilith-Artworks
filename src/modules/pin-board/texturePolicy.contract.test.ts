import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import {
  PIN_BOARD_NATIVE_TEXTURE_LIMIT,
  PIN_BOARD_RGBA_PREVIEW_BUDGET_BYTES,
} from "./texturePolicy";

/** vitest 的工作目录是仓库根，jsdom 环境下 import.meta.url 不是 file 协议，
 *  因此用 cwd 拼接原生源文件路径。 */
function nativeSource(): string {
  return readFileSync(
    join(process.cwd(), "src-tauri", "src", "pin_board", "dds.rs"),
    "utf8",
  );
}

describe("pin-board texture IPC contract", () => {
  it("keeps the frontend request ceiling aligned with the native decoder", () => {
    const nativeLimit = nativeSource()
      .match(/const MAX_TEXTURE_DIMENSION: u32 = (\d+);/);

    expect(nativeLimit?.[1]).toBe(String(PIN_BOARD_NATIVE_TEXTURE_LIMIT));
  });

  it("keeps the RGBA preview budget aligned with the native decoder", () => {
    const nativeBudget = nativeSource()
      .match(/const MAX_RGBA_PREVIEW_BYTES: u64 = (\d+) \* 1024 \* 1024;/);

    expect(nativeBudget?.[1]).toBe(String(PIN_BOARD_RGBA_PREVIEW_BUDGET_BYTES / (1024 * 1024)));
  });
});
