import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_PIN_BOARD_INTERACTION, type PinBoardRenderer } from "./renderer";
import { PinBoardToolbar, type PinBoardToolbarProps } from "./PinBoardToolbar";
import type { PinBoardView } from "./types";

const view: PinBoardView = {
  boardId: 1,
  name: "Board",
  items: [],
  missingTextures: 0,
  revision: "r1",
  nowStep: 0,
};

function toolbarProps(overrides: Partial<PinBoardToolbarProps> = {}): PinBoardToolbarProps {
  return {
    sidebar: true,
    title: "Board",
    view,
    renderer: {} as unknown as PinBoardRenderer,
    interaction: { ...DEFAULT_PIN_BOARD_INTERACTION, locked: false },
    clipboardCount: 0,
    transferBusy: false,
    fullscreen: false,
    drawbarVisible: true,
    settings: {
      arrangementGapRatio: 0.06,
      textureCacheLevel: "medium",
      autosave: false,
      lockShortcut: "CommandOrControl+R",
      fullscreenShortcut: "F11",
    },
    run: (action) => action(),
    onToggleSidebar: vi.fn(),
    onToggleDrawbar: vi.fn(),
    onCopy: vi.fn(),
    onCopyClipboard: vi.fn(),
    onPaste: vi.fn(),
    onImport: vi.fn(),
    onAddText: vi.fn(),
    onExport: vi.fn(),
    onExportCanvas: vi.fn(),
    onToggleLock: vi.fn(),
    onToggleFullscreen: vi.fn(),
    onSave: vi.fn(),
    ...overrides,
  };
}

describe("pin-board toolbar", () => {
  afterEach(() => cleanup());

  it("lights the pen button while the drawbar is visible and toggles it", () => {
    const onToggleDrawbar = vi.fn();
    const { rerender } = render(<PinBoardToolbar {...toolbarProps({ onToggleDrawbar })} />);

    const visible = screen.getByRole("button", { name: "隐藏标注工具条" });
    expect(visible.getAttribute("aria-pressed")).toBe("true");
    expect(visible.className).toContain("active");
    // 提示里的字母与按键分发读同一个常量，不会各说一套。
    expect(visible.getAttribute("title")).toBe("隐藏标注工具条 (T)");

    fireEvent.click(visible);
    expect(onToggleDrawbar).toHaveBeenCalledTimes(1);

    rerender(<PinBoardToolbar {...toolbarProps({ onToggleDrawbar, drawbarVisible: false })} />);
    const hidden = screen.getByRole("button", { name: "显示标注工具条" });
    expect(hidden.getAttribute("aria-pressed")).toBe("false");
    expect(hidden.className).not.toContain("active");

    fireEvent.click(hidden);
    expect(onToggleDrawbar).toHaveBeenCalledTimes(2);
  });

  it("disables the pen button without a board, without a renderer or during canvas export", () => {
    const disabled = () => screen.getByRole("button", { name: "隐藏标注工具条" })
      .hasAttribute("disabled");

    const { rerender } = render(<PinBoardToolbar {...toolbarProps({ view: null })} />);
    expect(disabled()).toBe(true);

    rerender(<PinBoardToolbar {...toolbarProps({ renderer: null })} />);
    expect(disabled()).toBe(true);

    // 锁定不拦它：显隐不写库，锁住画面后藏掉工具条仍是常见动作。
    rerender(<PinBoardToolbar {...toolbarProps({ interaction: {
      ...DEFAULT_PIN_BOARD_INTERACTION,
      locked: true,
    } })} />);
    expect(disabled()).toBe(false);

    rerender(<PinBoardToolbar {...toolbarProps({ interaction: {
      ...DEFAULT_PIN_BOARD_INTERACTION,
      exporting: true,
    } })} />);
    expect(disabled()).toBe(true);
  });
});
