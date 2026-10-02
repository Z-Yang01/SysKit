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
