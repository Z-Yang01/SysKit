/**
 * 概览/网络监控页纯逻辑测试。
 *
 * 重点覆盖「无数据 vs 0」的区分（这是 Python 侧反复强调的口径：应用没运行的日子
 * 显示「—」而不是 0），以及字节/时长格式化与三处实现（Python/Rust/TS）的一致性。
 */
import { describe, expect, it } from "vitest";

import {
  CHART_WINDOW,
  REPORT_COLUMNS,
  axisTimeLabels,
  clockTime,
  eventCategory,
  filterEventsByTime,
  type EventItem,
  eventLine,
  formatAxisBps,
  formatAxisTime,
  formatDuration,
  humanBps,
  humanBytes,
  peakPoint,
  pushTick,
  ema,
  freshness,
  smoothSeries,
  smoothTick,
  rateViewFromBuckets,
  rateViewFromTicks,
  seriesPoints,
  stateLabel,
  summarizeReport,
  trafficCell,
  type DayReport,
  type Series,
  type SeriesRow,
  type Tick,
} from "../src/dashboard";

function tick(over: Partial<Tick> = {}): Tick {
  return {
    down: 0,
    up: 0,
    cpu: 0,
    mem: 0,
    mem_used_gb: 0,
    disk_r: 0,
    disk_w: 0,
    today_sent: 0,
    today_recv: 0,
    ...over,
  };
}

function day(over: Partial<DayReport> = {}): DayReport {
  return {
    date: "2026-01-01",
    outages: 0,
    outage_sec: 0,
    sent: 0,
    recv: 0,
    has_events: false,
    has_traffic: false,
    ...over,
  };
}

describe("eventCategory 事件分组", () => {
  it("断网类归 net，质量/流量各自归组，未知归 other", () => {
    expect(eventCategory("ok")).toBe("net");
    expect(eventCategory("no_net")).toBe("net");
    expect(eventCategory("no_internet")).toBe("net");
    expect(eventCategory("offline")).toBe("net");
    expect(eventCategory("ncsi_false")).toBe("net");
    expect(eventCategory("quality_bad")).toBe("quality");
    expect(eventCategory("quality_ok")).toBe("quality");
    expect(eventCategory("traffic_warn")).toBe("traffic");
    expect(eventCategory("future_state")).toBe("other");
  });
});

describe("humanBytes", () => {
  it("与 Rust telemetry::human_bytes 的口径一致", () => {
    expect(humanBytes(0)).toBe("0 B");
    expect(humanBytes(512)).toBe("512 B");
    expect(humanBytes(1024)).toBe("1.0 KB");
    expect(humanBytes(1536)).toBe("1.5 KB");
    expect(humanBytes(1024 * 1024)).toBe("1.0 MB");
    expect(humanBytes(1024 ** 3)).toBe("1.0 GB");
    expect(humanBytes(1024 ** 4)).toBe("1.0 TB");
  });

  it("负数/NaN 夹到 0（不显示 -1 B 或 NaN）", () => {
    expect(humanBytes(-5)).toBe("0 B");
    expect(humanBytes(Number.NaN)).toBe("0 B");
    expect(humanBytes(Number.POSITIVE_INFINITY)).toBe("0 B");
  });

  it("速率带 /s 后缀", () => {
  });
});

describe("formatDuration", () => {
  it("0 秒显示「—」（无断网 ≠ 断网 0 秒）", () => {
    expect(formatDuration(0)).toBe("—");
    expect(formatDuration(-3)).toBe("—");
  });

  it("秒/分/时三档", () => {
    expect(formatDuration(45)).toBe("45 秒");
    expect(formatDuration(60)).toBe("1 分");
    expect(formatDuration(72)).toBe("1分12秒");
    expect(formatDuration(3599)).toBe("59分59秒");
    expect(formatDuration(3600)).toBe("1.0 小时");
    expect(formatDuration(9000)).toBe("2.5 小时");
  });
});

describe("trafficCell", () => {
  it("无记录显示「—」，有记录显示数值（含 0 字节）", () => {
    expect(trafficCell(false, 0)).toBe("—");
    expect(trafficCell(false, 12345)).toBe("—");
    expect(trafficCell(true, 0)).toBe("0 B");
    expect(trafficCell(true, 2048)).toBe("2.0 KB");
  });
});

describe("seriesPoints", () => {
  it("空/未加载返回空序列（图表不炸）", () => {
    expect(seriesPoints(null)).toEqual({ labels: [], sent: [], recv: [] });
  });

  it("时间正序映射到三个数组", () => {
    const s: Series = {
      granularity: "minute",
      ssid: null,
      rows: [
        { key: "10:05", sent: 1, recv: 2 },
        { key: "10:06", sent: 3, recv: 4 },
      ],
      sent: 4,
      recv: 6,
    };
    expect(seriesPoints(s)).toEqual({
      labels: ["10:05", "10:06"],
      sent: [1, 3],
      recv: [2, 4],
    });
  });
});

describe("pushTick", () => {
  it("顺序追加且不超窗口", () => {
    let h: Tick[] = [];
    for (let i = 0; i < 5; i++) h = pushTick(h, tick({ down: i }), 3);
    expect(h).toHaveLength(3);
    expect(h.map((t) => t.down)).toEqual([2, 3, 4]);
  });

  it("恰好等于窗口时不裁剪", () => {
    let h: Tick[] = [];
    for (let i = 0; i < CHART_WINDOW; i++) h = pushTick(h, tick());
    expect(h).toHaveLength(CHART_WINDOW);
    expect(h.length).toBe(120);
  });

  it("不修改传入的数组（纯函数）", () => {
    const original: Tick[] = [tick({ down: 1 })];
    const next = pushTick(original, tick({ down: 2 }));
    expect(original).toHaveLength(1);
    expect(next).toHaveLength(2);
  });
});

describe("humanBps", () => {
  it("humanBps 自适应单位并带 /s（各值各选最优单位）", () => {
    expect(humanBps(14.6 * 1024 * 1024)).toBe("14.6 MB/s");
    expect(humanBps(477.9 * 1024)).toBe("477.9 KB/s");
    expect(humanBps(0)).toBe("0 B/s");
    expect(humanBps(-5)).toBe("0 B/s");
  });
});

describe("formatAxisBps", () => {
  it("Y 轴紧凑刻度：去空格去尾零（对应裸字节 15,000,000 → 14.3MB）", () => {
    expect(formatAxisBps(0)).toBe("0");
    expect(formatAxisBps(512)).toBe("512B");
    expect(formatAxisBps(1024)).toBe("1KB");
    expect(formatAxisBps(1536)).toBe("1.5KB");
    expect(formatAxisBps(6 * 1024 * 1024)).toBe("6MB");
    expect(formatAxisBps(15_000_000)).toBe("14.3MB");
    expect(formatAxisBps(1024 ** 4)).toBe("1TB");
  });

  it("负数/NaN/Infinity 夹 0（速率不可能为负，轴不能出现坏刻度）", () => {
    expect(formatAxisBps(-3)).toBe("0");
    expect(formatAxisBps(Number.NaN)).toBe("0");
    expect(formatAxisBps(Number.POSITIVE_INFINITY)).toBe("0");
  });
});

describe("formatAxisTime / axisTimeLabels", () => {
  it("相对时刻：0 为最新一拍，向左为负，小数秒四舍五入", () => {
    expect(formatAxisTime(0)).toBe("0s");
    expect(formatAxisTime(-37.4)).toBe("-37s");
    expect(formatAxisTime(-119.6)).toBe("-120s");
  });

  it("有 ts 的窗口按时间戳差换算（-120s … 0s）", () => {
    const base = 1_700_000_000_000;
    const h = [tick({ ts: base - 2000 }), tick({ ts: base - 1000 }), tick({ ts: base })];
    expect(axisTimeLabels(h)).toEqual(["-2s", "-1s", "0s"]);
  });

  it("缺 ts 的旧点按「每拍 1 秒」用索引差兜底，与有 ts 的点混用不串位", () => {
    const base = 1_700_000_000_000;
    const h = [tick(), tick(), tick({ ts: base - 1000 }), tick({ ts: base })];
    expect(axisTimeLabels(h)).toEqual(["-3s", "-2s", "-1s", "0s"]);
  });

  it("全缺 ts 的窗口退化为索引差；空窗口返回空数组", () => {
    expect(axisTimeLabels([tick(), tick(), tick()])).toEqual(["-2s", "-1s", "0s"]);
    expect(axisTimeLabels([])).toEqual([]);
  });
});

describe("clockTime", () => {
  it("按本地时区输出 HH:MM:SS（tooltip 标题）", () => {
    const t = new Date(2026, 0, 1, 10, 5, 9).getTime(); // 本地分量构造，避免时区差
    expect(clockTime(t)).toBe("10:05:09");
    expect(clockTime(new Date(2026, 5, 3, 23, 0, 0).getTime())).toBe("23:00:00");
  });

  it("缺 ts / 非法值返回 null（由调用方决定兜底文案）", () => {
    expect(clockTime(undefined)).toBe(null);
    expect(clockTime(Number.NaN)).toBe(null);
  });
});

describe("peakPoint", () => {
  it("取窗口内首个最大值点", () => {
    expect(peakPoint([1, 5, 5, 2])).toEqual({ index: 1, value: 5 });
  });

  it("空 / 全非有限数返回 null；全 0 返回点（是否标注由调用方决定）", () => {
    expect(peakPoint([])).toBe(null);
    expect(peakPoint([Number.NaN])).toBe(null);
    expect(peakPoint([0, 0])).toEqual({ index: 0, value: 0 });
  });
});

describe("stateLabel", () => {
  it("五种状态文案与语义色 token 对齐 Python tips 表", () => {
    expect(stateLabel("ok")).toEqual({ text: "网络正常", token: "ok" });
    expect(stateLabel("no_net")).toEqual({ text: "已连接但无法上网", token: "warn" });
    expect(stateLabel("ncsi_false")).toEqual({ text: "NCSI 误报", token: "warn" });
    expect(stateLabel("no_internet")).toEqual({ text: "外网不可达", token: "err" });
    expect(stateLabel("offline")).toEqual({ text: "无网络连接", token: "sub" });
  });

  it("未知状态原样显示而不是吞掉", () => {
    expect(stateLabel("weird").text).toBe("weird");
    expect(stateLabel("").text).toBe("未知");
  });
});

describe("summarizeReport", () => {
  it("空报告全 0", () => {
    expect(summarizeReport([])).toEqual({
      outages: 0,
      outageSec: 0,
      sent: 0,
      recv: 0,
      daysWithEvents: 0,
    });
  });

  it("累加次数/时长/流量，并按「有事件」计天数", () => {
    const rows = [
      day({ date: "d1", outages: 1, outage_sec: 120, sent: 10, recv: 20, has_events: true }),
      day({ date: "d2", outages: 2, outage_sec: 60, sent: 5, recv: 5, has_events: true }),
      day({ date: "d3", has_traffic: false }),
    ];
    expect(summarizeReport(rows)).toEqual({
      outages: 3,
      outageSec: 180,
      sent: 15,
      recv: 25,
      daysWithEvents: 2,
    });
  });
});

describe("eventLine", () => {
  it("有说明时拼接，无说明时不补多余分隔", () => {
    expect(eventLine({ ts: "2026-01-01 10:00:00", state: "no_net", detail: "探测失败 ×2" })).toBe(
      "2026-01-01 10:00:00 · 已连接但无法上网 · 探测失败 ×2",
    );
    expect(eventLine({ ts: "2026-01-01 10:00:00", state: "ok", detail: "  " })).toBe(
      "2026-01-01 10:00:00 · 网络正常",
    );
  });
});

describe("报表列签署", () => {
  it("五列口径固定（与 Python 日报表一致）", () => {
    expect(REPORT_COLUMNS).toHaveLength(5);
    expect(REPORT_COLUMNS[2]).toBe("累计断网时长");
  });
});

describe("速率曲线窗口（1m / 1h / 1d）", () => {
  const day = (over: Partial<Tick>): Tick => ({
    down: 0, up: 0, cpu: 0, mem: 0, mem_used_gb: 0, disk_r: 0, disk_w: 0,
    today_sent: 0, today_recv: 0, ...over,
  });

  it("1m 窗口：最近 60 拍、相对秒轴、tooltip 绝对时刻", () => {
    const now = Date.now();
    const history: Tick[] = [];
    for (let i = 59; i >= 0; i--) history.push(day({ ts: now - i * 1000, down: 100, up: 10 }));
    const v = rateViewFromTicks(history)!;
    expect(v.labels).toHaveLength(60);
    expect(v.labels[0]).toBe("-59s");
    expect(v.labels[59]).toBe("0s");
    expect(v.down.every((x) => x === 100)).toBe(true);
    expect(v.tips[59]).toMatch(/^\d{2}:\d{2}:\d{2}$/);
  });

  it("1m 窗口空历史返回 null（组件保持空态）", () => {
    expect(rateViewFromTicks([])).toBeNull();
  });

  it("1h 窗口：分钟桶补空桶为 0 + 字节量÷60=均速", () => {
    const now = new Date(2026, 9, 7, 14, 32, 30).getTime(); // 14:32:30
    const rows: SeriesRow[] = [
      { key: "14:30", sent: 600, recv: 1200 },   // 整桶：/60 → 10 / 20 B/s
      { key: "14:32", sent: 30, recv: 60 },      // 进行中桶：已过 30s → /30 → 1 / 2
    ];
    const v = rateViewFromBuckets(rows, 60, 60, now);
    expect(v.labels).toHaveLength(60);
    expect(v.labels[0]).toBe("13:33");
    expect(v.labels[58]).toBe("14:31");
    expect(v.labels[59]).toBe("14:32");
    const i1430 = v.labels.indexOf("14:30");
    expect(v.down[i1430]).toBeCloseTo(20); // recv=1200/60
    expect(v.up[i1430]).toBeCloseTo(10);
    const i1431 = v.labels.indexOf("14:31");
    expect(v.down[i1431]).toBe(0);          // 无流量的分钟补 0
    expect(v.down[59]).toBeCloseTo(2);      // 进行中桶 /30s
    expect(v.up[59]).toBeCloseTo(1);
    expect(v.tips[59]).toMatch(/^10-07 14:32$/);
  });

  it("1d 窗口：小时桶 24 点、HH时 标签", () => {
    const now = new Date(2026, 9, 7, 5, 0, 0).getTime();
    const rows: SeriesRow[] = [{ key: "04时", sent: 3600, recv: 7200 }];
    const v = rateViewFromBuckets(rows, 3600, 24, now);
    expect(v.labels).toHaveLength(24);
    expect(v.labels[23]).toBe("05时");
    const i = v.labels.indexOf("04时");
    expect(v.down[i]).toBeCloseTo(2); // 7200/3600
    expect(v.up[i]).toBeCloseTo(1);
  });

  it("进行中桶已过 0 秒时按 1s 兜底（除零防护）", () => {
    const now = new Date(2026, 9, 7, 14, 32, 0).getTime(); // 桶起始瞬间
    const rows: SeriesRow[] = [{ key: "14:32", sent: 10, recv: 0 }];
    const v = rateViewFromBuckets(rows, 60, 60, now);
    expect(Number.isFinite(v.up[59])).toBe(true);
  });
});

describe("filterEventsByTime 时间范围过滤", () => {
  const now = Date.parse("2026-10-12T20:00:00") / 1000;
  const mk = (ts: string): EventItem => ({ ts, state: "ok", detail: "" });
  const events = [
    mk("2026-10-12 19:30:00"), // 30 分钟前
    mk("2026-10-12 10:00:00"), // 10 小时前
    mk("2026-10-10 20:00:00"), // 2 天前
    mk("2026-10-01 20:00:00"), // 11 天前
  ];

  it("各档位按 ts 过滤（1h/24h/7天）", () => {
    expect(filterEventsByTime(events, 3600, now)).toHaveLength(1);
    expect(filterEventsByTime(events, 86400, now)).toHaveLength(2);
    expect(filterEventsByTime(events, 604800, now)).toHaveLength(3);
  });

  it("null = 全部保留", () => {
    expect(filterEventsByTime(events, null, now)).toHaveLength(4);
  });

  it("解析失败的 ts 保守保留（诚实展示不丢行）", () => {
    const bad: EventItem = { ts: "not-a-date", state: "ok", detail: "" };
    expect(filterEventsByTime([bad], 60, now)).toEqual([bad]);
  });
});

/**
 * 速率平滑（EMA）。
 *
 * 后端每秒给的是「上一秒的字节增量」，直接显示会疯狂闪烁；这类显示问题
 * 用户第一反应是"软件坏了"。锁住三件事：首值不被削、能收敛到真值、
 * 只平滑速率字段（cpu/mem 是区间平均，再平滑会迟滞）。
 */
describe("速率平滑 ema / smoothTick", () => {
  it("首值直接采用，不被 0 拖低", () => {
    expect(ema(null, 1234)).toBe(1234);
    expect(ema(undefined, 88)).toBe(88);
    expect(ema(NaN, 50)).toBe(50);
    expect(ema(-1, 50)).toBe(50);
  });

  it("按 α 混合，且能收敛到真值", () => {
    expect(ema(0, 100, 0.5)).toBe(50);
    let v = 0;
    for (let i = 0; i < 20; i++) v = ema(v, 100, 0.35);
    expect(v).toBeGreaterThan(99);
    // 掉到 0 时同样会衰减，不会永远挂着旧值
    for (let i = 0; i < 20; i++) v = ema(v, 0, 0.35);
    expect(v).toBeLessThan(1);
  });

  it("非法输入夹到 0，不产生 NaN/负数", () => {
    expect(ema(10, NaN)).toBe(ema(10, 0));
    expect(ema(10, -5)).toBe(ema(10, 0));
    // 非有限值（Infinity）同样按 0 处理：宁可显示平滑后的旧值，也不能把
    // Infinity/NaN 送进 toFixed 或坐标轴
    expect(ema(10, Number.POSITIVE_INFINITY)).toBe(ema(10, 0));
    expect(Number.isFinite(ema(10, Number.POSITIVE_INFINITY))).toBe(true);
  });

  it("smoothSeries 与逐点 ema 完全一致", () => {
    const raw = [0, 1000, 0, 5000, 200, 0];
    const got = smoothSeries(raw, 0.35);
    let prev: number | undefined;
    const want = raw.map((v) => {
      const s = ema(prev, v, 0.35);
      prev = s;
      return s;
    });
    expect(got).toEqual(want);
    expect(got).toHaveLength(raw.length);
  });

  it("smoothTick 只动速率，不动 cpu/mem/累计流量", () => {
    const prev: Tick = {
      ts: 1, down: 1000, up: 1000, cpu: 10, mem: 20, mem_used_gb: 8,
      disk_r: 1000, disk_w: 1000, today_sent: 5, today_recv: 6,
    };
    const next: Tick = { ...prev, down: 0, up: 0, cpu: 99, mem: 88, disk_r: 0, disk_w: 0 };
    const s = smoothTick(prev, next, 0.5);
    expect(s.cpu).toBe(99);   // 不平滑
    expect(s.mem).toBe(88);
    expect(s.today_sent).toBe(5);
    expect(s.down).toBe(500); // 平滑
    expect(s.up).toBe(500);
    expect(s.disk_r).toBe(500);
  });
});

/** 数据新鲜度：数字停住而不提示，用户会把卡住的读数当成真实网速。 */
describe("freshness", () => {
  const now = 1_700_000_000_000;

  it("无数据时不谎报新鲜", () => {
    expect(freshness(null, now)).toEqual({ text: "等待数据…", stale: false });
    expect(freshness(undefined, now).stale).toBe(false);
  });

  it("刚更新与几秒前", () => {
    expect(freshness(now, now).text).toBe("刚刚更新");
    expect(freshness(now - 1000, now).text).toBe("刚刚更新");
    expect(freshness(now - 5000, now)).toEqual({ text: "5 秒前", stale: false });
  });

  it("超过 10 秒判为陈旧（漏 1–2 拍不算）", () => {
    expect(freshness(now - 10_000, now).stale).toBe(false);
    expect(freshness(now - 11_000, now).stale).toBe(true);
  });

  it("一分钟以上按分钟显示", () => {
    expect(freshness(now - 90_000, now)).toEqual({ text: "1 分钟前", stale: true });
    expect(freshness(now - 600_000, now).text).toBe("10 分钟前");
  });

  it("时间戳在将来（时钟回拨）按刚刚处理，不出现负数秒", () => {
    const f = freshness(now + 30_000, now);
    expect(f.text).toBe("刚刚更新");
    expect(f.text.includes("-")).toBe(false);
  });
});
