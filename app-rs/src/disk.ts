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
  root: string | null;
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
