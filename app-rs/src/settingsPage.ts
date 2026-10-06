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

/** 后端 `sk_monitor_state`。 */
export interface MonitorState {
  running: boolean;
  state: string;
  detail: string;
  last_check_ts: number;
  repair_count: number;
  last_ladder: string;
  fail_streak: number;
}

/** 后端 `sk_monitor_preview`。 */
export interface MonitorPreview {
  enabled: boolean;
  max_level: number;
  poll_interval_sec: number;
  fail_threshold: number;
  cooldown_sec: number;
  vpn_up: boolean;
  ladder: string[];
  skipped_by_vpn: string[];
  vpn_note: string;
  suggestions: { id: string; reason: string }[];
}

/** 监控状态码 → 展示文案（与 Python 的 ST_* 同名）。 */
export function monitorStateLabel(state: string): string {
  switch (state) {
    case "ok":
      return "网络正常";
    case "no_net":
      return "连通性异常";
    case "ncsi_false":
      return "NCSI 误报";
    case "offline":
      return "无活动网络连接";
    case "":
      return "尚未检查";
    default:
      return state;
  }
}

/**
 * 该状态是否值得告警。
 *
 * `offline`（没有活动物理网卡，例如没插网线或纯虚拟网卡）**不算故障**：
 * 它不是"网络坏了"，而是"本来就没接"。把它报成故障会让用户白折腾。
 */
export function monitorNeedsAttention(state: string): boolean {
  return state === "no_net" || state === "ncsi_false";
}

/** 梯度预览摘要。 */
export function ladderSummary(p: MonitorPreview | null): string {
  if (!p) return "尚未读取";
  if (!p.enabled) return "自动修复已关闭（仍会探测并显示状态）";
  const n = p.ladder.length;
  const skipped = p.skipped_by_vpn.length;
  return skipped > 0
    ? `将执行 ${n} 步，VPN 护栏跳过 ${skipped} 步`
    : `将执行 ${n} 步`;
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

// ---------------- 信息架构：左导航分类 + 统一过滤（设置页双栏重构） ----------------

/**
 * 左侧导航的分类与固定顺序（组名与 `groupKey` 的产出一一对应）。
 *
 * 为什么写死顺序：`groupSettings` 按后端键的首次出现排组，一旦搜索/过滤，
 * 组会忽隐忽现、顺序跳动；导航必须稳定，否则用户的肌肉记忆失效。
 */
export const SETTING_CATEGORIES: readonly string[] = [
  "自动修复",
  "界面与托盘",
  "网络",
  "剪贴板",
  "密码库",
  "命令面板",
  "通用",
];

/** 「全部」视图的伪分类 key（双下划线前缀，不会与任何 groupKey 产出撞车）。 */
export const ALL_CATEGORY = "__all__";

/** 导航里单个分类的条目：项数与已改动数（角标用）。 */
export interface CategoryEntry {
  group: string;
  total: number;
  changed: number;
}

/**
 * 导航条目：按 SETTING_CATEGORIES 固定顺序排列，无项的组不出现。
 * 计划外的新组（未来后端加键而前缀表没跟上）附在末尾——不吞项是底线。
 */
export function categoryEntries(items: SettingItem[]): CategoryEntry[] {
  const map = new Map<string, CategoryEntry>();
  for (const it of items) {
    const g = groupKey(it.key);
    const e = map.get(g) ?? { group: g, total: 0, changed: 0 };
    e.total += 1;
    if (isChanged(it)) e.changed += 1;
    map.set(g, e);
  }
  const ordered: CategoryEntry[] = [];
  for (const g of SETTING_CATEGORIES) {
    const e = map.get(g);
    if (e) ordered.push(e);
    map.delete(g);
  }
  for (const e of map.values()) ordered.push(e);
  return ordered;
}

/**
 * 左导航角标语义（纯逻辑，界面与测试共用同一口径）。
 *
 * 为什么不用「4/8」这种分数形式：它读不出"有几个被我改过"，用户得自己做减法。
 * 语义化两态：
 * - 无已改动项 → 只显示总数（灰字），界面安静；
 * - 有已改动项 → 改显红色「已改 N」，红色只在有改动时出现，不做常驻红点。
 */
export interface CategoryBadge {
  /** 角标文本：无改动 = 总数；有改动 = 「已改 N」。 */
  text: string;
  /** true = 红色"已改动"徽标（Tag color=error）；false = 灰色总数。 */
  changed: boolean;
}

/** 入参取 CategoryEntry 的子集即可：「全部」伪分类（无 group 字段）也直接可用。 */
export function categoryBadge(e: Pick<CategoryEntry, "total" | "changed">): CategoryBadge {
  return e.changed > 0
    ? { text: `已改 ${e.changed}`, changed: true }
    : { text: String(e.total), changed: false };
}

/**
 * 设置行悬浮 Tooltip 的文案：底层键名（poll_interval_sec 等）。
 *
 * 键名从行尾灰色小字**全局撤下**（每行少一个视觉噪音源），但对照配置文件、
 * 报障排查时仍需要它——收进"悬浮该行"的 Tooltip：要找的人能找到，
 * 不关心的人看不见。注意这只是显示层隐藏，搜索仍按键名匹配（matchesQuery）。
 */
export function settingKeyTip(item: SettingItem): string {
  return `配置键：${item.key}`;
}

/**
 * 搜索匹配：键名 / 名称 / 描述 / 分组 / 当前值，大小写不敏感。
 *
 * 名称（label）也参与匹配：重构后底层键名默认隐藏，用户看得见的是 label，
 * 搜不到 label 等于搜不到自己看得见的字。空查询匹配全部。
 */
export function matchesQuery(it: SettingItem, rawQuery: string): boolean {
  const q = rawQuery.trim().toLowerCase();
  if (!q) return true;
  return (
    it.key.toLowerCase().includes(q) ||
    it.label.toLowerCase().includes(q) ||
    (it.description ?? "").toLowerCase().includes(q) ||
    groupKey(it.key).toLowerCase().includes(q) ||
    String(displayValue(it)).toLowerCase().includes(q)
  );
}

/** 右侧内容的统一过滤条件（三者可任意组合）。 */
export interface SettingsFilter {
  /** 限定分类（ALL_CATEGORY = 全部；空值按全部处理）。 */
  category?: string;
  /** 只看已改动（工具条的「N 项已改动」过滤按钮）。 */
  onlyChanged?: boolean;
  /** 搜索词。 */
  query?: string;
}

/** 按条件过滤（分类 → 已改动 → 搜索；结果保持后端键顺序）。 */
export function filterSettings(items: SettingItem[], f: SettingsFilter): SettingItem[] {
  let out = items;
  if (f.category && f.category !== ALL_CATEGORY) {
    const cat = f.category;
    out = out.filter((it) => groupKey(it.key) === cat);
  }
  if (f.onlyChanged) out = out.filter(isChanged);
  const q = f.query ?? "";
  if (q.trim()) out = out.filter((it) => matchesQuery(it, q));
  return out;
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

// ---------------- 热键速查三方一致（注册链 × 设置页 × 侧栏，轮次36） ----------------

/** 热键类设置键：改任一键后侧栏速查都要即时刷新（否则三方显示分叉）。 */
export const HOTKEY_SETTING_KEYS: readonly string[] = [
  "palette_hotkey",
  "clip_quick_hotkey",
  "clip_paste_hotkey",
];

/** 该键是否为热键类设置。 */
export function isHotkeyKey(key: string): boolean {
  return HOTKEY_SETTING_KEYS.includes(key);
}

/** 侧栏底部热键速查的两条目（快速粘贴 / 命令面板）。 */
export interface HotkeyHints {
  quick: string;
  palette: string;
}

/**
 * 热键事件（被占回退 / 改键）后更新侧栏速查（纯函数）。
 *
 * `value` 必须是**实际生效**的持久化值（`sk://hotkey-reverted` 的载荷、
 * 或 `sk_settings_all` 的 current），不是用户请求值——被占回退后请求值
 * 并未注册，照它显示就是在展示一个按了没反应的组合。空串 = 已禁用。
 */
export function applyHotkeyEcho(h: HotkeyHints, key: string, value: string): HotkeyHints {
  const shown = value.trim() || "（未设）";
  if (key === "clip_quick_hotkey") return { ...h, quick: shown };
  if (key === "palette_hotkey") return { ...h, palette: shown };
  return h;
}
