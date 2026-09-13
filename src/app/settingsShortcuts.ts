/** 设置页快捷键输入的标签显示与键盘事件捕获，逻辑自 Lilith Client 设置页迁移。 */

export function shortcutLabel(value: string): string {
  return value
    .split("+")
    .map((part) => (part === "CommandOrControl" ? "Ctrl" : part))
    .join(" + ");
}

export function shortcutFromEvent(
  event: {
    ctrlKey: boolean;
    metaKey: boolean;
    altKey: boolean;
    shiftKey: boolean;
    code: string;
  },
  allowUnmodified = false,
): string | null {
  const modifiers: string[] = [];
  if (event.ctrlKey || event.metaKey) modifiers.push("CommandOrControl");
  if (event.altKey) modifiers.push("Alt");
  if (event.shiftKey) modifiers.push("Shift");

  const code = event.code;
  let key: string | null = null;
  if (/^Key[A-Z]$/.test(code)) key = code.slice(3);
  else if (/^Digit[0-9]$/.test(code)) key = code.slice(5);
  else if (/^F(?:[1-9]|1[0-9]|2[0-4])$/.test(code)) key = code;
  else {
    key = ({
      Space: "Space",
      Enter: "Enter",
      Tab: "Tab",
      ArrowUp: "ArrowUp",
      ArrowDown: "ArrowDown",
      ArrowLeft: "ArrowLeft",
      ArrowRight: "ArrowRight",
      Home: "Home",
      End: "End",
      PageUp: "PageUp",
      PageDown: "PageDown",
      Escape: "Escape",
      PrintScreen: "PrintScreen",
    } as Record<string, string>)[code] ?? null;
  }
  if (!key || (!allowUnmodified && modifiers.length === 0 && !key.startsWith("F") && key !== "PrintScreen")) return null;
  return [...modifiers, key].join("+");
}
