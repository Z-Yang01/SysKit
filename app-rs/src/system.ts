/**
 * 系统助手页纯逻辑（无 React/DOM，vitest 覆盖）。
 *
 * 本页对应 Python 侧 `ui/system_page.py`。M5 分批接入，当前已交付「性能监控」
 * （CPU/内存/磁盘曲线 + WiFi 状态）；其余分页（启动项/电池/清理/磁盘/剪贴板/场景）
 * 逐轮补，界面用 Pending 明示而不是给空壳。
 */

/** 后端 `sk://tick` 事件与 `sk_wifi_info` 的 WiFi 字段。 */
export interface WifiInfo {
  connected: boolean;
  ssid: string;
  signal: number | null;
  dbm: number | null;
  bssid: string;
  rx_kbps: number;
  tx_kbps: number;
  label: string;
}

/** 遥测一拍（只列本页用到的字段）。 */
export interface PerfTick {
  cpu: number;
  mem: number;
  mem_used_gb: number;
  disk_r: number;
  disk_w: number;
  wifi_signal?: number | null;
  wifi_ssid?: string;
}

/** 曲线采样点上限（与概览页一致：1 秒/点，保留 180 点 = 3 分钟）。 */
export const SERIES_LEN = 180;

/** 追加一个采样点并裁剪到上限（不修改入参）。 */
export function pushSample(series: number[], value: number, max = SERIES_LEN): number[] {
  const next = series.length >= max ? series.slice(series.length - max + 1) : series.slice();
  next.push(value);
  return next;
}

/**
 * 断流判定：页面隐藏期间采样被跳过，切回时若断流超过窗口就清空旧序列。
 *
 * 对应 Python `reset_if_stale(max_gap_sec=600)`——**不清空会把 10 分钟前的点与
 * 现在的点画成一条连续曲线**，看起来像"这段时间一直很平稳"，而实际是没有数据。
 */
export function isStale(lastTickMs: number | null, nowMs: number, maxGapSec = 600): boolean {
  if (lastTickMs === null) return false;
  return (nowMs - lastTickMs) / 1000 > maxGapSec;
}

/** CPU/内存占比 → 语义色 token（与概览页同一分档）。 */
export function usageToken(percent: number): string {
  if (percent >= 90) return "err";
  if (percent >= 70) return "warn";
  return "ok";
}

/** WiFi 信号 → 语义色 token（无信号为中性）。 */
export function signalToken(signal: number | null | undefined): string {
  if (signal === null || signal === undefined) return "sub";
  if (signal >= 75) return "ok";
  if (signal >= 50) return "warn";
  return "err";
}

/**
 * WiFi 摘要文案。
 *
 * 已连接但 SSID 为空 = 隐藏网络——必须显示成「隐藏网络」而不是空白，
 * 否则用户看到的是"已连接 + 空名字"，会以为是程序坏了。
 */
export function wifiSummary(w: WifiInfo | null): string {
  if (!w) return "未读取";
  if (!w.connected) return "未连接 WiFi";
  const name = w.ssid && w.ssid.length > 0 ? w.ssid : "隐藏网络";
  const sig = w.signal === null || w.signal === undefined ? "—" : `${w.signal}%`;
  const dbm = w.dbm === null || w.dbm === undefined ? "" : `（约 ${w.dbm} dBm）`;
  return `${name} · 信号 ${sig}${dbm} · ${w.label}`;
}

/** 链路速率展示（kbps → Mbps）。 */
export function formatRate(kbps: number): string {
  if (!kbps) return "—";
  return `${(kbps / 1000).toFixed(0)} Mbps`;
}

/** 后端 `sk_top_procs` 的一行。 */
export interface ProcRow {
  pid: number;
  name: string;
  bytes: number;
}

export interface TopProcs {
  rows: ProcRow[];
  count: number;
  total_bytes: number;
  limit: number;
}

/**
 * Top 进程占总内存的百分比。
 *
 * 用**进程内存合计**作分母而不是物理内存：两者含义不同（合计会重复计共享页），
 * 但用户看这一列是为了比较"谁占得多"，用合计作分母自洽且不需要额外的物理内存读数。
 * `tasklist` 报的是工作集，合计也确实会大于物理内存——所以文案里写明是"占进程合计"。
 */
export function procShare(bytes: number, totalBytes: number): number {
  if (totalBytes <= 0) return 0;
  return (bytes / totalBytes) * 100;
}

/** Top 进程列表 → 用于表格的展示行（带占比）。 */
export function procRows(data: TopProcs | null): (ProcRow & { share: number })[] {
  if (!data) return [];
  return data.rows.map((r) => ({ ...r, share: procShare(r.bytes, data.total_bytes) }));
}

/** 进程采集摘要文案。 */
export function procsSummary(data: TopProcs | null): string {
  if (!data) return "尚未采集";
  return `共 ${data.count} 个进程 · 合计 ${humanBytes(data.total_bytes)} · 显示前 ${data.rows.length}`;
}

/** 磁盘读写速率（字节/秒 → 人类可读）。 */
export function humanBytes(bps: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let v = Math.abs(bps);
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  const s = i === 0 ? String(Math.round(v)) : v.toFixed(v >= 100 ? 0 : 1);
  return `${bps < 0 ? "-" : ""}${s} ${units[i]}/s`;
}
