import { createContext, useContext } from "react";
import type { SettingsController } from "./useSettingsController";
import type { RepositoryMaintenance } from "./useRepositoryMaintenance";

export interface AppRuntime {
  settings: SettingsController;
  maintenance: RepositoryMaintenance;
}

/**
 * 运行时上下文的独立模块。
 *
 * context 对象**不能**和 Provider 组件放在同一个文件里：开发模式下保存文件会热更新
 * Provider 所在模块，模块被重新执行就会生成一个新的 context 对象，仍持有旧 `useContext`
 * 的消费者拿到 `null` 直接 throw，整棵 React 树随之卸载（2026-10-21 画板被带走即由此触发）。
 * 把 context 与 hook 留在只随接口变更才改动的模块里，热更新 Provider 就不再换身份。
 */
export const AppRuntimeContext = createContext<AppRuntime | null>(null);

export function useAppRuntime(): AppRuntime {
  const context = useContext(AppRuntimeContext);
  if (!context) {
    throw new Error("useAppRuntime 必须在 AppRuntimeProvider 内使用");
  }
  return context;
}
