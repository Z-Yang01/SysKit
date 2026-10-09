/**
 * 网络质量序列的纯逻辑（无 React/DOM）：丢包率派生与曲线数据整形。
 *
 * 后端 `sk_quality_series` 行只含有数据的桶（key 为 HH:MM / HH时 / MM-DD
 * 标签）；趋势卡按时间轴直接渲染这些行，不做跨桶插值。
 */

export interface QualityRow {
  key: string;
  /** 均值延迟（ms）；null = 该桶全丢包或无样本 */
  gw_avg_ms: number | null;
  gw_loss_pct: number | null;
  pub_avg_ms: number | null;
  pub_loss_pct: number | null;
  dns_ms: number | null;
}

export interface QualitySeries {
  granularity: string;
  rows: QualityRow[];
}

export type QualityRange = "minute" | "hour" | "day";

export const QUALITY_RANGES: { value: QualityRange; label: string }[] = [
  { value: "minute", label: "近 60 分钟" },
  { value: "hour", label: "近 48 小时" },
  { value: "day", label: "近 30 天" },
];

/** 丢包率安全读取：null（无样本）按 0 处理，仅用于数字展示。 */
export function lossPct(row: QualityRow, which: "gw" | "pub"): number {
  const v = which === "gw" ? row.gw_loss_pct : row.pub_loss_pct;
  return Number.isFinite(v) ? (v as number) : 0;
}

/** 区间平均丢包率（全部桶样本加权近似 = 各桶均值平均，桶粒度一致时精确）。 */
export function avgLossPct(rows: QualityRow[], which: "gw" | "pub"): number {
  if (rows.length === 0) return 0;
  const vals = rows
    .map((r) => lossPct(r, which))
    .filter((v) => Number.isFinite(v));
  if (vals.length === 0) return 0;
  return vals.reduce((a, b) => a + b, 0) / vals.length;
}

/** 区间平均延迟（仅成功样本均值再平均）。 */
export function avgLatency(rows: QualityRow[], which: "gw" | "pub"): number | null {
  const vals = rows
    .map((r) => (which === "gw" ? r.gw_avg_ms : r.pub_avg_ms))
    .filter((v): v is number => Number.isFinite(v as number));
  if (vals.length === 0) return null;
  return vals.reduce((a, b) => a + b, 0) / vals.length;
}

/** DNS 均值（null 安全）。 */
export function avgDnsMs(rows: QualityRow[]): number | null {
  const vals = rows.map((r) => r.dns_ms).filter((v): v is number => Number.isFinite(v as number));
  if (vals.length === 0) return null;
  return vals.reduce((a, b) => a + b, 0) / vals.length;
}
