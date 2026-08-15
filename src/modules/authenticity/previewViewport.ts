export interface PreviewViewport {
  scrollLeft: number;
  scrollTop: number;
  scrollWidth: number;
  scrollHeight: number;
  clientWidth: number;
  clientHeight: number;
}

export interface NavigatorRect {
  left: number;
  top: number;
  width: number;
  height: number;
}

const percent = (value: number, total: number) => total > 0 ? value / total * 100 : 0;

export function navigatorRect(viewport: PreviewViewport): NavigatorRect {
  return {
    left: percent(viewport.scrollLeft, viewport.scrollWidth),
    top: percent(viewport.scrollTop, viewport.scrollHeight),
    width: Math.min(100, percent(viewport.clientWidth, viewport.scrollWidth)),
    height: Math.min(100, percent(viewport.clientHeight, viewport.scrollHeight)),
  };
}

export function navigatorScrollTarget(
  viewport: PreviewViewport,
  xRatio: number,
  yRatio: number,
): Pick<PreviewViewport, "scrollLeft" | "scrollTop"> {
  const scrollLeft = xRatio * viewport.scrollWidth - viewport.clientWidth / 2;
  const scrollTop = yRatio * viewport.scrollHeight - viewport.clientHeight / 2;
  return {
    scrollLeft: Math.max(0, Math.min(viewport.scrollWidth - viewport.clientWidth, scrollLeft)),
    scrollTop: Math.max(0, Math.min(viewport.scrollHeight - viewport.clientHeight, scrollTop)),
  };
}
