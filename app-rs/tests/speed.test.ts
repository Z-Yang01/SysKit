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
  formatLatency,
  formatRate,
  isUsable,
  latencyToken,
  rateToken,
  recommendedProbe,
  speedStats,
  successRate,
  type DnsProbe,
  type DnsSpeedResult,
  type SpeedtestStatus,
} from "../src/speed";

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

describe("下载测速诚实性", () => {
  const unavailable: SpeedtestStatus = {
    available: false,
    reason: "Rust 版尚无 HTTPS 下载能力",
    python_urls: ["https://example.com/a.iso"],
  };

  it("未接入时必须 false", () => {
    expect(downloadSpeedAvailable(unavailable)).toBe(false);
    expect(downloadSpeedAvailable(null)).toBe(false);
  });

  it("只有 available=true 才算可用", () => {
    expect(downloadSpeedAvailable({ available: true, reason: "", python_urls: [] })).toBe(true);
  });

  it("未接入时说明文案不得为空", () => {
    expect(downloadSpeedNotice(unavailable)).toBe("Rust 版尚无 HTTPS 下载能力");
    expect(downloadSpeedNotice(null)).toContain("正在读取");
    // reason 为空串时也要有兜底，不能显示空白
    expect(
      downloadSpeedNotice({ available: false, reason: "", python_urls: [] }),
    ).toContain("尚未接入");
  });

  it("可用时说明为空（界面不显示警告）", () => {
    expect(downloadSpeedNotice({ available: true, reason: "x", python_urls: [] })).toBe("");
  });
});
