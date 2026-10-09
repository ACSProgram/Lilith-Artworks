import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ArtworkWorkspace } from "./ArtworkWorkspace";

const historyApi = vi.hoisted(() => ({ get: vi.fn() }));
const captured = vi.hoisted(() => ({ latest: null as unknown }));

vi.mock("../modules/history/HistoryModule", () => ({
  HistoryModule: () => <div data-testid="history-module" />,
}));

vi.mock("../modules/history/api", () => ({ historyApi }));

vi.mock("../modules/authenticity/AuthenticityModule", () => ({
  AuthenticityModule: (props: { mode: string }) => {
    captured.latest = props;
    return <div data-testid={`${props.mode}-module`} />;
  },
}));

const pinBoardSettings = {
  arrangementGapPx: 10,
  textureCacheLevel: "medium" as const,
  autosave: false,
  lockShortcut: "CommandOrControl+R",
  fullscreenShortcut: "F11",
};

const history = {
  artworkId: "artwork-1",
  artworkTitle: "Artwork",
  branches: [{ id: "branch-2", title: "Second", headHistoryId: "head-2" }],
  nodes: [],
};

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

function renderWorkspace(props: Partial<Parameters<typeof ArtworkWorkspace>[0]> = {}) {
  return render(<ArtworkWorkspace
    artworkId="artwork-1"
    pinBoardSettings={pinBoardSettings}
    onError={vi.fn()}
    onNavigateRecord={vi.fn()}
    onRetryFileCleanup={vi.fn().mockResolvedValue({ removed: 0, failures: [] })}
    {...props}
  />);
}

describe("ArtworkWorkspace", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    historyApi.get.mockResolvedValue(history);
  });

  afterEach(() => cleanup());

  it("mounts only the active workspace view", () => {
    const view = renderWorkspace();
    const { unmount } = view;

    expect(screen.getByTestId("history-module")).toBeTruthy();
    expect(screen.queryByTestId("publish-module")).toBeNull();
    expect(screen.queryByTestId("identify-module")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "发布与认证" }));
    expect(screen.queryByTestId("history-module")).toBeNull();
    expect(screen.getByTestId("publish-module")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "识别与溯源" }));
    expect(screen.queryByTestId("publish-module")).toBeNull();
    expect(screen.getByTestId("identify-module")).toBeTruthy();
    unmount();
  });

  it("lists the pin-board tab before the history tab", () => {
    const view = renderWorkspace();

    const tabs = view.getByRole("navigation", { name: "Artwork 工作区" });
    const labels = Array.from(tabs.querySelectorAll("button")).map((button) => button.textContent);
    expect(labels.indexOf("素材板")).toBeLessThan(labels.indexOf("版本历史"));
  });

  it("leaves the branch data to the history view while it is active", () => {
    renderWorkspace();

    // 历史页自带 controller，常规导航不产生额外的摘要请求。
    expect(historyApi.get).not.toHaveBeenCalled();
  });

  it("loads the workspace summary when mounted directly on the publish view", async () => {
    renderWorkspace({ initialView: "publish", initialBranchId: "branch-2" });

    await waitFor(() => expect((captured.latest as { branches: unknown[] }).branches).toHaveLength(1));
    const props = captured.latest as {
      branches: Array<{ id: string }>;
      selectedBranchId: string | null;
      branchesLoading: boolean;
      branchesError: string | null;
      artworkTitle: string;
    };
    // 跳转携带的 branchId 必须在摘要加载完成后被保留。
    expect(props.selectedBranchId).toBe("branch-2");
    expect(props.branchesLoading).toBe(false);
    expect(props.branchesError).toBeNull();
    expect(props.artworkTitle).toBe("Artwork");
    expect(historyApi.get).toHaveBeenCalledWith("artwork-1");
  });

  it("exposes the branch loading state before the summary resolves", async () => {
    const pending = deferred<typeof history>();
    historyApi.get.mockReturnValue(pending.promise);
    renderWorkspace({ initialView: "publish", initialBranchId: "branch-2" });

    const loadingProps = captured.latest as { branchesLoading: boolean; branches: unknown[] };
    expect(loadingProps.branchesLoading).toBe(true);
    expect(loadingProps.branches).toEqual([]);

    await act(async () => { pending.resolve(history); });
    await waitFor(() => expect((captured.latest as { branchesLoading: boolean }).branchesLoading).toBe(false));
  });

  it("reports the summary failure so the publish view can retry", async () => {
    historyApi.get.mockRejectedValue(new Error("无法读取作品历史"));
    const view = renderWorkspace({ initialView: "publish" });

    await waitFor(() => expect((captured.latest as { branchesError: string | null }).branchesError).toBe("无法读取作品历史"));
    historyApi.get.mockResolvedValue(history);
    fireEvent.click(view.getByRole("button", { name: "版本历史" }));
    fireEvent.click(view.getByRole("button", { name: "发布与认证" }));

    await waitFor(() => expect((captured.latest as { branches: unknown[] }).branches).toHaveLength(1));
  });
});
