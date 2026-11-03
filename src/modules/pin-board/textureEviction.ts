/**
 * 纹理缓存的淘汰顺序。
 *
 * 只负责「先淘汰谁」的策略，不持有缓存：调用方保留两张纹理表、预算与在途预留。
 * 顺序依次是「不可见优先 → 有缩略图兜底的优先 → 小图优先 → 最久未用优先」；
 * 受保护条目（当前正在读取的、导出块涉及的）永不进入候选。
 */

import type { LoadedTexture } from "./rendererGpu";

export type TextureEvictionOptions = {
  /** 当前正在读取或刚写入的条目：即使超预算也不淘汰它。 */
  protectedItemId?: number;
  visibleIds: ReadonlySet<number>;
  /** 整画布导出当前块涉及的条目。 */
  exportProtectedIds: ReadonlySet<number> | null;
  isPreview: boolean;
  hasPreview: (itemId: number) => boolean;
};

export function textureEvictionOrder(
  entries: Iterable<[number, LoadedTexture]>,
  options: TextureEvictionOptions,
): Array<[number, LoadedTexture]> {
  return [...entries]
    .filter(([itemId]) => (
      itemId !== options.protectedItemId && !options.exportProtectedIds?.has(itemId)
    ))
    .sort((left, right) => {
      const visibilityOrder = Number(options.visibleIds.has(left[0]))
        - Number(options.visibleIds.has(right[0]));
      if (visibilityOrder !== 0) return visibilityOrder;
      // 高清淘汰优先选择已有缩略图兜底的图片，避免淘汰后画面空白闪烁；
      // 缩略图淘汰时本项恒相等（预览表自身就是兜底），退化为 LRU。
      const leftCovered = Number(options.isPreview || options.hasPreview(left[0]));
      const rightCovered = Number(options.isPreview || options.hasPreview(right[0]));
      if (leftCovered !== rightCovered) return rightCovered - leftCovered;
      // 大图（纹理字节多）重新解码成本高、等待久，优先淘汰小图：小图重载
      // 快且有缩略图兜底，大图尽量保留，避免「划走再划回」反复等大图。
      if (left[1].byteSize !== right[1].byteSize) return left[1].byteSize - right[1].byteSize;
      return left[1].lastUsed - right[1].lastUsed;
    });
}
