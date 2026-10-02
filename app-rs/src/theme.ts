/**
 * 主题语义 token（单源）：与 syskit/ui/theme.py 的 _LIGHT/_DARK 逐值对齐。
 *
 * 奇偶校验由 tests/theme.parity.test.ts 强制（解析 theme.py 比对）——
 * Python 版退役前，改色必须两边同步改，防止视觉分叉。
 * 运行时经 applyTheme 写入 documentElement 的 CSS 变量（--sk-*），
 * 组件与 AntD 主题一律取变量，禁止写死色值（对应 QSS @token 纪律）。
 */

export const TOKEN_NAMES = [
  "bg", "card", "border", "text", "sub", "grid", "hover",
  "sel_bg", "sel_text", "accent", "accent_hover", "accent_disabled",
  "btn_border", "btn_pressed", "header_bg", "header_text", "alt_row",
  "danger", "danger_hover_bg", "danger_border", "scrollbar",
  "tooltip_bg", "tooltip_text", "ok", "warn", "err", "info",
  "accent_soft", "ok_soft", "warn_soft", "err_soft", "brand_sub",
] as const;

export type TokenName = (typeof TOKEN_NAMES)[number];
export type Palette = Record<TokenName, string>;

export const LIGHT: Palette = {
  bg: "#f3f5f9", card: "#ffffff", border: "#e2e8f0", text: "#2d3748",
  sub: "#7b8794", grid: "#d0d7e0", hover: "#eef2f7",
  sel_bg: "#dcebff", sel_text: "#1a3a6b", accent: "#3576e0",
  accent_hover: "#2a63c7", accent_disabled: "#a8c4f0",
  btn_border: "#cfd6e0", btn_pressed: "#e8f0fc", header_bg: "#edf1f6",
  header_text: "#4a5568", alt_row: "#f7f9fc", danger: "#c62828",
  danger_hover_bg: "#fdf1f1", danger_border: "#efc7c7", scrollbar: "#c5cddb",
  tooltip_bg: "#2f3a48", tooltip_text: "#ffffff",
  ok: "#2e9e44", warn: "#ef6c00", err: "#d94040", info: "#3576e0",
  accent_soft: "#e7eefc", ok_soft: "#e6f4e9", warn_soft: "#fdf1e5",
  err_soft: "#fdecec", brand_sub: "#8a93a0",
};

export const DARK: Palette = {
  bg: "#12161c", card: "#1c232e", border: "#2c3642", text: "#e2e8f0",
  sub: "#8b96a5", grid: "#303a47", hover: "#243040",
  sel_bg: "#2a3d58", sel_text: "#d5e4ff", accent: "#4a8af0",
  accent_hover: "#5d97f2", accent_disabled: "#3a4a63",
  btn_border: "#3a4554", btn_pressed: "#2a3d58", header_bg: "#222b36",
  header_text: "#9aa7b5", alt_row: "#181f28", danger: "#ff8a80",
  danger_hover_bg: "#332225", danger_border: "#5a3030", scrollbar: "#3d4855",
  tooltip_bg: "#e8edf3", tooltip_text: "#1c232e",
  ok: "#4caf50", warn: "#ffb74d", err: "#ef5350", info: "#64b5f6",
  accent_soft: "#1e2f49", ok_soft: "#1b2f22", warn_soft: "#33281a",
  err_soft: "#33211f", brand_sub: "#8b96a5",
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
