import {
  useCallback,
  useEffect,
  useMemo,
  useState,
  type Dispatch,
  type SetStateAction,
} from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { appApi } from "./api";
import {
  PIN_BOARD_ARRANGEMENT_GAP_RATIO,
  PIN_BOARD_FULLSCREEN_SHORTCUT,
  PIN_BOARD_LOCK_SHORTCUT,
} from "../modules/pin-board/PinBoardModule";
import { preparePinBoardRuntimeChange } from "../modules/pin-board/lifecycle";
import {
  diagnosticsLog,
  isTauriRuntime,
  startDiagnostics,
  type DiagnosticsStatus,
} from "../shared/diagnostics";
import { errorMessage } from "../shared/errors";
import type {
  AppSettings,
  PinBoardTextureCacheLevel,
  RepositoryStatus,
  SettingsSnapshot,
} from "./types";

export type SettingsPage = "general" | "repository" | "space" | "backup" | "pin-board" | "debug";

export const SETTINGS_PAGES: Array<{ id: SettingsPage; label: string }> = [
  { id: "general", label: "通用" },
  { id: "repository", label: "仓储" },
  { id: "space", label: "空间" },
  { id: "backup", label: "备份" },
  { id: "pin-board", label: "素材板" },
  { id: "debug", label: "调试" },
];

export const EMPTY_REPOSITORY_STATUS: RepositoryStatus = {
  configured: false,
  ready: false,
  rootPath: "",
  databasePath: "",
  error: null,
};

type Notify = (message: string | null) => void;

/**
 * 启动时的清理队列重放：best-effort 的维护动作，不决定「仓库能不能显示」。
 *
 * 重放本身是写命令（要取仓库写锁），自动备份刚启动时经常正好持锁；因此它作为
 * 独立异步步骤运行，失败只留一条诊断日志，等下一次启动或用户手动重试。
 */
async function replayCleanupQueue(): Promise<void> {
  try {
    // 先走只读命令：队列为空是启动时的常态，此时完全不必去抢写锁。
    const pending = await appApi.listPendingFileCleanup();
    if (pending.length === 0) return;
    const report = await appApi.retryFileCleanup([]);
    if (report.failures.length > 0) {
      diagnosticsLog(
        "info",
        `startup cleanup replay left ${report.failures.length} file(s) pending`,
      );
    }
  } catch (error) {
    diagnosticsLog("info", `startup cleanup replay skipped: ${errorMessage(error)}`);
  }
}

export interface SettingsController {
  snapshot: SettingsSnapshot | null;
  draft: AppSettings | null;
  setDraft: Dispatch<SetStateAction<AppSettings | null>>;
  repository: RepositoryStatus;
  setRepository: Dispatch<SetStateAction<RepositoryStatus>>;
  repositoryLabel: string;
  settingsOpen: boolean;
  settingsPage: SettingsPage;
  setSettingsPage: (page: SettingsPage) => void;
  busy: boolean;
  diagnostics: DiagnosticsStatus | null;
  pinBoardSettings: {
    arrangementGapRatio: number;
    textureCacheLevel: PinBoardTextureCacheLevel;
    autosave: boolean;
    lockShortcut: string;
    fullscreenShortcut: string;
  };
  openSettings: () => void;
  closeSettings: () => void;
  chooseRepository: () => Promise<void>;
  save: () => Promise<void>;
  toggleDiagnostics: (enabled: boolean) => Promise<void>;
}

/**
 * 设置页与仓库状态的唯一前端控制器。设置保存仍由后端 `SettingsService` 做
 * revision/CAS；这里负责素材板结算、repository status 刷新与冲突后的重新加载。
 */
export function useSettingsController(notify: Notify): SettingsController {
  const [snapshot, setSnapshot] = useState<SettingsSnapshot | null>(null);
  const [draft, setDraft] = useState<AppSettings | null>(null);
  const [repository, setRepository] = useState<RepositoryStatus>(EMPTY_REPOSITORY_STATUS);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [settingsPage, setSettingsPage] = useState<SettingsPage>("general");
  const [busy, setBusy] = useState(true);
  const [diagnostics, setDiagnostics] = useState<DiagnosticsStatus | null>(null);

  const load = useCallback(async () => {
    setBusy(true);
    let repositoryReady = false;
    try {
      const [nextSnapshot, nextRepository] = await Promise.all([
        appApi.getSettings(),
        appApi.getRepositoryStatus(),
      ]);
      // 先把已取到的快照与仓库状态回填：后续任何失败都不该让界面停在
      // EMPTY_REPOSITORY_STATUS 上显示「仓库不可用」。
      setSnapshot(nextSnapshot);
      setDraft(nextSnapshot.settings);
      setRepository(nextRepository);
      repositoryReady = nextRepository.ready;
      notify(nextSnapshot.warning ?? nextRepository.error ?? null);
    } catch (error) {
      notify(errorMessage(error));
    } finally {
      setBusy(false);
    }
    // 维护性重放放在状态回填之后，且不参与 settingsBusy。
    if (repositoryReady) void replayCleanupQueue();
  }, [notify]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    if (!isTauriRuntime()) return undefined;
    let disposed = false;
    let dispose: (() => void) | null = null;
    void appApi.getDiagnosticsStatus()
      .then((status) => {
        if (!disposed) setDiagnostics(status);
      })
      .catch(() => undefined);
    void startDiagnostics().then((off) => {
      if (disposed) off();
      else dispose = off;
    });
    return () => {
      disposed = true;
      dispose?.();
    };
  }, []);

  const repositoryLabel = useMemo(() => {
    if (repository.ready) return "仓库就绪";
    if (repository.configured) return "仓库不可用";
    return "尚未配置仓库";
  }, [repository]);

  const pinBoardSettings = useMemo(() => ({
    arrangementGapRatio: snapshot?.settings.pinBoard?.arrangementGapRatio ?? PIN_BOARD_ARRANGEMENT_GAP_RATIO,
    textureCacheLevel: snapshot?.settings.pinBoard?.textureCacheLevel ?? "medium",
    autosave: snapshot?.settings.pinBoard?.autosave ?? false,
    lockShortcut: snapshot?.settings.pinBoard?.lockShortcut ?? PIN_BOARD_LOCK_SHORTCUT,
    fullscreenShortcut: snapshot?.settings.pinBoard?.fullscreenShortcut ?? PIN_BOARD_FULLSCREEN_SHORTCUT,
  }), [snapshot]);

  const openSettings = useCallback(() => {
    setSettingsOpen(true);
    void appApi.getSettings().then((next) => {
      setSnapshot(next);
      setDraft(next.settings);
    }).catch((error) => notify(errorMessage(error)));
  }, [notify]);

  const closeSettings = useCallback(() => setSettingsOpen(false), []);

  const chooseRepository = useCallback(async () => {
    const selected = await open({ directory: true, multiple: false });
    if (typeof selected !== "string") return;
    setDraft((current) => current
      ? { ...current, repositoryPath: selected }
      : current);
  }, []);

  const save = useCallback(async () => {
    if (!draft) return;
    const repositoryChanged = draft.repositoryPath.trim() !== repository.rootPath;
    setBusy(true);
    notify(null);
    try {
      if (repositoryChanged) {
        // 切换仓库会卸载工作区并释放旧仓库，必须先结算素材板，否则最后一次
        // 编辑会落在已经释放的仓库上而丢失。结算失败即中止切换，保持旧仓库。
        diagnosticsLog("info", "repository switch: settling pin board before releasing repository");
        await preparePinBoardRuntimeChange();
        diagnosticsLog("info", "repository switch: pin board settled");
        setRepository(EMPTY_REPOSITORY_STATUS);
      }
      const next = await appApi.saveSettings(draft);
      setSnapshot(next);
      setDraft(next.settings);
      setRepository(await appApi.getRepositoryStatus());
      setSettingsOpen(false);
    } catch (error) {
      notify(errorMessage(error));
      if ((error as { code?: string }).code === "settings_conflict") {
        // 冲突时只能以磁盘上的新快照为准；重载后弹窗保持打开，由用户重新确认。
        // 保留未保存编辑会掩盖托盘/窗口写入，因此这里明确丢弃草稿。
        try {
          const [nextSnapshot, nextRepository] = await Promise.all([
            appApi.getSettings(),
            appApi.getRepositoryStatus(),
          ]);
          setSnapshot(nextSnapshot);
          setDraft(nextSnapshot.settings);
          setRepository(nextRepository);
        } catch {
          // 原冲突已经提示；重载失败不覆盖它。
        }
      } else if (repositoryChanged) {
        // 切换失败（含素材板结算失败）时工作区可能已被卸载：恢复当前仓库状态，
        // 但保留草稿，让用户修正输入后重试。
        try {
          setRepository(await appApi.getRepositoryStatus());
        } catch {
          // 原错误已经提示。
        }
      }
    } finally {
      setBusy(false);
    }
  }, [draft, notify, repository.rootPath]);

  const toggleDiagnostics = useCallback(async (enabled: boolean) => {
    try {
      setDiagnostics(await appApi.setDiagnosticsEnabled(enabled));
    } catch (error) {
      notify(errorMessage(error));
    }
  }, [notify]);

  return {
    snapshot,
    draft,
    setDraft,
    repository,
    setRepository,
    repositoryLabel,
    settingsOpen,
    settingsPage,
    setSettingsPage,
    busy,
    diagnostics,
    pinBoardSettings,
    openSettings,
    closeSettings,
    chooseRepository,
    save,
    toggleDiagnostics,
  };
}
