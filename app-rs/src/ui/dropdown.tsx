/**
 * 轻量 Dropdown 菜单（AntD Dropdown 替代）：点按弹出 + 点击外部关闭。
 *
 * 触发器由调用方给（trigger），菜单项与 AntD 同形（items + onClick）
 * 便于逐页平移。
 */
import { useEffect, useRef, useState, type ReactNode } from "react";
import { cx } from "./index";

export interface MenuItem {
  key: string;
  label: ReactNode;
  danger?: boolean;
  disabled?: boolean;
  type?: "divider";
}

export function Dropdown({
  trigger,
  items,
  onItemClick,
  align = "right",
}: {
  trigger: ReactNode;
  items: MenuItem[];
  onItemClick: (key: string) => void;
  align?: "left" | "right";
}) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLSpanElement | null>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [open]);

  return (
    <span className="relative inline-flex" ref={rootRef}>
      <span
        onClick={(e) => {
          e.stopPropagation();
          setOpen((v) => !v);
        }}
      >
        {trigger}
      </span>
      {open && (
        <div
          className={cx(
            "absolute z-[1600] mt-1 min-w-[160px] overflow-hidden rounded-[10px]",
            "border border-sk-glass bg-sk-card/95 shadow-[0_8px_28px_var(--sk-shadow_color)] backdrop-blur-[6px]",
            align === "right" ? "right-0" : "left-0",
          )}
        >
          {items.map((it, i) =>
            it.type === "divider" ? (
              <div key={`d${i}`} className="my-1 border-t border-sk-border" />
            ) : (
              <button
                key={it.key}
                disabled={it.disabled}
                onClick={(e) => {
                  e.stopPropagation();
                  setOpen(false);
                  if (!it.disabled) onItemClick(it.key);
                }}
                className={cx(
                  "block w-full px-3 py-1.5 text-left text-sm transition-colors duration-150",
                  it.danger ? "text-sk-err hover:bg-sk-err/10" : "text-sk-text hover:bg-sk-hover",
                  it.disabled && "cursor-not-allowed opacity-45",
                )}
              >
                {it.label}
              </button>
            ),
          )}
        </div>
      )}
    </span>
  );
}
