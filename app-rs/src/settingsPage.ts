/**
 * 设置 / 日志页纯逻辑（无 React/DOM，vitest 覆盖）。对应 Python 侧设置页与日志查看。
 *
 * 关键点：
 * - 范围校验**后端才是权威**（界面可被绕过），前端做即时反馈但必须与后端同口径；
 * - 「重启后生效」的项必须标注，否则用户会以为没生效是 bug。
 */

/** 后端 `sk_settings_all` 的一项。 */
export interface SettingItem {
  key: string;
  label: string;
  description: string;
  ty: "bool" | "int" | "str";
  default: unknown;
  current: unknown;
  min?: number;
  max?: number;
  options?: string[];
  needs_restart: boolean;
  /** 非空表示该设置**目前没有消费方**（改了不生效），内容为原因说明。 */
  unwired_reason?: string;
}

export interface SettingsAll {
  items: SettingItem[];
  count: number;
}

/** 后端 `sk_log_list`。 */
export interface LogFile {
  name: string;
  bytes: number;
  modified: number;
}

export interface LogList {
  dir: string;
  files: LogFile[];
  count: number;
}

/** 后端 `sk_log_tail`。 */
export interface LogTail {
  name: string;
  total_lines: number;
  shown: number;
  truncated: boolean;
  lines: string[];
}

/**
 * 前端即时校验（与后端 `set_one` 同口径：类型 → 范围 → 可选值）。
 *
 * **后端的校验才是权威**：这里只为省一次往返与即时反馈，两边规则必须一致，
 * 否则会出现"界面说可以、后端拒绝"或相反的错位。
 */
export function validateValue(
  item: SettingItem,
  raw: unknown,
): { ok: boolean; reason: string; coerced?: unknown } {
  if (item.ty === "bool") {
    if (typeof raw === "boolean") return { ok: true, reason: "", coerced: raw };
    // 后端只认 0/1 数字（字符串 "true" 会回退默认），前端同步这一语义
    if (raw === 0 || raw === 1) return { ok: true, reason: "", coerced: raw === 1 };
    return { ok: false, reason: "布尔项只接受 true/false 或 0/1" };
  }
  if (item.ty === "int") {
    const n = typeof raw === "number" ? raw : Number(String(raw).trim());
    if (!Number.isInteger(n)) {
      return { ok: false, reason: "请输入整数" };
    }
    if (item.min !== undefined && n < item.min) {
      return { ok: false, reason: `不能小于 ${item.min}` };
    }
    if (item.max !== undefined && n > item.max) {
      return { ok: false, reason: `不能大于 ${item.max}` };
    }
    return { ok: true, reason: "", coerced: n };
  }
  const s = String(raw ?? "");
  if (item.options && item.options.length > 0 && !item.options.includes(s)) {
    return { ok: false, reason: `只能是 ${item.options.join(" / ")} 之一` };
  }
  return { ok: true, reason: "", coerced: s };
}

/** 该项是否与默认值不同（界面标"已改动"）。 */
export function isChanged(item: SettingItem): boolean {
  return JSON.stringify(item.current) !== JSON.stringify(item.default);
}

/** 已改动的项数。 */
export function changedCount(items: SettingItem[]): number {
  return items.filter(isChanged).length;
}

/** 按前缀分组（key 的下划线前缀；无前缀归"通用"）。 */
export function groupKey(key: string): string {
  const prefixes: [string, string][] = [
    ["clip_", "剪贴板"],
    ["clipboard_", "剪贴板"],
    ["vault_", "密码库"],
    ["palette_", "命令面板"],
    ["auto_repair", "自动修复"],
    ["cooldown", "自动修复"],
    ["level3_", "自动修复"],
    ["fail_threshold", "自动修复"],
    ["poll_", "自动修复"],
    ["tray_", "界面与托盘"],
    ["minimize_", "界面与托盘"],
    ["dark_", "界面与托盘"],
    ["sound_", "界面与托盘"],
    ["dns_", "网络"],
  ];
  for (const [p, label] of prefixes) {
    if (key.startsWith(p)) return label;
  }
  return "通用";
}

/** 分组后的展示结构（保持后端给出的键顺序）。 */
export function groupSettings(items: SettingItem[]): { group: string; items: SettingItem[] }[] {
  const order: string[] = [];
  const map = new Map<string, SettingItem[]>();
  for (const it of items) {
    const g = groupKey(it.key);
    if (!map.has(g)) {
      map.set(g, []);
      order.push(g);
    }
    map.get(g)!.push(it);
  }
  return order.map((g) => ({ group: g, items: map.get(g)! }));
}

/** 值的展示文本。 */
export function displayValue(item: SettingItem): string {
  if (item.ty === "bool") return item.current ? "开" : "关";
  if (item.options) return String(item.current ?? "");
  const s = String(item.current ?? "");
  return s.length > 60 ? `${s.slice(0, 57)}…` : s;
}

/** 重启生效提示文案（为空表示无需重启）。 */
export function restartNote(item: SettingItem): string {
  return item.needs_restart ? "重启后生效" : "";
}

/**
 * 该设置是否"暂未生效"（无消费方，改了没有效果）。
 *
 * 界面必须把这类项**显眼标出**：给了开关却没人读，用户改了没效果只会以为
 * 程序坏了或自己改错了。诚实标注是唯一可接受的呈现。
 */
export function isUnwired(item: SettingItem): boolean {
  const r = item.unwired_reason ?? "";
  return r.startsWith("暂未生效") || r.startsWith("部分生效");
}

/** 未生效项的标签文案（标签很短，详情看 tooltip）。 */
export function unwiredBadge(item: SettingItem): string {
  const r = item.unwired_reason ?? "";
  if (r.startsWith("部分生效")) return "部分生效";
  return r.startsWith("暂未生效") ? "暂未生效" : "";
}

/** 统计未生效项数（页面顶部提示用）。 */
export function unwiredCount(items: SettingItem[]): number {
  return items.filter(isUnwired).length;
}

/** 日志文件大小展示。 */
export function humanLogSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

/** 日志时间戳（秒）→ 本地时间。 */
export function formatLogTime(ts: number): string {
  if (!ts) return "—";
  const d = new Date(ts * 1000);
  if (Number.isNaN(d.getTime())) return "—";
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

/** 尾部读取的摘要（说明是否被截断）。 */
export function tailSummary(t: LogTail | null): string {
  if (!t) return "尚未读取";
  const base = `${t.name}：共 ${t.total_lines} 行，显示最后 ${t.shown} 行`;
  return t.truncated ? `${base}（已截断，仅显示尾部）` : base;
}

/** 日志行 → 展示级别（用于着色；不改变内容）。 */
export function logLevel(line: string): "err" | "warn" | "info" {
  const l = line.toUpperCase();
  if (l.includes("ERROR") || l.includes("失败") || l.includes("错误")) return "err";
  if (l.includes("WARN") || l.includes("警告")) return "warn";
  return "info";
}
