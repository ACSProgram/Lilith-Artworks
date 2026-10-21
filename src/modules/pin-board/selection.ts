import { quadIntersectsBounds, type Bounds } from "./geometry";
import type { PinBoardItem } from "./types";

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
 *
 * `hitTest` 让调用方替换掉「四角框是否与框相交」这条默认口径。笔迹要按墨迹判（框住
 * 椭圆内部但没有碰到描边时不该选中），位图与文字仍用四角框——文字框本来就贴合字形，
 * 位图不读回纹理就无法知道 alpha。命中谓词只对已通过删除/可用性检查的条目调用。
 */
export function marqueeSelection(
  images: readonly PinBoardItem[],
  bounds: Bounds,
  base: Iterable<number>,
  hitTest?: (image: PinBoardItem) => boolean,
): Set<number> {
  const selected = new Set<number>(base);
  for (const image of images) {
    if (image.deleted || !image.available) continue;
    const hit = hitTest ? hitTest(image) : quadIntersectsBounds(image.points, bounds);
    if (hit) selected.add(image.itemId);
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
  for (const itemId of left) {
    if (!right.has(itemId)) return false;
  }
  return true;
}
