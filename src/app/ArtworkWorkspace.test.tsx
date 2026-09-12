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
    render(<ArtworkWorkspace
      artworkId="artwork-1"
      onError={vi.fn()}
      onNavigateRecord={vi.fn()}
      onRetryFileCleanup={vi.fn().mockResolvedValue({ removed: 0, failures: [] })}
    />);

    expect(screen.getByTestId("history-module")).toBeTruthy();
    expect(screen.queryByTestId("publish-module")).toBeNull();
    expect(screen.queryByTestId("identify-module")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "发布与认证" }));
    expect(screen.queryByTestId("history-module")).toBeNull();
    expect(screen.getByTestId("publish-module")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "识别与溯源" }));
    expect(screen.queryByTestId("publish-module")).toBeNull();
    expect(screen.getByTestId("identify-module")).toBeTruthy();
  });
});
