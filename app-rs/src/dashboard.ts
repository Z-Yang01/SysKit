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

/**
 * 速率（B/s → 自适应单位 + "/s"）：卡片 / tooltip / 峰值标注共用的人读口径。
 * 每个值独立选最优单位（14.6 MB/s 与 477.9 KB/s 并排显示时各用各的）。
 * bps = bytes per second（SysKit 全链路口径均为字节，与 bit/s 区分）。
 */
export function humanBps(bytesPerSec: number): string {
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

/**
 * Y 轴刻度的紧凑字节格式：0 / 512B / 1.5KB / 14.3MB（对应原来裸的 15,000,000）。
 * 与 humanBytes 的差别：轴上空间金贵——去空格、去小数尾零、0 显示「0」；
 * 只管字节量级、不带 "/s"（速率语境由图题与 tooltip 交代），速率/总量轴可共用。
 * 负值/NaN 夹 0：速率不可能为负，坐标轴更不能出现 NaN 刻度。
 */
export function formatAxisBps(n: number): string {
  const v = Number.isFinite(n) && n > 0 ? n : 0;
  if (v === 0) return "0";
  const units = ["B", "KB", "MB", "GB", "TB", "PB"];
  let x = v;
  let i = 0;
  while (x >= 1024 && i + 1 < units.length) {
    x /= 1024;
    i += 1;
  }
  const s = i === 0 ? String(Math.round(x)) : x.toFixed(1).replace(/\.0$/, "");
  return `${s}${units[i]}`;
}

/** 相对时刻文案（-120s … 0s）：0 为最新一拍，向左为负；小数秒四舍五入。 */
export function formatAxisTime(deltaSec: number): string {
  return `${Math.round(deltaSec)}s`;
}

// ---------------- 速率曲线窗口（1 分钟 / 1 小时 / 1 天） ----------------

/** 速率曲线窗口：1m=逐拍实时；1h/1d=分钟/小时桶的均值速率。 */
export type RateWindow = "1m" | "1h" | "1d";

export const RATE_WINDOWS: { value: RateWindow; label: string }[] = [
  { value: "1m", label: "1 分钟" },
  { value: "1h", label: "1 小时" },
  { value: "1d", label: "1 天" },
];

/** 速率图统一数据形：labels 为类目轴；tips[i] 为该点 tooltip 标题。 */
export interface RateView {
  labels: string[];
  tips: string[];
  down: number[];
  up: number[];
}

/** 1 分钟窗口：最近 60 拍。x 轴相对秒（-59s…0s），tooltip 绝对时刻 HH:MM:SS。 */
export function rateViewFromTicks(history: Tick[]): RateView | null {
  const h = history.slice(-60);
  if (h.length === 0) return null;
  return {
    labels: axisTimeLabels(h),
    tips: h.map((t) => clockTime(t.ts) ?? ""),
    down: h.map((t) => t.down),
    up: h.map((t) => t.up),
  };
}

const pad2 = (x: number) => String(x).padStart(2, "0");

/**
 * 桶序列（sk_stats_series 的 minute/hour 行）→ 连续桶轴上的均值速率（B/s）。
 *
 * - rows 只含有流量的桶（无流量的分钟/小时没有 key），这里按当前时刻往回
 *   生成 count 个连续桶标签把空桶补 0——否则曲线在稀疏点间拉直线，
 *   看起来像持续速率（撒谎）。
 * - 桶值 = 字节量 ÷ 桶秒数。**最新桶是进行中的**（分钟刚过 10 秒只有 10 秒
 *   的量），除以整桶秒数会把当前速率压低一个量级——按实际经过秒数折算。
 * - 匹配只靠标签（HH:MM / HH时 在窗口内唯一）：应用离线超过一个窗口再回
 *   来时，旧桶标签可能与当前窗口别名重合（昨日 14:32 撞今日 14:32）——
 *   离线期本无实时数据，此误差仅限离线场景，接受并在此明示。
 */
export function rateViewFromBuckets(
  rows: SeriesRow[],
  bucketSec: number,
  count: number,
  nowMs = Date.now(),
): RateView {
  const byLabel = new Map<string, [number, number]>();
  for (const r of rows) byLabel.set(r.key, [r.sent, r.recv]);

  const labels: string[] = [];
  const tipDates: string[] = [];
  for (let i = count - 1; i >= 0; i--) {
    const t = new Date(nowMs - i * bucketSec * 1000);
    if (bucketSec === 60) {
      labels.push(`${pad2(t.getHours())}:${pad2(t.getMinutes())}`);
    } else {
      labels.push(`${pad2(t.getHours())}时`);
    }
    tipDates.push(`${pad2(t.getMonth() + 1)}-${pad2(t.getDate())}`);
  }

  // 最新桶的已过秒数（≥1）：进行中桶按实际时长折算，别让刚开的桶显示成近零
  const bucketStartMs = Math.floor(nowMs / (bucketSec * 1000)) * bucketSec * 1000;
  const elapsedSec = Math.min(bucketSec, Math.max(1, Math.ceil((nowMs - bucketStartMs) / 1000)));

  const down: number[] = [];
  const up: number[] = [];
  const tips: string[] = [];
  labels.forEach((label, i) => {
    const isLast = i === labels.length - 1;
    const div = (isLast ? elapsedSec : bucketSec) || 1;
    const hit = byLabel.get(label);
    down.push(hit ? hit[1] / div : 0); // rows.sent=上行 / rows.recv=下行（与监控页同约定）
    up.push(hit ? hit[0] / div : 0);
    tips.push(`${tipDates[i]} ${label}`);
  });
  return { labels, tips, down, up };
}

/**
 * X 轴相对时间刻度：用 Tick.ts（前端订阅处打的毫秒戳）相对最新一拍换算。
 * 旧数据缺 ts 的点按「每拍 1 秒」用索引差兜底（tick 固定每秒一拍，误差可忽略）；
 * 混合窗口里两种来源各自正确，不会互相污染。
 */
export function axisTimeLabels(history: Tick[]): string[] {
  const n = history.length;
  if (n === 0) return [];
  const last = history[n - 1].ts;
  return history.map((t, i) => {
    const delta =
      last !== undefined && t.ts !== undefined ? (t.ts - last) / 1000 : i - (n - 1);
    return formatAxisTime(delta);
  });
}

/** 绝对时刻 HH:MM:SS（tooltip 标题用，本地时区）；缺 ts / 非法值返回 null 由调用方兜底。 */
export function clockTime(ts: number | undefined): string | null {
  if (ts === undefined || !Number.isFinite(ts)) return null;
  const d = new Date(ts);
  const p = (x: number) => String(x).padStart(2, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

/** 窗口内最大值点（取首个最大值）；空 / 全非有限数返回 null。全 0 也返回点，是否标注由调用方定。 */
export function peakPoint(values: number[]): { index: number; value: number } | null {
  let index = -1;
  let value = -Infinity;
  for (let i = 0; i < values.length; i++) {
    const v = values[i];
    if (Number.isFinite(v) && v > value) {
      value = v;
      index = i;
    }
  }
  return index >= 0 ? { index, value } : null;
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
    case "quality_bad":
      return { text: "网络质量告警", token: "warn" };
    case "quality_ok":
      return { text: "网络质量恢复", token: "ok" };
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
