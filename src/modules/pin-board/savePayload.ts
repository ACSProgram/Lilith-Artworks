import type {
  PinBoardEditableItem, PinBoardEditableText, PinBoardItem, PinBoardTextItem,
} from "./types";

type EditableFields = Pick<
  PinBoardEditableItem,
  "itemId" | "order" | "layer" | "deleted" | "points" | "uv"
>;

/**
 * 两条保存路径共用的字段克隆：几何四边形各复制一份，避免调用方后续改动
 * 渲染器里的条目时污染已经发出的请求。文字载荷是否携带由调用方决定——
 * 切换快照总是携带，常规保存只在签名变化时携带。
 */
export function editableItemFields(item: PinBoardItem): EditableFields {
  return {
    itemId: item.itemId,
    order: item.order,
    layer: item.layer,
    deleted: item.deleted,
    points: item.points.map((point) => [...point]) as PinBoardItem["points"],
    uv: item.uv.map((point) => [...point]) as PinBoardItem["uv"],
  };
}

export function editableTextPayload(item: PinBoardTextItem): PinBoardEditableText {
  return {
    text: item.text,
    fontSize: item.fontSize,
    color: item.color,
    align: item.align,
    measuredWidth: item.width,
    measuredHeight: item.height,
  };
}
