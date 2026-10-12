/**
 * 测速页纯逻辑：DNS 测速结果展示 + 下载测速可用性判定（无 React/DOM）。
 *
 * 下载测速已接入（后端 net/dl.rs，WinInet = 系统 TLS + 系统根证书 + 跟随系统代理，
 * 与 Python 版同一栈）。此处不做任何"假装测速"的实现——后端不可用时界面必须
 * 显示明确说明，因为一个假的速率数字比没有更糟（用户会据此判断网络好坏）。
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

/** 后端 `sk_dns_speed_history`：一轮 = { at, latencies: [[名称, ms], ...] }。 */
export interface DnsSpeedRound {
  at: string;
  latencies: [string, number][];
}

/** 某台服务器的历史延迟序列（旧→新，只含记录过它的轮）。 */
export function dnsHistorySeries(rounds: DnsSpeedRound[], server: string): { at: string; ms: number }[] {
  return rounds
    .filter((r) => r.latencies?.some(([n]) => n === server))
    .map((r) => ({
      at: r.at,
      ms: r.latencies.find(([n]) => n === server)![1],
    }));
}

/** 趋势一句话（最近 vs 前半段均值）：改善/持平/劣化，样本不足给 null。 */
export function dnsTrend(series: { at: string; ms: number }[]): { pct: number; label: string } | null {
  if (series.length < 4) return null;
  const half = Math.floor(series.length / 2);
  const avg = (xs: number[]) => xs.reduce((a, b) => a + b, 0) / xs.length;
  const oldAvg = avg(series.slice(0, half).map((x) => x.ms));
  const newAvg = avg(series.slice(half).map((x) => x.ms));
  if (oldAvg <= 0) return null;
  const pct = ((newAvg - oldAvg) / oldAvg) * 100;
  const label = pct <= -15 ? "变快" : pct >= 15 ? "变慢" : "持平";
  return { pct, label };
}

/** 后端 `sk_speedtest_status`。 */
export interface SpeedtestStatus {
  available: boolean;
  urls: string[];
}

/** 后端 `sk_download_test` 单 URL 结果。 */
export interface DownloadUrlResult {
  url: string;
  ok: boolean;
  err: string;
  bytes: number;
  avg_bps: number;
  peak_bps: number;
}

export interface DownloadTestResult {
  results: DownloadUrlResult[];
  ok_count: number;
  best_peak_bps: number;
}

/** 字节/秒 → 人类可读速率（MB/s 级测速用 MB 展示更直观）。 */
export function formatBps(bps: number): string {
  if (bps >= 1024 * 1024) return `${(bps / 1024 / 1024).toFixed(1)} MB/s`;
  if (bps >= 1024) return `${(bps / 1024).toFixed(0)} KB/s`;
  return `${bps.toFixed(0)} B/s`;
}

/** 字节数 → 人类可读大小。与 formatBps 同刻度但不带 /s——下载量是累计值，
 * 标成速率会把「这次下了多少」误报成「跑得多快」。 */
export function formatBytes(bytes: number): string {
  if (bytes >= 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${bytes.toFixed(0)} B`;
}

/** URL → 展示用的主机名（去掉协议/路径/参数）。测速表里完整 URL 又长又重复，
 * 主机名已足够辨认；解析失败就原文返回，绝不为展示吞掉真实来源。 */
export function urlHost(url: string): string {
  try {
    return new URL(url).hostname;
  } catch {
    return url;
  }
}

/** 峰值速率 → 语义色 token：≥10MB/s 好、≥1MB/s 中、有值差、无值中性。 */
export function peakToken(peakBps: number): string {
  if (peakBps <= 0) return "sub";
  if (peakBps >= 10 * 1024 * 1024) return "ok";
  if (peakBps >= 1024 * 1024) return "warn";
  return "err";
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

/** 可一键应用为系统 DNS 的测速服务器（与后端 repairs::DNS_PRESETS 对齐：
    百度/Cloudflare/Google 无预设，测出再好也只能手动改）。 */
const APPLYABLE_PRESETS = new Set(["阿里", "腾讯", "114"]);

/** 推荐项是否可一键应用为系统 DNS。 */
export function applyablePreset(name: string): boolean {
  return APPLYABLE_PRESETS.has(name);
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

/** 能力异常说明（恒可用后仅剩读取失败的兜底文案）。 */
export function downloadSpeedNotice(status: SpeedtestStatus | null): string {
  if (!status) return "正在读取测速能力状态…";
  return "";
}
