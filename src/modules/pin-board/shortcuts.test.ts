import { describe, expect, it } from "vitest";
import { shortcutCanHandle, shortcutMatches } from "./shortcuts";

function keyboardEvent(overrides: Partial<KeyboardEvent> = {}): KeyboardEvent {
  return {
    code: "KeyR",
    ctrlKey: false,
    metaKey: false,
    altKey: false,
    shiftKey: false,
    target: null,
    ...overrides,
  } as KeyboardEvent;
}

describe("pin-board shortcuts", () => {
  it("matches CommandOrControl with either platform modifier", () => {
    expect(shortcutMatches(keyboardEvent({ ctrlKey: true }), "CommandOrControl+R")).toBe(true);
    expect(shortcutMatches(keyboardEvent({ metaKey: true }), "CommandOrControl+R")).toBe(true);
  });

  it("rejects missing or extra modifiers", () => {
    expect(shortcutMatches(keyboardEvent(), "CommandOrControl+R")).toBe(false);
    expect(shortcutMatches(
      keyboardEvent({ ctrlKey: true, shiftKey: true }),
      "CommandOrControl+R",
    )).toBe(false);
  });

  it("matches function keys and digits from event codes", () => {
    expect(shortcutMatches(keyboardEvent({ code: "F11" }), "F11")).toBe(true);
    expect(shortcutMatches(
      keyboardEvent({ code: "Digit7", altKey: true }),
      "Alt+7",
    )).toBe(true);
  });

  it("does not handle editable targets", () => {
    const editable = { matches: () => true } as unknown as HTMLElement;
    expect(shortcutCanHandle(keyboardEvent({ target: editable }))).toBe(false);
    expect(shortcutCanHandle(keyboardEvent())).toBe(true);
  });
});
