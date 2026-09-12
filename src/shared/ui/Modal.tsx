import { useEffect, useId, type FormEvent, type ReactNode } from "react";
import { X } from "lucide-react";

interface ModalProps {
  className?: string;
  title: string;
  description?: string;
  children: ReactNode;
  footer: ReactNode;
  onClose?: () => void;
  onSubmit?: () => void;
}

export function Modal({
  className,
  title,
  description,
  children,
  footer,
  onClose,
  onSubmit,
}: ModalProps) {
  const titleId = useId();
  const descriptionId = useId();

  useEffect(() => {
    const handleKey = (event: KeyboardEvent) => {
      if (event.key === "Escape" && onClose) onClose();
    };
    window.addEventListener("keydown", handleKey);
    return () => window.removeEventListener("keydown", handleKey);
  }, [onClose]);

  const handleSubmit = (event: FormEvent) => {
    event.preventDefault();
    onSubmit?.();
  };

  return (
    <div className="modal-backdrop" role="presentation" onMouseDown={onClose}>
      <form
        className={`modal${className ? ` ${className}` : ""}`}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={description ? descriptionId : undefined}
        onSubmit={handleSubmit}
        onMouseDown={(event) => event.stopPropagation()}
      >
        <header className="modal-header">
          <div>
            <h2 id={titleId}>{title}</h2>
            {description && <p id={descriptionId}>{description}</p>}
          </div>
          {onClose && (
            <button className="icon-button" type="button" onClick={onClose} title="关闭" aria-label="关闭">
              <X size={18} aria-hidden="true" />
            </button>
          )}
        </header>
        <div className="modal-content">{children}</div>
        <footer className="modal-footer">{footer}</footer>
      </form>
    </div>
  );
}
