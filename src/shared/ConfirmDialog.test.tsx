import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ConfirmDialog } from "./ConfirmDialog";
import type { ConfirmView } from "./ConfirmDialog";

const view: ConfirmView = {
  icon: null,
  eyebrow: "不可撤销",
  title: "删除节点",
  subject: "已选择 2 个中间节点",
  description: "程序会逐个重建相邻节点的反向增量并重新连接历史链。",
  action: "确认删除",
  danger: true,
};

function backdrop(): HTMLElement {
  return document.querySelector(".dialog-backdrop") as HTMLElement;
}

describe("ConfirmDialog busy semantics", () => {
  afterEach(() => cleanup());

  it("turns the footer cancel into 取消操作 while a cancellable operation runs", () => {
    const onCancel = vi.fn();
    const onCancelOperation = vi.fn();
    render(<ConfirmDialog
      view={view}
      busy
      onCancelOperation={onCancelOperation}
      onCancel={onCancel}
      onConfirm={vi.fn()}
    />);

    // 运行期间弹窗不能随手关掉：背景点击与头部关闭按钮都失效。
    fireEvent.mouseDown(backdrop());
    expect(onCancel).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "关闭" }).hasAttribute("disabled")).toBe(true);

    fireEvent.click(screen.getByRole("button", { name: "取消操作" }));
    expect(onCancelOperation).toHaveBeenCalledTimes(1);
    expect(onCancel).not.toHaveBeenCalled();
  });

  it("keeps a plain dismiss when the operation cannot be interrupted", () => {
    const onCancel = vi.fn();
    render(<ConfirmDialog view={view} busy onCancel={onCancel} onConfirm={vi.fn()} />);

    fireEvent.click(screen.getByRole("button", { name: "取消" }));
    fireEvent.mouseDown(backdrop());
    expect(onCancel).toHaveBeenCalledTimes(2);
    expect(screen.getByRole("button", { name: "关闭" }).hasAttribute("disabled")).toBe(false);
  });

  it("confirms and dismisses normally while idle", () => {
    const onCancel = vi.fn();
    const onConfirm = vi.fn();
    render(<ConfirmDialog view={view} busy={false} onCancel={onCancel} onConfirm={onConfirm} />);

    fireEvent.click(screen.getByRole("button", { name: "确认删除" }));
    expect(onConfirm).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: "取消" }));
    expect(onCancel).toHaveBeenCalledTimes(1);
  });
});
