/**
 * 按应用流量（网络监控 tab）：今天 / 本周 / 本月 × 应用过滤 × Top N。
 *
 * 纯逻辑独立成函数（apptraffic.ts，vitest 覆盖）：
 * - topRows：按 sent+recv 降序截断（后端已排好序，这里只是防御性重复锁口径）
 * - sharePct：合计占比（除零夹 0）
 * 数据来自 sk_apptraffic_summary（ETW 采集，非管理员/会话被占时 available=false）。
 */

export interface AppUsageRow {
  app: string;
  sent: number;
  recv: number;
}

export interface AppTrafficSummary {
  available: boolean;
  reason: string;
  rows: AppUsageRow[];
  sent: number;
  recv: number;
}

export type AppTrafficRange = "day" | "week" | "month";

export const APP_RANGES: { value: AppTrafficRange; label: string }[] = [
  { value: "day", label: "今天" },
  { value: "week", label: "本周" },
  { value: "month", label: "本月" },
];

/** Top N 截断（后端已降序；此函数是前端展示口径的独立锁）。 */
export function topRows(rows: AppUsageRow[], limit = 15): AppUsageRow[] {
  return rows.slice(0, Math.max(1, limit));
}

/** 占比 0~100：合计 ≤0（无数据）时夹 0，进度条不出现 NaN。 */
export function sharePct(row: AppUsageRow, totalSent: number, totalRecv: number): number {
  const total = totalSent + totalRecv;
  if (total <= 0) return 0;
  const v = ((row.sent + row.recv) / total) * 100;
  return Number.isFinite(v) ? Math.max(0, Math.min(100, v)) : 0;
}
