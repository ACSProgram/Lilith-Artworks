export function shortcutMatches(event: KeyboardEvent, shortcut: string): boolean {
  const parts = shortcut.split("+").map((part) => part.trim().toLowerCase()).filter(Boolean);
  if (parts.length === 0) return false;

  const needsControl = parts.includes("commandorcontrol");
  const needsAlt = parts.includes("alt");
  const needsShift = parts.includes("shift");
  if ((event.ctrlKey || event.metaKey) !== needsControl
    || event.altKey !== needsAlt
    || event.shiftKey !== needsShift) return false;

  const key = parts.find((part) => !["commandorcontrol", "alt", "shift"].includes(part));
  if (!key) return false;
  const eventKey = /^Key[A-Z]$/.test(event.code)
    ? event.code.slice(3).toLowerCase()
    : /^Digit[0-9]$/.test(event.code)
      ? event.code.slice(5)
      : event.code.toLowerCase();
  return eventKey === key;
}

export function shortcutCanHandle(event: KeyboardEvent): boolean {
  const target = event.target as HTMLElement | null;
  return !target?.matches("input, textarea, [contenteditable='true']");
}
