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
  eventLine,
  formatDuration,
  humanBytes,
  humanRate,
  pushTick,
  seriesPoints,
  stateLabel,
  summarizeReport,
  trafficCell,
  type DayReport,
  type Series,
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
    expect(humanRate(1024)).toBe("1.0 KB/s");
    expect(humanRate(0)).toBe("0 B/s");
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
