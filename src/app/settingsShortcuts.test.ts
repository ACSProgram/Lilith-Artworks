import { describe, expect, it } from "vitest";
import { shortcutFromEvent, shortcutLabel } from "./settingsShortcuts";

function keyboardEvent(overrides: Partial<{ ctrlKey: boolean; altKey: boolean; shiftKey: boolean; code: string }> = {}) {
  return {
    ctrlKey: false,
    metaKey: false,
    altKey: false,
    shiftKey: false,
    code: "KeyK",
    ...overrides,
  };
}

describe("settings shortcut capture", () => {
  it("labels CommandOrControl as Ctrl", () => {
    expect(shortcutLabel("CommandOrControl+Shift+K")).toBe("Ctrl + Shift + K");
    expect(shortcutLabel("F11")).toBe("F11");
  });

  it("captures modified letter shortcuts", () => {
    expect(shortcutFromEvent(keyboardEvent({ ctrlKey: true, shiftKey: true }))).toBe("CommandOrControl+Shift+K");
    expect(shortcutFromEvent(keyboardEvent({ ctrlKey: true, code: "KeyR" }))).toBe("CommandOrControl+R");
  });

  it("captures unmodified function keys and bare keys when allowed", () => {
    expect(shortcutFromEvent(keyboardEvent({ code: "F11" }))).toBe("F11");
    expect(shortcutFromEvent(keyboardEvent({ code: "F11" }), true)).toBe("F11");
    expect(shortcutFromEvent(keyboardEvent(), true)).toBe("K");
  });

  it("rejects unmodified non-function keys by default", () => {
    expect(shortcutFromEvent(keyboardEvent())).toBeNull();
    expect(shortcutFromEvent(keyboardEvent({ code: "Digit7" }))).toBeNull();
  });
});
