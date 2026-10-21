import { Component, Fragment, type ErrorInfo, type ReactNode } from "react";
import { diagnosticsLog } from "../shared/diagnostics";
import { errorMessage } from "../shared/errors";

/** 兜底渲染拿到的状态：`retry` 只重挂本边界内的子树，不动应用其余部分。 */
export interface ErrorFallbackState {
  error: Error;
  retry: () => void;
}

interface ErrorBoundaryProps {
  /** 日志里的定位标签，例如 `root`、`workspace:<artworkId>`。 */
  label: string;
  children: ReactNode;
  /** 任一元素变化即清除错误状态：切换作品或标签页后不必手动点重试。 */
  resetKeys?: readonly unknown[];
  /** 以渲染函数（而不是 `<Fallback />`）调用，因此兜底自身不能用 hook；需要状态时在里面再包一层组件。 */
  fallback: (state: ErrorFallbackState) => ReactNode;
}

interface ErrorBoundaryState {
  error: Error | null;
  /** 重试代次，作为子树 key：出错的旧树必须丢弃，不能让 React 复用。 */
  epoch: number;
}

/** 单条日志的细节上限。堆栈只取开头，避免整份 componentStack 把日志行撑爆。 */
const MAX_LOG_DETAIL = 600;

function clip(text: string | null | undefined): string {
  if (!text) return "n/a";
  const singleLine = text.replace(/\s+/g, " ").trim();
  return singleLine.length > MAX_LOG_DETAIL
    ? `${singleLine.slice(0, MAX_LOG_DETAIL)}…`
    : singleLine;
}

function toError(error: unknown): Error {
  return error instanceof Error ? error : new Error(errorMessage(error));
}

function keysChanged(
  previous: readonly unknown[] | undefined,
  next: readonly unknown[] | undefined,
): boolean {
  if (previous === next) return false;
  if (!previous || !next || previous.length !== next.length) return true;
  return next.some((key, index) => !Object.is(key, previous[index]));
}

/**
 * 全项目唯一的错误边界。React 没有函数式等价物，所以这里是唯一的 class 组件。
 *
 * 只接渲染、构造与生命周期里的同步异常；事件回调、Promise 拒绝和 effect 的异步失败不会
 * 到这里，它们由 `notify` 与 `shared/diagnostics.ts` 的全局捕获负责。捕获后必须做两件事：
 * 写日志（否则异常被静默吞掉）和给出可见出口（重试或重载），缺一不可。
 *
 * 兜底位置决定影响面：根边界只保证窗口不空白，工作区/画板边界让单个模块崩掉时其余部分
 * 仍可用。卸载时素材板会把未保存改动交给后台结算队列（见 `renderer.destroy`），所以
 * 「重试」比重载安全——重载会丢掉只在内存里的队列。
 */
export class ErrorBoundary extends Component<ErrorBoundaryProps, ErrorBoundaryState> {
  state: ErrorBoundaryState = { error: null, epoch: 0 };

  static getDerivedStateFromError(error: unknown): Partial<ErrorBoundaryState> {
    return { error: toError(error) };
  }

  componentDidCatch(error: unknown, info: ErrorInfo): void {
    const component = info.componentStack?.split("\n").find((line) => line.trim()) ?? "";
    const stack = error instanceof Error ? error.stack : "";
    diagnosticsLog(
      "error",
      `[boundary] ${this.props.label} caught: ${clip(errorMessage(error))} | component=${clip(component)} | stack=${clip(stack)}`,
    );
  }

  componentDidUpdate(previous: ErrorBoundaryProps): void {
    if (!this.state.error) return;
    if (keysChanged(previous.resetKeys, this.props.resetKeys)) this.retry();
  }

  private retry = (): void => {
    this.setState((current) => ({ error: null, epoch: current.epoch + 1 }));
  };

  render(): ReactNode {
    const { error, epoch } = this.state;
    if (error) return this.props.fallback({ error, retry: this.retry });
    return <Fragment key={epoch}>{this.props.children}</Fragment>;
  }
}
