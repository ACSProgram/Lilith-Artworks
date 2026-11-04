import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PinBoardSidebar, type PinBoardSidebarProps } from "./PinBoardSidebar";

function props(patch: Partial<PinBoardSidebarProps> = {}): PinBoardSidebarProps {
  return {
    active: true,
    boards: [],
    trash: [],
    selected: null,
    exporting: false,
    transferBusy: false,
    unavailable: false,
    onDismissCanvasMenu: vi.fn(),
    onCreate: vi.fn(),
    onSelect: vi.fn(),
    onMoveBoard: vi.fn(),
    onRename: vi.fn(),
    onTrash: vi.fn(),
    onRestore: vi.fn(),
    onPermanentDelete: vi.fn(),
    onEmptyTrash: vi.fn(),
    ...patch,
  };
}

describe("PinBoardSidebar", () => {
  afterEach(cleanup);

  it("disables the new-board entry and stops pointing at it while unavailable", () => {
    render(<PinBoardSidebar {...props({ unavailable: true })} />);

    expect((screen.getByRole("button", { name: "新建画板" }) as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByText("素材板不可用，新建画板已停用")).toBeTruthy();
  });

  it("keeps the new-board entry usable when the renderer is available", () => {
    render(<PinBoardSidebar {...props()} />);

    expect((screen.getByRole("button", { name: "新建画板" }) as HTMLButtonElement).disabled).toBe(false);
    expect(screen.getByText("尚无画板，点击右上角“新建画板”")).toBeTruthy();
  });
});
