import { describe, expect, it } from "vitest";
import {
  PIN_BOARD_FULLSCREEN_SHORTCUT,
  PIN_BOARD_LOCK_SHORTCUT,
  pinBoardLayersPanelDisabled,
  pinBoardLockIndicatorVisible,
  pinBoardUnavailableNotice,
} from "./PinBoardModule";
import { shortcutMatches } from "./shortcuts";
import { PIN_BOARD_DRAWBAR_SHORTCUT, toolForShortcut } from "./strokeDrawing";

describe("pin-board module entry", () => {
  it("loads through its public React entry without starting native work", async () => {
    const module = await import("./PinBoardModule");
    expect(module.PinBoardModule).toBeTypeOf("function");
  });
});

describe("pin-board default shortcuts", () => {
  it("keeps the Client default Ctrl+R for the lock action", () => {
    // 锁定沿用 Client 的默认键位；WebView 刷新由应用层只取消默认行为来屏蔽，
    // 因此 Ctrl+R 必须仍然能命中锁定快捷键。
    expect(PIN_BOARD_LOCK_SHORTCUT).toBe("CommandOrControl+R");
    expect(shortcutMatches(
      { key: "r", code: "KeyR", ctrlKey: true, metaKey: false, altKey: false, shiftKey: false } as KeyboardEvent,
      PIN_BOARD_LOCK_SHORTCUT,
    )).toBe(true);
    expect(PIN_BOARD_FULLSCREEN_SHORTCUT).toBe("F11");
  });

  it("binds the drawbar toggle to an unmodified T", () => {
    // 这个键位必须与工具键位空开（`toolForShortcut` 返回 null），否则按下 T 会先
    // 切工具。主工具栏按钮的渲染、点亮态与置灰在 `PinBoardToolbar.test.tsx` 里覆盖。
    expect(PIN_BOARD_DRAWBAR_SHORTCUT).toBe("T");
    expect(toolForShortcut(PIN_BOARD_DRAWBAR_SHORTCUT)).toBeNull();
    expect(shortcutMatches(
      { key: "t", code: "KeyT", ctrlKey: false, metaKey: false, altKey: false, shiftKey: false } as KeyboardEvent,
      PIN_BOARD_DRAWBAR_SHORTCUT,
    )).toBe(true);
  });
});

describe("pin-board unavailable notice", () => {
  it("states the graphics requirement and keeps the raw failure reason", () => {
    const notice = pinBoardUnavailableNotice("无法初始化 WebGPU 适配器");
    expect(notice).toContain("WebGPU");
    expect(notice).toContain("BC7");
    expect(notice).toContain("无法初始化 WebGPU 适配器");
    expect(notice).toContain("其余功能不受影响");
  });

  it("hides the lock indicator while the renderer could not boot", () => {
    // 渲染器为空时画板停在默认锁定态，那不是用户锁的：只显示不可用说明。
    expect(pinBoardLockIndicatorVisible({
      hasView: true,
      locked: true,
      unavailable: true,
    })).toBe(false);
  });

  it("shows the lock indicator only for a loaded, user-locked board", () => {
    expect(pinBoardLockIndicatorVisible({
      hasView: true,
      locked: true,
      unavailable: false,
    })).toBe(true);
    expect(pinBoardLockIndicatorVisible({
      hasView: true,
      locked: false,
      unavailable: false,
    })).toBe(false);
    expect(pinBoardLockIndicatorVisible({
      hasView: false,
      locked: true,
      unavailable: false,
    })).toBe(false);
  });
});

describe("pin-board layer panel availability", () => {
  it("stays usable while the board is locked", () => {
    // 显隐是纯视图状态：锁定只挡改模型的操作，藏掉一层看底图恰恰是锁定时的常用动作。
    // 条件里没有 locked 这一项，这条用例就是防止它被并回「编辑禁用」那组。
    expect(pinBoardLayersPanelDisabled({
      hasRenderer: true,
      exporting: false,
      textEditing: false,
    })).toBe(false);
  });

  it("greys out during the whole-board export", () => {
    // 导出渲染的是点下那一刻的画面：中途改显隐会让输出与用户看到的对不上。
    expect(pinBoardLayersPanelDisabled({
      hasRenderer: true,
      exporting: true,
      textEditing: false,
    })).toBe(true);
  });

  it("greys out while a text edit session is open", () => {
    // 切层属于模型操作：编辑中的文字不能在图层被藏起来后继续看不见地编辑。
    expect(pinBoardLayersPanelDisabled({
      hasRenderer: true,
      exporting: false,
      textEditing: true,
    })).toBe(true);
  });

  it("greys out without a renderer", () => {
    expect(pinBoardLayersPanelDisabled({
      hasRenderer: false,
      exporting: false,
      textEditing: false,
    })).toBe(true);
    expect(pinBoardLayersPanelDisabled({
      hasRenderer: false,
      exporting: true,
      textEditing: false,
    })).toBe(true);
  });
});
