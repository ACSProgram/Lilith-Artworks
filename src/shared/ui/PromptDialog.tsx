import { useEffect, useRef, useState } from "react";
import { Modal } from "./Modal";

interface PromptDialogProps {
  title: string;
  label?: string;
  initialValue?: string;
  confirmLabel?: string;
  cancelLabel?: string;
  onConfirm: (value: string) => void;
  onCancel: () => void;
}

/**
 * 统一的文本输入对话框，替代 `window.prompt`。确认后把当前输入值交给
 * `onConfirm`；输入为空时确认按钮不生效。
 *
 * 注意：Modal 根元素本身是 `<form>`，本组件内容不能再嵌套 form（HTML
 * 不允许嵌套 form，且 footer 的 submit 按钮会提交外层 Modal form、触发
 * 未提供的 onSubmit）。因此确认按钮使用显式 onClick 提交，Enter 由输入框
 * 的 keydown 处理，两者走同一条确认路径。
 */
export function PromptDialog({
  title,
  label,
  initialValue = "",
  confirmLabel = "确认",
  cancelLabel = "取消",
  onConfirm,
  onCancel,
}: PromptDialogProps) {
  const [value, setValue] = useState(initialValue);
  const inputRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    inputRef.current?.focus();
    inputRef.current?.select();
  }, []);

  const confirm = () => {
    const trimmed = value.trim();
    if (trimmed) onConfirm(trimmed);
  };

  return (
    <Modal
      title={title}
      onClose={onCancel}
      footer={(
        <>
          <button className="secondary-button" type="button" onClick={onCancel}>
            {cancelLabel}
          </button>
          <button className="primary-button" type="button" disabled={!value.trim()} onClick={confirm}>
            {confirmLabel}
          </button>
        </>
      )}
    >
      <div className="prompt-dialog-form">
        {label && <label className="prompt-dialog-label" htmlFor="prompt-dialog-input">{label}</label>}
        <input
          id="prompt-dialog-input"
          ref={inputRef}
          className="prompt-dialog-input"
          value={value}
          onChange={(event) => setValue(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter" && !event.shiftKey) {
              event.preventDefault();
              confirm();
            }
          }}
        />
      </div>
    </Modal>
  );
}
