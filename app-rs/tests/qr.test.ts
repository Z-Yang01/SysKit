/**
 * 二维码 data URL 构造（svgDataUrl）。
 *
 * 锁住三条底线：前缀正确、载荷可无损还原、XML 特殊字符必须被百分号编码
 * （否则 `<img src>` 的属性边界/URL 语义可能被载荷破坏）。
 */
import { describe, expect, it } from "vitest";

import { svgDataUrl } from "../src/qr";

describe("svgDataUrl", () => {
  it("产出 image/svg+xml data URL 前缀", () => {
    const url = svgDataUrl('<svg xmlns="http://www.w3.org/2000/svg"></svg>');
    expect(url.startsWith("data:image/svg+xml;charset=utf-8,")).toBe(true);
  });

  it("载荷可无损还原（round-trip）", () => {
    const svg = `<svg viewBox="0 0 33 33"><rect width="33" height="33" fill="#fff"/></svg>`;
    const url = svgDataUrl(svg);
    const payload = url.slice("data:image/svg+xml;charset=utf-8,".length);
    expect(decodeURIComponent(payload)).toBe(svg);
  });

  it("XML 特殊字符被百分号编码（不破坏 src 属性与 URL 语义）", () => {
    const url = svgDataUrl(`<svg a="1&2<3>4'5"6>`);
    expect(url).not.toMatch(/&(?!amp;|#|dash)/);
    expect(url).toContain("%26");
    expect(url).toContain("%3C");
    expect(url).not.toContain('"6');
    // 解码后仍是原文
    const payload = url.slice("data:image/svg+xml;charset=utf-8,".length);
    expect(decodeURIComponent(payload)).toBe(`<svg a="1&2<3>4'5"6>`);
  });

  it("空字符串也返回合法 data URL（失败路径的收起态不产生非法 src）", () => {
    expect(svgDataUrl("")).toBe("data:image/svg+xml;charset=utf-8,");
  });
});
