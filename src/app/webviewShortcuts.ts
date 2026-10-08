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

/**
 * 阻止把外部文件拖进窗口时 WebView 直接导航到该文件。
 *
 * 只取消默认行为，不停止事件传播：识别页等模块仍可在自己的元素上处理同一次
 * 拖放；未被任何模块处理的拖放落到 window 时被取消，避免整页跳转丢失状态。
 * 素材板与作品树的内部 HTML5 拖拽同样不受影响。
 */
export function preventWebViewFileDrop(event: DragEvent) {
  event.preventDefault();
}
