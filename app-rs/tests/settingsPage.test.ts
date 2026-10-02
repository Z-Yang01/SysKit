/**
 * 设置 / 日志页纯逻辑测试。
 *
 * 重点是**前端校验与后端同口径**：两边规则不一致会出现"界面说可以、后端拒绝"
 * 或更糟的"界面拦住了后端其实允许的值"。另：重启生效必须标注。
 */
import { describe, expect, it } from "vitest";

import {
  changedCount,
  displayValue,
  formatLogTime,
  groupKey,
  groupSettings,
  humanLogSize,
  isChanged,
  logLevel,
  restartNote,
  tailSummary,
  validateValue,
  type LogTail,
  type SettingItem,
} from "../src/settingsPage";

const item = (over: Partial<SettingItem> = {}): SettingItem => ({
  key: "poll_interval_sec",
  label: "检测间隔（秒）",
  description: "后台自动诊断的间隔",
  ty: "int",
  default: 30,
  current: 30,
  min: 10,
  max: 3600,
  needs_restart: false,
  ...over,
});

describe("validateValue 与后端同口径", () => {
  it("整数：范围内外", () => {
    const it = item();
    expect(validateValue(it, 10).ok).toBe(true);
    expect(validateValue(it, 3600).ok).toBe(true);
    expect(validateValue(it, 9).ok).toBe(false);
    expect(validateValue(it, 3601).ok).toBe(false);
    expect(validateValue(it, 9).reason).toContain("不能小于 10");
    expect(validateValue(it, 3601).reason).toContain("不能大于 3600");
  });

  it("整数：数字字符串也接受（后端会收敛）", () => {
    const v = validateValue(item(), "120");
    expect(v.ok).toBe(true);
    expect(v.coerced).toBe(120);
  });

  it("整数：小数与非数字被拒", () => {
    expect(validateValue(item(), 10.5).ok).toBe(false);
    expect(validateValue(item(), "abc").ok).toBe(false);
    expect(validateValue(item(), "abc").reason).toContain("整数");
  });

  it("0 在允许为 0 的项上通过", () => {
    const it = item({ key: "clipboard_retain_days", min: 0, max: 3650 });
    expect(validateValue(it, 0).ok).toBe(true);
  });

  it("布尔：只认 bool 与 0/1（与后端 coerce 一致，不认字符串 true）", () => {
    const it = item({ ty: "bool", default: false, current: false, min: undefined, max: undefined });
    expect(validateValue(it, true).ok).toBe(true);
    expect(validateValue(it, 1).ok).toBe(true);
    expect(validateValue(it, 1).coerced).toBe(true);
    expect(validateValue(it, 0).coerced).toBe(false);
    const bad = validateValue(it, "true");
    expect(bad.ok).toBe(false);
    expect(bad.reason).toContain("0/1");
  });

  it("枚举：只接受声明的选项", () => {
    const it = item({
      key: "dns_preset",
      ty: "str",
      options: ["阿里", "腾讯", "114", "360"],
      min: undefined,
      max: undefined,
      default: "阿里",
      current: "阿里",
    });
    expect(validateValue(it, "腾讯").ok).toBe(true);
    const bad = validateValue(it, "不存在");
    expect(bad.ok).toBe(false);
    expect(bad.reason).toContain("只能是");
  });

  it("普通字符串：任意值通过", () => {
    const it = item({ ty: "str", options: undefined, min: undefined, max: undefined });
    expect(validateValue(it, "任意文本").ok).toBe(true);
    expect(validateValue(it, "").ok).toBe(true);
  });
});

describe("isChanged / changedCount", () => {
  it("与默认值比较", () => {
    expect(isChanged(item())).toBe(false);
    expect(isChanged(item({ current: 120 }))).toBe(true);
  });

  it("计数", () => {
    const items = [item(), item({ key: "b", current: 5 }), item({ key: "c", current: 9 })];
    expect(changedCount(items)).toBe(2);
    expect(changedCount([])).toBe(0);
  });

  it("对象/数组值也能比较（不因引用不同而误判）", () => {
    const a = item({ ty: "str", default: ["x"], current: ["x"] });
    expect(isChanged(a)).toBe(false);
    const b = item({ ty: "str", default: ["x"], current: ["y"] });
    expect(isChanged(b)).toBe(true);
  });
});

describe("groupKey", () => {
  it("按前缀分组", () => {
    expect(groupKey("clipboard_max")).toBe("剪贴板");
    expect(groupKey("clip_sensitive_ttl")).toBe("剪贴板");
    expect(groupKey("vault_autolock_min")).toBe("密码库");
    expect(groupKey("palette_hotkey")).toBe("命令面板");
    expect(groupKey("poll_interval_sec")).toBe("自动修复");
    expect(groupKey("tray_rate_enabled")).toBe("界面与托盘");
    expect(groupKey("dns_preset")).toBe("网络");
    expect(groupKey("migrated_v2")).toBe("通用");
  });
});

describe("groupSettings", () => {
  it("保持后端给出的键顺序，分组内顺序不变", () => {
    const items = [
      item({ key: "poll_interval_sec" }),
      item({ key: "dark_mode" }),
      item({ key: "fail_threshold" }),
    ];
    const g = groupSettings(items);
    expect(g.map((x) => x.group)).toEqual(["自动修复", "界面与托盘"]);
    expect(g[0].items.map((i) => i.key)).toEqual(["poll_interval_sec", "fail_threshold"]);
  });

  it("空列表返回空", () => {
    expect(groupSettings([])).toEqual([]);
  });

  it("所有项都会被分到某个组（不丢项）", () => {
    const items = [item({ key: "a" }), item({ key: "clip_x" }), item({ key: "vault_y" })];
    const total = groupSettings(items).reduce((n, g) => n + g.items.length, 0);
    expect(total).toBe(items.length);
  });
});

describe("displayValue", () => {
  it("布尔显示开/关", () => {
    expect(displayValue(item({ ty: "bool", current: true }))).toBe("开");
    expect(displayValue(item({ ty: "bool", current: false }))).toBe("关");
  });

  it("长字符串截断（避免撑破表格）", () => {
    const long = "x".repeat(200);
    const s = displayValue(item({ ty: "str", current: long }));
    expect(s.length).toBeLessThanOrEqual(60);
    expect(s.endsWith("…")).toBe(true);
  });

  it("短字符串原样", () => {
    expect(displayValue(item({ ty: "str", current: "阿里", options: ["阿里"] }))).toBe("阿里");
  });
});

describe("restartNote", () => {
  it("需要重启时给出标注（否则用户以为没生效是 bug）", () => {
    expect(restartNote(item({ needs_restart: true }))).toBe("重启后生效");
    expect(restartNote(item({ needs_restart: false }))).toBe("");
  });
});

describe("日志展示", () => {
  it("humanLogSize 三档", () => {
    expect(humanLogSize(512)).toBe("512 B");
    expect(humanLogSize(2048)).toBe("2.0 KB");
    expect(humanLogSize(3 * 1024 * 1024)).toBe("3.0 MB");
  });

  it("formatLogTime 本地时间，非法值为「—」", () => {
    expect(formatLogTime(1_700_000_000)).toMatch(/^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/);
    expect(formatLogTime(0)).toBe("—");
  });

  it("tailSummary 说明截断", () => {
    const full: LogTail = { name: "a.log", total_lines: 10, shown: 10, truncated: false, lines: [] };
    expect(tailSummary(full)).toContain("共 10 行");
    expect(tailSummary(full)).not.toContain("截断");
    const cut: LogTail = { name: "a.log", total_lines: 5000, shown: 500, truncated: true, lines: [] };
    expect(tailSummary(cut)).toContain("截断");
    expect(tailSummary(null)).toBe("尚未读取");
  });

  it("logLevel 只影响着色，不改内容", () => {
    expect(logLevel("2026-01-01 ERROR something failed")).toBe("err");
    expect(logLevel("诊断失败：超时")).toBe("err");
    expect(logLevel("WARN 磁盘紧张")).toBe("warn");
    expect(logLevel("2026-01-01 INFO 启动完成")).toBe("info");
  });
});
