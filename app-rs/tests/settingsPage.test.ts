/**
 * 设置 / 日志页纯逻辑测试。
 *
 * 重点是**前端校验与后端同口径**：两边规则不一致会出现"界面说可以、后端拒绝"
 * 或更糟的"界面拦住了后端其实允许的值"。另：重启生效必须标注。
 */
import { describe, expect, it } from "vitest";

import {
  ALL_CATEGORY,
  SETTING_CATEGORIES,
  categoryBadge,
  categoryEntries,
  changedCount,
  displayValue,
  filterSettings,
  formatLogTime,
  groupKey,
  groupSettings,
  humanLogSize,
  applyHotkeyEcho,
  isChanged,
  isHotkeyKey,
  isUnwired,
  ladderSummary,
  monitorNeedsAttention,
  monitorStateLabel,
  logLevel,
  matchesQuery,
  restartNote,
  settingKeyTip,
  tailSummary,
  unwiredBadge,
  unwiredCount,
  validateValue,
  type LogTail,
  type MonitorPreview,
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

describe("未生效标注（诚实性）", () => {
  it("「暂未生效」被判为未生效", () => {
    const it = item({ unwired_reason: "暂未生效：自动诊断未移植" });
    expect(isUnwired(it)).toBe(true);
    expect(unwiredBadge(it)).toBe("暂未生效");
  });

  it("「部分生效」单独成档（不能混成完全未生效）", () => {
    const it = item({ unwired_reason: "部分生效：关闭有效，开启只登记占位条目" });
    expect(isUnwired(it)).toBe(true);
    expect(unwiredBadge(it)).toBe("部分生效");
  });

  it("无标注的项不算未生效", () => {
    expect(isUnwired(item())).toBe(false);
    expect(unwiredBadge(item())).toBe("");
    expect(isUnwired(item({ unwired_reason: "" }))).toBe(false);
  });

  it("其它文案不会被误判（必须以前缀识别）", () => {
    // 后端约定用「暂未生效」/「部分生效」开头；其它说明不该触发徽标
    expect(isUnwired(item({ unwired_reason: "注意：需要管理员权限" }))).toBe(false);
  });

  it("计数", () => {
    const items = [
      item({ key: "a", unwired_reason: "暂未生效：x" }),
      item({ key: "b", unwired_reason: "部分生效：y" }),
      item({ key: "c" }),
    ];
    expect(unwiredCount(items)).toBe(2);
    expect(unwiredCount([])).toBe(0);
  });
});

describe("双栏导航与统一过滤（信息架构重构）", () => {
  // 覆盖全部 7 个分类的样本；ty/min/max 按类型收口，label 各不相同（搜索按名称找）
  const boolItem = (key: string, label: string, current: boolean, dflt: boolean) =>
    item({ key, label, ty: "bool", default: dflt, current, min: undefined, max: undefined });
  const strItem = (key: string, label: string, current: string, dflt: string, options?: string[]) =>
    item({ key, label, ty: "str", default: dflt, current, options, min: undefined, max: undefined });

  const sample: SettingItem[] = [
    item({ key: "poll_interval_sec" }), // 自动修复
    item({ key: "fail_threshold", label: "失败阈值", current: 5 }), // 自动修复，已改动
    boolItem("tray_rate_enabled", "托盘限速", true, true), // 界面与托盘
    boolItem("dark_mode", "深色模式", true, false), // 界面与托盘，已改动
    strItem("dns_preset", "DNS 预设", "阿里", "阿里", ["阿里", "腾讯"]), // 网络
    item({ key: "clipboard_max", label: "剪贴板上限" }), // 剪贴板
    item({ key: "vault_autolock_min", label: "密码库自动锁定（分）" }), // 密码库
    strItem("palette_hotkey", "命令面板热键", "Ctrl+Alt+K", "Ctrl+Alt+K"), // 命令面板
    boolItem("migrated_v2", "已迁移标记", false, false), // 通用
  ];

  it("SETTING_CATEGORIES 顺序固定，且覆盖 groupKey 的全部产出", () => {
    expect([...SETTING_CATEGORIES]).toEqual([
      "自动修复",
      "界面与托盘",
      "网络",
      "剪贴板",
      "密码库",
      "命令面板",
      "通用",
    ]);
    // 前缀表里的每条映射都落在导航表内——未来加前缀忘了更新导航会在这里暴露
    for (const k of [
      "clip_x",
      "clipboard_x",
      "vault_x",
      "palette_x",
      "auto_repair_x",
      "cooldown_x",
      "level3_x",
      "fail_threshold",
      "poll_x",
      "tray_x",
      "minimize_x",
      "dark_x",
      "sound_x",
      "dns_x",
      "whatever_else",
    ]) {
      expect(SETTING_CATEGORIES).toContain(groupKey(k));
    }
  });

  it("categoryEntries：计数与已改动数按组汇总", () => {
    const auto = categoryEntries(sample).find((e) => e.group === "自动修复")!;
    expect(auto.total).toBe(2);
    expect(auto.changed).toBe(1);
    const tray = categoryEntries(sample).find((e) => e.group === "界面与托盘")!;
    expect(tray.total).toBe(2);
    expect(tray.changed).toBe(1);
    const net = categoryEntries(sample).find((e) => e.group === "网络")!;
    expect(net).toEqual({ group: "网络", total: 1, changed: 0 });
  });

  it("categoryEntries：顺序固定（不随键出现顺序跳动），空列表为空", () => {
    // 故意把「通用」放最前：导航仍必须按 SETTING_CATEGORIES 排
    const shuffled = [
      boolItem("migrated_v2", "已迁移标记", false, false),
      item({ key: "poll_interval_sec" }),
    ];
    expect(categoryEntries(shuffled).map((e) => e.group)).toEqual(["自动修复", "通用"]);
    expect(categoryEntries([])).toEqual([]);
  });

  it("categoryEntries：总数不丢项（各组 total 之和 = 全部项数）", () => {
    const sum = categoryEntries(sample).reduce((n, e) => n + e.total, 0);
    expect(sum).toBe(sample.length);
  });

  it("filterSettings：分类过滤（ALL 返回全部，具体组只留该组）", () => {
    expect(filterSettings(sample, { category: ALL_CATEGORY }).length).toBe(sample.length);
    expect(filterSettings(sample, {}).length).toBe(sample.length); // 未指定 = 全部
    expect(filterSettings(sample, { category: "网络" }).map((i) => i.key)).toEqual(["dns_preset"]);
  });

  it("filterSettings：只看已改动（可与分类叠加）", () => {
    expect(filterSettings(sample, { onlyChanged: true }).map((i) => i.key).sort()).toEqual([
      "dark_mode",
      "fail_threshold",
    ]);
    expect(
      filterSettings(sample, { onlyChanged: true, category: "自动修复" }).map((i) => i.key),
    ).toEqual(["fail_threshold"]);
  });

  it("filterSettings：搜索匹配名称（新增 label）/键名/分组/描述，大小写不敏感", () => {
    expect(filterSettings(sample, { query: "检测间隔" }).map((i) => i.key)).toEqual([
      "poll_interval_sec",
    ]); // label
    expect(matchesQuery(item({ key: "poll_interval_sec" }), "POLL_")).toBe(true); // 键名大小写
    expect(filterSettings(sample, { query: "自动修复" }).length).toBe(2); // 分组名
    expect(filterSettings(sample, { query: "   " })).toHaveLength(sample.length); // 空白 = 全部
  });

  it("filterSettings：三条件组合", () => {
    expect(
      filterSettings(sample, { category: "界面与托盘", onlyChanged: true, query: "dark" }).map(
        (i) => i.key,
      ),
    ).toEqual(["dark_mode"]);
    expect(filterSettings(sample, { category: "网络", onlyChanged: true })).toEqual([]);
  });
});

describe("导航角标语义与键名悬浮提示（用户反馈：语义不清 / 键名降噪）", () => {
  it("categoryBadge：无已改动项 → 只显示总数（灰字，不出红色）", () => {
    expect(categoryBadge({ total: 8, changed: 0 })).toEqual({ text: "8", changed: false });
  });

  it("categoryBadge：有已改动项 → 红色「已改 N」徽标（不再并排 4/8 分数）", () => {
    expect(categoryBadge({ total: 8, changed: 4 })).toEqual({ text: "已改 4", changed: true });
    // 全部改完仍是同一语义
    expect(categoryBadge({ total: 3, changed: 3 }).text).toBe("已改 3");
  });

  it("categoryBadge：与 categoryEntries 的产出直接对接（「全部」伪分类同语义）", () => {
    // sample 里的自动修复组：2 项，1 项已改动（fail_threshold）
    const auto = categoryEntries([
      item({ key: "poll_interval_sec" }),
      item({ key: "fail_threshold", label: "失败阈值", current: 5 }),
    ]).find((e) => e.group === "自动修复")!;
    expect(categoryBadge(auto)).toEqual({ text: "已改 1", changed: true });
  });

  it("settingKeyTip：悬浮提示含底层键名（行尾小字撤下后仍可查）", () => {
    expect(settingKeyTip(item())).toBe("配置键：poll_interval_sec");
    expect(settingKeyTip(item({ key: "auto_repair_max_level" }))).toBe(
      "配置键：auto_repair_max_level",
    );
  });
});

describe("后台监控展示", () => {
  const preview = (over: Partial<MonitorPreview> = {}): MonitorPreview => ({
    enabled: true,
    max_level: 3,
    poll_interval_sec: 30,
    fail_threshold: 2,
    cooldown_sec: 180,
    vpn_up: false,
    ladder: ["flush_dns", "start_nlasvc", "renew_dhcp"],
    skipped_by_vpn: [],
    vpn_note: "",
    suggestions: [],
    ...over,
  });

  it("状态码文案与后端 ST_* 对齐", () => {
    expect(monitorStateLabel("ok")).toBe("网络正常");
    expect(monitorStateLabel("no_net")).toContain("异常");
    expect(monitorStateLabel("ncsi_false")).toContain("NCSI");
    expect(monitorStateLabel("offline")).toContain("无活动");
    expect(monitorStateLabel("")).toBe("尚未检查");
  });

  it("offline 不算故障（没接网线不等于网络坏了）", () => {
    // 报成故障会让用户白折腾
    expect(monitorNeedsAttention("offline")).toBe(false);
    expect(monitorNeedsAttention("ok")).toBe(false);
    expect(monitorNeedsAttention("no_net")).toBe(true);
    expect(monitorNeedsAttention("ncsi_false")).toBe(true);
  });

  it("梯度摘要含步数与 VPN 跳过数", () => {
    expect(ladderSummary(preview())).toContain("3 步");
    expect(ladderSummary(preview({ skipped_by_vpn: ["renew_dhcp"] }))).toContain("跳过 1 步");
  });

  it("关闭自动修复时明确说明（仍会探测）", () => {
    expect(ladderSummary(preview({ enabled: false, ladder: [] }))).toContain("已关闭");
  });

  it("未读取时不为空", () => {
    expect(ladderSummary(null)).toBe("尚未读取");
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



describe("热键速查三方一致（轮次36）", () => {
  const base = { quick: "Ctrl+Alt+V", palette: "Ctrl+Alt+K" };

  it("isHotkeyKey 覆盖三把热键、拒绝普通键", () => {
    expect(isHotkeyKey("palette_hotkey")).toBe(true);
    expect(isHotkeyKey("clip_quick_hotkey")).toBe(true);
    expect(isHotkeyKey("clip_paste_hotkey")).toBe(true);
    expect(isHotkeyKey("poll_interval_sec")).toBe(false);
  });

  it("被占回退后按**实际生效值**回显（不是请求值）", () => {
    // sk://hotkey-reverted 载荷 = 后端已持久化的实际生效值
    const reverted = applyHotkeyEcho(base, "clip_quick_hotkey", "Ctrl+Alt+V");
    expect(reverted.quick).toBe("Ctrl+Alt+V");
    expect(reverted.palette).toBe("Ctrl+Alt+K");
    const moved = applyHotkeyEcho(base, "palette_hotkey", "Ctrl+Alt+H");
    expect(moved.palette).toBe("Ctrl+Alt+H");
    expect(moved.quick).toBe("Ctrl+Alt+V");
  });

  it("空值回显为「未设」（禁用态不得显示旧组合）", () => {
    const off = applyHotkeyEcho(base, "clip_quick_hotkey", "");
    expect(off.quick).toBe("（未设）");
    const blank = applyHotkeyEcho(base, "palette_hotkey", "   ");
    expect(blank.palette).toBe("（未设）");
  });

  it("非侧栏展示的热键（面板键）不改速查", () => {
    const same = applyHotkeyEcho(base, "clip_paste_hotkey", "Ctrl+Win+V");
    expect(same).toEqual(base);
    const unknown = applyHotkeyEcho(base, "bogus_key", "X");
    expect(unknown).toEqual(base);
  });

  it("不改变未涉及条目（不可变更新）", () => {
    const out = applyHotkeyEcho(base, "clip_quick_hotkey", "Ctrl+Shift+V");
    expect(out).toEqual({ quick: "Ctrl+Shift+V", palette: "Ctrl+Alt+K" });
  });
});
