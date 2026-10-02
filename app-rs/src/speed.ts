/**
 * 测速页纯逻辑：DNS 测速结果展示 + 下载测速可用性判定（无 React/DOM）。
 *
 * 特别注意：**下载测速在 Rust 版尚未接入**（缺 HTTPS/TLS 能力，Python 版测速源全是
 * https）。此处不做任何"假装测速"的实现——`available=false` 时界面必须显示明确的
 * 未接入说明，因为一个假的速率数字比没有更糟（用户会据此判断网络好坏）。
 */

/** 后端 `sk_dns_speed` 的单行结果。 */
export interface DnsProbe {
  server: string;
  name: string;
  avg_ms: number | null;
  ok_count: number;
  total: number;
  recommended: boolean;
}

export interface DnsSpeedResult {
  domain: string;
  times: number;
  results: DnsProbe[];
  usable: number;
  failed: number;
  recommendation: string;
}

/** 后端 `sk_speedtest_status`。 */
export interface SpeedtestStatus {
  available: boolean;
  reason: string;
  python_urls: string[];
}

/** 成功率（0~1）。 */
export function successRate(p: DnsProbe): number {
  return p.total === 0 ? 0 : p.ok_count / p.total;
}

/** 结果是否可用（至少成功一次）。 */
export function isUsable(p: DnsProbe): boolean {
  return p.avg_ms !== null && p.ok_count > 0;
}

/** 延迟 → 语义色 token：<30ms 好、<100ms 中、其余差、无值中性。 */
export function latencyToken(avgMs: number | null): string {
  if (avgMs === null) return "sub";
  if (avgMs < 30) return "ok";
  if (avgMs < 100) return "warn";
  return "err";
}

/** 成功率 → 语义色 token。 */
export function rateToken(rate: number): string {
  if (rate >= 1) return "ok";
  if (rate > 0) return "warn";
  return "err";
}

/** 延迟展示（无值显示「—」）。 */
export function formatLatency(avgMs: number | null): string {
  if (avgMs === null) return "—";
  return `${avgMs.toFixed(0)} ms`;
}

/** 成功率展示。 */
export function formatRate(p: DnsProbe): string {
  return `${Math.round(successRate(p) * 100)}%`;
}

/** 统计可用/失败台数（后端已给，这里做兜底以免字段缺失）。 */
export function speedStats(result: DnsSpeedResult | null): { usable: number; failed: number } {
  if (!result) return { usable: 0, failed: 0 };
  if (typeof result.usable === "number" && typeof result.failed === "number") {
    return { usable: result.usable, failed: result.failed };
  }
  const usable = result.results.filter(isUsable).length;
  return { usable, failed: result.results.length - usable };
}

/** 推荐项（后端已标，兜底自行判定）。 */
export function recommendedProbe(result: DnsSpeedResult | null): DnsProbe | null {
  if (!result) return null;
  const marked = result.results.find((p) => p.recommended);
  if (marked) return marked;
  const usable = result.results.filter(isUsable);
  if (usable.length === 0) return null;
  return usable.reduce((a, b) => ((a.avg_ms ?? 0) <= (b.avg_ms ?? 0) ? a : b));
}

/** 下载测速是否可用（未接入时必须为 false，界面据此显示说明）。 */
export function downloadSpeedAvailable(status: SpeedtestStatus | null): boolean {
  return status?.available === true;
}

/** 未接入说明文案（保证不会空白）。 */
export function downloadSpeedNotice(status: SpeedtestStatus | null): string {
  if (!status) return "正在读取测速能力状态…";
  if (status.available) return "";
  return status.reason || "下载测速尚未接入";
}
