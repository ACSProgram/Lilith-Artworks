import { describe, expect, it } from "vitest";
import { preventWebViewReload } from "./webviewShortcuts";

function keyboardEvent(
  overrides: Partial<{ key: string; ctrlKey: boolean; metaKey: boolean; altKey: boolean; shiftKey: boolean }> = {},
) {
  const state = { prevented: false };
  const event = {
    key: "",
    ctrlKey: false,
    metaKey: false,
    altKey: false,
    shiftKey: false,
    ...overrides,
    preventDefault: () => { state.prevented = true; },
  } as unknown as KeyboardEvent;
  return { event, state };
}

describe("preventWebViewReload", () => {
  it("cancels Ctrl+R and F5 so the webview cannot reload", () => {
    const ctrlR = keyboardEvent({ key: "r", ctrlKey: true });
    preventWebViewReload(ctrlR.event);
    expect(ctrlR.state.prevented).toBe(true);

    const f5 = keyboardEvent({ key: "F5" });
    preventWebViewReload(f5.event);
    expect(f5.state.prevented).toBe(true);
  });

  it("leaves unrelated keys untouched so module shortcuts still work", () => {
    // 只取消默认行为，不停止传播：模块仍能拿到 Ctrl+R 作为“锁定画板”。
    const plainR = keyboardEvent({ key: "r" });
    preventWebViewReload(plainR.event);
    expect(plainR.state.prevented).toBe(false);

    const ctrlK = keyboardEvent({ key: "k", ctrlKey: true });
    preventWebViewReload(ctrlK.event);
    expect(ctrlK.state.prevented).toBe(false);
  });
});
