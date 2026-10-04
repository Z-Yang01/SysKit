/**
 * 诊断/修复页纯逻辑（无 React/DOM，vitest 覆盖）。
 *
 * 对应 Python 侧 repairs.py 的展示纪律：
 * - 风险三级分组（安全/改配置/强力(需重启)），标签文案与 Python `RISK_LABEL` 一致；
 * - **非管理员时改配置+ 必须显式标注「将被跳过」**，而不是等点了才失败；
 * - 组合修复的每一步都要显示「跳过及原因」——静默略过会让用户以为已执行。
 */

export type Risk = "safe" | "config" | "aggressive";

export interface ParamSpec {
  name: string;
  desc: string;
  required: boolean;
}

/** 后端 `sk_repair_list` 的动作行。 */
export interface RepairAction {
  id: string;
  title: string;
  risk: Risk;
  risk_label: string;
  description: string;
  restore_hint: string;
  brief: string;
  need_reboot: boolean;
  params: ParamSpec[];
}

export interface Combo {
  key: string;
  title: string;
  actions: string[];
}

export interface RepairList {
  actions: RepairAction[];
  combos: Combo[];
  is_admin: boolean;
}

/** 后端 `sk_repair_run` 的返回。 */
export interface RepairOutcome {
  success: boolean;
  message: string;
  need_reboot: boolean;
  output: string;
}

/** 后端 `sk_repair_plan` 的一步。 */
export interface ComboStep {
  id: string;
  title: string;
  risk: Risk;
  skipped_reason: string | null;
}

/** 风险分级顺序（与 Python RISK_SAFE/CONFIG/AGGRESSIVE 的严重度一致）。 */
export const RISK_ORDER: Risk[] = ["safe", "config", "aggressive"];

/** 语义色 token（复用主题 --sk-*，与 Python pal() 取色同源）。 */
export function riskToken(risk: Risk): string {
  switch (risk) {
    case "safe":
      return "ok";
    case "config":
      return "warn";
    case "aggressive":
      return "err";
  }
}

/** 风险中文标签（后端也返回，前端保留兜底以免字段缺失时空白）。 */
export function riskLabel(risk: Risk): string {
  switch (risk) {
    case "safe":
      return "安全";
    case "config":
      return "改配置";
    case "aggressive":
      return "强力(需重启)";
  }
}

/** 按风险分组，组内保持后端顺序（= Python ACTION_IDS 顺序）。 */
export function groupByRisk(actions: RepairAction[]): { risk: Risk; items: RepairAction[] }[] {
  return RISK_ORDER.map((risk) => ({
    risk,
    items: actions.filter((a) => a.risk === risk),
  })).filter((g) => g.items.length > 0);
}

/**
 * 非管理员时该动作是否会被跳过。
 *
 * 与后端 `combo_plan` 同一判据（只有 safe 免确认自动执行）：
 * 前端提前标注，用户点之前就知道哪些不会跑。
 */
export function willBeSkipped(action: RepairAction, isAdmin: boolean): boolean {
  return !isAdmin && action.risk !== "safe";
}

/** 跳过原因文案（与后端一致的说明）。 */
export function skipReason(action: RepairAction, isAdmin: boolean): string | null {
  if (!willBeSkipped(action, isAdmin)) return null;
  return `${action.risk === "aggressive" ? "强力" : "改配置"}类动作需要管理员权限，将被跳过`;
}

/** 动作执行前的确认文案（改配置/强力类必须点确认）。 */
export function confirmText(action: RepairAction): string {
  const notes: string[] = [action.description];
  if (action.restore_hint && action.restore_hint !== "无") {
    notes.push(`还原方式：${action.restore_hint}`);
  }
  if (action.need_reboot) {
    notes.push("⚠ 执行后需要重启电脑才能生效。");
  }
  return notes.join("\n\n");
}

/** 动作是否必须弹确认框（安全类免确认，与后端 auto_ok 语义对齐）。 */
export function needsConfirm(risk: Risk): boolean {
  return risk !== "safe";
}

/** 结果 → 展示用的提示级别。 */
export function outcomeLevel(outcome: RepairOutcome): "success" | "warning" | "error" {
  if (outcome.success) return "success";
  // 「尚未接入」不是用户操作失败，用 warning 更准确，避免误导
  if (outcome.message.includes("尚未")) return "warning";
  return "error";
}

/**
 * 动作结果 → 折叠面板标签上的 Tag 颜色与文案。
 *
 * 统一收口「尚未接入显示橙色『未接入』」的判定——此前这段 includes("尚未")
 * 散落在组件里写了两遍，改文案时容易漏改一处。
 */
export function outcomeTag(outcome: RepairOutcome): {
  color: "green" | "orange" | "red";
  text: string;
} {
  const level = outcomeLevel(outcome);
  if (level === "success") return { color: "green", text: "已成功" };
  if (level === "warning") return { color: "orange", text: "未接入" };
  return { color: "red", text: "失败" };
}

/** 组合计划摘要：执行/跳过计数。 */
export function summarizePlan(steps: ComboStep[]): {
  run: number;
  skipped: number;
  needsReboot: boolean;
} {
  let run = 0;
  let skipped = 0;
  for (const s of steps) {
    if (s.skipped_reason) skipped += 1;
    else run += 1;
  }
  return { run, skipped, needsReboot: steps.some((s) => s.risk === "aggressive") };
}

// ---------------- 诊断结果展示（netcheck 15 项） ----------------

export type CheckStatus = "pass" | "warn" | "fail" | "skip" | "info";

/** 后端 `sk_diag_run` 的单条结果。 */
export interface CheckResult {
  id: string;
  name: string;
  status: CheckStatus;
  detail: string;
  fix_ids: string[];
}

export interface DiagReport {
  results: CheckResult[];
  verdict: string;
  verdict_text: string;
  counts: Record<string, number>;
  quick: boolean;
}

/** 状态中文标签。 */
export function statusLabel(status: CheckStatus): string {
  switch (status) {
    case "pass":
      return "通过";
    case "warn":
      return "警告";
    case "fail":
      return "失败";
    case "skip":
      return "跳过";
    case "info":
      return "提示";
  }
}

/** 状态符号（与 Python STATUS_MARK 逐字一致）。 */
export function statusMark(status: CheckStatus): string {
  switch (status) {
    case "pass":
      return "√";
    case "warn":
      return "!";
    case "fail":
      return "×";
    case "skip":
      return "-";
    case "info":
      return "·";
  }
}

/** 状态语义色 token（复用 --sk-*）。 */
export function statusToken(status: CheckStatus): string {
  switch (status) {
    case "pass":
      return "ok";
    case "warn":
      return "warn";
    case "fail":
      return "err";
    default:
      return "sub";
  }
}

/** AntD Tag 颜色。 */
export function statusTagColor(status: CheckStatus): string {
  switch (status) {
    case "pass":
      return "green";
    case "warn":
      return "orange";
    case "fail":
      return "red";
    case "skip":
      return "default";
    default:
      return "blue";
  }
}

/** 综合判定 → 语义色（normal 绿 / 误报与 DNS 橙 / 无外网红）。 */
export function verdictColor(verdict: string): string {
  switch (verdict) {
    case "normal":
      return "green";
    case "ncsi_false_alarm":
    case "dns_broken":
      return "orange";
    case "no_internet":
      return "red";
    default:
      return "default";
  }
}

/**
 * 摘要计数：把后端 counts 补齐成固定顺序，缺失记 0。
 *
 * 固定顺序很重要——否则「0 个失败」会被隐藏，用户看不出"全过"还是"没跑"。
 */
export function summarizeDiag(report: DiagReport | null): {
  status: CheckStatus;
  label: string;
  count: number;
}[] {
  const order: CheckStatus[] = ["pass", "warn", "fail", "skip", "info"];
  const labels: Record<CheckStatus, string> = {
    pass: "通过",
    warn: "警告",
    fail: "失败",
    skip: "跳过",
    info: "提示",
  };
  return order.map((s) => ({
    status: s,
    label: labels[s],
    count: report?.counts?.[s] ?? 0,
  }));
}

/** 按状态排序：失败 → 警告 → 提示 → 跳过 → 通过（让问题顶到上面）。 */
export function sortBySeverity(results: CheckResult[]): CheckResult[] {
  const rank: Record<CheckStatus, number> = {
    fail: 0,
    warn: 1,
    info: 2,
    skip: 3,
    pass: 4,
  };
  return [...results].sort((a, b) => rank[a.status] - rank[b.status]);
}

/**
 * 报告里是否有值得用户去点的修复建议。
 *
 * 传入 `knownIds`（动作库里确实存在的 id）时只返回有入口可跳的——
 * 体检的 fix_ids 可能指向尚未移植的动作（如 restart_wlan），
 * 这种给个链接点进去是空的，不如直接不推荐。
 */
export function actionableFixes(
  report: DiagReport | null,
  knownIds?: Iterable<string>,
): string[] {
  if (!report) return [];
  const known = knownIds ? new Set(knownIds) : null;
  const seen = new Set<string>();
  for (const r of report.results) {
    for (const id of r.fix_ids) {
      if (known && !known.has(id)) continue;
      seen.add(id);
    }
  }
  return [...seen];
}

/**
 * 单条检查结果 → 复制到剪贴板的纯文本（表格行里的「复制」按钮用）。
 *
 * 带上状态与建议动作，贴给别人才是自包含的一段信息，而不是光秃秃的 detail。
 */
export function formatCheckForCopy(r: CheckResult): string {
  const lines = [`【${statusLabel(r.status)}】${r.name}：${r.detail}`];
  if (r.fix_ids.length > 0) {
    lines.push(`建议动作：${r.fix_ids.join("、")}`);
  }
  return lines.join("\n");
}

/**
 * 整份报告 → 复制到剪贴板的纯文本（对齐 Python `_report_lines` 的格式）。
 *
 * 时间戳可注入便于测试；格式与 Python 逐行一致（报告头 / [√] 标题 / 缩进详情 /
 * 分隔线 / 结论行），用户把 Rust 版报告贴进旧的求助帖也不违和。
 */
export function formatReportForCopy(report: DiagReport, now: Date = new Date()): string {
  const p = (n: number) => String(n).padStart(2, "0");
  const stamp = `${now.getFullYear()}-${p(now.getMonth() + 1)}-${p(now.getDate())} ${p(
    now.getHours(),
  )}:${p(now.getMinutes())}`;
  const lines = [`SysKit 诊断报告  ${stamp}`, "-".repeat(50)];
  for (const r of report.results) {
    lines.push(`[${statusMark(r.status)}] ${r.name}`);
    lines.push(`      ${r.detail}`);
  }
  lines.push("-".repeat(50));
  lines.push(`结论：${report.verdict_text}`);
  return lines.join("\n");
}

// ---------------- 修复记录（手动 / 组合 / 后台监控自动修复共用） ----------------

/** 后端 monitor.rs `sk://monitor-event` 的事件载荷。 */
export interface MonitorEvent {
  /** action=自动修复动作已执行；suggest=建议手动修复；recovered=修复后恢复 */
  kind: "action" | "suggest" | "recovered";
  id: string;
  title: string;
  ok: boolean;
  message: string;
}

/** 按字符数截断（对齐 Python `message[:90]` 的展示纪律，中文按 1 字符计）。 */
export function clipText(s: string, maxChars: number): string {
  return [...s].slice(0, maxChars).join("");
}

/** 修复记录条目。 */
export interface RepairRecord {
  /** HH:MM:SS */
  time: string;
  mark: "√" | "×" | "!";
  text: string;
}

/** 事件 → 记录行（mark 语义与 Python `_add_repair_record` 一致：√成 ×败 !建议）。 */
export function formatMonitorEvent(ev: MonitorEvent): { mark: "√" | "×" | "!"; text: string } {
  if (ev.kind === "suggest") {
    return { mark: "!", text: `${ev.title}（建议手动修复）：${clipText(ev.message, 90)}` };
  }
  if (ev.kind === "recovered") {
    return { mark: "√", text: clipText(ev.message || ev.title, 90) };
  }
  return { mark: ev.ok ? "√" : "×", text: `${ev.title} — ${clipText(ev.message, 90)}` };
}

/** 本地时钟 → HH:MM:SS（记录行前缀，注入 Date 便于测试）。 */
export function hhmmss(d: Date): string {
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

/**
 * 追加一条记录（新的在前）并截断到 max 条。
 *
 * 上限 60 对齐 Python `list_repair_log`：无上限的话挂机几天的记录会把列表
 * 撑到卡顿，而排查只关心最近发生了什么。
 */
export function pushRecord(list: RepairRecord[], rec: RepairRecord, max = 60): RepairRecord[] {
  const next = [rec, ...list];
  return next.length > max ? next.slice(0, max) : next;
}

// ---------------- 「上次结果」的会话内保存 ----------------

/**
 * 诊断报告的 sessionStorage 键。
 *
 * Python 侧 `_diag_results` 在窗口存活期间一直保留（切页不丢）；Rust 前端切页
 * 会卸载组件，用 sessionStorage 补上同语义——关窗即清，不把诊断结论（含内网
 * 信息）写进持久存储。
 */
export const REPORT_STORAGE_KEY = "syskit.diag.report.v1";

/** 序列化报告到注入的 writer（Node 测试环境没有 sessionStorage，注入便于测试）。 */
export function saveReportTo(report: DiagReport, set: (key: string, value: string) => void): void {
  try {
    set(REPORT_STORAGE_KEY, JSON.stringify(report));
  } catch {
    // 存不进就算了：「上次结果」是便利性恢复，不值得为它打扰用户
  }
}

/** 从注入的 reader 恢复报告；损坏/缺字段一律返回 null（绝不把脏数据当报告）。 */
export function loadReportFrom(get: (key: string) => string | null): DiagReport | null {
  try {
    const raw = get(REPORT_STORAGE_KEY);
    if (!raw) return null;
    const obj = JSON.parse(raw) as DiagReport;
    if (!Array.isArray(obj.results) || typeof obj.verdict !== "string") return null;
    return obj;
  } catch {
    return null;
  }
}

/** 会话保存（webview 的 sessionStorage）。 */
export function saveReport(report: DiagReport): void {
  saveReportTo(report, (k, v) => sessionStorage.setItem(k, v));
}

/** 会话恢复（webview 的 sessionStorage）。 */
export function loadReport(): DiagReport | null {
  return loadReportFrom((k) => sessionStorage.getItem(k));
}

