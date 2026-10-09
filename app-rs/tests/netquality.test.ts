/**
 * 网络质量序列纯逻辑测试：丢包率派生与均值口径。
 * 后端 series 已把 loss%/avg 算好（null=无样本），前端只做展示聚合。
 */
import { describe, expect, it } from "vitest";

import { avgDnsMs, avgLatency, avgLossPct, type QualityRow } from "../src/netquality";

const row = (over: Partial<QualityRow>): QualityRow => ({
  key: "00:00",
  gw_avg_ms: null,
  gw_loss_pct: null,
  pub_avg_ms: null,
  pub_loss_pct: null,
  dns_ms: null,
  ...over,
});

describe("网络质量聚合（netquality.ts）", () => {
  it("lossPct：null 无样本按 0；有限值原样", () => {
    expect(avgLossPct([row({ gw_loss_pct: 33.3 })], "gw")).toBeCloseTo(33.3);
    expect(avgLossPct([row({})], "gw")).toBe(0);
  });

  it("avgLossPct 多桶取均值", () => {
    const rows = [row({ gw_loss_pct: 10 }), row({ gw_loss_pct: 30 })];
    expect(avgLossPct(rows, "gw")).toBeCloseTo(20);
  });

  it("avgLatency 忽略 null（全丢包桶无延迟样本）", () => {
    const rows = [
      row({ gw_avg_ms: 2.0, pub_avg_ms: 10 }),
      row({ gw_avg_ms: 4.0, pub_avg_ms: 20 }),
      row({}), // 全丢包桶
    ];
    expect(avgLatency(rows, "gw")).toBeCloseTo(3.0);
    expect(avgLatency(rows, "pub")).toBeCloseTo(15.0);
    expect(avgDnsMs([row({ dns_ms: 6 }), row({})])).toBeCloseTo(6);
  });
});
