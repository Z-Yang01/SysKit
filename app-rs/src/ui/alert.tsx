/**
 * 轻量提示块（AntD Alert 替代）：info / warning 两档 + 可选图标字符。
 */
import type { ReactNode } from "react";
import { cx } from "./index";

export function Alert({
  tone = "info",
  title,
  children,
  className,
}: {
  tone?: "info" | "warning" | "success" | "danger";
  title: ReactNode;
  children?: ReactNode;
  className?: string;
}) {
  const toneCls = {
    info: "border-sk-accent/35 bg-sk-accent/8",
    warning: "border-sk-warn/45 bg-sk-warn/10",
    success: "border-sk-ok/45 bg-sk-ok/10",
    danger: "border-sk-err/45 bg-sk-err/10",
  }[tone];
  const mark = { info: "ℹ", warning: "⚠", success: "✓", danger: "✕" }[tone];
  const markCls = {
    info: "text-sk-accent",
    warning: "text-sk-warn",
    success: "text-sk-ok",
    danger: "text-sk-err",
  }[tone];
  return (
    <div className={cx("flex gap-2.5 rounded-[10px] border px-3 py-2.5", toneCls, className)}>
      <span className={cx("mt-0.5 font-bold", markCls)}>{mark}</span>
      <div className="min-w-0 flex-1">
        <div className="font-medium text-sk-text">{title}</div>
        {children && <div className="mt-1 text-[13px] leading-relaxed text-sk-sub">{children}</div>}
      </div>
    </div>
  );
}
