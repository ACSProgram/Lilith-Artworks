import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SlideToConfirm } from "./SlideToConfirm";
import { dragSlideToEnd, slidePointer, slideThumb } from "./slideToConfirmTesting";

const thumb = () => slideThumb("拖动到底确认删除");

describe("SlideToConfirm", () => {
  afterEach(() => cleanup());

  it("confirms only after the thumb reaches the end of the track", () => {
    const onConfirm = vi.fn();
    render(<SlideToConfirm label="拖动到底确认删除" onConfirm={onConfirm} />);

    // 半途松手按取消处理，并回弹到起点。
    fireEvent(thumb(), slidePointer("pointerdown", 0));
    fireEvent(window, slidePointer("pointermove", 80));
    fireEvent(window, slidePointer("pointerup", 80));
    expect(onConfirm).not.toHaveBeenCalled();
    expect(thumb().getAttribute("aria-valuenow")).toBe("0");

    dragSlideToEnd();
    expect(onConfirm).toHaveBeenCalledTimes(1);
    expect(thumb().getAttribute("aria-valuenow")).toBe("100");
  });

  it("never confirms from a plain click", () => {
    const onConfirm = vi.fn();
    render(<SlideToConfirm label="拖动到底确认删除" onConfirm={onConfirm} />);

    fireEvent.click(thumb());
    fireEvent.click(thumb());
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it("completes from the keyboard and steps back with the left arrow", () => {
    const onConfirm = vi.fn();
    render(<SlideToConfirm label="拖动到底确认删除" onConfirm={onConfirm} />);

    fireEvent.keyDown(thumb(), { key: "ArrowRight" });
    fireEvent.keyDown(thumb(), { key: "ArrowLeft" });
    expect(thumb().getAttribute("aria-valuenow")).toBe("0");

    for (let step = 0; step < 3; step += 1) fireEvent.keyDown(thumb(), { key: "ArrowRight" });
    expect(thumb().getAttribute("aria-valuenow")).toBe("75");
    fireEvent.keyDown(thumb(), { key: "ArrowRight" });
    expect(onConfirm).toHaveBeenCalledTimes(1);
  });

  it("ignores interaction while disabled", () => {
    const onConfirm = vi.fn();
    render(<SlideToConfirm label="拖动到底确认删除" disabled onConfirm={onConfirm} />);

    fireEvent(thumb(), slidePointer("pointerdown", 0));
    fireEvent(window, slidePointer("pointermove", 200));
    fireEvent(window, slidePointer("pointerup", 200));
    fireEvent.keyDown(thumb(), { key: "ArrowRight" });

    expect(onConfirm).not.toHaveBeenCalled();
    expect(thumb().getAttribute("tabindex")).toBe("-1");
  });
});
