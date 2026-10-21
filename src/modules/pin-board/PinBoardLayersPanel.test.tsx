import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PinBoardLayersPanel } from "./PinBoardLayersPanel";
import { STROKE_LAYER, type PinBoardLayerVisibility } from "./layers";

function visibility(...hidden: number[]): PinBoardLayerVisibility {
  const value: PinBoardLayerVisibility = [true, true, true, true];
  for (const layer of hidden) value[layer] = false;
  return value;
}

describe("pin-board layers panel", () => {
  afterEach(() => cleanup());

  it("starts expanded and lists the four layers from top to bottom", () => {
    render(<PinBoardLayersPanel visibility={visibility()} onToggle={vi.fn()} disabled={false} />);

    expect(screen.getAllByRole("button").map((button) => (
      button.getAttribute("aria-label") ?? button.textContent?.trim()
    ))).toEqual([
      "收起图层",
      "标注层",
      "顶层",
      "中层",
      "底层",
    ]);
    // 面板里只有开关，不再附一行操作说明。
    expect(screen.queryByText(/不会被点选/)).toBeNull();
  });

  it("toggles the clicked layer and dims hidden rows", () => {
    const onToggle = vi.fn();
    const { rerender } = render(
      <PinBoardLayersPanel visibility={visibility()} onToggle={onToggle} disabled={false} />,
    );

    fireEvent.click(screen.getByRole("button", { name: "标注层" }));
    expect(onToggle).toHaveBeenCalledWith(STROKE_LAYER, false);

    rerender(
      <PinBoardLayersPanel visibility={visibility(STROKE_LAYER)} onToggle={onToggle} disabled={false} />,
    );
    const annotation = screen.getByRole("button", { name: "标注层" });
    expect(annotation.getAttribute("aria-pressed")).toBe("false");
    expect(annotation.className).toContain("hidden");
    expect(screen.getByRole("button", { name: "底层" }).getAttribute("aria-pressed")).toBe("true");
  });

  it("shows per-layer item counts without changing the accessible names", () => {
    const { rerender } = render(
      <PinBoardLayersPanel
        visibility={visibility()}
        counts={[3, 0, 7, 12]}
        onToggle={vi.fn()}
        disabled={false}
      />,
    );

    // 标注层不显示数字；其余三层按 layer 下标取数，0 也照常显示。
    const counts = screen.getAllByText(/^\d+$/).map((node) => node.textContent);
    expect(counts).toEqual(["7", "0", "3"]);
    for (const node of screen.getAllByText(/^\d+$/)) {
      expect(node.getAttribute("aria-hidden")).toBe("true");
    }
    // 数字不参与可访问名：四个开关仍以层名可达。
    for (const name of ["标注层", "顶层", "中层", "底层"]) {
      expect(screen.getByRole("button", { name })).toBeTruthy();
    }

    // 隐藏一层只改变外观，数字不动（口径是「这一层有多少条目」，不是「看得见多少」）。
    rerender(
      <PinBoardLayersPanel
        visibility={visibility(0)}
        counts={[3, 0, 7, 12]}
        onToggle={vi.fn()}
        disabled={false}
      />,
    );
    expect(screen.getAllByText(/^\d+$/).map((node) => node.textContent)).toEqual(["7", "0", "3"]);
  });

  it("omits counts entirely when the caller passes none", () => {
    render(<PinBoardLayersPanel visibility={visibility()} onToggle={vi.fn()} disabled={false} />);
    expect(screen.queryAllByText(/^\d+$/)).toHaveLength(0);
  });

  it("collapses to one button and comes back expanded", () => {
    render(
      <PinBoardLayersPanel visibility={visibility(0)} onToggle={vi.fn()} disabled={false} />,
    );
    fireEvent.click(screen.getByRole("button", { name: "收起图层" }));

    // 收起后只剩图层按钮本身；有图层被隐藏时按钮高亮。
    expect(screen.queryByText("图层")).toBeNull();
    const collapsed = screen.getByRole("button", { name: "图层可见性" });
    expect(collapsed.className).toContain("filtering");

    fireEvent.click(collapsed);
    expect(screen.getByRole("button", { name: "收起图层" })).toBeTruthy();
    expect(screen.getAllByRole("button")).toHaveLength(5);
  });
});
