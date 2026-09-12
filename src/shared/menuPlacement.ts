export interface MenuAnchor {
  x: number;
  y: number;
}

export interface MenuSize {
  width: number;
  height: number;
}

export interface ViewportSize {
  width: number;
  height: number;
}

export interface MenuPosition {
  left: number;
  top: number;
}

export const MENU_VIEWPORT_MARGIN = 8;

/**
 * Keeps a fixed-positioned context menu inside the viewport. The anchor is the
 * pointer position; size is the measured menu box, so long menus (which shrink
 * through their own max-height) are placed without guessing.
 */
export function placeMenu(
  anchor: MenuAnchor,
  size: MenuSize,
  viewport: ViewportSize,
  margin: number = MENU_VIEWPORT_MARGIN,
): MenuPosition {
  const maxLeft = Math.max(margin, viewport.width - size.width - margin);
  const maxTop = Math.max(margin, viewport.height - size.height - margin);
  return {
    left: Math.min(Math.max(anchor.x, margin), maxLeft),
    top: Math.min(Math.max(anchor.y, margin), maxTop),
  };
}
