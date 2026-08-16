import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { BranchSettings } from "./HistoryControls";
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
