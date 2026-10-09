/**
 * 磁盘空间与垃圾清理纯逻辑（无 React/DOM，vitest 覆盖）。
 * 对应 Python 侧 `DiskTab` / `CleanupTab`。
 *
 * 清理是**不可逆**操作，所以这里的校验与文案不是装饰：
 * - 未勾选任何项就点清理必须被拦住；
 * - 结果必须如实汇报"跳过了多少"——只报"清理成功"会让用户以为清干净了，
 *   而正在使用的文件其实一个都没动。
 */

/** 后端 `sk_disk_list` 的一行。 */
export interface DriveRow {
  letter: string;
  label: string;
  fs: string;
  total: number;
  used: number;
  free_total: number;
  free_to_caller: number;
  used_percent: number;
  removable: boolean;
  token: string;
}

export interface DiskList {
  rows: DriveRow[];
  count: number;
}

/** 后端 `sk_clean_targets` / `sk_clean_scan` 的清理目标。 */
export interface CleanTargetInfo {
  key: string;
  title: string;
  /** v1.3：多根目录（崩溃转储在用户与系统两处）；旧后端单根形态兼容 root */
  roots?: string[];
  root?: string | null;
  bytes?: number;
  human?: string;
}

export interface CleanScan {
  rows: CleanTargetInfo[];
  total_bytes: number;
  total_human: string;
}

/** 后端 `sk_clean_run` 的单目标结果。 */
export interface CleanTargetResult {
  key: string;
  title: string;
  ok: boolean;
  deleted?: number;
  freed?: number;
  failed?: number;
  message: string;
}

export interface CleanRunResult {
  summary: string;
  per_target: CleanTargetResult[];
  outcome: { deleted: number; freed: number; failed: number };
}

// ---------------- 磁盘 ----------------

/** 容量人类可读（与后端 `human_size` 同口径）。 */
export function humanSize(bytes: number): string {
  const UNITS = ["B", "KB", "MB", "GB", "TB"];
  let v = bytes;
  let i = 0;
  while (v >= 1024 && i < UNITS.length - 1) {
    v /= 1024;
    i += 1;
  }
  if (i === 0) return `${bytes} ${UNITS[i]}`;
  return v >= 100 ? `${v.toFixed(0)} ${UNITS[i]}` : `${v.toFixed(1)} ${UNITS[i]}`;
}

/** 用量比例 → AntD Tag 颜色。 */
export function usageColor(token: string): string {
  switch (token) {
    case "ok":
      return "green";
    case "warn":
      return "orange";
    case "err":
      return "red";
    default:
      return "default";
  }
}

// ---------------- 目录逐级展开（下钻分析） ----------------

/** 后端 `sk_disk_scan_children` 的一行子项（目录或文件）。 */
export interface ChildEntry {
  name: string;
  path: string;
  /** 目录=直接文件合计；文件=自身大小 */
  size: number;
  /** 目录=递归总大小；文件=自身大小 */
  total: number;
  file_count: number;
  error_count: number;
  is_dir: boolean;
}

/** 后端 `sk_disk_scan_children` 的返回：某目录直接子项的单层摘要。 */
export interface ChildrenScan {
  path: string;
  name: string;
  size: number;
  total: number;
  file_count: number;
  error_count: number;
  children: ChildEntry[];
  truncated: boolean;
  ext_stats: [string, number][];
  top_files: { size: number; path: string }[];
  duration_ms: number;
  cancelled: boolean;
}

/** 子项行排序：按 total 降序（目录=递归、文件=自身），并列按名称升序稳定。 */
export function sortChildRows(rows: ChildEntry[]): ChildEntry[] {
  return [...rows].sort(
    (a, b) => b.total - a.total || a.name.localeCompare(b.name),
  );
}

/** 行首图标（与剪贴板/粘贴面板的字符图标同风格，不引图标库）。 */
export function childIcon(row: Pick<ChildEntry, "is_dir">): string {
  return row.is_dir ? "📁" : "📄";
}

/** 展开状态翻转（仅内存态，不持久化）。 */
export function toggleExpanded(
  expanded: Record<string, boolean>,
  path: string,
): Record<string, boolean> {
  const next = { ...expanded };
  if (next[path]) delete next[path];
  else next[path] = true;
  return next;
}

/** 展开失败的回退：把该行收回起态（不得卡在「已展开但没内容」的假态）。 */
export function collapsePath(
  expanded: Record<string, boolean>,
  path: string,
): Record<string, boolean> {
  const next = { ...expanded };
  delete next[path];
  return next;
}

/** 拍平后的可见行：普通子项行，或「已展开但结果未到」的加载占位行。 */
export interface FlatChildRow {
  kind: "row" | "loading";
  depth: number;
  row?: ChildEntry;
  path?: string;
}

/**
 * 把根层子项与各已展开目录的子项拍平成带缩进层级的可见行序列。
 * 展开但扫描未返回的目录输出 loading 占位行（行内「扫描中…」就靠它）。
 */
export function flattenChildRows(
  root: ChildrenScan | null,
  expanded: Record<string, boolean>,
  scans: Record<string, ChildrenScan>,
): FlatChildRow[] {
  const out: FlatChildRow[] = [];
  const walk = (rows: ChildEntry[], depth: number) => {
    for (const row of sortChildRows(rows)) {
      out.push({ kind: "row", row, depth });
      if (row.is_dir && expanded[row.path]) {
        const sub = scans[row.path];
        if (sub) walk(sub.children, depth + 1);
        else out.push({ kind: "loading", depth: depth + 1, path: row.path });
      }
    }
  };
  if (root) walk(root.children, 0);
  return out;
}

/** 下钻扫描失败的提示文案（toast 用）。 */
export function drillErrorText(path: string, err: unknown): string {
  return `展开 ${path} 失败：${String(err)}`;
}

/**
 * 根扫描的表格空态文案。取消残留（cancelled=true，children 通常为空）
 * 不得冒充「空目录」——轮次112 审计实锤：根扫描点「取消扫描」后表格
 * 只剩「（空目录）」，与轮次111「取消残留如实标注」的承诺相悖。
 */
export function rootEmptyText(root: ChildrenScan | null): string {
  if (!root) return "输入路径后点「开始扫描」";
  return root.cancelled ? "扫描已取消（结果不完整）" : "（空目录）";
}

/**
 * 截断提示（根概要行与深层展开行共用）：null=未截断。深层目录被
 * MAX_CHILD_ROWS 截断时过去只有根层提示，下钻行静默丢 500 名之后
 * 的小项——同一文案现在两处都标（轮次112 审计）。
 */
export function truncatedNote(
  scan: Pick<ChildrenScan, "children" | "truncated">,
): string | null {
  return scan.truncated ? `仅显示最大的前 ${scan.children.length} 项` : null;
}

// ---------------- 磁盘分析总览（sk_disk_scan_tree） ----------------

/**
 * 后端 `sk_disk_scan_tree` 的树节点。**只有目录入树**——文件被聚合进
 * 所在目录的 size/file_count（与后端 scan_tree 一致），所以一级 children
 * 全是目录、全部可下钻。
 */
export interface DirNode {
  name: string;
  path: string;
  /** 直接文件大小合计（不含子目录） */
  size: number;
  /** 递归总大小（含全部子目录） */
  total: number;
  file_count: number;
  error_count: number;
  children: DirNode[];
}

/** 后端 `sk_disk_scan_tree` 的返回（全量树 + 类型分布 + 大文件 Top200）。 */
export interface ScanTreeResult {
  root: DirNode;
  /** 扩展名（小写、无点；无后缀归「无后缀」）→ 总字节，后端按字节降序 */
  ext_stats: [string, number][];
  /** 大文件 Top200（后端 TOP_FILES_LIMIT=200，按大小降序） */
  top_files: { size: number; path: string }[];
  file_count: number;
  error_count: number;
  duration_ms: number;
  cancelled: boolean;
}

/** 树节点（目录）计数：降级决策的输入。 */
export function countTreeNodes(root: DirNode): number {
  return 1 + root.children.reduce((sum, c) => sum + countTreeNodes(c), 0);
}

/** 一级条形/扩展名条形最多渲染行数（防单目录几千子项撑爆 DOM）。 */
export const OVERVIEW_MAX_ROWS = 100;

/**
 * 大树降级阈值（目录节点数）。轮次113 实测：C:\Windows\System32 全树
 * 1769 节点≈0.37MB JSON；真实用户目录 152,754 节点≈34MB JSON/45 秒。
 * 5000 在两个实测档之间留足余量：超过即降级提示。
 */
export const TREE_NODE_LIMIT = 5000;

/** 大树降级决策：超过阈值只提示「总览仅一级 + 用下钻面板」。
 *  「不整树渲染」不靠本判定——总览**无论是否降级都只渲染一级**（结构保证，
 *  见 firstLevelBars），降级只决定要不要向用户解释为什么没有更深层。 */
export interface TreeGuard {
  degraded: boolean;
  note: string | null;
}

export function treeGuard(nodeCount: number): TreeGuard {
  if (nodeCount > TREE_NODE_LIMIT) {
    return {
      degraded: true,
      note: `目录树过大（${nodeCount} 个目录），总览仅显示一级子目录，更深层请用下方「目录逐级展开」逐级下钻`,
    };
  }
  return { degraded: false, note: null };
}

/** 条形宽度百分比（0-100）：total≤0 或 value≤0 → 0；非零值至少 1%
 *  （否则小项宽度为 0 完全不可见，用户会以为没占空间）。 */
export function barPercent(value: number, total: number): number {
  if (total <= 0 || value <= 0) return 0;
  return Math.min(100, Math.max(1, (value / total) * 100));
}

/** 总览条形的一行（一级子目录或扩展名）。 */
export interface OverviewBarRow {
  label: string;
  /** 子目录行=可下钻路径；扩展名行=空串 */
  path: string;
  bytes: number;
  percent: number;
}

/**
 * 根的一级子目录条形行：total 降序（并列按名称稳定），占比分母=根递归总大小。
 * **只取 children 一层、绝不递归**——这是「树再大 DOM 也不膨胀」的结构锁。
 */
export function firstLevelBars(
  root: DirNode,
  maxRows: number = OVERVIEW_MAX_ROWS,
): { rows: OverviewBarRow[]; hidden: number } {
  const sorted = [...root.children].sort(
    (a, b) => b.total - a.total || a.name.localeCompare(b.name),
  );
  const rows = sorted.slice(0, maxRows).map<OverviewBarRow>((c) => ({
    label: c.name,
    path: c.path,
    bytes: c.total,
    percent: barPercent(c.total, root.total),
  }));
  return { rows, hidden: Math.max(0, sorted.length - maxRows) };
}

/** 扩展名统计条形行：后端已降序，这里防御性重排（契约锁双保险）+ 截 Top-N。 */
export function extBars(
  extStats: [string, number][],
  maxRows: number,
): { rows: OverviewBarRow[]; hidden: number } {
  const total = extStats.reduce((s, [, b]) => s + b, 0);
  const sorted = [...extStats].sort(
    (a, b) => b[1] - a[1] || a[0].localeCompare(b[0]),
  );
  const rows = sorted.slice(0, maxRows).map<OverviewBarRow>(([ext, bytes]) => ({
    label: ext,
    path: "",
    bytes,
    percent: barPercent(bytes, total),
  }));
  return { rows, hidden: Math.max(0, sorted.length - maxRows) };
}

/** Top200 展示行：名次从 1 起 + 人类可读大小（后端已按大小降序）。 */
export function topFileRows(files: { size: number; path: string }[]): {
  rank: number;
  size: number;
  human: string;
  path: string;
}[] {
  return files.map((f, i) => ({
    rank: i + 1,
    size: f.size,
    human: humanSize(f.size),
    path: f.path,
  }));
}

/** 后端大文件上限（diskusage.rs TOP_FILES_LIMIT，前端无法读取只能对齐）。 */
export const TOP_FILES_BACKEND_LIMIT = 200;

/** Top200 截断提示（null=未截断）：列表长度顶到后端上限时如实说明。 */
export function topFilesNote(files: { size: number; path: string }[]): string | null {
  return files.length >= TOP_FILES_BACKEND_LIMIT
    ? `仅记录最大的前 ${files.length} 个文件`
    : null;
}

// ---------------- 总览 → 下钻面板联动 ----------------

/** 总览点击子目录 → 下钻面板换根请求；token 每次请求都不同。 */
export interface DrillRequest {
  path: string;
  token: number;
}

/**
 * 联动换根作废判定：token 变化才算新请求（同一路径重复点击也换根=刷新）；
 * applied 为 null（从未应用过）时任何非空请求都触发；next 为 null（初始态）
 * 不触发。下钻面板据此调用既有换根扫描（isRoot 分支作废全部缓存）。
 */
export function drillResetNeeded(
  applied: DrillRequest | null,
  next: DrillRequest | null,
): boolean {
  if (!next) return false;
  return !applied || applied.token !== next.token;
}

/** 卷显示名：`C:` + 卷标。 */
export function driveLabel(d: DriveRow): string {
  return d.label ? `${d.letter} ${d.label}` : d.letter;
}

/** 是否需要提醒空间不足（可移动盘不提醒——U 盘常满且不是系统盘）。 */
export function needsSpaceWarning(d: DriveRow): boolean {
  return !d.removable && d.used_percent >= 90;
}

/** 磁盘摘要：最紧张的固定盘。 */
export function diskSummary(rows: DriveRow[]): string {
  const fixed = rows.filter((d) => !d.removable);
  if (fixed.length === 0) return "没有本地固定磁盘";
  const tightest = fixed.reduce((a, b) => (a.used_percent >= b.used_percent ? a : b));
  return `${fixed.length} 个本地卷 · 最紧张 ${tightest.letter} 已用 ${tightest.used_percent.toFixed(0)}%（剩余 ${humanSize(tightest.free_total)}）`;
}

// ---------------- 清理 ----------------

/** 全部目标的 key（"全选"用）。 */
export function allTargetKeys(rows: CleanTargetInfo[]): string[] {
  return rows.map((r) => r.key);
}

/** 有内容（bytes > 0）的目标 key（"只选有内容的"用——清 0 字节的目标毫无收益）。 */
export function meaningfulKeys(rows: CleanTargetInfo[]): string[] {
  return rows.filter((r) => (r.bytes ?? 0) > 0).map((r) => r.key);
}

/** 单行点选/取消（勾选列表支持点整行切换时的状态翻转）。 */
export function toggleKey(selected: string[], key: string): string[] {
  return selected.includes(key) ? selected.filter((k) => k !== key) : [...selected, key];
}

/** 选中项合计大小（用于确认框）。 */
export function selectedBytes(rows: CleanTargetInfo[], selected: string[]): number {
  return rows
    .filter((r) => selected.includes(r.key))
    .reduce((sum, r) => sum + (r.bytes ?? 0), 0);
}

/** 清理前的校验。 */
export function validateClean(selected: string[]): { ok: boolean; reason: string } {
  if (selected.length === 0) {
    return { ok: false, reason: "请先勾选要清理的项目" };
  }
  return { ok: true, reason: "" };
}

/** 清理确认文案（说明不可逆与跳过行为）。 */
export function cleanConfirmText(rows: CleanTargetInfo[], selected: string[]): string {
  const picked = rows.filter((r) => selected.includes(r.key));
  const total = selectedBytes(rows, selected);
  const names = picked.map((r) => r.title).join("、");
  return (
    `将清理：${names}\n` +
    `预计释放约 ${humanSize(total)}（实际以扫描时为准）。\n\n` +
    `只删除这些位置下的**文件**，不会删除目录；正在使用的文件会自动跳过。\n` +
    `删除不可撤销，但这些都是缓存/临时文件，系统会按需重建。`
  );
}

/** 清理结果是否"部分成功"（有跳过项）。 */
export function hasSkipped(result: CleanRunResult): boolean {
  return (result.outcome?.failed ?? 0) > 0;
}

/** 结果摘要：如实包含跳过数。 */
export function cleanResultSummary(result: CleanRunResult): string {
  if (!result) return "";
  const o = result.outcome ?? { deleted: 0, freed: 0, failed: 0 };
  let s = `已删除 ${o.deleted} 个文件，释放 ${humanSize(o.freed)}`;
  if (o.failed > 0) {
    s += `；${o.failed} 个文件被占用或无权限，已跳过（正在使用的文件属正常）`;
  }
  return s;
}

/** 单目标结果 → 展示行文案。 */
export function targetResultText(r: CleanTargetResult): string {
  if (r.key === "recycle") return r.message;
  const freed = humanSize(r.freed ?? 0);
  const extra = (r.failed ?? 0) > 0 ? `（跳过 ${r.failed}）` : "";
  return `${r.deleted ?? 0} 个文件 / ${freed}${extra}`;
}
