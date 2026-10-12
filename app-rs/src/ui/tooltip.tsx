/**
 * 轻量 Tooltip：hover 悬浮显示提示气泡（CSS 过渡，无定位库）。
 *
 * - 提示内容可换行（max-w + normal whitespace）；
 * - 泡泡渲染在子元素内部（absolute），适合短文本；超长提示请控制文案。
 */
import type { ReactNode } from "react";
import { cx } from "./index";

export function Tooltip({
  title,
  children,
  className,
}: {
  title: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <span className={cx("group/tt relative inline-flex", className)}>
      {children}
      <span
        role="tooltip"
        className={cx(
          "pointer-events-none absolute left-1/2 top-full z-[1500] mt-1.5 w-max max-w-[320px]",
          "-translate-x-1/2 whitespace-normal rounded-md border border-sk-border bg-sk-card px-2 py-1",
          "text-xs leading-relaxed text-sk-text opacity-0 shadow-[0_4px_16px_var(--sk-shadow_color)]",
          "transition-opacity duration-150 group-hover/tt:opacity-100",
        )}
      >
        {title}
      </span>
    </span>
  );
}
