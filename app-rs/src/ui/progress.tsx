/**
 * 轻量 Progress 条（AntD Progress line 替代，只覆盖项目用的 size small 形态）。
 */
import { cx } from "./index";

export function Progress({
  percent,
  showInfo = true,
  className,
  tone = "accent",
}: {
  percent: number;
  showInfo?: boolean;
  className?: string;
  tone?: "accent" | "ok" | "warn" | "err";
}) {
  const v = Math.max(0, Math.min(100, Number.isFinite(percent) ? percent : 0));
  const bar = {
    accent: "bg-sk-accent",
    ok: "bg-sk-ok",
    warn: "bg-sk-warn",
    err: "bg-sk-err",
  }[tone];
  return (
    <span className={cx("inline-flex items-center gap-2", className)}>
      <span className="h-1.5 flex-1 overflow-hidden rounded-full bg-sk-border/70">
        <span className={cx("block h-full rounded-full transition-[width] duration-200", bar)} style={{ width: `${v}%` }} />
      </span>
      {showInfo && <span className="text-xs text-sk-sub">{Math.round(v)}%</span>}
    </span>
  );
}
