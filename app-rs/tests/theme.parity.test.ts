/**
 * 主题 token 冻结契约：src/theme.ts 的 LIGHT/DARK 与本文件的快照逐值比对。
 *
 * 历史：Python 版退役前，本测试解析 syskit/ui/theme.py 的 _LIGHT/_DARK 做
 * 跨语言奇偶校验（换主题时两版必须同色）。Python 版归档后契约本体固化为
 * 下方快照——值 = python-final 归档时与 theme.py 逐值核对过的终态（32×2）。
 * 改任何 token 色值都必须有意识地更新快照并在提交说明里写明理由，否则此测试红。
 */
import { describe, expect, it } from "vitest";
import { DARK, LIGHT, TOKEN_NAMES, type Palette } from "../src/theme";

const FROZEN_LIGHT: Palette = {
  bg: "#f3f5f9",
  card: "#ffffff",
  border: "#e2e8f0",
  text: "#2d3748",
  sub: "#7b8794",
  grid: "#d0d7e0",
  hover: "#eef2f7",
  sel_bg: "#dcebff",
  sel_text: "#1a3a6b",
  accent: "#3576e0",
  accent_hover: "#2a63c7",
  accent_disabled: "#a8c4f0",
  btn_border: "#cfd6e0",
  btn_pressed: "#e8f0fc",
  header_bg: "#edf1f6",
  header_text: "#4a5568",
  alt_row: "#f7f9fc",
  danger: "#c62828",
  danger_hover_bg: "#fdf1f1",
  danger_border: "#efc7c7",
  scrollbar: "#c5cddb",
  tooltip_bg: "#2f3a48",
  tooltip_text: "#ffffff",
  ok: "#2e9e44",
  warn: "#ef6c00",
  err: "#d94040",
  info: "#3576e0",
  accent_soft: "#e7eefc",
  ok_soft: "#e6f4e9",
  warn_soft: "#fdf1e5",
  err_soft: "#fdecec",
  brand_sub: "#8a93a0",
};

const FROZEN_DARK: Palette = {
  bg: "#12161c",
  card: "#1c232e",
  border: "#2c3642",
  text: "#e2e8f0",
  sub: "#8b96a5",
  grid: "#303a47",
  hover: "#243040",
  sel_bg: "#2a3d58",
  sel_text: "#d5e4ff",
  accent: "#4a8af0",
  accent_hover: "#5d97f2",
  accent_disabled: "#3a4a63",
  btn_border: "#3a4554",
  btn_pressed: "#2a3d58",
  header_bg: "#222b36",
  header_text: "#9aa7b5",
  alt_row: "#181f28",
  danger: "#ff8a80",
  danger_hover_bg: "#332225",
  danger_border: "#5a3030",
  scrollbar: "#3d4855",
  tooltip_bg: "#e8edf3",
  tooltip_text: "#1c232e",
  ok: "#4caf50",
  warn: "#ffb74d",
  err: "#ef5350",
  info: "#64b5f6",
  accent_soft: "#1e2f49",
  ok_soft: "#1b2f22",
  warn_soft: "#33281a",
  err_soft: "#33211f",
  brand_sub: "#8b96a5",
};

describe("theme token 冻结契约（theme.ts vs 快照）", () => {
  it("token 名单一致（32 个，无多无少）", () => {
    expect(TOKEN_NAMES).toHaveLength(32);
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
