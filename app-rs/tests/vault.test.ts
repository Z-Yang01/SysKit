/**
 * 密码库页纯逻辑测试。
 *
 * 最要紧的一组：**数据层未实现时，界面绝不能暗示"可以存密码"**。
 * 让用户填完表单再失败、或让空列表看起来像"还没有密码"，都是把
 * "功能没做"伪装成"暂无数据"——密码这种数据丢了就是丢了。
 */
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

import {
  canAddPassword,
  canSetMasterPassword,
  modeLabel,
  passwordStrength,
  setupConfirmText,
  statusSummary,
  storageAvailable,
  storageNotice,
  vaultPhase,
  type VaultStatus,
} from "../src/vault";

const status = (over: Partial<VaultStatus> = {}): VaultStatus => ({
  path: "C:\\Users\\x\\AppData\\Roaming\\SysKit\\vault.key",
  provisioned: true,
  unlocked: true,
  has_master_password: true,
  mode: "password",
  capabilities: {
    key_management: true,
    password_storage: false,
    password_storage_reason: "数据层未引入",
  },
  ...over,
});

const withStorage = (): VaultStatus =>
  status({
    capabilities: { key_management: true, password_storage: true, password_storage_reason: "" },
  });

describe("vaultPhase", () => {
  it("未初始化 / 已锁定 / 已解锁", () => {
    expect(vaultPhase(status({ provisioned: false }))).toBe("unprovisioned");
    expect(vaultPhase(status({ unlocked: false }))).toBe("locked");
    expect(vaultPhase(status({ unlocked: true }))).toBe("unlocked");
  });

  it("null 视为未初始化（不假装已就绪）", () => {
    expect(vaultPhase(null)).toBe("unprovisioned");
  });
});

describe("modeLabel", () => {
  it("两种模式的展示名", () => {
    expect(modeLabel("password")).toContain("主密码");
    expect(modeLabel("dpapi")).toContain("DPAPI");
    expect(modeLabel("")).toContain("尚未初始化");
    expect(modeLabel("weird")).toBe("weird");
  });
});

describe("statusSummary", () => {
  it("含模式与锁定状态", () => {
    const s = statusSummary(status());
    expect(s).toContain("主密码");
    expect(s).toContain("已解锁");
  });

  it("未创建时明确说明", () => {
    expect(statusSummary(status({ provisioned: false }))).toContain("尚未创建");
  });

  it("null 时明确说明", () => {
    expect(statusSummary(null)).toBe("尚未读取");
  });
});

describe("storageAvailable 诚实性", () => {
  it("后端说不可用就是不可用", () => {
    expect(storageAvailable(status())).toBe(false);
  });

  it("仅当后端明确为 true 才算可用", () => {
    expect(storageAvailable(withStorage())).toBe(true);
    expect(storageAvailable(null)).toBe(false);
  });

  it("capabilities 缺失时按不可用（保守方向）", () => {
    const broken = { ...status(), capabilities: undefined } as unknown as VaultStatus;
    expect(storageAvailable(broken)).toBe(false);
  });
});

describe("storageNotice", () => {
  it("不可用时不得为空（否则界面显示空白）", () => {
    expect(storageNotice(status())).toBe("数据层未引入");
    // reason 为空串时也要有兜底
    const empty = status({
      capabilities: { key_management: true, password_storage: false, password_storage_reason: "" },
    });
    expect(storageNotice(empty).length).toBeGreaterThan(0);
    expect(storageNotice(empty)).toContain("尚未实现");
  });

  it("可用时为空（不显示警告）", () => {
    expect(storageNotice(withStorage())).toBe("");
  });

  it("null 时说明正在读取", () => {
    expect(storageNotice(null)).toContain("正在读取");
  });
});

describe("canAddPassword", () => {
  it("数据层不可用时禁用，并说明原因", () => {
    const c = canAddPassword(status());
    expect(c.ok).toBe(false);
    expect(c.reason).toBeTruthy();
  });

  it("即使已解锁且数据层可用，未解锁也要拦", () => {
    const c = canAddPassword(withStorage());
    expect(c.ok).toBe(true);
    expect(canAddPassword({ ...withStorage(), unlocked: false }).ok).toBe(false);
  });

  it("未解锁且数据层可用时报「请先解锁」", () => {
    const c = canAddPassword({ ...withStorage(), unlocked: false });
    expect(c.ok).toBe(false);
    expect(c.reason).toContain("解锁");
  });

  it("数据层不可用时优先报「存取未实现」（不能说「先解锁」）", () => {
    // 说「请先解锁」会让用户以为解锁后就能存密码——解完发现还是不行
    const c = canAddPassword(status({ unlocked: false }));
    expect(c.reason).not.toContain("请先解锁");
    expect(c.reason).toBeTruthy();
  });

  it("null 时禁用", () => {
    expect(canAddPassword(null).ok).toBe(false);
  });
});

describe("canSetMasterPassword", () => {
  it("免密模式且已解锁时可用", () => {
    const s = status({ has_master_password: false, mode: "dpapi" });
    expect(canSetMasterPassword(s).ok).toBe(true);
  });

  it("已是主密码模式时引导改用「修改主密码」", () => {
    const c = canSetMasterPassword(status({ has_master_password: true }));
    expect(c.ok).toBe(false);
    expect(c.reason).toContain("修改主密码");
  });

  it("未解锁时不可用", () => {
    const c = canSetMasterPassword(status({ has_master_password: false, unlocked: false }));
    expect(c.ok).toBe(false);
    expect(c.reason).toContain("解锁");
  });
});

describe("passwordStrength", () => {
  it("短密码一律 weak 并说明主密码不可找回", () => {
    const s = passwordStrength("abc");
    expect(s.level).toBe("weak");
    expect(s.hint).toContain("无法找回");
  });

  it("长且混合多种字符 → good", () => {
    expect(passwordStrength("Tr0ub4dor&3xyz").level).toBe("good");
  });

  it("够长但字符种类少 → fair（12 位纯小写强于 8 位混合，不该判 weak）", () => {
    expect(passwordStrength("abcdefghijkl").level).toBe("fair");
  });

  it("中等长度多字符 → fair", () => {
    expect(passwordStrength("Abc12345").level).toBe("fair");
  });

  it("提示文案不为空", () => {
    for (const pw of ["", "a", "abcdefgh", "Ab1!", "Tr0ub4dor&3xyz"]) {
      expect(passwordStrength(pw).hint.length).toBeGreaterThan(0);
    }
  });
});

describe("setupConfirmText", () => {
  it("免密模式必须说明绑定当前用户与换机器解不开", () => {
    const t = setupConfirmText("dpapi");
    expect(t).toContain("DPAPI");
    expect(t).toContain("当前 Windows 用户");
    expect(t).toContain("换机器");
  });

  it("主密码模式必须说明无法找回", () => {
    const t = setupConfirmText("password");
    expect(t).toContain("无法找回");
    expect(t).toContain("PBKDF2");
    expect(t).toContain("不落盘");
  });
});

/**
 * 敏感复制纪律（源码守卫，Node 侧读源码——契约审计 contract.test.ts 同路）。
 *
 * 后端注释里写明的纪律：密码类内容只能经由 sk_clip_copy_sensitive 进入
 * 剪贴板（不进本应用历史 / 不进 Win+V 与云剪贴板 / 到期清空）。TOTP 动态码
 * 是一次性凭证，Python 版 _copy_totp 走 _copy_sensitive 同一条路；此前 Rust
 * 前端把它接到 sk_clip_copy_plain，动态码会被监听器记进历史——这里锁死。
 */
describe("TOTP 复制走敏感通道（源码守卫）", () => {
  // 规范化换行后再切：git autocrlf 可能落成 CRLF，直接找 \n 会静默失配
  const src = readFileSync(resolve(__dirname, "../src/VaultPage.tsx"), "utf-8").replace(
    /\r\n/g,
    "\n",
  );
  const start = src.indexOf("const showTotp");
  // showTotp 回调体：从定义处到顶层收尾「  );」（useCallback 的收尾）
  const region = src.slice(start, src.indexOf("\n  );", start));

  it("showTotp 区块存在且动态码复制按钮调 sk_clip_copy_sensitive", () => {
    expect(start).toBeGreaterThan(-1);
    expect(region).toContain("sk_clip_copy_sensitive");
  });

  it("动态码路径不得出现 sk_clip_copy_plain（明文复制会让动态码进历史）", () => {
    expect(region).not.toContain("sk_clip_copy_plain");
  });
});
