import { useLayoutEffect, useRef, useState } from "react";
import type { CSSProperties } from "react";
import { MENU_VIEWPORT_MARGIN, placeMenu } from "./menuPlacement";
import type { MenuAnchor, MenuPosition } from "./menuPlacement";

export interface MenuPlacement {
  ref: (element: HTMLElement | null) => void;
  style: CSSProperties;
}

/**
 * Places a fixed-positioned menu at the pointer and clamps it to the viewport
 * using the measured menu box. The measurement runs before paint, so the menu
 * never flashes outside the window.
 */
export function useMenuPlacement(anchor: MenuAnchor | null, margin: number = MENU_VIEWPORT_MARGIN): MenuPlacement {
  const [element, setElement] = useState<HTMLElement | null>(null);
  const [position, setPosition] = useState<MenuPosition | null>(null);
  const anchorX = anchor?.x ?? null;
  const anchorY = anchor?.y ?? null;

  useLayoutEffect(() => {
    if (anchorX === null || anchorY === null) {
      setPosition(null);
      return;
    }
    const apply = () => {
      const size = element
        ? { width: element.offsetWidth, height: element.offsetHeight }
        : { width: 0, height: 0 };
      setPosition(placeMenu(
        { x: anchorX, y: anchorY },
        size,
        { width: window.innerWidth, height: window.innerHeight },
        margin,
      ));
    };
    apply();
    window.addEventListener("resize", apply);
    return () => window.removeEventListener("resize", apply);
  }, [anchorX, anchorY, element, margin]);

  return {
    ref: (node: HTMLElement | null) => setElement((current) => (current === node ? current : node)),
    style: position
      ? { left: position.left, top: position.top }
      : { left: anchorX ?? 0, top: anchorY ?? 0, visibility: "hidden" },
  };
}
