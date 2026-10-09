/**
 * 轻量 Checkbox / Radio.Group（AntD 同名组件替代）。
 */
import type { ReactNode } from "react";
import { cx } from "./index";

export function Checkbox({
  checked,
  onChange,
  disabled,
  children,
  "aria-label": ariaLabel,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
  children?: ReactNode;
  "aria-label"?: string;
}) {
  return (
    <label
      className={cx(
        "inline-flex cursor-pointer select-none items-center gap-1.5 text-sm text-sk-text",
        disabled && "cursor-not-allowed opacity-45",
      )}
    >
      <input
        type="checkbox"
        checked={checked}
        disabled={disabled}
        aria-label={ariaLabel}
        onChange={(e) => onChange(e.target.checked)}
        className="h-3.5 w-3.5 accent-[var(--sk-accent)]"
      />
      {children}
    </label>
  );
}

export function Radio<T extends string>({
  value,
  onChange,
  options,
  disabled,
  className,
  "aria-label": ariaLabel,
}: {
  value: T;
  onChange: (v: T) => void;
  options: { value: T; label: ReactNode }[];
  disabled?: boolean;
  className?: string;
  "aria-label"?: string;
}) {
  return (
    <div
      role="radiogroup"
      aria-label={ariaLabel}
      className={cx("inline-flex items-center gap-3.5 text-sm", disabled && "opacity-45", className)}
    >
      {options.map((o) => (
        <label key={o.value} className="inline-flex cursor-pointer select-none items-center gap-1.5">
          <input
            type="radio"
            checked={o.value === value}
            disabled={disabled}
            onChange={() => onChange(o.value)}
            className="h-3.5 w-3.5 accent-[var(--sk-accent)]"
          />
          {o.label}
        </label>
      ))}
    </div>
  );
}
