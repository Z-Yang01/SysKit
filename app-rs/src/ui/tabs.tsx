/**
 * 轻量 Tabs（AntD Tabs 替代）：受控/非受控两用，下划线式页签。
 */
import { useState, type ReactNode } from "react";
import { cx } from "./index";

export interface TabItem {
  key: string;
  label: string;
  children?: ReactNode;
}

export function Tabs({
  items,
  activeKey,
  onChange,
  right,
  className,
}: {
  items: TabItem[];
  /** 受控模式；不传则内部自持 */
  activeKey?: string;
  onChange?: (key: string) => void;
  /** 页签栏右侧插槽（AntD tabBarExtraContent 的替代：速率窗口切换器等） */
  right?: ReactNode;
  className?: string;
}) {
  const [inner, setInner] = useState(items[0]?.key ?? "");
  const cur = activeKey ?? inner;
  const set = (k: string) => {
    setInner(k);
    onChange?.(k);
  };
  const active = items.find((i) => i.key === cur);
  return (
    <div className={className}>
      <div className="flex items-center justify-between gap-3 border-b border-sk-border">
        <div className="flex min-w-0 gap-1">
          {items.map((i) => (
            <button
              key={i.key}
              onClick={() => set(i.key)}
              className={cx(
                "relative -mb-px px-3 py-2 text-sm font-medium transition-colors duration-150",
                i.key === cur
                  ? "text-sk-accent after:absolute after:inset-x-0 after:-bottom-px after:h-0.5 after:bg-sk-accent"
                  : "text-sk-sub hover:text-sk-text",
              )}
            >
              {i.label}
            </button>
          ))}
        </div>
        {right && <div className="flex shrink-0 items-center">{right}</div>}
      </div>
      <div className="pt-3">{active?.children}</div>
    </div>
  );
}
