import {
  ClipboardCopy,
  ClipboardPaste,
  Copy,
  Download,
  FlipHorizontal2,
  FlipVertical2,
  Fullscreen,
  ImageDown,
  Layers3,
  LayoutGrid,
  LocateFixed,
  Minimize2,
  Trash2,
  Type,
  Upload,
} from "lucide-react";
import { ContextMenu, type ContextMenuItem } from "../../shared/ui/ContextMenu";
import type { PinBoardContextMenuRequest, PinBoardRenderer } from "./renderer";

/** 图层菜单在选中含笔迹时的置灰说明：笔迹固定在标注层，原生端保存时也会强制写 3。 */
const STROKE_LAYER_LOCKED_TITLE = "笔迹固定在标注层，取消选中笔迹后再改图层";

export interface PinBoardContextMenuProps {
  request: PinBoardContextMenuRequest;
  renderer: PinBoardRenderer;
  fullscreen: boolean;
  canPaste: boolean;
  onCopy: () => void;
  onCopyClipboard: () => void;
  onPaste: () => void;
  onImport: () => void;
  onAddText: () => void;
  onExport: () => void;
  onExportCanvas: () => void;
  onDelete: () => void;
  onToggleFullscreen: () => void;
  onClose: () => void;
}

/** 画布右键菜单：条目操作、图层与空白处的画板级入口。 */
export function PinBoardContextMenu({
  request,
  renderer,
  fullscreen,
  canPaste,
  onCopy,
  onCopyClipboard,
  onPaste,
  onImport,
  onAddText,
  onExport,
  onExportCanvas,
  onDelete,
  onToggleFullscreen,
  onClose,
}: PinBoardContextMenuProps) {
  const items: ContextMenuItem[] = request.targetImage
    ? [
        ...(request.singleTextSelected
          ? [
            {
              key: "edit-text",
              label: "编辑文字",
              icon: <Type size={16} />,
              onClick: () => renderer.beginTextEdit(),
            },
            { key: "sep-edit-text", divider: true },
          ]
          : []),
        { key: "copy-board", label: "复制画板条目", icon: <Copy size={16} />, onClick: onCopy },
        // 单条导出与系统剪贴板复制只认位图：选中文字或笔迹时两项一起置灰。
        { key: "copy-clipboard", label: "复制图片到剪贴板", icon: <ClipboardCopy size={16} />, disabled: request.selectedCount !== 1 || request.exportableCount === 0, disabledTitle: "只有单个位图条目能复制到系统剪贴板", onClick: onCopyClipboard },
        { key: "export", label: "导出图片", icon: <Download size={16} />, disabled: request.exportableCount === 0, disabledTitle: "只有位图条目能单条导出", onClick: onExport },
        { key: "sep-flip", divider: true },
        { key: "flip-h", label: "水平翻转", icon: <FlipHorizontal2 size={16} />, onClick: () => renderer.flipSelected(true) },
        { key: "flip-v", label: "垂直翻转", icon: <FlipVertical2 size={16} />, onClick: () => renderer.flipSelected(false) },
        { key: "arrange", label: "阵列排序", icon: <LayoutGrid size={16} />, disabled: request.arrangeableCount < 2, onClick: () => void renderer.arrangeSelected() },
        { key: "sep-layer", divider: true },
        { key: "layer-label", heading: true, label: "图层", icon: <Layers3 size={15} /> },
        { key: "layer-top", label: "顶层", icon: <span className="pin-board-layer-swatch top" />, disabled: request.selectionHasStroke, disabledTitle: STROKE_LAYER_LOCKED_TITLE, onClick: () => renderer.setSelectedLayer(2) },
        { key: "layer-middle", label: "中层", icon: <span className="pin-board-layer-swatch middle" />, disabled: request.selectionHasStroke, disabledTitle: STROKE_LAYER_LOCKED_TITLE, onClick: () => renderer.setSelectedLayer(1) },
        { key: "layer-bottom", label: "底层", icon: <span className="pin-board-layer-swatch bottom" />, disabled: request.selectionHasStroke, disabledTitle: STROKE_LAYER_LOCKED_TITLE, onClick: () => renderer.setSelectedLayer(0) },
        { key: "sep-delete", divider: true },
        { key: "delete", label: "删除图片", icon: <Trash2 size={16} />, danger: true, onClick: onDelete },
      ]
    : [
        { key: "paste", label: "粘贴画板条目", icon: <ClipboardPaste size={16} />, disabled: !request.canEditBoard || !canPaste, onClick: onPaste },
        { key: "import", label: "导入图片", icon: <Upload size={16} />, disabled: !request.canEditBoard, onClick: onImport },
        { key: "text", label: "添加文字", icon: <Type size={16} />, disabled: !request.canEditBoard, onClick: onAddText },
        { key: "export-canvas", label: "导出画布", icon: <ImageDown size={16} />, disabled: !request.canEditBoard, disabledTitle: "画板已锁定，无法导出", onClick: onExportCanvas },
        { key: "sep-view", divider: true },
        { key: "reset-view", label: "重置视图", icon: <LocateFixed size={16} />, onClick: () => renderer.resetViewport() },
        {
          key: "fullscreen",
          label: fullscreen ? "退出全屏" : "全屏显示",
          icon: fullscreen ? <Minimize2 size={16} /> : <Fullscreen size={16} />,
          onClick: onToggleFullscreen,
        },
      ];

  return (
    <ContextMenu
      x={request.clientX}
      y={request.clientY}
      maxHeight={480}
      items={items}
      onClose={onClose}
    />
  );
}
