import { quadIntersectsBounds, type Bounds } from "./geometry";
import type { PinBoardImage } from "./types";

/**
 * 加选修饰键：Ctrl/Cmd 或 Shift。
 *
 * 素材板把两者都当作「加选」——按住点击图片切换单张选中，按住在画布空白处拖拽则做
 * 累加框选。Shift 与 Ctrl 等价，是为了让加选在点选与框选上表现一致。
 */
export function isAdditiveSelectionModifier(event: {
  ctrlKey: boolean;
  metaKey: boolean;
  shiftKey: boolean;
}): boolean {
  return event.ctrlKey || event.metaKey || event.shiftKey;
}

/**
 * 框选结果：在 `base` 基础上并入与框选包围框相交的未删除图片。
 *
 * `base` 由调用方按修饰键决定——按住加选修饰键时传入当前选中（累加框选，已选中的
 * 不会取消，可多次框选累加），否则传空集合（替换框选）。返回新的集合，不修改入参。
 */
export function marqueeSelection(
  images: readonly PinBoardImage[],
  bounds: Bounds,
  base: Iterable<number>,
): Set<number> {
  const selected = new Set<number>(base);
  for (const image of images) {
    if (!image.deleted && image.available && quadIntersectsBounds(image.points, bounds)) {
      selected.add(image.imageId);
    }
  }
  return selected;
}

/** 两个选中集合是否包含完全相同的图片 id。 */
export function sameSelection(
  left: ReadonlySet<number>,
  right: ReadonlySet<number>,
): boolean {
  if (left === right) return true;
  if (left.size !== right.size) return false;
  for (const imageId of left) {
    if (!right.has(imageId)) return false;
  }
  return true;
}
