import {
  createContext,
  useContext,
  useEffect,
  type ReactNode,
} from "react";
import { listen } from "@tauri-apps/api/event";
import { appApi } from "./api";
import { useNotice } from "./NoticeCenter";
import { RuntimeProgressProvider } from "./RuntimeProgressProvider";
import { useSettingsController, type SettingsController } from "./useSettingsController";
import {
  useRepositoryMaintenance,
  type RepositoryMaintenance,
} from "./useRepositoryMaintenance";
import { preparePinBoardRuntimeChange } from "../modules/pin-board/lifecycle";
import { diagnosticsLog } from "../shared/diagnostics";
import { errorMessage } from "../shared/errors";
import { setRuntimeEventRepositoryKey } from "../shared/useRuntimeSubscription";

export interface AppRuntime {
  settings: SettingsController;
  maintenance: RepositoryMaintenance;
}

const AppRuntimeContext = createContext<AppRuntime | null>(null);

const SETTLE_TIMEOUT_MS = 5_000;

/**
 * 应用运行时组合根：设置、仓库状态、维护流程与素材板退出结算在这里装配。
 * `App` 只负责路由与注入，不再持有跨领域的运行状态。
 */
export function AppRuntimeProvider({ children }: { children: ReactNode }) {
  return (
    <RuntimeProgressProvider>
      <AppRuntimeState>{children}</AppRuntimeState>
    </RuntimeProgressProvider>
  );
}

function AppRuntimeState({ children }: { children: ReactNode }) {
  const notice = useNotice();
  const settings = useSettingsController(notice.notify);
  const maintenance = useRepositoryMaintenance(settings, notice.notify);
  const draft = settings.draft;

  useEffect(() => {
    document.documentElement.dataset.theme = draft?.theme ?? "system";
    document.documentElement.dataset.density = draft?.content.density ?? "comfortable";
  }, [draft?.content.density, draft?.theme]);

  // 事件 bridge 的仓库范围：切换仓库后旧仓库的在途命令仍可能广播事件，
  // 不能让它覆盖新仓库的运行状态或历史刷新。
  useEffect(() => {
    setRuntimeEventRepositoryKey(
      settings.repository.ready ? settings.repository.rootPath : null,
    );
  }, [settings.repository.ready, settings.repository.rootPath]);

  // 原生端关闭窗口或托盘退出时不再直接结束进程，而是先请求 webview 结算素材板
  // （保存并截断步骤历史），确认完成后再由 confirm_app_shutdown 退出；Rust 侧
  // 有 15 秒兜底强退，因此这里无论结算成败都必须回复确认，不能阻塞退出。
  const pinBoardSaveOnExit = settings.snapshot?.settings.pinBoard?.saveOnExit ?? true;
  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    void listen("app_shutdown_requested", () => {
      const started = performance.now();
      diagnosticsLog(
        "info",
        `shutdown requested received: saveOnExit=${pinBoardSaveOnExit}`,
      );
      const elapsed = () => Math.round(performance.now() - started);
      const timeout = window.setTimeout(() => {
        diagnosticsLog(
          "warn",
          `[watchdog] shutdown settlement still running: elapsed_ms=${elapsed()}, save_on_exit=${pinBoardSaveOnExit}`,
        );
      }, SETTLE_TIMEOUT_MS);
      const reportSettlement = (level: "info" | "warn", outcome: string) => diagnosticsLog(
        level,
        `shutdown settlement finished: ${outcome}, elapsed_ms=${elapsed()}`,
      );
      const settle = pinBoardSaveOnExit
        // 用 Promise.resolve 包一层，确保结算函数即使同步抛错也走 catch，
        // `finally` 仍会发送确认，不把退出留给原生端 15 秒兜底。
        ? Promise.resolve()
          .then(() => preparePinBoardRuntimeChange())
          .then(() => reportSettlement("info", "outcome=ok"))
          .catch((error) => reportSettlement(
            "warn",
            `outcome=failed, save_on_exit=${pinBoardSaveOnExit}, error=${errorMessage(error)}`,
          ))
        : Promise.resolve(
          diagnosticsLog("info", "shutdown settlement skipped: saveOnExit disabled"),
        );
      void settle.finally(() => {
        window.clearTimeout(timeout);
        diagnosticsLog("info", `confirm_app_shutdown sent: elapsed_ms=${elapsed()}`);
        void appApi.confirmShutdown().catch(() => undefined);
      });
    })
      .then((off) => {
        if (disposed) off();
        else unlisten = off;
      })
      .catch(() => {
        // 监听不可用时（例如非 Tauri 环境）依赖原生端 15 秒兜底强退，不阻塞退出。
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [pinBoardSaveOnExit]);

  return (
    <AppRuntimeContext.Provider value={{ settings, maintenance }}>
      {children}
    </AppRuntimeContext.Provider>
  );
}

export function useAppRuntime(): AppRuntime {
  const context = useContext(AppRuntimeContext);
  if (!context) {
    throw new Error("useAppRuntime 必须在 AppRuntimeProvider 内使用");
  }
  return context;
}
