/**
 * 二维码识别结果纯逻辑测试：WIFI: 解析（转义还原/大小写/无效输入）与
 * URL 判定。与 Rust qrdecode.rs 的 round-trip 测试互补——那边锁解码器，
 * 这里锁前端展示分支的输入分类。
 */
import { describe, expect, it } from "vitest";

import { isHttpUrl, parseWifiQr } from "../src/qrdecode";

describe("parseWifiQr", () => {
  it("标准 WPA 码：转义字段还原（; : , \" 均可）", () => {
    const w = parseWifiQr("WIFI:T:WPA;S:Home\\;5G;P:p@ss\\:word;;");
    expect(w).toEqual({
      auth: "WPA",
      ssid: "Home;5G",
      password: "p@ss:word",
      hidden: false,
    });
  });

  it("nopass + 隐藏标记 + 小写键名", () => {
    const w = parseWifiQr("WIFI:t:nopass;s:Office;h:true;;");
    expect(w).not.toBeNull();
    expect(w!.ssid).toBe("Office");
    expect(w!.password).toBe("");
    expect(w!.hidden).toBe(true);
  });

  it("非 WIFI 串 / 缺 SSID 返回 null", () => {
    expect(parseWifiQr("https://example.com")).toBeNull();
    expect(parseWifiQr("WIFI:T:WPA;;")).toBeNull();
    expect(parseWifiQr("")).toBeNull();
  });
});

describe("isHttpUrl", () => {
  it("只放行 http/https（大小写不敏感），其他协议不算链接", () => {
    expect(isHttpUrl("https://example.com/a?b=1")).toBe(true);
    expect(isHttpUrl("HTTP://EXAMPLE.COM")).toBe(true);
    expect(isHttpUrl("ftp://example.com")).toBe(false);
    expect(isHttpUrl("file:///C:/x")).toBe(false);
    expect(isHttpUrl("WIFI:T:WPA;;")).toBe(false);
    expect(isHttpUrl("  https://space.com  ")).toBe(true);
  });
});
