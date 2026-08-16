import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { BranchScheduleStatus, BranchSettings } from "./HistoryControls";
import type { ArtworkBranch } from "./types";

const branch = (backupEnabled: boolean): ArtworkBranch => ({
  id: "branch-1",
  title: "Main",
  sourcePath: "C:\\work\\artwork.psd",
  headHistoryId: null,
  createdFromHistoryId: null,
  backupEnabled,
  backupIntervalMinutes: 10,
  lastCheckMs: null,
  lastSuccessMs: null,
  lastError: backupEnabled ? null : "persistent failure",
  consecutiveBackupFailures: backupEnabled ? 0 : 5,
  backupRetryAtMs: null,
  backupDisableNoticePending: !backupEnabled,
  finalArtifactLocked: false,
  publishedCount: 0,
});

describe("BranchSettings", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it("merges a server-side automatic disable into an unrelated user draft", async () => {
    vi.useFakeTimers();
    const onSave = vi.fn().mockResolvedValue(undefined);
    const view = render(<BranchSettings branch={branch(true)} disabled={false} onSave={onSave} />);

    fireEvent.change(screen.getByLabelText("名称"), { target: { value: "Draft title" } });
    view.rerender(<BranchSettings branch={branch(false)} disabled={false} onSave={onSave} />);

    expect((screen.getByLabelText("名称") as HTMLInputElement).value).toBe("Draft title");
    expect((screen.getByRole("checkbox") as HTMLInputElement).checked).toBe(false);

    await act(async () => {
      vi.advanceTimersByTime(650);
      await Promise.resolve();
    });
    expect(onSave).toHaveBeenCalledWith({
      branchId: "branch-1",
      title: "Draft title",
      expectedBackupEnabled: false,
      backupEnabled: false,
      backupIntervalMinutes: 10,
    });
  });
});

describe("BranchScheduleStatus", () => {
  it("keeps the status summary short and exposes the complete error for copying", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });
    const failedBranch = {
      ...branch(true),
      lastError: "无法读取工作文件元数据：系统找不到指定的路径，且该错误需要完整保留用于诊断。",
      consecutiveBackupFailures: 2,
    };

    render(<BranchScheduleStatus branch={failedBranch} />);

    expect(screen.getByText("备份失败，将按策略重试")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "查看备份失败详情" }));
    expect(screen.getByText(failedBranch.lastError)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "复制备份失败详情" }));

    await waitFor(() => expect(writeText).toHaveBeenCalledWith(failedBranch.lastError));
    expect(await screen.findByTitle("已复制")).toBeTruthy();
  });
});
