/**
 * 测速页纯逻辑测试。
 *
 * 重点是**下载测速的诚实性**：未接入时必须报 available=false，
 * 且说明文案不能为空——一个假的速率数字会被用来判断网络好坏。
 */
import { describe, expect, it } from "vitest";

import {
  downloadSpeedAvailable,
  downloadSpeedNotice,
  formatBytes,
  formatLatency,
  formatRate,
  isUsable,
  latencyToken,
  rateToken,
  recommendedProbe,
  speedStats,
  successRate,
  formatBps,
  peakToken,
  urlHost,
  type DnsProbe,
  type DnsSpeedResult,
  dnsHistorySeries,
  dnsTrend,
  type DnsSpeedRound,
} from "../src/dnsSpeed";

const probe = (over: Partial<DnsProbe> = {}): DnsProbe => ({
  server: "223.5.5.5",
  name: "阿里",
  avg_ms: 12,
  ok_count: 3,
  total: 3,
  recommended: false,
  ...over,
});

const result = (over: Partial<DnsSpeedResult> = {}): DnsSpeedResult => ({
  domain: "www.baidu.com",
  times: 3,
  results: [probe()],
  usable: 1,
  failed: 0,
  recommendation: "推荐使用 223.5.5.5（阿里）",
  ...over,
});

describe("successRate / isUsable", () => {
  it("成功率按 total 计算，total=0 不除零", () => {
    expect(successRate(probe({ ok_count: 3, total: 3 }))).toBe(1);
    expect(successRate(probe({ ok_count: 1, total: 4 }))).toBe(0.25);
    expect(successRate(probe({ ok_count: 0, total: 0 }))).toBe(0);
  });

  it("可用要求至少成功一次且延迟非空", () => {
    expect(isUsable(probe())).toBe(true);
    expect(isUsable(probe({ avg_ms: null, ok_count: 0 }))).toBe(false);
    expect(isUsable(probe({ avg_ms: null, ok_count: 2 }))).toBe(false);
  });
});

describe("配色 token", () => {
  it("延迟三档 + 无值中性", () => {
    expect(latencyToken(10)).toBe("ok");
    expect(latencyToken(29)).toBe("ok");
    expect(latencyToken(30)).toBe("warn");
    expect(latencyToken(99)).toBe("warn");
    expect(latencyToken(100)).toBe("err");
    expect(latencyToken(null)).toBe("sub");
  });

  it("成功率三档", () => {
    expect(rateToken(1)).toBe("ok");
    expect(rateToken(0.5)).toBe("warn");
    expect(rateToken(0)).toBe("err");
  });
});

describe("格式化", () => {
  it("延迟取整，无值显示「—」", () => {
    expect(formatLatency(12.4)).toBe("12 ms");
    expect(formatLatency(12.6)).toBe("13 ms");
    expect(formatLatency(null)).toBe("—");
  });

  it("成功率百分比", () => {
    expect(formatRate(probe({ ok_count: 3, total: 3 }))).toBe("100%");
    expect(formatRate(probe({ ok_count: 1, total: 3 }))).toBe("33%");
    expect(formatRate(probe({ ok_count: 0, total: 0 }))).toBe("0%");
  });
});

describe("speedStats", () => {
  it("优先后端计数", () => {
    expect(speedStats(result({ usable: 4, failed: 2 }))).toEqual({ usable: 4, failed: 2 });
  });

  it("字段缺失时自行统计", () => {
    const r = {
      ...result(),
      usable: undefined,
      failed: undefined,
      results: [
        probe({ server: "a", avg_ms: 10, ok_count: 3, total: 3 }),
        probe({ server: "b", avg_ms: null, ok_count: 0, total: 3 }),
      ],
    } as unknown as DnsSpeedResult;
    expect(speedStats(r)).toEqual({ usable: 1, failed: 1 });
  });

  it("null 返回全 0", () => {
    expect(speedStats(null)).toEqual({ usable: 0, failed: 0 });
  });
});

describe("recommendedProbe", () => {
  it("优先后端标记项", () => {
    const r = result({
      results: [
        probe({ server: "slow", avg_ms: 90 }),
        probe({ server: "fast", avg_ms: 10, recommended: true }),
      ],
    });
    expect(recommendedProbe(r)?.server).toBe("fast");
  });

  it("无标记时按最小延迟兜底", () => {
    const r = result({
      results: [probe({ server: "slow", avg_ms: 90 }), probe({ server: "fast", avg_ms: 10 })],
    });
    expect(recommendedProbe(r)?.server).toBe("fast");
  });

  it("全失败时无推荐", () => {
    const r = result({
      results: [probe({ avg_ms: null, ok_count: 0 }), probe({ server: "b", avg_ms: null, ok_count: 0 })],
    });
    expect(recommendedProbe(r)).toBeNull();
  });

  it("null 结果返回 null", () => {
    expect(recommendedProbe(null)).toBeNull();
  });
});

describe("下载测速", () => {
  it("WinInet 接入后恒可用", () => {
    expect(downloadSpeedAvailable({ available: true, urls: [] })).toBe(true);
    expect(downloadSpeedAvailable(null)).toBe(false);
  });

  it("异常兜底文案不为空", () => {
    expect(downloadSpeedNotice(null)).toContain("正在读取");
    expect(downloadSpeedNotice({ available: true, urls: [] })).toBe("");
  });

  it("formatBps 分级展示", () => {
    expect(formatBps(0)).toBe("0 B/s");
    expect(formatBps(2048)).toBe("2 KB/s");
    expect(formatBps(3 * 1024 * 1024)).toBe("3.0 MB/s");
  });

  it("formatBytes 同刻度但不带 /s（下载量是累计值不是速率）", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(2048)).toBe("2 KB");
    expect(formatBytes(3 * 1024 * 1024)).toBe("3.0 MB");
    // 与 formatBps 的差别正是单位后缀——回归用例防再次标错
    expect(formatBytes(2048)).not.toBe(formatBps(2048));
  });

  it("urlHost 取主机名，失败原文返回", () => {
    expect(urlHost("https://speed.cloudflare.com/__down?bytes=10000000")).toBe(
      "speed.cloudflare.com",
    );
    expect(urlHost("http://ipv4.speedtest.sjtu.edu.cn:8080/path/file")).toBe(
      "ipv4.speedtest.sjtu.edu.cn",
    );
    expect(urlHost("not a url")).toBe("not a url");
    expect(urlHost("")).toBe("");
  });

  it("peakToken 按量分级", () => {
    expect(peakToken(0)).toBe("sub");
    expect(peakToken(512 * 1024)).toBe("err");
    expect(peakToken(2 * 1024 * 1024)).toBe("warn");
    expect(peakToken(20 * 1024 * 1024)).toBe("ok");
  });
});


// ---------------- DNS 测速历史 ----------------

describe("dnsHistorySeries / dnsTrend", () => {
  const rounds: DnsSpeedRound[] = [
    { at: "t1", latencies: [["阿里", 20], ["腾讯", 30]] },
    { at: "t2", latencies: [["腾讯", 32]] },
    { at: "t3", latencies: [["阿里", 12], ["腾讯", 28]] },
    { at: "t4", latencies: [["阿里", 14], ["腾讯", 26]] },
    { at: "t5", latencies: [["阿里", 10], ["腾讯", 24]] },
  ];

  it("series 只含记录过该服务器的轮", () => {
    const ali = dnsHistorySeries(rounds, "阿里");
    expect(ali).toEqual([
      { at: "t1", ms: 20 },
      { at: "t3", ms: 12 },
      { at: "t4", ms: 14 },
      { at: "t5", ms: 10 },
    ]);
    expect(dnsHistorySeries(rounds, "无")).toEqual([]);
  });

  it("趋势：样本 <4 为 null；后半段明显变快给出负百分比", () => {
    expect(dnsTrend([{ at: "t", ms: 1 }])).toBeNull();
    const ali = dnsTrend(dnsHistorySeries(rounds, "阿里"));
    expect(ali?.label).toBe("变快");
    expect(ali!.pct).toBeLessThan(0);
    // 腾讯 30→(28+26)/2 也变快
    expect(dnsTrend(dnsHistorySeries(rounds, "腾讯"))?.label).toBe("变快");
  });

  it("±15% 内为持平", () => {
    const flat = Array.from({ length: 6 }, (_, i) => ({ at: `t${i}`, ms: 100 + (i % 2) }));
    expect(dnsTrend(flat)?.label).toBe("持平");
  });
});
