/**
 * 画板快照、脏标记与常规保存载荷的纯数据转换。
 *
 * 撤销/重做用的「条目子集 → 不可变快照」、脏标记的状态串与常规保存的条目列表都在这里；
 * 后台结算走另一条路径，由 `pendingSettle.ts` 用 `savePayload.ts` 自己构造全量载荷。
 */

import type {
  PinBoardEditableItem,
  PinBoardItem,
  PinBoardTextAlign,
} from "./types";
import { editableItemFields, editableTextPayload } from "./savePayload";
import { pinBoardTextSignature } from "./textEditing";
import type { Point, Quad } from "./geometry";

export type ImageSnapshot = {
  itemId: number;
  deleted: boolean;
  layer: number;
  points: Quad;
  uv: Quad;
  /** 文字条目的载荷与度量；位图/笔迹为 undefined。 */
  text?: {
    text: string;
    fontSize: number;
    color: string;
    align: PinBoardTextAlign;
    width: number;
    height: number;
  };
};

export function cloneQuad(quad: Quad): Quad {
  return quad.map((point) => [...point] as Point) as Quad;
}

export function cloneBoardItem(image: PinBoardItem): PinBoardItem {
  return { ...image, points: cloneQuad(image.points), uv: cloneQuad(image.uv) };
}

/** 按 id 取当前条目并复制成快照；不存在的 id 直接跳过。 */
export function snapshotImages(
  imageById: ReadonlyMap<number, PinBoardItem>,
  itemIds: readonly number[],
): ImageSnapshot[] {
  return itemIds.flatMap((itemId) => {
    const image = imageById.get(itemId);
    if (!image) return [];
    return [{
      itemId,
      deleted: image.deleted,
      layer: image.layer,
      points: cloneQuad(image.points),
      uv: cloneQuad(image.uv),
      ...(image.kind === "text"
        ? {
          text: {
            text: image.text,
            fontSize: image.fontSize,
            color: image.color,
            align: image.align,
            width: image.width,
            height: image.height,
          },
        }
        : {}),
    }];
  });
}

/**
 * 把快照写回条目，返回文字签名确实变化的条目 id（调用方据此让旧纹理失效）。
 */
export function restoreSnapshots(
  imageById: ReadonlyMap<number, PinBoardItem>,
  snapshots: readonly ImageSnapshot[],
): number[] {
  const changedTextIds: number[] = [];
  for (const snapshot of snapshots) {
    const image = imageById.get(snapshot.itemId);
    if (!image) continue;
    const textBefore = image.kind === "text" ? pinBoardTextSignature(image) : "";
    image.deleted = snapshot.deleted;
    image.layer = snapshot.layer;
    image.points = cloneQuad(snapshot.points);
    image.uv = cloneQuad(snapshot.uv);
    if (image.kind === "text" && snapshot.text) {
      image.text = snapshot.text.text;
      image.fontSize = snapshot.text.fontSize;
      image.color = snapshot.text.color;
      image.align = snapshot.text.align;
      image.width = snapshot.text.width;
      image.height = snapshot.text.height;
      if (pinBoardTextSignature(image) !== textBefore) changedTextIds.push(image.itemId);
    }
  }
  return changedTextIds;
}

/** 脏标记用的画板状态串：变换、顺序、图层与文字签名共同参与。 */
export function boardStateKey(images: readonly PinBoardItem[]): string {
  return JSON.stringify(images.map((image) => ({
    itemId: image.itemId,
    order: image.order,
    layer: image.layer,
    deleted: image.deleted,
    points: image.points,
    uv: image.uv,
    // 文字内容/样式/度量参与脏标记：只改字不动四边形时也必须保存。
    ...(image.kind === "text" ? { text: pinBoardTextSignature(image) } : {}),
  })));
}

/**
 * 常规保存的载荷：文字只在签名变化时携带，避免无变化也推进步骤。
 * 后台结算快照另有全量携带路径，两者不能合并。
 */
export function editableBoardImages(
  images: readonly PinBoardItem[],
  savedTextSignatures: ReadonlyMap<number, string>,
): PinBoardEditableItem[] {
  return images.map((image) => {
    const text = image.kind === "text"
      && pinBoardTextSignature(image) !== savedTextSignatures.get(image.itemId)
      ? editableTextPayload(image)
      : undefined;
    return {
      ...editableItemFields(image),
      ...(text ? { text } : {}),
    };
  });
}
