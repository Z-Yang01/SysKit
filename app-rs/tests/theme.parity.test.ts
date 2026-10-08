/**
 * 主题 token 冻结契约：src/theme.ts 的 LIGHT/DARK 与本文件的快照逐值比对。
 *
 * v1.3 界面美化（shadcn 风格 + 玻璃拟态）：token 从 32 扩到 36——新增
 * glass_border / glass_highlight / shadow_color / accent_soft2（玻璃层与
 * 层级 token）；LIGHT/DARK 全量色值重调（色板 shadcn 化，见该次提交说明）。
 * 快照 = 本次美化后的对齐终态：再改任何 token 必须有意识更新快照并写明理由。
 */
import { describe, expect, it } from "vitest";
import { DARK, LIGHT, TOKEN_NAMES, type Palette } from "../src/theme";

const FROZEN_LIGHT: Palette = {
  bg: "#f6f8fb",
  card: "#ffffff",
  border: "#e6eaf0",
  text: "#1e2530",
  sub: "#6b7686",
  grid: "#e3e8ef",
  hover: "#f0f4f9",
  sel_bg: "#e3edff",
  sel_text: "#173a6b",
  accent: "#3b82f6",
  accent_hover: "#2563eb",
  accent_disabled: "#a5c4f5",
  btn_border: "#d8dfe8",
  btn_pressed: "#eaf1fe",
  header_bg: "#f2f5f9",
  header_text: "#3f4a5a",
  alt_row: "#f8fafc",
  danger: "#dc2626",
  danger_hover_bg: "#fef2f2",
  danger_border: "#fecaca",
  scrollbar: "#cbd5e1",
  tooltip_bg: "#1e2530",
  tooltip_text: "#f8fafc",
  ok: "#16a34a",
  warn: "#ea8a00",
  err: "#dc2626",
  info: "#3b82f6",
  accent_soft: "#e8f0fe",
  ok_soft: "#ecfdf3",
  warn_soft: "#fff7e8",
  err_soft: "#fef2f2",
  brand_sub: "#8b96a5",
  glass_border: "rgba(255, 255, 255, 0.65)",
  glass_highlight: "rgba(255, 255, 255, 0.85)",
  shadow_color: "rgba(15, 23, 42, 0.08)",
  accent_soft2: "#dbeafe",
};

const FROZEN_DARK: Palette = {
  bg: "#0f141a",
  card: "#161d27",
  border: "#2a3441",
  text: "#e6ebf2",
  sub: "#8b96a5",
  grid: "#2a3441",
  hover: "#1f2937",
  sel_bg: "#1e3a5f",
  sel_text: "#d5e4ff",
  accent: "#5b9bff",
  accent_hover: "#7bacff",
  accent_disabled: "#33455e",
  btn_border: "#333e4d",
  btn_pressed: "#1e3a5f",
  header_bg: "#131a22",
  header_text: "#9aa7b5",
  alt_row: "#131a22",
  danger: "#ff8a80",
  danger_hover_bg: "#332225",
  danger_border: "#5a3030",
  scrollbar: "#333e4d",
  tooltip_bg: "#e6ebf2",
  tooltip_text: "#161d27",
  ok: "#4ade80",
  warn: "#fbbf24",
  err: "#f87171",
  info: "#60a5fa",
  accent_soft: "#16283f",
  ok_soft: "#12291c",
  warn_soft: "#2e2413",
  err_soft: "#331d1d",
  brand_sub: "#8b96a5",
  glass_border: "rgba(255, 255, 255, 0.08)",
  glass_highlight: "rgba(255, 255, 255, 0.05)",
  shadow_color: "rgba(0, 0, 0, 0.45)",
  accent_soft2: "#0f2137",
};

describe("theme token 冻结契约（theme.ts vs 快照）", () => {
  it("token 名单一致（36 个，无多无少）", () => {
    expect(TOKEN_NAMES).toHaveLength(36);
    expect(Object.keys(FROZEN_LIGHT).sort()).toEqual([...TOKEN_NAMES].sort());
    expect(Object.keys(FROZEN_DARK).sort()).toEqual([...TOKEN_NAMES].sort());
  });

  it("LIGHT 逐值一致", () => {
    for (const name of TOKEN_NAMES) {
      expect(LIGHT[name], `LIGHT.${name}`).toBe(FROZEN_LIGHT[name]);
    }
  });

  it("DARK 逐值一致", () => {
    for (const name of TOKEN_NAMES) {
      expect(DARK[name], `DARK.${name}`).toBe(FROZEN_DARK[name]);
    }
  });
});
