/**
 * Hosts 编辑页纯逻辑（无 React/DOM，vitest 覆盖）。
 *
 * 对应 Python 侧 ui/toolbox_page.py::HostsTab 的展示与校验规则：
 * 列表行筛选、新增输入校验（IP/主机名/别名）、保存前护栏提示。
 * 校验放这里是因为「写坏 hosts 会影响整机解析」——错误必须在点保存之前拦住。
 */

export type EntryKind = "entry" | "disabled" | "comment";

/** 后端 `sk_hosts_read` 的条目。 */
export interface HostEntry {
  raw: string;
  kind: EntryKind;
  ip: string;
  host: string;
  aliases: string[];
  comment: string;
  enabled: boolean;
  had_content: boolean;
}

/** 后端 `sk_hosts_info`。 */
export interface HostsInfo {
  path: string;
  exists: boolean;
  writable: boolean;
  is_admin: boolean;
}

/** 表格行（带原始下标，供删除/切换定位）。 */
export interface HostRow extends HostEntry {
  idx: number;
}

/** 只展示主机映射（注释/空行不进表）。 */
export function mappingRows(entries: HostEntry[]): HostRow[] {
  return entries
    .map((e, idx) => ({ ...e, idx }))
    .filter((e) => e.kind === "entry" || e.kind === "disabled");
}

/** 条目筛选维度。`comment` 仅在「显示注释行」打开时提供——注释被隐藏时选它必然筛出空表。 */
export type HostFilter = "all" | "enabled" | "disabled" | "comment";

/**
 * 在已构建的行上做筛选 + 关键字过滤。
 *
 * 与 `filterPorts` 同思路：筛选放前端是为了**即时**反馈（hosts 大文件上百行，
 * 等后端往返的筛选体验不可接受）。关键字匹配「IP 主机名 别名 备注」整行文本，
 * 大小写不敏感——主机名本身不区分大小写，筛选也应一致。
 */
export function filterHostRows(rows: HostRow[], filter: HostFilter, keyword: string): HostRow[] {
  const kw = keyword.trim().toLowerCase();
  return rows.filter((r) => {
    if (filter === "enabled" && r.kind !== "entry") return false;
    if (filter === "disabled" && r.kind !== "disabled") return false;
    if (filter === "comment" && r.kind !== "comment") return false;
    if (!kw) return true;
    const hay = [r.raw, r.ip, r.host, r.aliases.join(" "), r.comment]
      .join(" ")
      .toLowerCase();
    return hay.includes(kw);
  });
}

/**
 * 该映射是否已存在（新增去重）。
 *
 * hosts 里同一 `IP 主机名` 重复一行不会报错，但语义混乱且难排查；
 * 主机名不区分大小写（`Dev.local` 与 `dev.local` 是同一条），IP 按原文比对
 * （IP 本身无大小写问题，宽松比对反而可能放过 `010.0.0.1` 这类边界形态）。
 */
export function isDuplicateMapping(entries: HostEntry[], ip: string, host: string): boolean {
  const h = host.trim().toLowerCase();
  const i = ip.trim();
  return entries.some((e) => e.host.trim().toLowerCase() === h && e.ip.trim() === i);
}

/** 行的显示文本：`IP 主机名 别名…`。 */
export function rowText(row: HostRow): string {
  const parts = [row.ip, row.host, ...row.aliases];
  return parts.filter((p) => p && p.length > 0).join(" ");
}

/** 校验 IPv4（严格四段 0-255）。 */
export function isValidIpv4(ip: string): boolean {
  const parts = ip.trim().split(".");
  if (parts.length !== 4) return false;
  return parts.every((p) => {
    if (!/^\d{1,3}$/.test(p)) return false;
    const n = Number(p);
    return n >= 0 && n <= 255;
  });
}

/** 校验 IPv6（宽松：含冒号且非空段）。 */
export function isValidIpv6(ip: string): boolean {
  const t = ip.trim();
  if (!t.includes(":")) return false;
  // 不允许连续三个冒号、不允许非法字符
  if (t.includes(":::")) return false;
  return /^[0-9a-fA-F:]+$/.test(t);
}

export function isValidIp(ip: string): boolean {
  const t = ip.trim();
  return isValidIpv4(t) || isValidIpv6(t);
}

/** 校验主机名：字母数字与 . - _，不能以 . 或 - 开头结尾。 */
export function isValidHost(host: string): boolean {
  const t = host.trim();
  if (!t || t.length > 253) return false;
  if (/^[.-]|[.-]$/.test(t)) return false;
  return /^[A-Za-z0-9._-]+$/.test(t);
}

export interface ValidationResult {
  ok: boolean;
  errors: string[];
}

/** 新增条目的完整校验（IP + 主机名 + 别名）。 */
export function validateNewEntry(ip: string, host: string, aliases: string[]): ValidationResult {
  const errors: string[] = [];
  if (!ip.trim()) errors.push("IP 不能为空");
  else if (!isValidIp(ip)) errors.push(`IP 格式非法: ${ip.trim()}`);
  if (!host.trim()) errors.push("主机名不能为空");
  else if (!isValidHost(host)) errors.push(`主机名非法: ${host.trim()}`);
  for (const a of aliases) {
    if (a.trim() && !isValidHost(a)) errors.push(`别名非法: ${a.trim()}`);
  }
  return { ok: errors.length === 0, errors };
}

/** 解析逗号/空格分隔的别名输入。 */
export function parseAliases(text: string): string[] {
  return text
    .split(/[\s,，]+/)
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

/** 保存前护栏提示（不可写/非管理员时先说清后果）。 */
export function saveWarnings(info: HostsInfo | null): string[] {
  if (!info) return [];
  const out: string[] = [];
  if (!info.writable) {
    out.push("hosts 文件当前不可写——保存会失败，请以管理员身份运行 SysKit");
  } else if (!info.is_admin) {
    out.push(
      "当前不是管理员：Windows 通常只允许管理员改写 hosts，保存可能被拒绝（会保留原文件）",
    );
  }
  if (!info.exists) {
    out.push("hosts 文件不存在，保存将新建一个");
  }
  return out;
}

/**
 * 变更摘要（保存前确认框用）。
 *
 * **只统计主机映射行**：注释/空行的增删不算"改动"——它们不改变解析语义，
 * 计入会让「有未保存改动」的提示因为删掉一行注释而亮起（首版即如此，测试逮到）。
 */
export function summarizeChanges(original: HostEntry[], current: HostEntry[]): {
  added: number;
  removed: number;
  toggled: number;
} {
  const isMappingEntry = (e: HostEntry) => e.kind === "entry" || e.kind === "disabled";
  const key = (e: HostEntry) => `${e.ip}|${e.host}|${e.comment}`;
  const origMap = new Map(original.filter(isMappingEntry).map((e) => [key(e), e]));
  const curMap = new Map(current.filter(isMappingEntry).map((e) => [key(e), e]));
  let added = 0;
  let removed = 0;
  let toggled = 0;
  for (const [k, e] of curMap) {
    const o = origMap.get(k);
    if (!o) {
      added += 1;
    } else if (o.enabled !== e.enabled) {
      toggled += 1;
    }
  }
  for (const k of origMap.keys()) {
    if (!curMap.has(k)) removed += 1;
  }
  return { added, removed, toggled };
}

/** 统计信息（页面顶部）。 */
export function hostsStats(entries: HostEntry[]): {
  total: number;
  enabled: number;
  disabled: number;
  comments: number;
} {
  let enabled = 0;
  let disabled = 0;
  let comments = 0;
  for (const e of entries) {
    if (e.kind === "entry") enabled += 1;
    else if (e.kind === "disabled") disabled += 1;
    else comments += 1;
  }
  return { total: entries.length, enabled, disabled, comments };
}
