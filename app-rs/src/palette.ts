/**
 * 命令面板纯逻辑（无 React/DOM，vitest 覆盖）。
 *
 * 匹配与排序在 Rust 侧（与 Python 同口径），这里只做**展示与交互**规则。
 * 唯一例外是算式反馈的展示判定——它决定"输入时会不会弹错误提示"。
 */

/** 后端 `sk_palette_actions` 的一行。 */
export interface PaletteAction {
  id: string;
  label: string;
  hint: string;
  keywords: string[];
  source: string;
}

export interface PaletteActions {
  rows: PaletteAction[];
  count: number;
  total: number;
  custom_count: number;
}

/** 后端 `sk_palette_calc`。 */
export interface CalcResult {
  is_math: boolean;
  ok: boolean;
  display: string;
  error: string;
}

/** 输入分类：算式 / 可用动作 / 无匹配。 */
export type InputKind = "calc" | "actions" | "empty";

/**
 * 输入分类。
 *
 * `is_math` 为真时**优先显示算式结果**——用户输入 `1+1` 就是想算数，
 * 而不是想搜名为 "1+1" 的页面（后者不可能存在）。
 */
export function classifyInput(query: string, calc: CalcResult | null, actionCount: number): InputKind {
  if (!query.trim()) return "empty";
  if (calc?.is_math) return "calc";
  return actionCount > 0 ? "actions" : "empty";
}

/**
 * 算式反馈的展示级别。
 *
 * 关键：`is_math=false` 时**不显示任何提示**——用户只是在搜别的东西，
 * 弹"表达式不合法"是噪声。只有确认"像算式"且求值失败时才报错。
 */
export function calcLevel(calc: CalcResult | null): "success" | "error" | "none" {
  if (!calc || !calc.is_math) return "none";
  return calc.ok ? "success" : "error";
}

/** 算式结果文案（含"="便于一眼看出是算出来的）。 */
export function calcText(calc: CalcResult | null): string {
  if (!calc || !calc.is_math) return "";
  return calc.ok ? `= ${calc.display}` : calc.error;
}

/** 结果计数文案。 */
export function resultSummary(data: PaletteActions | null, query: string): string {
  if (!data) return "";
  if (!query.trim()) return `共 ${data.total} 个动作`;
  if (data.count === 0) return "没有匹配的动作";
  return `${data.count} / ${data.total} 个匹配`;
}

/** 默认选中项下标（列表为空返回 -1）。 */
export function defaultIndex(count: number): number {
  return count > 0 ? 0 : -1;
}

/**
 * 上下移动选中项（**循环**：到底再按下回到开头）。
 *
 * 循环是命令面板的通行做法：用户按方向键时不该被"卡在边界"打断。
 */
export function moveIndex(current: number, delta: number, count: number): number {
  if (count <= 0) return -1;
  if (current < 0) return delta > 0 ? 0 : count - 1;
  return (current + delta + count * 1000) % count;
}

/** 数字键直选（1-9 → 下标 0-8）；超出范围返回 -1。 */
export function digitIndex(digit: number, count: number): number {
  if (digit < 1 || digit > 9) return -1;
  const idx = digit - 1;
  return idx < count ? idx : -1;
}

/** 动作来源 → AntD Tag 颜色。 */
export function sourceColor(source: string): string {
  switch (source) {
    case "自定义":
      return "purple";
    case "内置":
      return "blue";
    case "剪贴板":
      return "green";
    case "密码库":
      return "gold";
    default:
      return "default";
  }
}

/** 自定义命令的键值编辑：解析/序列化（设置页用）。 */
export interface CustomCommandDraft {
  name: string;
  kind: "url" | "cmd";
  value: string;
}

/** 解析面板自定义命令的 JSON 文本；失败返回空数组（与后端同口径）。 */
export function parseCustomCommands(raw: string): CustomCommandDraft[] {
  if (!raw.trim()) return [];
  let data: unknown;
  try {
    data = JSON.parse(raw);
  } catch {
    return [];
  }
  if (!Array.isArray(data)) return [];
  const out: CustomCommandDraft[] = [];
  for (const it of data) {
    if (typeof it !== "object" || it === null) continue;
    const o = it as Record<string, unknown>;
    const name = String(o.name ?? "").trim();
    const value = String(o.value ?? "").trim();
    if (!name || !value) continue;
    // 未知类型收敛成 url（保守方向：url 需显式点击才打开，cmd 会执行命令）
    const kind = o.type === "cmd" ? "cmd" : "url";
    out.push({ name, kind, value });
  }
  return out;
}

/** 序列化回 JSON 文本（写回 settings.palette_commands）。 */
export function serializeCustomCommands(cmds: CustomCommandDraft[]): string {
  if (cmds.length === 0) return "";
  return JSON.stringify(
    cmds.map((c) => ({ name: c.name, type: c.kind, value: c.value })),
    null,
    0,
  );
}
