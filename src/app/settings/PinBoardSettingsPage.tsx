import { Fragment } from "react";
import {
  BookOpen, Eye, FolderOpen, Images, Keyboard, Layers, LogOut,
  MousePointer2, MoveHorizontal, Pencil, Save, X,
  type LucideIcon,
} from "lucide-react";
import { PIN_BOARD_ARRANGEMENT_GAP_RATIO_MAX } from "../../modules/pin-board/PinBoardModule";
import { shortcutFromEvent, shortcutLabel } from "../settingsShortcuts";
import type { AppSettings } from "../types";
import type { SettingsUpdate } from "./shared";

/**
 * 素材板操作手册内容。与 `renderer.ts` 的交互实现一一对应：改动交互时同步更新此处，
 * 保证设置页展示的操作说明与实际行为一致。`keys` 中的每一项渲染为一个按键胶囊。
 */
const PIN_BOARD_MANUAL: Array<{
  title: string;
  icon: LucideIcon;
  items: Array<{ label: string; detail: string; keys?: string[] }>;
}> = [
  {
    title: "视图",
    icon: Eye,
    items: [
      { label: "缩放视图", detail: "以光标为中心缩放，滚轮向前放大、向后缩小。", keys: ["滚轮"] },
      { label: "平移视图", detail: "画板锁定时无需按住修饰键，直接拖拽即可平移。", keys: ["中键拖拽"] },
      { label: "重置视图", detail: "回到全部内容的最小包围框；打开画板时也会自动适配。" },
    ],
  },
  {
    title: "选择",
    icon: MousePointer2,
    items: [
      { label: "单选", detail: "点击一张图片将其选中。", keys: ["左键点击"] },
      { label: "加选 / 减选", detail: "按住修饰键点击图片，切换该图片的选中状态。", keys: ["Ctrl / Shift", "点击"] },
      { label: "框选", detail: "在画布空白处按住左键拖拽，框住即选中。", keys: ["空白拖拽"] },
      { label: "连续加选框选", detail: "按住修饰键在空白处拖拽，可多次框选累加；已选中的图片不会取消，框到的新图加入。", keys: ["Ctrl / Shift", "拖拽"] },
      { label: "全选", detail: "选中整块画板的全部图片，不限当前视口。", keys: ["Ctrl", "A"] },
      { label: "取消选择", detail: "在空白处单击清空选择。", keys: ["空白单击"] },
    ],
  },
  {
    title: "编辑",
    icon: Pencil,
    items: [
      { label: "移动", detail: "拖动选中的图片。", keys: ["拖拽图片"] },
      { label: "旋转", detail: "拖拽选中框上方的旋转手柄；按住 Shift 吸附到 15° 整数倍。", keys: ["拖拽手柄"] },
      { label: "缩放", detail: "拖拽选中框四角的缩放手柄。", keys: ["拖拽手柄"] },
      { label: "阵列排序", detail: "按设置的间距平铺选中的图片（需选中两张以上）。" },
      { label: "删除", detail: "删除当前选中的图片。", keys: ["Delete"] },
      { label: "撤销", detail: "撤销上一步编辑。", keys: ["Ctrl", "Z"] },
      { label: "重做", detail: "重做已撤销的编辑。", keys: ["Ctrl", "Y"] },
      { label: "复制", detail: "复制选中的图片。", keys: ["Ctrl", "C"] },
      { label: "粘贴", detail: "把复制的图片粘贴到画板。", keys: ["Ctrl", "V"] },
    ],
  },
  {
    title: "画板与文件",
    icon: FolderOpen,
    items: [
      { label: "导入图片", detail: "把图片文件拖到画布，或使用工具栏「导入图片」。", keys: ["拖入文件"] },
      { label: "添加文字", detail: "生成透明 PNG 文字素材并导入画板。" },
      { label: "导出图片", detail: "把选中的图片导出为 PNG 到指定目录。" },
      { label: "复制到剪贴板", detail: "把选中的单张图片复制到系统剪贴板。" },
      { label: "保存", detail: "立即保存画板；可在上方开启自动保存。", keys: ["Ctrl", "S"] },
      { label: "锁定 / 全屏", detail: "使用上方「锁定画板快捷键」与「画板全屏快捷键」。" },
    ],
  },
];

interface PinBoardSettingsPageProps {
  draft: AppSettings;
  update: SettingsUpdate;
  setPinBoardShortcut: (
    key: "lockShortcut" | "fullscreenShortcut",
    shortcut: string,
  ) => void;
}

export function PinBoardSettingsPage({
  draft,
  update,
  setPinBoardShortcut,
}: PinBoardSettingsPageProps) {
  return <>
    <div className="settings-section">
      <div className="settings-section-title"><Images aria-hidden="true" size={17} /><h3>素材板</h3></div>
      <div className="settings-preference-list">
        <div className="settings-preference-row">
          <span className="settings-row-icon"><Layers aria-hidden="true" size={17} /></span>
          <span className="settings-row-copy is-descriptive">
            <strong>纹理缓存等级</strong>
            <small>
              决定素材板可常驻的纹理数量与内存占用；同时决定导出画布时的最长边
              （10000 / 20000 / 30000 像素）。
            </small>
          </span>
          <div
            className="segmented-control pin-board-cache-control"
            role="radiogroup"
            aria-label="纹理缓存等级"
          >
            {([
              { level: "low", label: "低", memory: "约 256 MB" },
              { level: "medium", label: "中", memory: "约 512 MB" },
              { level: "high", label: "高", memory: "约 1 GB" },
            ] as const).map(({ level, label, memory }) => (
              <button
                key={level}
                type="button"
                role="radio"
                aria-checked={draft.pinBoard.textureCacheLevel === level}
                className={draft.pinBoard.textureCacheLevel === level ? "active" : ""}
                onClick={() => update({ pinBoard: { ...draft.pinBoard, textureCacheLevel: level } })}
              >
                <strong>{label}</strong>
                <small>{memory}</small>
              </button>
            ))}
          </div>
        </div>
        <div className="settings-preference-row">
          <span className="settings-row-icon"><MoveHorizontal aria-hidden="true" size={17} /></span>
          <span className="settings-row-copy is-descriptive">
            <label htmlFor="pin-board-arrangement-gap"><strong>阵列图片间距</strong></label>
            <small>按图片高度的百分比计算，与画布缩放无关；0 表示图片紧贴。整理与导入共用。</small>
          </span>
          <input
            id="pin-board-arrangement-gap"
            className="number-control"
            type="number"
            min={0}
            max={Math.round(PIN_BOARD_ARRANGEMENT_GAP_RATIO_MAX * 100)}
            step={1}
            value={Math.round(draft.pinBoard.arrangementGapRatio * 100)}
            onChange={(event) => update({
              pinBoard: {
                ...draft.pinBoard,
                arrangementGapRatio: Math.min(
                  PIN_BOARD_ARRANGEMENT_GAP_RATIO_MAX,
                  Math.max(0, Number(event.target.value) || 0) / 100,
                ),
              },
            })}
          />
        </div>
        <label className="settings-preference-row">
          <span className="settings-row-icon"><Save aria-hidden="true" size={17} /></span>
          <span className="settings-row-copy is-descriptive">
            <strong>自动保存</strong>
            <small>编辑停止约 1.5 秒后自动保存摆放进度，崩溃或强退最多丢失几秒内的改动。</small>
          </span>
          <input className="switch-input" type="checkbox" checked={draft.pinBoard.autosave} onChange={(event) => update({ pinBoard: { ...draft.pinBoard, autosave: event.target.checked } })} />
        </label>
        <label className="settings-preference-row">
          <span className="settings-row-icon"><LogOut aria-hidden="true" size={17} /></span>
          <span className="settings-row-copy is-descriptive">
            <strong>关闭时保存</strong>
            <small>退出应用前先保存并结算素材板；关闭后退出即结束，未保存的编辑会丢失。</small>
          </span>
          <input className="switch-input" type="checkbox" checked={draft.pinBoard.saveOnExit} onChange={(event) => update({ pinBoard: { ...draft.pinBoard, saveOnExit: event.target.checked } })} />
        </label>
        <div className="settings-preference-row">
          <span className="settings-row-icon"><Keyboard aria-hidden="true" size={17} /></span>
          <span className="settings-row-copy is-descriptive">
            <label htmlFor="pin-board-lock-shortcut"><strong>锁定画板快捷键</strong></label>
            <small>锁定后无法编辑图片，仅可缩放和移动视图。</small>
          </span>
          <div className="shortcut-control">
            <Keyboard size={16} aria-hidden="true" />
            <input
              id="pin-board-lock-shortcut"
              className="shortcut-input"
              value={shortcutLabel(draft.pinBoard.lockShortcut)}
              placeholder="未设置"
              readOnly
              spellCheck={false}
              onKeyDown={(event) => {
                if (event.key === "Backspace" || event.key === "Delete") {
                  event.preventDefault();
                  setPinBoardShortcut("lockShortcut", "");
                  return;
                }
                const shortcut = shortcutFromEvent(event, true);
                if (!shortcut) return;
                event.preventDefault();
                setPinBoardShortcut("lockShortcut", shortcut);
              }}
              onFocus={(event) => event.currentTarget.select()}
            />
            {draft.pinBoard.lockShortcut && (
              <button
                className="icon-button"
                type="button"
                title="清除锁定画板快捷键"
                aria-label="清除锁定画板快捷键"
                onClick={() => setPinBoardShortcut("lockShortcut", "")}
              >
                <X size={15} />
              </button>
            )}
          </div>
        </div>
        <div className="settings-preference-row">
          <span className="settings-row-icon"><Keyboard aria-hidden="true" size={17} /></span>
          <span className="settings-row-copy is-descriptive">
            <label htmlFor="pin-board-fullscreen-shortcut"><strong>画板全屏快捷键</strong></label>
            <small>切换素材板内容区全屏，全屏时隐藏顶部工具栏。</small>
          </span>
          <div className="shortcut-control">
            <Keyboard size={16} aria-hidden="true" />
            <input
              id="pin-board-fullscreen-shortcut"
              className="shortcut-input"
              value={shortcutLabel(draft.pinBoard.fullscreenShortcut)}
              placeholder="未设置"
              readOnly
              spellCheck={false}
              onKeyDown={(event) => {
                if (event.key === "Backspace" || event.key === "Delete") {
                  event.preventDefault();
                  setPinBoardShortcut("fullscreenShortcut", "");
                  return;
                }
                const shortcut = shortcutFromEvent(event, true);
                if (!shortcut) return;
                event.preventDefault();
                setPinBoardShortcut("fullscreenShortcut", shortcut);
              }}
              onFocus={(event) => event.currentTarget.select()}
            />
            {draft.pinBoard.fullscreenShortcut && (
              <button
                className="icon-button"
                type="button"
                title="清除画板全屏快捷键"
                aria-label="清除画板全屏快捷键"
                onClick={() => setPinBoardShortcut("fullscreenShortcut", "")}
              >
                <X size={15} />
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
    <div className="settings-section">
      <div className="settings-section-title"><BookOpen aria-hidden="true" size={17} /><h3>操作手册</h3></div>
      <div className="pin-board-manual">
        {PIN_BOARD_MANUAL.map((group) => {
          const GroupIcon = group.icon;
          return (
            <section className="pin-board-manual-group" key={group.title}>
              <header><GroupIcon aria-hidden="true" size={15} /><h4>{group.title}</h4></header>
              <div className="pin-board-manual-rows">
                {group.items.map((item) => (
                  <div className="pin-board-manual-row" key={item.label}>
                    <span className="pin-board-manual-label">{item.label}</span>
                    <span className="pin-board-manual-detail">{item.detail}</span>
                    <span className="pin-board-manual-keys">
                      {item.keys?.map((key, index) => (
                        <Fragment key={key}>
                          {index > 0 && <span className="pin-board-manual-plus" aria-hidden="true">+</span>}
                          <kbd>{key}</kbd>
                        </Fragment>
                      ))}
                    </span>
                  </div>
                ))}
              </div>
            </section>
          );
        })}
      </div>
    </div>
  </>;
}
