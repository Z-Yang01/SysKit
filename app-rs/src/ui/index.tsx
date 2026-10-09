/**
 * shadcn 风格基础组件库（自建，Tailwind + --sk-* token）。
 *
 * 这是「全面重写」迁移的组件基座：新页面一律用本目录组件，存量 AntD
 * 页面逐页迁移，全部完成后卸载 antd。
 *
 * 设计约束：
 * - 颜色一律走 sk-* token（Tailwind colors.sk.*），禁止写死色值——
 *   深浅主题与换肤自动跟随；
 * - 组件只做展示与最小交互，业务逻辑留在页面（与 AntD 时代同分工）；
 * - 无全局副作用（不引 Provider 的组件可独立使用）。
 */
import type { ButtonHTMLAttributes, HTMLAttributes, InputHTMLAttributes } from "react";

export function cx(...parts: Array<string | false | null | undefined>): string {
  return parts.filter(Boolean).join(" ");
}

// ---------------- Button ----------------

type ButtonVariant = "primary" | "default" | "danger" | "ghost";
type ButtonSize = "sm" | "md";

const BUTTON_VARIANTS: Record<ButtonVariant, string> = {
  primary:
    "bg-sk-accent text-white hover:bg-sk-accent-hover shadow-[0_2px_10px_rgba(59,130,246,0.35)] border border-transparent",
  default:
    "bg-transparent border border-sk-border text-sk-text hover:bg-sk-hover hover:border-sk-accent/40",
  danger:
    "bg-transparent border border-sk-err/50 text-sk-err hover:bg-sk-err/10",
  ghost: "bg-transparent border border-transparent text-sk-sub hover:bg-sk-hover hover:text-sk-text",
};

const BUTTON_SIZES: Record<ButtonSize, string> = {
  sm: "h-7 px-2.5 text-xs gap-1",
  md: "h-8 px-3.5 text-sm gap-1.5",
};

export function Button({
  variant = "default",
  size = "md",
  loading,
  className,
  children,
  disabled,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: ButtonVariant;
  size?: ButtonSize;
  loading?: boolean;
}) {
  return (
    <button
      className={cx(
        "inline-flex items-center justify-center rounded-[8px] font-medium",
        "transition-colors duration-150 select-none",
        "focus-visible:outline focus-visible:outline-2 focus-visible:outline-sk-accent/50",
        "disabled:opacity-45 disabled:pointer-events-none",
        BUTTON_VARIANTS[variant],
        BUTTON_SIZES[size],
        className,
      )}
      disabled={disabled || loading}
      {...rest}
    >
      {loading && (
        <span
          aria-hidden
          className="mr-1 inline-block h-3 w-3 animate-spin rounded-full border-2 border-current border-t-transparent"
        />
      )}
      {children}
    </button>
  );
}

// ---------------- Card ----------------

export function Card({ className, ...rest }: HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={cx(
        "rounded-[14px] border border-sk-glass",
        "bg-sk-card/78 backdrop-blur-[2px]",
        "shadow-[inset_0_1px_0_var(--sk-glass_highlight),0_4px_16px_var(--sk-shadow_color)]",
        className,
      )}
      {...rest}
    />
  );
}

export function CardHeader({ className, ...rest }: HTMLAttributes<HTMLDivElement>) {
  return (
    <div
      className={cx("flex items-center justify-between gap-2 px-4 pt-3.5 pb-2.5", className)}
      {...rest}
    />
  );
}

export function CardTitle({ className, ...rest }: HTMLAttributes<HTMLDivElement>) {
  return <div className={cx("text-[15px] font-semibold text-sk-text", className)} {...rest} />;
}

export function CardBody({ className, ...rest }: HTMLAttributes<HTMLDivElement>) {
  return <div className={cx("px-4 pb-4 pt-1", className)} {...rest} />;
}

// ---------------- Input / Textarea ----------------

export function Input({
  className,
  ...rest
}: InputHTMLAttributes<HTMLInputElement> & { ref?: React.Ref<HTMLInputElement> }) {
  return (
    <input
      className={cx(
        "h-8 w-full rounded-[8px] border border-sk-border bg-transparent px-2.5 text-sm text-sk-text",
        "placeholder:text-sk-sub/70",
        "focus:outline-none focus:border-sk-accent/60 focus:ring-2 focus:ring-sk-accent/20",
        "disabled:opacity-45",
        className,
      )}
      {...rest}
    />
  );
}

export function Textarea({
  className,
  ...rest
}: React.TextareaHTMLAttributes<HTMLTextAreaElement>) {
  return (
    <textarea
      className={cx(
        "w-full rounded-[8px] border border-sk-border bg-transparent px-2.5 py-1.5 text-sm text-sk-text",
        "placeholder:text-sk-sub/70",
        "focus:outline-none focus:border-sk-accent/60 focus:ring-2 focus:ring-sk-accent/20",
        className,
      )}
      {...rest}
    />
  );
}

// ---------------- Badge ----------------

type BadgeTone = "neutral" | "accent" | "ok" | "warn" | "err";

const BADGE_TONES: Record<BadgeTone, string> = {
  neutral: "border-sk-border text-sk-sub",
  accent: "border-sk-accent/40 text-sk-accent bg-sk-accent/10",
  ok: "border-sk-ok/40 text-sk-ok bg-sk-ok/10",
  warn: "border-sk-warn/40 text-sk-warn bg-sk-warn/10",
  err: "border-sk-err/40 text-sk-err bg-sk-err/10",
};

export function Badge({
  tone = "neutral",
  className,
  ...rest
}: HTMLAttributes<HTMLSpanElement> & { tone?: BadgeTone }) {
  return (
    <span
      className={cx(
        "inline-flex items-center rounded-md border px-1.5 py-0.5 text-xs leading-none",
        BADGE_TONES[tone],
        className,
      )}
      {...rest}
    />
  );
}

// ---------------- Switch ----------------

export function Switch({
  checked,
  onChange,
  disabled,
  "aria-label": ariaLabel,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
  "aria-label"?: string;
}) {
  return (
    <button
      role="switch"
      aria-checked={checked}
      aria-label={ariaLabel}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={cx(
        "relative inline-flex h-[20px] w-[38px] shrink-0 items-center rounded-full border transition-colors duration-150",
        checked ? "bg-sk-accent border-sk-accent" : "bg-sk-border/60 border-sk-border",
        "disabled:opacity-45",
      )}
    >
      <span
        className={cx(
          "inline-block h-[14px] w-[14px] rounded-full bg-white shadow transition-transform duration-150",
          checked ? "translate-x-[20px]" : "translate-x-[3px]",
        )}
      />
    </button>
  );
}

// ---------------- Segmented ----------------

export interface SegmentedOption<T extends string> {
  value: T;
  label: string;
}

export function Segmented<T extends string>({
  value,
  onChange,
  options,
  size = "md",
}: {
  value: T;
  onChange: (v: T) => void;
  options: SegmentedOption<T>[];
  size?: "sm" | "md";
}) {
  return (
    <div
      className={cx(
        "inline-flex items-center rounded-[8px] border border-sk-glass bg-sk-card/60 p-0.5",
        size === "sm" ? "text-xs" : "text-sm",
      )}
    >
      {options.map((o) => (
        <button
          key={o.value}
          onClick={() => onChange(o.value)}
          className={cx(
            "rounded-[6px] px-2.5 transition-colors duration-150",
            size === "sm" ? "h-6" : "h-7",
            o.value === value
              ? "bg-sk-card text-sk-accent font-medium shadow-[0_1px_4px_var(--sk-shadow_color)]"
              : "text-sk-sub hover:text-sk-text",
          )}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

// ---------------- Empty ----------------

export function Empty({ description }: { description: string }) {
  return (
    <div className="flex flex-col items-center justify-center gap-2 py-8 text-center">
      <svg viewBox="0 0 48 40" className="h-11 w-11 opacity-40" fill="none" aria-hidden>
        <path
          d="M6 16 L14 6 h20 l8 10 v16 a2 2 0 0 1 -2 2 H8 a2 2 0 0 1 -2 -2 Z"
          stroke="currentColor"
          strokeWidth="1.6"
          className="text-sk-sub"
        />
        <path d="M6 16 h12 a6 6 0 0 0 12 0 h12" stroke="currentColor" strokeWidth="1.6" className="text-sk-sub" />
      </svg>
      <div className="text-[13px] text-sk-sub">{description}</div>
    </div>
  );
}

// ---------------- 第三批（表单/表格/菜单/进度） ----------------
export { Slider } from './slider';
export { Checkbox, Radio } from './checkbox';
export { Table } from './table';
export type { Column } from './table';
export { Dropdown } from './dropdown';
export type { MenuItem } from './dropdown';
export { Progress } from './progress';

// 密码输入语义别名（type=password 由调用方传；占位圆点渲染同 Input）
export { Input as PasswordInput };
