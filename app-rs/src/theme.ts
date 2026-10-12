/**
 * 主题语义 token（单源）：shadcn 风格 + 玻璃拟态体系（v1.3 界面美化）。
 *
 * 历史：与 python-final 的 theme.py 曾逐值对齐；Python 版退役后本文件成为
 * 唯一契约，tests/theme.parity.test.ts 的冻结快照 = 本文件的对齐终态——
 * 改任何 token（加/删/改值）都必须有意识更新快照并在提交说明里写理由。
 *
 * 运行时经 applyTheme 写入 documentElement 的 CSS 变量（--sk-*），
 * 组件与 AntD 主题一律取变量，禁止写死色值。
 *
 * v1.3 设计语言（shadcn 质感）：
 * - LIGHT：#f6f8fb 冷灰底、纯白玻璃卡、slate 系低饱和边框、蓝色主色收敛；
 * - DARK：#0f141a 深空底、#161d27 玻璃卡、边框整体压暗提对比、accent 提亮；
 * - 新增玻璃/层级 token：glass_border（半透明描边）、glass_highlight
 *   （内高光）、shadow_color（投影基色）、accent_soft2（主色渐变端色）、
 *   radius（全局圆角基准，供 ConfigProvider/皮肤类引用）。
 */

export const TOKEN_NAMES = [
  "bg", "card", "border", "text", "sub", "grid", "hover",
  "sel_bg", "sel_text", "accent", "accent_hover", "accent_disabled",
  "btn_border", "btn_pressed", "header_bg", "header_text", "alt_row",
  "danger", "danger_hover_bg", "danger_border", "scrollbar",
  "tooltip_bg", "tooltip_text", "ok", "warn", "err", "info",
  "accent_soft", "ok_soft", "warn_soft", "err_soft", "brand_sub",
  "glass_border", "glass_highlight", "shadow_color", "accent_soft2",
] as const;

type TokenName = (typeof TOKEN_NAMES)[number];
export type Palette = Record<TokenName, string>;

export const LIGHT: Palette = {
  bg: "#f6f8fb", card: "#ffffff", border: "#e6eaf0", text: "#1e2530",
  sub: "#6b7686", grid: "#e3e8ef", hover: "#f0f4f9",
  sel_bg: "#e3edff", sel_text: "#173a6b", accent: "#3b82f6",
  accent_hover: "#2563eb", accent_disabled: "#a5c4f5",
  btn_border: "#d8dfe8", btn_pressed: "#eaf1fe", header_bg: "#f2f5f9",
  header_text: "#3f4a5a", alt_row: "#f8fafc", danger: "#dc2626",
  danger_hover_bg: "#fef2f2", danger_border: "#fecaca", scrollbar: "#cbd5e1",
  tooltip_bg: "#1e2530", tooltip_text: "#f8fafc",
  ok: "#16a34a", warn: "#ea8a00", err: "#dc2626", info: "#3b82f6",
  accent_soft: "#e8f0fe", ok_soft: "#ecfdf3", warn_soft: "#fff7e8",
  err_soft: "#fef2f2", brand_sub: "#8b96a5",
  // 玻璃/层级
  glass_border: "rgba(255, 255, 255, 0.65)",
  glass_highlight: "rgba(255, 255, 255, 0.85)",
  shadow_color: "rgba(15, 23, 42, 0.08)",
  accent_soft2: "#dbeafe",
};

export const DARK: Palette = {
  bg: "#0f141a", card: "#161d27", border: "#2a3441", text: "#e6ebf2",
  sub: "#8b96a5", grid: "#2a3441", hover: "#1f2937",
  sel_bg: "#1e3a5f", sel_text: "#d5e4ff", accent: "#5b9bff",
  accent_hover: "#7bacff", accent_disabled: "#33455e",
  btn_border: "#333e4d", btn_pressed: "#1e3a5f", header_bg: "#131a22",
  header_text: "#9aa7b5", alt_row: "#131a22", danger: "#ff8a80",
  danger_hover_bg: "#332225", danger_border: "#5a3030", scrollbar: "#333e4d",
  tooltip_bg: "#e6ebf2", tooltip_text: "#161d27",
  ok: "#4ade80", warn: "#fbbf24", err: "#f87171", info: "#60a5fa",
  accent_soft: "#16283f", ok_soft: "#12291c", warn_soft: "#2e2413",
  err_soft: "#331d1d", brand_sub: "#8b96a5",
  // 玻璃/层级
  glass_border: "rgba(255, 255, 255, 0.08)",
  glass_highlight: "rgba(255, 255, 255, 0.05)",
  shadow_color: "rgba(0, 0, 0, 0.45)",
  accent_soft2: "#0f2137",
};

export function palette(dark: boolean): Palette {
  return dark ? DARK : LIGHT;
}

/** 把 token 写进 :root CSS 变量（--sk-<name>），并回设 data-theme。 */
export function applyTheme(dark: boolean): void {
  const pal = palette(dark);
  const root = document.documentElement;
  for (const name of TOKEN_NAMES) {
    root.style.setProperty(`--sk-${name}`, pal[name]);
  }
  root.dataset.theme = dark ? "dark" : "light";
}
