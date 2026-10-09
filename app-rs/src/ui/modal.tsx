/**
 * 轻量 Dialog（AntD Modal 的替代）：portal + 遮罩点击/Esc 关闭 + 焦点初置。
 * 无动画依赖，玻璃风格与全局一致。
 */
import { useEffect, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { Button } from "./index";

export function Modal({
  open,
  title,
  onClose,
  children,
  footer,
  width = 520,
}: {
  open: boolean;
  title: string;
  onClose: () => void;
  children: ReactNode;
  footer?: ReactNode;
  width?: number;
}) {
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  if (!open) return null;

  return createPortal(
    <div
      className="fixed inset-0 z-[2000] flex items-start justify-center bg-black/45 pt-[12vh]"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        className="max-h-[76vh] overflow-auto rounded-[14px] border border-sk-glass bg-sk-card shadow-[0_12px_40px_var(--sk-shadow_color)]"
        style={{ width }}
        role="dialog"
        aria-modal
      >
        <div className="px-4 pb-1 pt-3.5 text-[15px] font-semibold text-sk-text">{title}</div>
        <div className="px-4 pb-4 pt-1 text-sm text-sk-text">{children}</div>
        {footer !== undefined ? (
          <div className="flex justify-end gap-2 px-4 pb-3.5">{footer}</div>
        ) : null}
      </div>
    </div>,
    document.body,
  );
}

/** 常用 footer：单个关闭按钮。 */
export function CloseFooter({ onClose, text = "关闭" }: { onClose: () => void; text?: string }) {
  return <Button onClick={onClose}>{text}</Button>;
}
