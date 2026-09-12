import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TrashDialog } from "./LibraryDialogs";
import type { LibraryTrashEntry } from "./types";

const entry: LibraryTrashEntry = {
  id: "entry-1",
  kind: "artwork",
  title: "草图作品",
  deletedMs: 1_700_000_000_000,
  descendantCount: 0,
  artworkCount: 1,
  originalParentTitle: null,
};

function renderDialog(overrides: Partial<Parameters<typeof TrashDialog>[0]> = {}) {
  const callbacks = {
    onClose: vi.fn(),
    onRestore: vi.fn().mockResolvedValue(undefined),
    onDelete: vi.fn().mockResolvedValue(undefined),
    onEmpty: vi.fn().mockResolvedValue(undefined),
    onRetryCleanup: vi.fn().mockResolvedValue(undefined),
  };
  render(<TrashDialog entries={[entry]} busy={false} cleanupFailures={[]} {...callbacks} {...overrides} />);
  return callbacks;
}

describe("TrashDialog", () => {
  afterEach(cleanup);

  it("requires a second confirmation before emptying the trash", async () => {
    const { onEmpty } = renderDialog();

    fireEvent.click(screen.getByRole("button", { name: "清空回收站" }));

    expect(onEmpty).not.toHaveBeenCalled();
    expect(screen.getByRole("alertdialog")).toBeTruthy();
    expect(screen.getByRole("heading", { name: "清空回收站" })).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "取消" }));
    expect(onEmpty).not.toHaveBeenCalled();
    expect(screen.queryByRole("alertdialog")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "清空回收站" }));
    fireEvent.click(screen.getByRole("button", { name: "永久删除 1 个项目" }));

    expect(onEmpty).toHaveBeenCalledTimes(1);
  });

  it("requires a second confirmation before permanently deleting one entry", async () => {
    const { onDelete } = renderDialog();

    fireEvent.click(screen.getByTitle("永久删除"));

    expect(onDelete).not.toHaveBeenCalled();
    expect(screen.getByText("此操作不进入其他回收站，也无法通过恢复历史节点找回。")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "确认永久删除" }));

    expect(onDelete).toHaveBeenCalledWith(entry);
  });

  it("does not open a confirmation while another operation is running", () => {
    const { onEmpty } = renderDialog({ busy: true });

    const button = screen.getByRole("button", { name: "清空回收站" }) as HTMLButtonElement;
    expect(button.disabled).toBe(true);
    fireEvent.click(button);

    expect(onEmpty).not.toHaveBeenCalled();
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });
});
