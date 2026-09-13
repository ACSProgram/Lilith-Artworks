/**
 * 阻止 WebView 级别的整页刷新快捷键（F5 / Ctrl+R）逃出桌面应用。
 *
 * 只取消默认行为，不停止事件传播：素材板等模块仍可把同一组合键作为自己的
 * 快捷键（例如默认的“锁定画板” Ctrl+R）。实现与 Lilith Client 保持一致。
 */
export function preventWebViewReload(event: KeyboardEvent) {
  const key = event.key.toLowerCase();
  const reloadRequested = key === "f5"
    || (key === "r" && (event.ctrlKey || event.metaKey));
  if (reloadRequested) event.preventDefault();
}
