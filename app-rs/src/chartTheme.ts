/**
 * ECharts 主题取色（纯模块：无 DOM / 无 antd / 无 tauri 依赖）。
 *
 * canvas 画笔吃的是颜色字符串本身（zrender 不解析 var() 引用），图表 option
 * 必须持具体色值。取色一律直取 theme.ts 的 palette(dark)（单源），而不要经
 * getComputedStyle 读 --sk-*：applyTheme 在 App 的 effect 里、渲染提交之后
 * 才写 CSS 变量——切主题瞬间 option memo 重算（依赖含 dark）读到的仍是旧
 * 主题色，速率图在桶窗口下 deps 不含每秒数据，会一直停在旧色直到换窗口。
 * 本模块保证 option 是 (data, dark) 的纯函数，与 CSS 变量写入时序彻底解耦，
 * 切主题当帧即换色（轮次115 审计实锤修复）。
 */
import { palette } from "./theme";

/** 图表用到的主题色子集（键名映射：tooltip_bg → tooltipBg 等驼峰化）。 */
export function chartColors(dark: boolean) {
  const p = palette(dark);
  return {
    text: p.text,
    sub: p.sub,
    grid: p.grid,
    accent: p.accent,
    ok: p.ok,
    warn: p.warn,
    tooltipBg: p.tooltip_bg,
    tooltipText: p.tooltip_text,
  };
}

/** 图表统一风格：透明底 + palette(dark) 直取，切主题随 memo 的 dark 依赖立即换。 */
export function baseChartOption(dark: boolean) {
  const c = chartColors(dark);
  return {
    // 实时监控图每秒 setOption：关动画省 GPU/CPU（动画重放在 1s 数据节拍下
    // 是纯开销，且会造成曲线"呼吸"感）
    animation: false as const,
    textStyle: { color: c.text, fontSize: 12 },
    grid: { left: 8, right: 8, top: 28, bottom: 4, containLabel: true },
    tooltip: {
      trigger: "axis" as const,
      axisPointer: {
        type: "line" as const, // 悬浮垂直参考线
        lineStyle: { color: c.sub },
      },
      confine: true, // 卡片较窄时 tooltip 不出界
      // tooltip 走主题的 tooltip token（与 Qt 弹层同源），避免玻璃卡片上突兀的白底硬块
      backgroundColor: c.tooltipBg,
      borderWidth: 0,
      padding: [6, 10],
      textStyle: { color: c.tooltipText, fontSize: 12 },
      extraCssText: "border-radius: 8px; box-shadow: 0 4px 16px rgba(0, 0, 0, 0.2);",
    },
    legend: { textStyle: { color: c.sub }, top: 0 },
    xAxis: {
      type: "category" as const,
      axisLine: { lineStyle: { color: c.grid } },
      axisTick: { show: false },
      axisLabel: { color: c.sub },
    },
    yAxis: {
      type: "value" as const,
      splitLine: { lineStyle: { color: c.grid } },
      axisLabel: { color: c.sub },
    },
  };
}

/** 语义色：与 baseChartOption 同源（palette(dark)），调用处传当前 dark。 */
export const chartAccent = (dark: boolean) => chartColors(dark).accent;
export const chartOk = (dark: boolean) => chartColors(dark).ok;
export const chartWarn = (dark: boolean) => chartColors(dark).warn;
