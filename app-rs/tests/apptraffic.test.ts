/**
 * 按应用流量纯逻辑测试：Top 截断与占比计算（后端 summary 已降序，
 * 这里锁前端展示口径）。
 */
import { describe, expect, it } from "vitest";

import { sharePct, topRows, type AppUsageRow } from "../src/apptraffic";

const row = (app: string, sent: number, recv: number): AppUsageRow => ({
  app,
  sent,
  recv,
});

describe("topRows", () => {
  it("截断 Top N，limit 下限 1", () => {
    const rows = [row("a", 1, 1), row("b", 2, 2), row("c", 3, 3)];
    expect(topRows(rows, 2)).toHaveLength(2);
    expect(topRows(rows, 0)).toHaveLength(1); // 下限 1
    expect(topRows([], 5)).toHaveLength(0);
  });
});

describe("sharePct", () => {
  it("按 sent+recv 合计算占比", () => {
    const r = row("a", 30, 30);
    // 全表 100（a 60 + 其他 40）
    expect(sharePct(r, 40, 60)).toBe(60);
  });

  it("合计 0 夹 0（不出 NaN）", () => {
    expect(sharePct(row("a", 1, 1), 0, 0)).toBe(0);
  });

  it("占比封顶 100（防御性：单行超出总数时不越界）", () => {
    expect(sharePct(row("a", 999, 999), 1, 1)).toBe(100);
  });
});
