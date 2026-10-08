/**
 * otpauth:// 解析测试：标准 GitHub/Google 迁移码形状、参数缺省、
 * 非默认参数警示清单、hotp 拒绝、畸形容错。
 */
import { describe, expect, it } from "vitest";

import { nonDefaultParams, parseOtpauth } from "../src/otpauth";

describe("parseOtpauth", () => {
  it("标准 GitHub 形状：secret/issuer 参数齐全", () => {
    const u =
      "otpauth://totp/GitHub:alice?secret=JBSWY3DPEHPK3PXP&issuer=GitHub";
    const r = parseOtpauth(u)!;
    expect(r.secret).toBe("JBSWY3DPEHPK3PXP");
    expect(r.issuer).toBe("GitHub");
    expect(r.label).toBe("GitHub:alice");
    expect(r.algorithm).toBe("SHA1"); // 缺省
    expect(r.digits).toBe(6);
    expect(r.period).toBe(30);
  });

  it("issuer 缺失时从 label 前缀取；URL 编码还原", () => {
    const u = "otpauth://totp/My%20Corp:alice%40x.com?secret=JBSWY3DPEHPK3PXP";
    const r = parseOtpauth(u)!;
    expect(r.issuer).toBe("My Corp");
    expect(r.label).toBe("My Corp:alice@x.com");
  });

  it("非默认参数透出（algorithm/digits/period）", () => {
    const u =
      "otpauth://totp/x?secret=JBSWY3DPEHPK3PXP&algorithm=SHA256&digits=8&period=60";
    const r = parseOtpauth(u)!;
    expect(r.algorithm).toBe("SHA256");
    expect(r.digits).toBe(8);
    expect(r.period).toBe(60);
    expect(nonDefaultParams(r)).toEqual(["算法 SHA256", "位数 8", "周期 60s"]);
  });

  it("默认参数 nonDefaultParams 为空", () => {
    const r = parseOtpauth("otpauth://totp/a?secret=JBSWY3DPEHPK3PXP")!;
    expect(nonDefaultParams(r)).toEqual([]);
  });

  it("secret 容忍空格与连字符（粘贴形态）并大写化", () => {
    const r = parseOtpauth("otpauth://totp/a?secret=jbsw y3dp-ehpk 3pxp")!;
    expect(r.secret).toBe("JBSWY3DPEHPK3PXP");
  });

  it("hotp / 非 otpauth / 缺 secret 返回 null", () => {
    expect(parseOtpauth("otpauth://hotp/a?secret=X&counter=1")).toBeNull();
    expect(parseOtpauth("https://example.com")).toBeNull();
    expect(parseOtpauth("otpauth://totp/a?issuer=x")).toBeNull();
    expect(parseOtpauth("")).toBeNull();
  });
});
