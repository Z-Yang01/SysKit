/**
 * 轻量数字输入（AntD InputNumber 替代）：min/max 夹取 + 步进按钮。
 *
 * 与 AntD 的差异：输入中的非法中间态（如 "-"）由调用方的 commitOnBlur
 * 语义兜底（设置页既有模式），本组件只在失焦时夹取范围。
 */
import { useEffect, useState } from "react";
import { cx } from "./index";

export function InputNumber({
  value,
  onChange,
  onBlur,
  min,
  max,
  className,
  disabled,
  "aria-label": ariaLabel,
}: {
  value: number | null;
  onChange: (v: number | null) => void;
  onBlur?: () => void;
  min?: number;
  max?: number;
  className?: string;
  disabled?: boolean;
  "aria-label"?: string;
}) {
  // 显示态：允许输入中间态（空串/负号），失焦才解析夹取
  const [text, setText] = useState(value === null ? "" : String(value));

  useEffect(() => {
    setText(value === null ? "" : String(value));
  }, [value]);

  const commit = () => {
    const trimmed = text.trim();
    if (trimmed === "") {
      onChange(null);
      onBlur?.();
      return;
    }
    let n = Number(trimmed);
    if (!Number.isFinite(n)) {
      setText(value === null ? "" : String(value)); // 非法回落原值
      onBlur?.();
      return;
    }
    if (min !== undefined) n = Math.max(min, n);
    if (max !== undefined) n = Math.min(max, n);
    onChange(n);
    onBlur?.();
  };

  const step = (delta: number) => {
    const base = Number.isFinite(Number(text)) ? Number(text) : (min ?? 0);
    let n = base + delta;
    if (min !== undefined) n = Math.max(min, n);
    if (max !== undefined) n = Math.min(max, n);
    onChange(n);
  };

  return (
    <span
      className={cx(
        "inline-flex h-8 items-stretch overflow-hidden rounded-[8px] border border-sk-border",
        "focus-within:border-sk-accent/60 focus-within:ring-2 focus-within:ring-sk-accent/20",
        disabled && "opacity-45",
        className,
      )}
    >
      <button
        type="button"
        tabIndex={-1}
        disabled={disabled}
        onClick={() => step(-1)}
        className="w-7 border-r border-sk-border text-sk-sub hover:bg-sk-hover hover:text-sk-text"
      >
        −
      </button>
      <input
        inputMode="numeric"
        aria-label={ariaLabel}
        disabled={disabled}
        value={text}
        onChange={(e) => setText(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") commit();
        }}
        className="w-16 bg-transparent text-center text-sm text-sk-text focus:outline-none"
      />
      <button
        type="button"
        tabIndex={-1}
        disabled={disabled}
        onClick={() => step(1)}
        className="w-7 border-l border-sk-border text-sk-sub hover:bg-sk-hover hover:text-sk-text"
      >
        +
      </button>
    </span>
  );
}
