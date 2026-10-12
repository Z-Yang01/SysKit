/**
 * 轻量 Select（AntD Select 替代）：原生 select 皮肤化。
 *
 * 用原生元素而非自绘浮层——键盘可达性/滚动锁定/点击外收起全部白拿，
 * 只有「多列超长列表虚拟化」这类需求才值得自绘，设置页的下拉用不到。
 */
import { cx } from "./index";

interface SelectOption {
  label: string;
  value: string;
}

export function Select({
  value,
  onChange,
  options,
  placeholder,
  className,
  disabled,
  "aria-label": ariaLabel,
}: {
  value: string;
  onChange: (v: string) => void;
  options: SelectOption[];
  placeholder?: string;
  className?: string;
  disabled?: boolean;
  "aria-label"?: string;
}) {
  return (
    <select
      value={value}
      disabled={disabled}
      aria-label={ariaLabel}
      onChange={(e) => onChange(e.target.value)}
      className={cx(
        "h-8 rounded-[8px] border border-sk-border bg-sk-card px-2 text-sm text-sk-text",
        "focus:outline-none focus:border-sk-accent/60 focus:ring-2 focus:ring-sk-accent/20",
        "disabled:opacity-45",
        className,
      )}
    >
      {placeholder && <option value="">{placeholder}</option>}
      {options.map((o) => (
        <option key={o.value} value={o.value}>
          {o.label}
        </option>
      ))}
    </select>
  );
}
