import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { BranchPublication, CertificationConfig } from "./types";
import { usePublicationController } from "./useAuthenticityController";

const api = vi.hoisted(() => ({
  getPublication: vi.fn(),
  previewArtifact: vi.fn(),
  estimate: vi.fn(),
}));

vi.mock("./api", () => ({ authenticityApi: api }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(), save: vi.fn() }));

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

const config = (branchId: string): CertificationConfig => ({
  branchId,
  title: branchId,
  creator: "Creator",
  rightsStatement: "",
  authenticationContent: "",
  trustmarkEnabled: false,
  certificatePath: "C:/certificate.pem",
  signingAlgorithm: "es256",
  timestampUrl: null,
  jpegQuality: 90,
  backgroundColor: "#ffffff",
  watermarkStrength: 1,
  additionalRegions: [],
  updatedMs: 0,
});

const publication = (branchId: string): BranchPublication => ({
  branchId,
  artifact: null,
  config: config(branchId),
  records: [],
  modelsReady: false,
  modelVariant: "Q/BCH_SUPER",
  encoderSha256: null,
  decoderSha256: null,
});

describe("usePublicationController", () => {
  beforeEach(() => {
    localStorage.clear();
    vi.clearAllMocks();
  });

  it("keeps the latest branch when an older request resolves last", async () => {
    const first = deferred<BranchPublication>();
    const second = deferred<BranchPublication>();
    api.getPublication.mockImplementation((branchId: string) => branchId === "first" ? first.promise : second.promise);
    const onError = vi.fn();
    const branches = [
      { id: "first", title: "First", headHistoryId: "head-1" },
      { id: "second", title: "Second", headHistoryId: "head-2" },
    ];
    const options = (selectedBranchId: string) => ({
      artworkTitle: "Artwork",
      branches,
      selectedBranchId,
      onError,
      onNavigateRecord: vi.fn(),
      onRetryFileCleanup: vi.fn().mockResolvedValue({ failures: [] }),
    });
    const { result, rerender } = renderHook(
      ({ selectedBranchId }) => usePublicationController(options(selectedBranchId)),
      { initialProps: { selectedBranchId: "first" } },
    );

    rerender({ selectedBranchId: "second" });
    await act(async () => { second.resolve(publication("second")); });
    await waitFor(() => expect(result.current.publication?.branchId).toBe("second"));

    await act(async () => { first.resolve(publication("first")); });
    expect(result.current.publication?.branchId).toBe("second");
    expect(onError).not.toHaveBeenCalled();
  });
});
