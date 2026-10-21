import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ErrorBoundary } from "./ErrorBoundary";

const diagnosticsLog = vi.hoisted(() => vi.fn());
vi.mock("../shared/diagnostics", () => ({ diagnosticsLog }));

function Fallback({ error, retry }: { error: Error; retry: () => void }) {
  return <div>
    <p>兜底：{error.message}</p>
    <button type="button" onClick={retry}>重试</button>
  </div>;
}

/** React 会把捕获到的异常再打一份到 console.error；测试里不需要这层噪声。 */
function muteReactErrorLog() {
  vi.spyOn(console, "error").mockImplementation(() => undefined);
}

function Boom(): never {
  throw new Error("渲染失败");
}

afterEach(() => {
  cleanup();
  diagnosticsLog.mockClear();
});

describe("ErrorBoundary", () => {
  it("捕获渲染期异常，写诊断日志并渲染兜底", () => {
    muteReactErrorLog();
    render(<ErrorBoundary label="test" fallback={(state) => <Fallback {...state} />}><Boom /></ErrorBoundary>);

    expect(screen.getByText("兜底：渲染失败")).toBeTruthy();
    expect(diagnosticsLog).toHaveBeenCalledWith(
      "error",
      expect.stringContaining("[boundary] test caught: 渲染失败"),
    );
  });

  it("重试会重挂子树，错误消失后恢复内容", () => {
    muteReactErrorLog();
    let failing = true;
    function Flaky() {
      if (failing) throw new Error("偶发失败");
      return <p>已恢复</p>;
    }

    render(<ErrorBoundary label="retry" fallback={(state) => <Fallback {...state} />}><Flaky /></ErrorBoundary>);
    expect(screen.getByText("兜底：偶发失败")).toBeTruthy();

    failing = false;
    fireEvent.click(screen.getByText("重试"));
    expect(screen.getByText("已恢复")).toBeTruthy();
  });

  it("resetKeys 变化时自动清除错误，不需要手动重试", () => {
    muteReactErrorLog();
    const { rerender } = render(
      <ErrorBoundary label="workspace" resetKeys={["artwork-a"]} fallback={(state) => <Fallback {...state} />}>
        <Boom />
      </ErrorBoundary>,
    );
    expect(screen.getByText("兜底：渲染失败")).toBeTruthy();

    rerender(
      <ErrorBoundary label="workspace" resetKeys={["artwork-b"]} fallback={(state) => <Fallback {...state} />}>
        <p>新作品</p>
      </ErrorBoundary>,
    );
    expect(screen.getByText("新作品")).toBeTruthy();
  });

  it("没有异常时原样渲染子节点且不写日志", () => {
    render(<ErrorBoundary label="normal" fallback={(state) => <Fallback {...state} />}><p>正常内容</p></ErrorBoundary>);

    expect(screen.getByText("正常内容")).toBeTruthy();
    expect(diagnosticsLog).not.toHaveBeenCalled();
  });
});
