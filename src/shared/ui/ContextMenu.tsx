import { useEffect, type ReactNode } from "react";

export interface ContextMenuItem {
  key: string;
  /** 左侧图标（lucide 组件）。 */
  icon?: ReactNode;
  /** 菜单项文本。 */
  label?: ReactNode;
  /** 分隔线（忽略其它字段）。 */
  divider?: boolean;
  /** 不可点击的标题行（忽略 onClick）。 */
  heading?: boolean;
  /** 危险操作样式（删除/清空）。 */
  danger?: boolean;
  disabled?: boolean;
  /** 禁用时的提示文本。 */
  disabledTitle?: string;
  /** 右侧快捷键提示。 */
  shortcut?: string;
  onClick?: () => void;
}

interface ContextMenuProps {
  x: number;
  y: number;
  items: ContextMenuItem[];
  /** 追加到共享 `.context-menu` 类之后的模块类名。 */
  className?: string;
  /** 菜单的无障碍标签。 */
  ariaLabel?: string;
  /** 视口边缘保护用的最大宽度/高度。 */
  maxWidth?: number;
  maxHeight?: number;
  onClose: () => void;
}

/**
 * 统一的右键菜单。所有模块共用一套关闭监听：
 * 点击菜单外、Escape、窗口 resize、任意滚动都会关闭；
 * 菜单自身点击与右键不会冒泡触发关闭。
 */
export function ContextMenu({
  x,
  y,
  items,
  className,
  ariaLabel,
  maxWidth = 230,
  maxHeight = 240,
  onClose,
}: ContextMenuProps) {
  useEffect(() => {
    const onPointerDown = (event: PointerEvent) => {
      if (!(event.target instanceof Element) || !event.target.closest(".context-menu")) {
        onClose();
      }
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    const onViewportChange = () => onClose();
    window.addEventListener("pointerdown", onPointerDown);
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("resize", onViewportChange);
    window.addEventListener("scroll", onViewportChange, true);
    return () => {
      window.removeEventListener("pointerdown", onPointerDown);
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("resize", onViewportChange);
      window.removeEventListener("scroll", onViewportChange, true);
    };
  }, [onClose]);

  const left = Math.min(x, Math.max(window.innerWidth - maxWidth, 8));
  const top = Math.min(y, Math.max(window.innerHeight - maxHeight, 8));

  return (
    <div
      className={`context-menu${className ? ` ${className}` : ""}`}
      role="menu"
      aria-label={ariaLabel}
      style={{ left, top }}
      onPointerDown={(event) => event.stopPropagation()}
      onContextMenu={(event) => event.preventDefault()}
    >
      {items.map((item) => {
        if (item.divider) {
          return <div key={item.key} className="context-menu-separator" role="separator" />;
        }
        if (item.heading) {
          return (
            <div key={item.key} className="context-menu-heading">
              {item.icon}
              <span>{item.label}</span>
            </div>
          );
        }
        return (
          <button
            key={item.key}
            type="button"
            role="menuitem"
            disabled={item.disabled}
            title={item.disabled ? item.disabledTitle : undefined}
            className={item.danger ? "danger" : undefined}
            onClick={() => {
              item.onClick?.();
              onClose();
            }}
          >
            {item.icon}
            <span>{item.label}</span>
            {item.shortcut && <kbd>{item.shortcut}</kbd>}
          </button>
        );
      })}
    </div>
  );
}
