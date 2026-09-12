import { act, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ArtworkHistory, BackupRuntimeStatus } from "./types";

const historyApi = vi.hoisted(() => ({
  get: vi.fn(),
  runtime: vi.fn(),
}));

vi.mock("./api", () => ({ historyApi }));

import { useHistoryController } from "./useHistoryController";

const history: ArtworkHistory = {
  artworkId: "artwork-1",
  artworkTitle: "Artwork",
  branches: [],
  nodes: [],
};

const runtime = (completionRevision: number): BackupRuntimeStatus => ({
  busy: false,
  activeBranchId: null,
  operation: null,
  progressLabel: null,
  progressCurrent: 0,
  progressTotal: 0,
  automaticScheduling: true,
  completionRevision,
});

function Harness() {
  const controller = useHistoryController({
    artworkId: "artwork-1",
    refreshVersion: 0,
    onHistoryChanged: vi.fn(),
    onError: vi.fn(),
  });
  return <span>{controller.history?.artworkTitle ?? "loading"}</span>;
}

describe("useHistoryController runtime polling", () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.clearAllMocks();
  });

  it("reloads after a short operation that was never observed busy", async () => {
    vi.useFakeTimers();
    let currentRuntime = runtime(0);
    historyApi.get.mockResolvedValue(history);
    historyApi.runtime.mockImplementation(async () => currentRuntime);

    render(<Harness />);
    await act(async () => {
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(screen.getByText("Artwork")).toBeTruthy();
    expect(historyApi.get).toHaveBeenCalledTimes(1);

    currentRuntime = runtime(1);
    await act(async () => {
      vi.advanceTimersByTime(3_000);
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(historyApi.get).toHaveBeenCalledTimes(2);
  });
});
