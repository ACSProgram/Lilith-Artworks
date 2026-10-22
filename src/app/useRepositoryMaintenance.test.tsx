import { act, cleanup, renderHook } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDLE_BACKUP_RUNTIME, RuntimeProgressContext } from "./runtimeProgressContext";
import type { SettingsController } from "./useSettingsController";
import { useRepositoryMaintenance } from "./useRepositoryMaintenance";

const appApi = vi.hoisted(() => ({
  listPendingFileCleanup: vi.fn(async () => []),
  getLastRepositoryBackupRun: vi.fn(async () => null),
  scrubRepositoryIntegrity: vi.fn(),
  createRepositoryBackup: vi.fn(),
  scanRepositoryUnreferenced: vi.fn(),
  scanRepositoryStorageDisk: vi.fn(),
  getRepositoryStorageOverview: vi.fn(),
  retryFileCleanup: vi.fn(),
  cleanupRepositoryUnreferenced: vi.fn(),
}));

vi.mock("./api", () => ({ appApi }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

function settingsController(): SettingsController {
  return {
    settingsOpen: false,
    settingsPage: "space",
    repository: { ready: true, rootPath: "C:\\repository" },
    busy: false,
  } as unknown as SettingsController;
}

function harness(refresh: () => Promise<typeof IDLE_BACKUP_RUNTIME>) {
  const notify = vi.fn();
  const wrapper = ({ children }: { children: ReactNode }) => (
    <RuntimeProgressContext.Provider
      value={{
        runtime: IDLE_BACKUP_RUNTIME,
        cancelPending: false,
        refresh,
        setPolling: vi.fn(),
        cancel: vi.fn(async () => true),
      }}
    >
      {children}
    </RuntimeProgressContext.Provider>
  );
  const { result } = renderHook(
    () => useRepositoryMaintenance(settingsController(), notify),
    { wrapper },
  );
  return { result, notify };
}

describe("useRepositoryMaintenance long-running settings operations", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  afterEach(() => cleanup());

  it("refuses to start a scan while another backup operation is running", async () => {
    const { result, notify } = harness(async () => ({ ...IDLE_BACKUP_RUNTIME, busy: true }));

    await act(async () => {
      await result.current.scanUnreferenced();
    });

    expect(appApi.scanRepositoryUnreferenced).not.toHaveBeenCalled();
    expect(notify).toHaveBeenCalledTimes(1);
    expect(notify).toHaveBeenCalledWith(
      "已有备份操作正在运行，请等待完成或先取消当前操作。",
    );
    expect(result.current.settingsOperation).toBeNull();
  });

  it("runs the scan and reports its result when the runtime is idle", async () => {
    appApi.scanRepositoryUnreferenced.mockResolvedValueOnce([]);
    const { result, notify } = harness(async () => IDLE_BACKUP_RUNTIME);

    await act(async () => {
      await result.current.scanUnreferenced();
    });

    expect(appApi.scanRepositoryUnreferenced).toHaveBeenCalledTimes(1);
    expect(notify).toHaveBeenLastCalledWith("垃圾文件扫描完成：没有发现可清理的文件。");
    expect(result.current.settingsOperation).toBeNull();
  });

  it("routes a failed integrity scan through the shared error path", async () => {
    appApi.scrubRepositoryIntegrity.mockRejectedValueOnce(new Error("扫描失败"));
    const { result, notify } = harness(async () => IDLE_BACKUP_RUNTIME);

    await act(async () => {
      await result.current.scrubRepository();
    });

    expect(notify).toHaveBeenLastCalledWith("扫描失败");
    expect(result.current.settingsOperation).toBeNull();
  });

  it("reports a runtime read failure without starting the command", async () => {
    const { result, notify } = harness(async () => {
      throw new Error("运行状态不可读");
    });

    await act(async () => {
      await result.current.scanStorageDisk();
    });

    expect(appApi.scanRepositoryStorageDisk).not.toHaveBeenCalled();
    expect(notify).toHaveBeenCalledTimes(1);
    expect(notify).toHaveBeenCalledWith("运行状态不可读");
  });
});
