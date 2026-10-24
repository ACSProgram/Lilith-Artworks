import { ChevronUp, Eye, EyeOff, Layers3 } from "lucide-react";
import { Fragment, useState } from "react";
import { STROKE_LAYER, isLayerVisible, type PinBoardLayerVisibility } from "./layers";

/**
 * 图层顺序按「从上到下」列：标注层（笔迹）永远在最上，随后是顶层、中层、底层。
 * `layer` 即条目的 layer 值，0–2 由图片与文字共用，3 是笔迹的固定层。
 */
const LAYER_ROWS: { layer: number; label: string; swatch: string }[] = [
  { layer: STROKE_LAYER, label: "标注层", swatch: "annotation" },
  { layer: 2, label: "顶层", swatch: "top" },
  { layer: 1, label: "中层", swatch: "middle" },
  { layer: 0, label: "底层", swatch: "bottom" },
];

/**
 * 画布左上角的图层浮层：收起后只剩一个按钮，展开是四个可见性开关与每层的条目数。
 *
 * 与标注工具条同属画布内的浮动控件，不占主工具栏：主工具栏已有二十来个按钮，
 * 再排四个会被横向滚动藏掉；全屏时工具栏整体隐藏，浮层仍然可用。
 *
 * 显隐是纯视图状态：不改模型、不标记 dirty，因此这里只回调 `onToggle`，
 * 由渲染器决定重绘与选中清理。条目数由调用方按 layer 下标给出；数字带
 * `aria-hidden`，否则四个开关的可访问名会变成「顶层 12」。
 *
 * 展开状态是组件本地状态，默认收起：每次启动应用都从只剩图层按钮的形态开始，
 * 展开后在同一次进程内切画板、切视图都不收起，组件卸载（换仓库或退出）后回到收起。
 */
export function PinBoardLayersPanel({
  visibility,
  counts,
  onToggle,
  disabled,
}: {
  visibility: PinBoardLayerVisibility;
  /** 每层未删除条目数（下标即 layer）；缺省或越界时不显示数字。 */
  counts?: readonly number[];
  onToggle: (layer: number, visible: boolean) => void;
  disabled: boolean;
}) {
  const [open, setOpen] = useState(false);
  /** 有任一层被隐藏时按钮高亮，否则隐藏了图层却看不出来。 */
  const filtering = visibility.some((visible) => !visible);

  return (
    <div className="pin-board-layers">
      {open ? (
        <section className="pin-board-layers-panel" aria-label="图层可见性">
          <header className="pin-board-layers-head">
            <Layers3 size={15} aria-hidden="true" />
            <span>图层</span>
            <button
              type="button"
              className="icon-button"
              disabled={disabled}
              onClick={() => setOpen(false)}
              title="收起图层"
              aria-label="收起图层"
            >
              <ChevronUp size={15} />
            </button>
          </header>
          {LAYER_ROWS.map((row, index) => {
            const visible = isLayerVisible(visibility, row.layer);
            // 标注层不显示数字：那一层只有笔迹，数字既不是用户关心的、也容易和层名混读。
            const count = row.layer === STROKE_LAYER ? null : counts?.[row.layer];
            return (
              <Fragment key={row.layer}>
                {index === 1 && <span className="pin-board-layers-divider" aria-hidden="true" />}
                <button
                  type="button"
                  className={`pin-board-layer-row${visible ? "" : " hidden"}`}
                  aria-pressed={visible}
                  aria-label={row.label}
                  disabled={disabled}
                  onClick={() => onToggle(row.layer, !visible)}
                  title={`${visible ? "隐藏" : "显示"}${row.label}`}
                >
                  {visible ? <Eye size={15} aria-hidden="true" /> : <EyeOff size={15} aria-hidden="true" />}
                  <span className={`pin-board-layer-swatch ${row.swatch}`} />
                  <span className="pin-board-layer-name">{row.label}</span>
                  {count === undefined || count === null ? null : (
                    <span className="pin-board-layer-count" aria-hidden="true">{count}</span>
                  )}
                </button>
              </Fragment>
            );
          })}
        </section>
      ) : (
        <button
          type="button"
          className={`pin-board-layers-toggle${filtering ? " filtering" : ""}`}
          disabled={disabled}
          onClick={() => setOpen(true)}
          title={filtering ? "图层（有图层被隐藏）" : "图层可见性"}
          aria-label="图层可见性"
          aria-expanded={false}
        >
          <Layers3 size={18} />
        </button>
      )}
    </div>
  );
}
