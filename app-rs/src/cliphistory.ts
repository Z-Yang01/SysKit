/**
 * 剪贴板历史管理页纯逻辑（无 React/DOM，vitest 覆盖）。对应 Python 侧 ClipboardTab。
 *
 * 与快捷粘贴面板（`ClipPanel`）的分工：面板只做"输入即搜 + 回车粘贴"，
 * 这里是**完整管理**（筛选、置顶、删除、清空、按天数清理、另存为）。
 * 两者共用同一份历史数据与同一批后端命令。
 */

/** 类型筛选项（与 Python `KIND_FILTERS` 同口径，含「网页」措辞）。 */
export const KIND_FILTERS: { key: string; label: string }[] = [
  { key: "", label: "全部类型" },
  { key: "text", label: "文本" },
  { key: "image", label: "图片" },
  { key: "file", label: "文件" },
  { key: "html", label: "网页" },
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
  /** 敏感内容拦截计数（clip_sensitive_detect 命中未入库的累计数；v1.3+） */
  skipped_sensitive?: number;
  by_kind: Record<string, number>;
}

/**
 * 顶部类型筛选（横向 Segmented）的选项：label 带实时计数，如「全部(42)」。
 *
 * 计数直接取后端 summary（全部用 count，其余按 by_kind），前端**不重复统计**——
 * 两套口径迟早算岔；by_kind 缺某类型时按 0 计。summary 未读到时退化为纯标签。
 * html 沿用「网页」措辞（前一轮与 Python 口径对齐过，不要写 HTML）。
 */
export function kindFilterOptions(s: ClipSummary | null): { key: string; label: string }[] {
  return KIND_FILTERS.map((f) => {
    if (!s) return { key: f.key, label: f.key === "" ? "全部" : f.label };
    const n = f.key === "" ? s.count : (s.by_kind?.[f.key] ?? 0);
    return { key: f.key, label: `${f.key === "" ? "全部" : f.label}(${n})` };
  });
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
      return "网页";
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

/** 时间戳（秒）→ 本地时间文本（含秒：同一秒内的连续复制靠它分辨先后）。 */
export function formatTs(ts: number): string {
  if (!ts || ts <= 0) return "—";
  const d = new Date(ts * 1000);
  if (Number.isNaN(d.getTime())) return "—";
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
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

// ---------------- 按天查看 ----------------

/**
 * 本地日期键 `YYYY-MM-DD`。
 *
 * **必须按本地时区算**（`getFullYear/Month/Date`）而不是 `toISOString()`：
 * 后者是 UTC，东八区用户在 00:00–08:00 之间复制的内容会被归到"昨天"，
 * 这正是"按天查看"最容易被用户发现的错误。
 */
export function dayKey(ts: number): string {
  if (!ts || ts <= 0) return "";
  const d = new Date(ts * 1000);
  if (Number.isNaN(d.getTime())) return "";
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

/** 某天的显示名：今天 / 昨天 / YYYY-MM-DD。 */
export function dayLabel(key: string, nowMs = Date.now()): string {
  if (!key) return "未知日期";
  const today = new Date(nowMs);
  const yesterday = new Date(nowMs);
  yesterday.setDate(today.getDate() - 1);
  const tk = dayKey(Math.floor(today.getTime() / 1000));
  const yk = dayKey(Math.floor(yesterday.getTime() / 1000));
  if (key === tk) return "今天";
  if (key === yk) return "昨天";
  return key;
}

/** 日期筛选下拉项（key="" 表示全部）。 */
export interface DayBucket {
  key: string;
  label: string;
  count: number;
}

/**
 * 按天聚合（保持首次出现顺序）。
 *
 * 用 Map 而不是"相邻两行比较"：置顶条目排在列表最前、可能来自更早的日期，
 * 相邻比较会把同一天切成好几段、出现重复的日期头。这里一天只出现一次，
 * 组内保持后端原有顺序（置顶仍在其所在组的前面）。
 */
export function dayBuckets(rows: ClipRow[], nowMs = Date.now()): DayBucket[] {
  const seen = new Map<string, DayBucket>();
  for (const r of rows) {
    const k = dayKey(r.ts);
    const hit = seen.get(k);
    if (hit) hit.count += 1;
    else seen.set(k, { key: k, label: dayLabel(k, nowMs), count: 1 });
  }
  return [...seen.values()];
}

/** 只保留某一天（key 为空 = 不过滤）。 */
export function filterByDay<T extends { ts: number }>(rows: T[], key: string): T[] {
  if (!key) return rows;
  return rows.filter((r) => dayKey(r.ts) === key);
}

/** 把当前页的行按天分组（渲染日期小标题用）。 */
export function groupRowsByDay(rows: ClipRow[]): { key: string; rows: ClipRow[] }[] {
  const groups = new Map<string, ClipRow[]>();
  for (const r of rows) {
    const k = dayKey(r.ts);
    const list = groups.get(k);
    if (list) list.push(r);
    else groups.set(k, [r]);
  }
  return [...groups.entries()].map(([key, rs]) => ({ key, rows: rs }));
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

/** 按天数清理的确认文案。
 *
 * 必须如实说明**置顶条目同样会过期**：Python purge_older_than 的语义是
 * 「置顶条目同样过期——按天是时间语义」，置顶只豁免数量淘汰（max_items），
 * 不豁免时间清理。文案若许诺「置顶保留」，用户会拿置顶当永久收藏然后被清掉。
 */
export function purgeConfirmText(days: number): string {
  return (
    `将删除 ${days} 天前的历史条目（不可撤销）。\n\n` +
    `置顶条目也会一并删除（置顶只豁免数量淘汰，不豁免按时间清理）。` +
    `图片文件同步删除。`
  );
}

/** 隐私暂停的说明（避免用户以为恢复后会补录）。 */
export function pauseHint(paused: boolean, enabled: boolean): string {
  if (!enabled) return "监听已关闭——点「开启监听」开始记录。";
  return paused
    ? "已暂停记录；恢复后不会补录暂停期间的内容（隐私考虑）。"
    : "暂停记录后，期间复制的内容不会被保存；恢复后也不会补录。";
}

/** 监听开关按钮的标签（三态：关→开；开且未暂停→暂停；已暂停→恢复）。 */
export function watchToggleLabel(enabled: boolean, paused: boolean): string {
  if (!enabled) return "开启监听";
  return paused ? "恢复记录" : "暂停记录";
}

/** 开启监听的确认文案（对齐 Python 版确认框：把记录范围一次说清）。 */
export function watchEnableConfirmText(): string {
  const nl = String.fromCharCode(10);
  return [
    "开启后将自动记录复制的：",
    "· 文本 / 图片 / 文件路径 / HTML；",
    "· 前台进程在黑名单内（如密码管理器）时**不记录**；",
    "· 密码库或本应用标记的敏感内容不进历史、不进 Win+V、到期自动清空；",
    "· 历史按保留天数自动清理，可随时暂停或一键清空。",
  ].join(nl);
}

/** 恢复记录的确认文案（不补录是隐私语义，必须说清）。 */
export function watchResumeConfirmText(): string {
  return "恢复记录后，暂停期间复制的内容**不会**补录（隐私考虑）。";
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

/** 行内「···」菜单的一项（纯数据：UI 映射为 antd Menu，vitest 可断言）。 */
export interface RowMenuItem {
  key: "preview" | "qrdecode" | "pin" | "save" | "actions" | "sep" | "remove";
  label: string;
  danger?: boolean;
  disabled?: boolean;
  /** 不可用原因（如图片文件已被清理）——拼进 label，让用户知道为什么置灰。 */
  disabledReason?: string;
}

/** 行操作收拢进「···」菜单后的项序：全文/识别二维码/置顶/另存为/动作 + 分隔 + 删除。 */
export function rowMenuItems(r: ClipRow): RowMenuItem[] {
  const items: RowMenuItem[] = [];
  // 图片条目的大图预览由缩略图（AntD Image）承担，全文只服务文本类条目
  if (r.kind !== "image" && r.text) items.push({ key: "preview", label: "📄 查看全文" });
  // 图片条目：本机二维码识别（截图里的 WiFi 码/链接码是剪贴板高频内容）
  if (r.kind === "image") items.push({ key: "qrdecode", label: "🔍 识别二维码" });
  items.push({ key: "pin", label: r.pinned ? "📌 取消置顶" : "📌 置顶" });
  const save = canSaveAs(r);
  items.push({
    key: "save",
    label: "💾 另存为",
    disabled: !save.ok,
    disabledReason: save.ok ? undefined : save.reason,
  });
  // 内容规则动作：识别这段文本能做什么（URL/二维码/JSON/颜色/路径/IP）
  items.push({ key: "actions", label: "🔍 识别可执行动作" });
  // 破坏性操作（删除）与其余动作隔开，防手滑
  items.push({ key: "sep", label: "" });
  items.push({ key: "remove", label: "🗑️ 删除", danger: true });
  return items;
}

// ---------------- 灯箱缩放（纯数值，测试覆盖） ----------------

/** 缩放下限：1 = 适应窗口（再小没有意义）。 */
export const ZOOM_MIN = 1;
/** 缩放上限：8 足够看清二维码/验证码的局部，再大只是看马赛克。 */
export const ZOOM_MAX = 8;

/** 夹取到 [1,8]；非法输入（NaN/负数/0）回 1。 */
export function clampZoom(v: number): number {
  if (!Number.isFinite(v) || v <= ZOOM_MIN) return ZOOM_MIN;
  return Math.min(ZOOM_MAX, v);
}

/** 以 factor 步进缩放（滚轮 1.15、按钮 1.25 都走这里）。 */
export function zoomBy(scale: number, factor: number): number {
  return clampZoom(scale * factor);
}
