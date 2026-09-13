import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { ArtworkWorkspace } from "./ArtworkWorkspace";

vi.mock("../modules/history/HistoryModule", () => ({
  HistoryModule: () => <div data-testid="history-module" />,
}));

vi.mock("../modules/authenticity/AuthenticityModule", () => ({
  AuthenticityModule: ({ mode }: { mode: string }) => <div data-testid={`${mode}-module`} />,
}));

describe("ArtworkWorkspace", () => {
  it("mounts only the active workspace view", () => {
    const view = render(<ArtworkWorkspace
      artworkId="artwork-1"
      pinBoardSettings={{
        arrangementGapPx: 10,
        textureCacheLevel: "medium",
        lockShortcut: "CommandOrControl+Shift+K",
        fullscreenShortcut: "F11",
      }}
      onError={vi.fn()}
      onNavigateRecord={vi.fn()}
      onRetryFileCleanup={vi.fn().mockResolvedValue({ removed: 0, failures: [] })}
    />);
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
    const view = render(<ArtworkWorkspace
      artworkId="artwork-1"
      pinBoardSettings={{
        arrangementGapPx: 10,
        textureCacheLevel: "medium",
        lockShortcut: "CommandOrControl+Shift+K",
        fullscreenShortcut: "F11",
      }}
      onError={vi.fn()}
      onNavigateRecord={vi.fn()}
      onRetryFileCleanup={vi.fn().mockResolvedValue({ removed: 0, failures: [] })}
    />);

    const tabs = view.getByRole("navigation", { name: "Artwork 工作区" });
    const labels = Array.from(tabs.querySelectorAll("button")).map((button) => button.textContent);
    expect(labels.indexOf("素材板")).toBeLessThan(labels.indexOf("版本历史"));
  });
});
