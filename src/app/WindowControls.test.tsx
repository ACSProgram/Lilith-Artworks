import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { WindowControls } from "./WindowControls";

afterEach(() => cleanup());

describe("WindowControls", () => {
  it("渲染最小化、最大化与关闭三个按钮", () => {
    render(<WindowControls />);

    expect(screen.getByTitle("最小化")).toBeTruthy();
    expect(screen.getByTitle("最大化或还原")).toBeTruthy();
    expect(screen.getByTitle("关闭")).toBeTruthy();
  });

  it("非 Tauri 宿主里点击不抛错，把原因交给上报出口", () => {
    // jsdom 没有 `__TAURI_INTERNALS__`，`getCurrentWindow()` 会同步抛错——根兜底正处在
    // 这种环境里（组件树已卸载、没有 provider），按钮再抛一次就彻底没出口了。
    const onError = vi.fn();
    render(<WindowControls onError={onError} />);

    expect(() => fireEvent.click(screen.getByTitle("关闭"))).not.toThrow();
    expect(onError).toHaveBeenCalledTimes(1);
    expect(onError.mock.calls[0][0]).toEqual(expect.any(String));
  });
});
