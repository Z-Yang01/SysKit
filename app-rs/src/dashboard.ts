/**
 * 概览页 / 网络监控页的纯逻辑（无 React、无 DOM，vitest 全覆盖）。
 *
 * 对应 Python 侧 ui/dashboard_page.py + charts.py 的取数与展示规则：
 * 字节格式化、图表数据点、日报表格、粒度切换、事件时间线文案。
 * 把这些放在这里而不是组件内，是为了让「无数据 vs 0」这类边界能被断言钉住。
 */

/** 后端 `sk_tick` / `sk://tick` 的载荷。 */
export interface Tick {
  /** 前端收到时的本地毫秒时间戳（后端 payload 无 ts，订阅处补充；旧数据可缺省） */
  ts?: number;
  down: number;
  up: number;
  cpu: number;
  mem: number;
  mem_used_gb: number;
  disk_r: number;
  disk_w: number;
  today_sent: number;
  today_recv: number;
}

/** 后端 `sk_stats_series` 的返回。 */
export interface SeriesRow {
  key: string;
  sent: number;
  recv: number;
}

export interface Series {
  granularity: string;
  ssid: string | null;
  rows: SeriesRow[];
  sent: number;
  recv: number;
}

/** 后端 `sk_daily_report` 的单行。 */
export interface DayReport {
  date: string;
  outages: number;
  outage_sec: number;
  sent: number;
  recv: number;
  has_events: boolean;
  has_traffic: boolean;
}

/** 后端 `sk_events` 的条目。 */
export interface EventItem {
  ts: string;
  state: string;
  detail: string;
}

export type Granularity = "minute" | "hour" | "day" | "month";

export const GRANULARITIES: { value: Granularity; label: string }[] = [
  { value: "minute", label: "近 60 分钟" },
  { value: "hour", label: "近 48 小时" },
  { value: "day", label: "近 30 天" },
  { value: "month", label: "近 12 月" },
];

/**
 * 人类可读字节数（对齐 Python charts.human_bytes 与 Rust telemetry::human_bytes）。
 * 三处口径必须一致，否则同一份数据在托盘/界面显示不同。
 */
export function humanBytes(n: number): string {
  const units = ["B", "KB", "MB", "GB", "TB", "PB"];
  let v = Number.isFinite(n) && n > 0 ? n : 0;
  let i = 0;
  while (v >= 1024 && i + 1 < units.length) {
    v /= 1024;
    i += 1;
  }
  return i === 0 ? `${Math.round(v)} ${units[i]}` : `${v.toFixed(1)} ${units[i]}`;
}

/** 速率显示（字节/秒 → 人类可读/秒）。 */
export function humanRate(bytesPerSec: number): string {
  return `${humanBytes(bytesPerSec)}/s`;
}

/** 断网时长格式化：<60 秒 → 「N 秒」；<60 分 → 「N分M秒」；否则「N.N 小时」。 */
export function formatDuration(sec: number): string {
  const s = Math.max(0, Math.floor(sec));
  if (s === 0) return "—";
  if (s < 60) return `${s} 秒`;
  if (s < 3600) {
    const m = Math.floor(s / 60);
    const r = s % 60;
    return r === 0 ? `${m} 分` : `${m}分${r}秒`;
  }
  return `${(s / 3600).toFixed(1)} 小时`;
}

/** 流量单元格：无记录显示「—」而不是 0（对应 Python 的不插值口径）。 */
export function trafficCell(has: boolean, bytes: number): string {
  return has ? humanBytes(bytes) : "—";
}

/** 曲线数据点（时间正序，ECharts 直接吃）。 */
export function seriesPoints(series: Series | null): { labels: string[]; sent: number[]; recv: number[] } {
  const rows = series?.rows ?? [];
  return {
    labels: rows.map((r) => r.key),
    sent: rows.map((r) => r.sent),
    recv: rows.map((r) => r.recv),
  };
}

/**
 * 环形缓冲区：把逐拍 tick 压成固定长度的曲线窗口。
 *
 * Python 侧用 pyqtgraph 的滚动窗口；这里保持同样的「只看最近 N 拍」语义。
 */
export const CHART_WINDOW = 120;

export function pushTick(history: Tick[], tick: Tick, cap = CHART_WINDOW): Tick[] {
  if (tick.ts === undefined) tick.ts = Date.now();
  const next = history.length >= cap ? history.slice(history.length - cap + 1) : history.slice();
  next.push(tick);
  return next;
}

/** 网络状态 → 中文文案与语义色 token（与 tray.py 的 tips 表同源）。 */
export function stateLabel(state: string): { text: string; token: string } {
  switch (state) {
    case "ok":
      return { text: "网络正常", token: "ok" };
    case "no_net":
      return { text: "已连接但无法上网", token: "warn" };
    case "ncsi_false":
      return { text: "NCSI 误报", token: "warn" };
    case "no_internet":
      return { text: "外网不可达", token: "err" };
    case "offline":
      return { text: "无网络连接", token: "sub" };
    default:
      return { text: state || "未知", token: "sub" };
  }
}

/** 日报汇总（网络监控页顶部的合计卡）。 */
export function summarizeReport(rows: DayReport[]): {
  outages: number;
  outageSec: number;
  sent: number;
  recv: number;
  daysWithEvents: number;
} {
  let outages = 0;
  let outageSec = 0;
  let sent = 0;
  let recv = 0;
  let daysWithEvents = 0;
  for (const r of rows) {
    outages += r.outages;
    outageSec += r.outage_sec;
    sent += r.sent;
    recv += r.recv;
    if (r.has_events) daysWithEvents += 1;
  }
  return { outages, outageSec, sent, recv, daysWithEvents };
}

/** 事件行文案（时间 + 状态 + 说明；说明为空则不补「—」后面的空白）。 */
export function eventLine(ev: EventItem): string {
  const s = stateLabel(ev.state);
  const detail = ev.detail?.trim();
  return detail ? `${ev.ts} · ${s.text} · ${detail}` : `${ev.ts} · ${s.text}`;
}

/** 日报表头（列顺序与 Python 日报表一致）。 */
export const REPORT_COLUMNS = [
  "日期",
  "断网次数",
  "累计断网时长",
  "下行",
  "上行",
] as const;
