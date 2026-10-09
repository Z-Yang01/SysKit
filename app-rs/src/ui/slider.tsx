/**
 * 轻量 Slider（AntD Slider 替代）：受控单值滑条 + 刻度值显示。
 */
import { cx } from "./index";

export function Slider({
  value,
  onChange,
  min = 0,
  max = 100,
  step = 1,
  disabled,
  className,
  "aria-label": ariaLabel,
}: {
  value: number;
  onChange: (v: number) => void;
  min?: number;
  max?: number;
  step?: number;
  disabled?: boolean;
  className?: string;
  "aria-label"?: string;
}) {
  const pct = max > min ? ((value - min) / (max - min)) * 100 : 0;
  return (
    <span className={cx("inline-flex items-center gap-2.5", className)}>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        disabled={disabled}
        aria-label={ariaLabel}
        onChange={(e) => onChange(Number(e.target.value))}
        className={cx(
          "h-1.5 w-44 cursor-pointer appearance-none rounded-full outline-none",
          "disabled:opacity-45",
          // 轨道 + 已填充段（accent 到当前值）全部用渐变实现
          "[&::-webkit-slider-thumb]:h-3.5 [&::-webkit-slider-thumb]:w-3.5 [&::-webkit-slider-thumb]:appearance-none",
          "[&::-webkit-slider-thumb]:rounded-full [&::-webkit-slider-thumb]:bg-white",
          "[&::-webkit-slider-thumb]:shadow-[0_1px_4px_var(--sk-shadow_color)]",
        )}
        style={{
          background: `linear-gradient(to right, var(--sk-accent) ${pct}%, var(--sk-border) ${pct}%)`,
        }}
      />
      <span className="min-w-[2.5em] text-center text-sm text-sk-sub">{value}</span>
    </span>
  );
}
