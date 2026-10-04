import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { BranchScheduleStatus, BranchSettings } from "./HistoryControls";
import type { ArtworkBranch } from "./types";

const dialog = vi.hoisted(() => ({ open: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => dialog);

const appApi = vi.hoisted(() => ({ revealPathInFolder: vi.fn().mockResolvedValue(undefined) }));
vi.mock("../../app/api", () => ({ appApi }));

const branch = (backupEnabled: boolean): ArtworkBranch => ({
  id: "branch-1",
  title: "Main",
  sourcePath: "C:\\work\\artwork.psd",
  headHistoryId: null,
  createdFromHistoryId: null,
  backupEnabled,
  backupIntervalMinutes: 10,
  backupQuickEnabled: false,
  lastCheckMs: null,
  lastSuccessMs: null,
  lastError: backupEnabled ? null : "persistent failure",
  consecutiveBackupFailures: backupEnabled ? 0 : 5,
  backupRetryAtMs: null,
  backupDisableNoticePending: !backupEnabled,
  finalArtifactLocked: false,
  publishedCount: 0,
  verifyError: null,
  verifiedMs: null,
});

describe("BranchSettings", () => {
  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
    vi.useRealTimers();
  });

  it("merges a server-side automatic disable into an unrelated user draft", async () => {
    vi.useFakeTimers();
    const onSave = vi.fn().mockResolvedValue(undefined);
    const view = render(<BranchSettings branch={branch(true)} disabled={false} onSave={onSave} />);

    fireEvent.change(screen.getByLabelText("名称"), { target: { value: "Draft title" } });
    view.rerender(<BranchSettings branch={branch(false)} disabled={false} onSave={onSave} />);

    expect((screen.getByLabelText("名称") as HTMLInputElement).value).toBe("Draft title");
    expect((screen.getByRole("checkbox", { name: "自动备份" }) as HTMLInputElement).checked).toBe(false);

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
      backupQuickEnabled: false,
    });
  });

  it("saves the quick-check toggle and reveals the working file folder", async () => {
    vi.useFakeTimers();
    const onSave = vi.fn().mockResolvedValue(undefined);
    render(<BranchSettings branch={branch(true)} disabled={false} onSave={onSave} />);

    fireEvent.click(screen.getByRole("checkbox", { name: "快速检查" }));
    fireEvent.click(screen.getByTitle("打开所在文件夹"));

    await act(async () => {
      vi.advanceTimersByTime(650);
      await Promise.resolve();
    });
    expect(appApi.revealPathInFolder).toHaveBeenCalledWith("C:\\work\\artwork.psd");
    expect(onSave).toHaveBeenCalledWith(expect.objectContaining({
      backupEnabled: true,
      backupQuickEnabled: true,
    }));
  });

  it("restores the persisted path when a path update fails", async () => {
    vi.useFakeTimers();
    const onSave = vi.fn().mockRejectedValue(new Error("invalid path"));
    dialog.open.mockResolvedValue("C:\\work\\replacement.psd");
    render(<BranchSettings branch={branch(true)} disabled={false} onSave={onSave} />);

    await act(async () => { fireEvent.click(screen.getByTitle("修改工作文件")); await Promise.resolve(); });
    await act(async () => {
      vi.advanceTimersByTime(650);
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(onSave).toHaveBeenCalledWith(expect.objectContaining({
      sourcePath: "C:\\work\\replacement.psd",
    }));
    expect(screen.getByLabelText("工作文件").textContent)
      .toBe("C:\\work\\artwork.psd");
  });

  it("keeps automatic backup greyed out and closed without a working file", async () => {
    vi.useFakeTimers();
    const onSave = vi.fn().mockResolvedValue(undefined);
    render(<BranchSettings
      branch={{ ...branch(true), sourcePath: "" }}
      disabled={false}
      onSave={onSave}
    />);

    const toggle = screen.getByRole("checkbox", { name: "自动备份" }) as HTMLInputElement;
    expect(toggle.disabled).toBe(true);
    expect(toggle.checked).toBe(false);
    expect((screen.getByRole("checkbox", { name: "快速检查" }) as HTMLInputElement).disabled).toBe(true);
    expect((screen.getByRole("spinbutton") as HTMLInputElement).disabled).toBe(true);
    expect(screen.getByLabelText("工作文件").textContent).toBe("未选择工作文件");
    expect(screen.getByRole("button", { name: "选择文件" })).toBeTruthy();

    await act(async () => {
      vi.advanceTimersByTime(650);
      await Promise.resolve();
    });
    expect(onSave).toHaveBeenCalledWith(expect.objectContaining({
      backupEnabled: false,
    }));
  });

  it("clears the persisted working file path and closes automatic backup", async () => {
    vi.useFakeTimers();
    const onSave = vi.fn().mockResolvedValue(undefined);
    render(<BranchSettings branch={branch(true)} disabled={false} onSave={onSave} />);

    fireEvent.click(screen.getByTitle("清除工作文件路径"));
    expect(screen.getByLabelText("工作文件").textContent).toBe("未选择工作文件");
    expect((screen.getByRole("checkbox", { name: "自动备份" }) as HTMLInputElement).checked).toBe(false);

    await act(async () => {
      vi.advanceTimersByTime(650);
      await Promise.resolve();
    });
    expect(onSave).toHaveBeenCalledWith(expect.objectContaining({
      sourcePath: "",
      backupEnabled: false,
    }));
  });
});

describe("BranchScheduleStatus", () => {
  afterEach(() => cleanup());

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

  it("mentions quick check for branches that opted in", () => {
    render(<BranchScheduleStatus branch={{ ...branch(true), backupQuickEnabled: true }} />);
    expect(screen.getByText(/每 10 分钟自动备份 · 快速检查/)).toBeTruthy();
  });

  it("shows a chain verification failure separately and re-queues it", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    });
    const onReverify = vi.fn();
    const verifyFailedBranch = {
      ...branch(true),
      verifyError: "链路校验 snapshot 摘要与历史数据库不匹配",
      verifiedMs: null,
    };

    render(<BranchScheduleStatus branch={verifyFailedBranch} onReverify={onReverify} />);

    // 备份状态正常时仍独立显示校验失败。
    expect(screen.getByText(/每 10 分钟自动备份/)).toBeTruthy();
    expect(screen.getByText("链路校验失败")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "查看链路校验失败详情" }));
    expect(screen.getByText(verifyFailedBranch.verifyError)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "复制链路校验失败详情" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(verifyFailedBranch.verifyError));

    fireEvent.click(screen.getByRole("button", { name: "重新校验此分支" }));
    expect(onReverify).toHaveBeenCalledWith("branch-1");
  });
});
