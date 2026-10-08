/**
 * 图表主题取色回归锁（轮次115 审计）。
 *
 * 实锤背景：S3 之前 option useMemo 依赖不含 dark，切主题图表不换色；S3 补了
 * 依赖，但取色仍经 getComputedStyle 读 --sk-*，而 applyTheme 在 App 的
 * effect 里（渲染提交后）才写变量——memo 在渲染期重算读到的还是旧主题色，
 * 速率图在桶窗口（1h/1d，deps 无每秒数据）下会一直停在旧色。
 * 修复：取色收敛到 src/chartTheme.ts，直取 palette(dark)（纯函数、无 DOM）。
 * 本文件锁两件事：① 取色与 palette 逐键一致；② option 的底色/轴线/浮层
 * 随 dark 换色。若有人改回 getComputedStyle（node 下 document 不存在直接
 * 崩）或让 chartColors 无视 dark，这里立刻红。
 */
import { describe, expect, it } from "vitest";

import { baseChartOption, chartColors } from "../src/chartTheme";
import { palette } from "../src/theme";

const COLOR_KEYS = [
  "text",
  "sub",
  "grid",
  "accent",
  "ok",
  "warn",
  "tooltipBg",
  "tooltipText",
] as const;

describe("chartColors：palette(dark) 的图表子集", () => {
  it("双主题逐键等于 palette(dark) 对应 token（驼峰映射不出错）", () => {
    for (const dark of [false, true]) {
      const c = chartColors(dark);
      const p = palette(dark);
      expect(c).toEqual({
        text: p.text,
        sub: p.sub,
        grid: p.grid,
        accent: p.accent,
        ok: p.ok,
        warn: p.warn,
        tooltipBg: p.tooltip_bg,
        tooltipText: p.tooltip_text,
      });
    }
  });

  it("每个键都随主题变化——不存在『切了主题图上某处仍用旧色』的死角", () => {
    const light = chartColors(false);
    const dark = chartColors(true);
    for (const k of COLOR_KEYS) {
      expect(light[k], `键 ${k} 必须随主题切换变化`).not.toBe(dark[k]);
    }
  });
});

describe("baseChartOption(dark)：option 是 (data, dark) 的纯函数", () => {
  it("文字/轴线/网格/浮层底全部随 dark 换色（applyTheme 时序解耦的锁）", () => {
    const light = baseChartOption(false);
    const dark = baseChartOption(true);
    const pl = palette(false);
    const pd = palette(true);

    expect(light.textStyle.color).toBe(pl.text);
    expect(dark.textStyle.color).toBe(pd.text);

    expect(light.xAxis.axisLine.lineStyle.color).toBe(pl.grid);
    expect(dark.xAxis.axisLine.lineStyle.color).toBe(pd.grid);
    expect(light.yAxis.splitLine.lineStyle.color).toBe(pl.grid);
    expect(dark.yAxis.splitLine.lineStyle.color).toBe(pd.grid);

    expect(light.xAxis.axisLabel.color).toBe(pl.sub);
    expect(dark.xAxis.axisLabel.color).toBe(pd.sub);

    expect(light.tooltip.backgroundColor).toBe(pl.tooltip_bg);
    expect(dark.tooltip.backgroundColor).toBe(pd.tooltip_bg);
    expect(light.tooltip.textStyle.color).toBe(pl.tooltip_text);
    expect(dark.tooltip.textStyle.color).toBe(pd.tooltip_text);

    expect(light.legend.textStyle.color).toBe(pl.sub);
    expect(dark.legend.textStyle.color).toBe(pd.sub);
  });

  it("对同一 dark 重复求值结果稳定（可安全放进 useMemo）", () => {
    expect(baseChartOption(true)).toEqual(baseChartOption(true));
    expect(baseChartOption(false)).toEqual(baseChartOption(false));
  });
});
