/**
 * 剪贴板历史管理页纯逻辑（无 React/DOM，vitest 覆盖）。对应 Python 侧 ClipboardTab。
 *
 * 与快捷粘贴面板（`ClipPanel`）的分工：面板只做"输入即搜 + 回车粘贴"，
 * 这里是**完整管理**（筛选、置顶、删除、清空、按天数清理、另存为）。
 * 两者共用同一份历史数据与同一批后端命令。
 */

/** 类型筛选项（与 Python `KIND_FILTERS` 同口径）。 */
export const KIND_FILTERS: { key: string; label: string }[] = [
  { key: "", label: "全部类型" },
  { key: "text", label: "文本" },
  { key: "image", label: "图片" },
  { key: "file", label: "文件" },
  { key: "html", label: "HTML" },
];

/** 后端 `sk_clip_list` / `sk_panel_init` 的行。 */
export interface ClipRow {
  idx: number;
  ts: number;
  text: string;
  kind: string;
  pinned: boolean;
  data: string;
  preview: string;
}

/** 后端 `sk_clip_summary`。 */
export interface ClipSummary {
  count: number;
  pinned: number;
  max_items: number;
  watch_enabled: boolean;
  watch_paused: boolean;
  by_kind: Record<string, number>;
}

/** 类型 → 展示名。 */
export function kindLabel(kind: string): string {
  switch (kind) {
    case "text":
      return "文本";
    case "image":
      return "图片";
    case "file":
      return "文件";
    case "html":
      return "HTML";
    default:
      return kind || "未知";
  }
}

/** 类型 → AntD Tag 颜色。 */
export function kindColor(kind: string): string {
  switch (kind) {
    case "text":
      return "blue";
    case "image":
      return "purple";
    case "file":
      return "green";
    case "html":
      return "orange";
    default:
      return "default";
  }
}

/** 时间戳（秒）→ 本地时间文本。 */
export function formatTs(ts: number): string {
  if (!ts || ts <= 0) return "—";
  const d = new Date(ts * 1000);
  if (Number.isNaN(d.getTime())) return "—";
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

/** 相对时间（更直观：刚刚 / 5 分钟前 / 3 小时前）。 */
export function relativeTime(ts: number, nowMs: number): string {
  if (!ts || ts <= 0) return "—";
  const diff = Math.floor(nowMs / 1000) - ts;
  if (diff < 0) return "刚刚";
  if (diff < 60) return "刚刚";
  if (diff < 3600) return `${Math.floor(diff / 60)} 分钟前`;
  if (diff < 86400) return `${Math.floor(diff / 3600)} 小时前`;
  return `${Math.floor(diff / 86400)} 天前`;
}

/**
 * 置顶条目**不会被自动淘汰**，且文本重复时不再入库。
 * 所以"哪些会被淘汰"的判断要排除置顶项——用户需要知道置顶是不是真的保护了它。
 */
export function evictionCandidates(rows: ClipRow[], maxItems: number): number {
  const unpinned = rows.filter((r) => !r.pinned).length;
  return Math.max(0, unpinned - maxItems);
}

/** 摘要文案。 */
export function clipSummaryText(s: ClipSummary | null): string {
  if (!s) return "尚未读取";
  const kinds = Object.entries(s.by_kind ?? {})
    .filter(([, n]) => n > 0)
    .map(([k, n]) => `${kindLabel(k)} ${n}`)
    .join(" · ");
  let out = `共 ${s.count}/${s.max_items} 条`;
  if (s.pinned > 0) out += ` · 置顶 ${s.pinned}`;
  if (kinds) out += ` · ${kinds}`;
  if (!s.watch_enabled) out += " · 监听已关闭";
  else if (s.watch_paused) out += " · 已隐私暂停";
  return out;
}

/** 清空确认文案（不可逆）。 */
export function clearConfirmText(count: number, pinned: number): string {
  let t = `将删除全部 ${count} 条剪贴板历史（不可撤销）。`;
  if (pinned > 0) {
    t += `\n\n其中 ${pinned} 条已置顶——清空会一并删除置顶条目。`;
  }
  t += "\n\n图片条目对应的 PNG 文件也会被删除。";
  return t;
}

/** 按天数清理的确认文案。 */
export function purgeConfirmText(days: number): string {
  return (
    `将删除 ${days} 天前的历史条目（不可撤销）。\n\n` +
    `置顶条目会被保留。图片文件同步删除。`
  );
}

/** 隐私暂停的说明（避免用户以为恢复后会补录）。 */
export function pauseHint(paused: boolean, enabled: boolean): string {
  if (!enabled) return "监听已关闭，无「暂停」可言——需要先在设置里开启剪贴板监听。";
  return paused
    ? "已暂停记录；恢复后**不会**补录暂停期间的内容（隐私考虑）。"
    : "暂停记录后，期间复制的内容不会被保存；恢复后也不会补录。";
}

/** 另存为按钮是否可用（图片文件可能已被外部删除）。 */
export function canSaveAs(row: ClipRow): { ok: boolean; reason: string } {
  if (row.kind === "image" && !row.data) {
    return { ok: false, reason: "该图片条目没有对应文件（可能已被清理）" };
  }
  if (!row.text && row.kind !== "image") {
    return { ok: false, reason: "条目内容为空，无可保存内容" };
  }
  return { ok: true, reason: "" };
}
