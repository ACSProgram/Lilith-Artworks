/**
 * 图层可见性：**纯视图状态**。
 *
 * 下标即条目的 `layer`：0 底层 / 1 中层 / 2 顶层 / 3 标注层（笔迹固定占 3）。
 * 文字跟随它所在的 0–2 层一起显示或隐藏，不需要单独一档。
 *
 * 隐藏一层只影响显示与命中，不写库、不标记 dirty——否则「看一眼底图」就会变成
 * 一次仓库写操作。状态随画板会话（`session.ts`）在进程内恢复。
 */

/** 笔迹固定占用的标注层。与原生端 `save_pin_board` 强制写入的值一致。 */
export const STROKE_LAYER = 3;

/** 图层总数（底层 / 中层 / 顶层 / 标注层）。 */
export const PIN_BOARD_LAYER_COUNT = STROKE_LAYER + 1;

/** 按 layer 下标的可见性；`true` 表示显示。 */
export type PinBoardLayerVisibility = [boolean, boolean, boolean, boolean];

/** 默认全部显示。 */
export const DEFAULT_LAYER_VISIBILITY: PinBoardLayerVisibility = [true, true, true, true];

/**
 * 归一化成一份新的四元组：缺项、越界与非法值一律按「显示」处理。
 *
 * 会话对象来自内存，理论上不会缺项；但会话是跨视图保留的长期状态，这里按防御性
 * 口径补齐，避免旧会话（或测试替身）少了字段就让某一层永远消失。
 */
export function normalizeLayerVisibility(
  source: readonly boolean[] | null | undefined,
): PinBoardLayerVisibility {
  return [
    source?.[0] !== false,
    source?.[1] !== false,
    source?.[2] !== false,
    source?.[3] !== false,
  ];
}

/** 该层是否显示。未知 layer 按显示处理，不把异常数据藏起来。 */
export function isLayerVisible(
  visibility: PinBoardLayerVisibility,
  layer: number,
): boolean {
  return visibility[layer] !== false;
}

/**
 * 找出「已选中、但所在图层刚被隐藏」的条目 id。
 *
 * 隐藏某层后必须把这些条目踢出选中：否则它们看不见却能继续被删除、导出或改图层。
 */
export function hiddenSelectionIds(
  selectedIds: Iterable<number>,
  items: readonly { itemId: number; layer: number }[],
  visibility: PinBoardLayerVisibility,
): number[] {
  const layerByItem = new Map(items.map((item) => [item.itemId, item.layer]));
  return [...selectedIds].filter((itemId) => {
    const layer = layerByItem.get(itemId);
    return layer !== undefined && !isLayerVisible(visibility, layer);
  });
}
