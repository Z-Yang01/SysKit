/**
 * WiFi 密码查看逻辑测试。
 *
 * 重点是**纪律**：密码必须经敏感路径复制；以及提示文案必须如实转述保护强度
 * （用户看到"25s 后自动清空"才知道密码不会一直留在剪贴板里）。
 */
import { describe, expect, it } from "vitest";

import {
  copyResultMessage,
  maskPassword,
  needsAdminHint,
  passwordErrorHint,
  requiresSensitiveCopy,
  SENSITIVE_COPY_COMMAND,
  strengthHint,
} from "../src/wifipassword";

describe("敏感复制纪律", () => {
  it("密码必须以敏感路径复制", () => {
    expect(requiresSensitiveCopy("password")).toBe(true);
  });

  it("普通文本走普通路径", () => {
    expect(requiresSensitiveCopy("plain")).toBe(false);
  });

  it("命令名集中定义（守卫测试与实现引用同一来源）", () => {
    // 拼错命令名会静默走到普通复制，密码就漏了三重保护
    expect(SENSITIVE_COPY_COMMAND).toBe("sk_clip_copy_sensitive");
  });
});

describe("strengthHint 本地判断", () => {
  it("无密码", () => {
    const h = strengthHint("");
    expect(h.score).toBe(0);
    expect(h.advice).toContain("开放网络");
  });

  it("短密码判弱并给长度建议", () => {
    const h = strengthHint("abc123");
    expect(h.score).toBeLessThanOrEqual(2);
    expect(h.advice).toContain("8 位");
  });

  it("长且混合的密码判强", () => {
    const h = strengthHint("Tr0ub4dor&3xyz");
    expect(h.score).toBe(4);
    expect(h.label).toBe("很强");
  });

  it("只有长度没有字符类是中等", () => {
    const h = strengthHint("aaaaaaaaaaaa");
    expect(h.score).toBe(2);
    expect(h.advice).toContain("大小写");
  });

  it("中文密码按长度与字符类照常评估（不崩溃）", () => {
    const h = strengthHint("我的密码很长的中文密码");
    expect(h.score).toBeGreaterThanOrEqual(0);
    expect(h.label).toBeTruthy();
  });

  it("分数上限为 4", () => {
    expect(strengthHint("Aa1!Bb2@Cc3#Dd4$Ee5%").score).toBe(4);
  });
});

describe("错误提示", () => {
  it("识别管理员权限问题并给出可操作建议", () => {
    const err = "读取明文密钥失败——通常需要以管理员身份运行（netsh key=clear 接口限制）";
    expect(needsAdminHint(err)).toBe(true);
    const hint = passwordErrorHint(err);
    expect(hint).toContain("管理员");
    expect(hint).toContain("不是程序缺陷");
  });

  it("找不到配置文件时说明可能已删除", () => {
    expect(passwordErrorHint("找不到配置文件 MyWiFi")).toContain("已被删除");
  });

  it("其它错误原样转述（不吞掉信息）", () => {
    expect(passwordErrorHint("netsh 无法执行: 超时")).toBe("netsh 无法执行: 超时");
  });

  it("空错误给兜底文案", () => {
    expect(passwordErrorHint("")).toBe("读取失败");
  });
});

describe("copyResultMessage", () => {
  it("如实转述清空时间（用户据此判断保护强度）", () => {
    expect(copyResultMessage({ ttl: 25, note: "25s 后自动清空剪贴板" })).toContain("25s");
  });

  it("不清空时也要给出说明", () => {
    const m = copyResultMessage({ ttl: 0, note: "不会自动清空（敏感内容仍不进历史与系统剪贴板历史）" });
    expect(m).toContain("不会自动清空");
  });

  it("后端没给文案时按 TTL 兜底", () => {
    expect(copyResultMessage({ ttl: 30, note: "" })).toContain("30s");
    expect(copyResultMessage({ ttl: 0, note: "" })).toContain("已复制");
  });
});

describe("maskPassword", () => {
  it("默认遮罩，不直接显示明文", () => {
    const m = maskPassword("secret123", false);
    expect(m).not.toContain("secret");
    expect(m).toContain("•");
  });

  it("长度影响遮罩长度但有上下限", () => {
    expect(maskPassword("ab", false).length).toBe(8);
    expect(maskPassword("x".repeat(100), false).length).toBe(16);
  });

  it("显式请求时显示明文", () => {
    expect(maskPassword("secret123", true)).toBe("secret123");
  });

  it("空密码给出明确文案", () => {
    expect(maskPassword("", false)).toBe("（无密钥）");
  });
});
