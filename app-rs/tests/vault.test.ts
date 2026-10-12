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
  PWGEN_AMBIGUOUS,
  PWGEN_DEFAULT_LENGTH,
  PWGEN_DIGITS,
  PWGEN_LOWER,
  PWGEN_MAX_LENGTH,
  PWGEN_MIN_LENGTH,
  PWGEN_SYMBOLS,
  PWGEN_UPPER,
  pwgenAlphabetSize,
  pwgenEntropyBits,
  pwgenMeterPercent,
  pwgenTier,
  pwgenUsable,
  setupConfirmText,
  statusSummary,
  storageAvailable,
  storageNotice,
  vaultPhase,
  type PwgenOptions,
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

// ---------------- 密码生成器（Rust pwgen.rs 的前端镜像 + 接线守卫） ----------------

const genOpts = (over: Partial<PwgenOptions> = {}): PwgenOptions => ({
  length: 16,
  upper: true,
  lower: true,
  digits: true,
  symbols: true,
  exclude_ambiguous: false,
  ...over,
});

describe("pwgen 纯逻辑（字母表 / 熵 / 分档）", () => {
  it("字母表大小：全开 94，排除易混淆后 85（24+25+8+28）", () => {
    expect(pwgenAlphabetSize(genOpts())).toBe(94);
    expect(pwgenAlphabetSize(genOpts({ exclude_ambiguous: true }))).toBe(85);
  });

  it("单类大小与排除效果：大写 26→24、小写 26→25、数字 10→8、符号 32→28", () => {
    const one = (o: Partial<PwgenOptions>) => pwgenAlphabetSize(genOpts({ upper: false, lower: false, digits: false, symbols: false, ...o }));
    expect(one({ upper: true })).toBe(26);
    expect(one({ upper: true, exclude_ambiguous: true })).toBe(24);
    expect(one({ lower: true })).toBe(26);
    expect(one({ lower: true, exclude_ambiguous: true })).toBe(25);
    expect(one({ digits: true })).toBe(10);
    expect(one({ digits: true, exclude_ambiguous: true })).toBe(8);
    expect(one({ symbols: true })).toBe(32);
    expect(one({ symbols: true, exclude_ambiguous: true })).toBe(28);
  });

  it("熵 = 长度 × log2(字符集大小)；全关闭 → 0", () => {
    expect(pwgenEntropyBits(genOpts())).toBeCloseTo(16 * Math.log2(94), 2);
    expect(pwgenEntropyBits(genOpts({ length: 8, symbols: false }))).toBeCloseTo(
      8 * Math.log2(62),
      2,
    );
    expect(
      pwgenEntropyBits(genOpts({ upper: false, lower: false, digits: false, symbols: false })),
    ).toBe(0);
  });

  it("分档：＜50 弱，50~79 中，≥80 强（边界值钉死）", () => {
    expect(pwgenTier(49.9).label).toBe("弱");
    expect(pwgenTier(50).label).toBe("中");
    expect(pwgenTier(79.9).label).toBe("中");
    expect(pwgenTier(80).label).toBe("强");
    expect(pwgenTier(0).level).toBe("weak");
  });

  it("强度条百分比：熵 128 位封顶，负值钳到 0", () => {
    expect(pwgenMeterPercent(128)).toBe(100);
    expect(pwgenMeterPercent(400)).toBe(100);
    expect(pwgenMeterPercent(64)).toBe(50);
    expect(pwgenMeterPercent(0)).toBe(0);
    expect(pwgenMeterPercent(-5)).toBe(0);
  });

  it("可用性预检：全关闭报原因；区间外报原因；正常通过", () => {
    const none = pwgenUsable(genOpts({ upper: false, lower: false, digits: false, symbols: false }));
    expect(none.ok).toBe(false);
    expect(none.reason).toContain("至少勾选一类");
    const short = pwgenUsable(genOpts({ length: PWGEN_MIN_LENGTH - 1 }));
    expect(short.ok).toBe(false);
    expect(short.reason).toContain("长度");
    const long = pwgenUsable(genOpts({ length: PWGEN_MAX_LENGTH + 1 }));
    expect(long.ok).toBe(false);
    expect(pwgenUsable(genOpts({ length: PWGEN_DEFAULT_LENGTH })).ok).toBe(true);
  });
});

/**
 * 生成器接线守卫（源码守卫，与 TOTP 敏感通道守卫同路）。
 *
 * 1. 表单「生成」必须调 sk_vault_generate_password 并把结果填进 vf-acc-password；
 * 2. 未保存的生成密码复制**只能**走 sk_clip_copy_sensitive——它尚未入库，
 *    但已经是凭据材料，暴露窗口与已保存密码一致，降级到明文通道没有任何
 *    理由（TOTP 误接明文通道的同款事故，这里先锁死）。
 */
describe("生成面板接线（源码守卫）", () => {
  const pageSrc = readFileSync(resolve(__dirname, "../src/VaultPage.tsx"), "utf-8").replace(
    /\r\n/g,
    "\n",
  );
  const panelStart = pageSrc.indexOf("function PasswordGenPanel");
  // 面板区域：定义处到 VaultPage 组件（模块内紧随其后）
  const panel = pageSrc.slice(panelStart, pageSrc.indexOf("export function VaultPage"));
  const accStart = pageSrc.indexOf("const accountModal");
  // accountModal 回调体：从定义处到顶层收尾「  );」（useCallback 的收尾）
  const accRegion = pageSrc.slice(accStart, pageSrc.indexOf("\n  );", accStart));

  it("面板区域存在且生成按钮调 sk_vault_generate_password（六个参数齐全）", () => {
    expect(panelStart).toBeGreaterThan(-1);
    expect(panel).toContain("sk_vault_generate_password");
    for (const key of ["length", "upper", "lower", "digits", "symbols", "excludeAmbiguous"]) {
      expect(panel).toContain(key);
    }
  });

  it("面板复制走敏感通道：出现 sk_clip_copy_sensitive，且绝不出现 sk_clip_copy_plain", () => {
    expect(panel).toContain("sk_clip_copy_sensitive");
    expect(panel).not.toContain("sk_clip_copy_plain");
  });

  it("表单接线：生成回调把结果写进受控 state（v1.4 迁移后不再走 DOM 同步）", () => {
    // accountModal 只负责开模态（setAccountForm 初始化）；生成面板在模态 JSX 里
    expect(accRegion).toContain("setAccountForm");
    // 生成面板挂在模态 JSX 中，生成结果经 onGenerated 落到受控 password 字段
    expect(pageSrc).toContain("PasswordGenPanel");
    expect(pageSrc).toContain('onGenerated={(pw) => setAccountForm((f) => (f ? { ...f, password: pw } : f))}');
  });
});

/**
 * 双端字符集 parity（读 pwgen.rs 的 b"..." 字面量与 vault.ts 常量比对）。
 *
 * 前端熵 = 长度 × log2(字符集大小)，字符集大小必须与后端实际生成用的字母表
 * 逐字一致——任何一侧单独扩/缩字符集，强度条就开始撒谎。这里直接读 Rust 源
 * 里的字面量做唯一真相比对（contract.test.ts 读 lib.rs 注册清单同路）。
 */
describe("pwgen 字符集双端 parity（源码守卫）", () => {
  const rs = readFileSync(
    resolve(__dirname, "../src-tauri/src/vault/pwgen.rs"),
    "utf-8",
  ).replace(/\r\n/g, "\n");

  /** Rust 字节串字面量 → 原文（处理 \\ \" \' \n \t \0）。 */
  function rustUnescape(s: string): string {
    let out = "";
    for (let i = 0; i < s.length; i++) {
      if (s[i] !== "\\") {
        out += s[i];
        continue;
      }
      const c = s[i + 1] ?? "";
      out += c === "n" ? "\n" : c === "t" ? "\t" : c === "0" ? "\0" : c;
      i++;
    }
    return out;
  }

  function rustConst(name: string): string {
    const m = rs.match(
      new RegExp(`(?:pub )?const ${name}: &\\[u8\\] = b"((?:[^"\\\\]|\\\\.)*)";`),
    );
    expect(m, `pwgen.rs 应声明 ${name} 字面量`).toBeTruthy();
    return rustUnescape(m![1]);
  }

  const rsSets = {
    UPPER: rustConst("UPPER"),
    LOWER: rustConst("LOWER"),
    DIGITS: rustConst("DIGITS"),
    SYMBOLS: rustConst("SYMBOLS"),
    AMBIGUOUS: rustConst("AMBIGUOUS"),
  };

  it("五组字符集字面量逐字一致（大小写/数字/符号/易混淆）", () => {
    expect(rsSets.UPPER).toBe(PWGEN_UPPER);
    expect(rsSets.LOWER).toBe(PWGEN_LOWER);
    expect(rsSets.DIGITS).toBe(PWGEN_DIGITS);
    expect(rsSets.SYMBOLS).toBe(PWGEN_SYMBOLS);
    expect(rsSets.AMBIGUOUS).toBe(PWGEN_AMBIGUOUS);
  });

  it("前端字母表大小与 Rust 字面量推导值一致（排除前后各一档）", () => {
    const filtered = (set: string, ex: boolean) =>
      ex ? [...set].filter((c) => !rsSets.AMBIGUOUS.includes(c)).length : set.length;
    const rustSize = (o: PwgenOptions) =>
      (o.upper ? filtered(rsSets.UPPER, o.exclude_ambiguous) : 0) +
      (o.lower ? filtered(rsSets.LOWER, o.exclude_ambiguous) : 0) +
      (o.digits ? filtered(rsSets.DIGITS, o.exclude_ambiguous) : 0) +
      (o.symbols ? filtered(rsSets.SYMBOLS, o.exclude_ambiguous) : 0);
    const combos: PwgenOptions[] = [
      genOpts(),
      genOpts({ exclude_ambiguous: true }),
      genOpts({ symbols: false }),
      genOpts({ lower: true, upper: false, digits: false, symbols: false }),
      genOpts({ digits: true, upper: false, lower: false, symbols: false, exclude_ambiguous: true }),
    ];
    for (const o of combos) {
      expect(pwgenAlphabetSize(o), JSON.stringify(o)).toBe(rustSize(o));
    }
    // 长度常量同样双端一致（前端文案/滑杆区间与后端 validate 同口径）
    expect(rs).toContain(`pub const DEFAULT_LENGTH: usize = ${PWGEN_DEFAULT_LENGTH};`);
    expect(rs).toContain(`pub const MIN_LENGTH: usize = ${PWGEN_MIN_LENGTH};`);
    expect(rs).toContain(`pub const MAX_LENGTH: usize = ${PWGEN_MAX_LENGTH};`);
  });
});
