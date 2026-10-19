/**
 * 轻量 Dropdown 菜单（AntD Dropdown 替代）：点按弹出 + 点击外部关闭。
 *
 * 触发器由调用方给（trigger），菜单项与 AntD 同形（items + onClick）
 * 便于逐页平移。
 *
 * **为什么用 Portal + fixed 定位**（2026-10-19 修）：菜单原来 `absolute`
 * 挂在触发行里——密码库的账号表在 `overflow-x-auto` 容器里、外层卡片又是
 * `overflow-hidden`，行尾 ··· 菜单一打开就被容器边缘裁掉（用户实测截图：
 * 「显示密码」整条被切得看不见）。fixed 定位天然不受任何 overflow 祖先
 * 裁剪；空间不足时向上翻。
 */
import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
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
  const [pos, setPos] = useState<{ top: number; left: number } | null>(null);
  const rootRef = useRef<HTMLSpanElement | null>(null);
  const menuRef = useRef<HTMLDivElement | null>(null);

  /** 按触发行与菜单实际尺寸定位；贴边收进视口，下方放不下则向上翻。 */
  const place = () => {
    const t = rootRef.current?.getBoundingClientRect();
    const m = menuRef.current?.getBoundingClientRect();
    if (!t) return;
    const w = m?.width ?? 176;
    const h = m?.height ?? items.length * 36;
    let left = align === "right" ? t.right - w : t.left;
    left = Math.max(8, Math.min(left, window.innerWidth - w - 8));
    let top = t.bottom + 4;
    if (top + h > window.innerHeight - 8) {
      top = Math.max(8, t.top - h - 4);
    }
    setPos({ top, left });
  };

  useLayoutEffect(() => {
    // 先挂载（visibility hidden）量尺寸，再摆位——避免首帧闪现在左上角
    if (open) place();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node;
      // 菜单是 Portal，不在 rootRef 里——只查 root 会把「点菜单项」也当成
      // 点外部，mousedown 先关菜单、click 就永远到不了菜单项（全变哑按钮）
      if (rootRef.current?.contains(t) || menuRef.current?.contains(t)) return;
      setOpen(false);
    };
    // 菜单是 fixed 定位：滚动/缩放后必然错位，直接收起最稳
    const onScroll = () => setOpen(false);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    window.addEventListener("resize", onScroll);
    window.addEventListener("scroll", onScroll, true);
    window.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      window.removeEventListener("resize", onScroll);
      window.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("keydown", onKey);
    };
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
      {open &&
        createPortal(
          <div
            ref={menuRef}
            style={{
              position: "fixed",
              top: pos?.top ?? -9999,
              left: pos?.left ?? -9999,
              // 未量到尺寸前先藏住（首帧不闪现）
              visibility: pos ? "visible" : "hidden",
            }}
            className={cx(
              "z-[2400] min-w-[176px] overflow-hidden rounded-[10px]",
              "border border-sk-border bg-sk-card shadow-[0_4px_16px_var(--sk-shadow_color)]",
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
                    "block w-full whitespace-nowrap px-3 py-1.5 text-left text-sm transition-colors duration-150",
                    it.danger ? "text-sk-err hover:bg-sk-err/10" : "text-sk-text hover:bg-sk-hover",
                    it.disabled && "cursor-not-allowed opacity-45",
                  )}
                >
                  {it.label}
                </button>
              ),
            )}
          </div>,
          document.body,
        )}
    </span>
  );
}
