/**
 * 两栏页「行高填满」的回归守卫。
 *
 * 2026-10-20 的真实故障：hook 里写死 `querySelector(".ant-layout-content")`
 * 取滚动容器，而 v1.4 已移除 AntD——选择器永远匹配不到，measure() 直接
 * return，高度一直是 undefined，于是 WiFi 列表 30+ 项时整页（含标签栏）
 * 被撑开滚动。**这类失败不会报错、不会变红，只会表现为"滚动位置不对"**，
 * 所以用源码守卫 + 纯函数用例双重钉住。
 */
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

import { fillRowHeight } from "../src/useFillRow";

describe("fillRowHeight", () => {
  it("扣掉行顶偏移与底部留白", () => {
    expect(fillRowHeight(700, 100)).toBe(700 - 100 - 14);
  });

  it("极矮窗口有下限，不会算出 0 或负数", () => {
    expect(fillRowHeight(200, 180)).toBe(220);
    expect(fillRowHeight(0, 0)).toBe(220);
  });

  it("容器滚过以后行顶按内容区算（传入已修正的偏移）", () => {
    // 调用方保证 rowOffset 已加回 scrollTop，这里只验证数学关系
    expect(fillRowHeight(600, 0)).toBe(586);
  });
});

describe("useFillRow 不再依赖 AntD 类名（源码守卫）", () => {
  // 注释里会提到旧写法（解释为什么改），先剥掉注释再断言实际代码
  const src = readFileSync(resolve(__dirname, "../src/useFillRow.ts"), "utf-8")
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/^\s*\/\/.*$/gm, "");

  it("不得按 .ant-* 类名取容器", () => {
    expect(src).not.toMatch(/querySelector[^)]*\.ant-/);
  });

  it("必须按计算样式找滚动祖先", () => {
    expect(src).toContain("getComputedStyle");
    expect(src).toContain("overflowY");
  });
});
