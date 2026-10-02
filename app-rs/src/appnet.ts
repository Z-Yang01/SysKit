/**
 * 应用联网页纯逻辑（无 React/DOM，vitest 覆盖）。
 *
 * 核心是**三态展示的诚实性**：`drift`（规则还在但动作被改成"允许"）绝不能被显示成
 * "已禁止联网"——用户会以为程序已经被拦住，实际上它正畅通无阻。这是安全控制类
 * 功能里最不能犯的错。
 */

/** 规则实际状态（与后端 `RuleState` 对应）。 */
export type RuleState = "on" | "off" | "drift" | "missing";

/** 后端 `sk_appnet_list` 的一行。 */
export interface AppNetRow {
  program: string;
  rule: string;
  blocked: boolean;
  added: string;
  state: RuleState;
  blocking: boolean;
}

export interface AppNetList {
  rows: AppNetRow[];
  count: number;
  is_admin: boolean;
}

/** 后端 `sk_appnet_validate`。 */
export interface AppNetValidate {
  program: string;
  exists: boolean;
  rule: string;
  error: string;
  is_admin: boolean;
  admin_msg: string;
}

/** 状态 → 展示文案。`drift` 必须与 `off` 区分开。 */
export function stateLabel(state: RuleState): string {
  switch (state) {
    case "on":
      return "已禁止联网";
    case "off":
      return "已放行（规则停用）";
    case "drift":
      return "已失效：动作被改成了允许";
    case "missing":
      return "规则不存在";
  }
}

/** 状态 → AntD Tag 颜色。 */
export function stateColor(state: RuleState): string {
  switch (state) {
    case "on":
      return "green";
    case "off":
      return "default";
    case "drift":
      return "red";
    case "missing":
      return "orange";
  }
}

/**
 * 状态是否需要提醒用户。`drift` 与 `missing` 都算——台账说有、实际拦不住。
 */
export function needsAttention(row: AppNetRow): boolean {
  return row.state === "drift" || row.state === "missing";
}

/**
 * 该行**是否真的在阻断**。以实际状态为准，不看台账的愿望值。
 *
 * 台账 `blocked=true` 只表示"我们当初想拦"；用户可能在 wf.msc 里改掉。
 * 显示必须依赖 `state`。
 */
export function isReallyBlocking(row: AppNetRow): boolean {
  return row.state === "on";
}

/** 提醒文案（表格上方）。 */
export function attentionNotice(rows: AppNetRow[]): string {
  const drift = rows.filter((r) => r.state === "drift").length;
  const missing = rows.filter((r) => r.state === "missing").length;
  const parts: string[] = [];
  if (drift > 0) {
    parts.push(
      `${drift} 条规则的动作被外部改成了「允许」，当前**并没有**在阻断——请重新应用或删除`,
    );
  }
  if (missing > 0) {
    parts.push(`${missing} 条规则已不在系统里（台账残留），可点「移除」清理`);
  }
  return parts.join("；");
}

/** 统计：真的在拦 / 已放行 / 有问题。 */
export function appNetStats(rows: AppNetRow[]): {
  total: number;
  blocking: number;
  released: number;
  attention: number;
} {
  let blocking = 0;
  let released = 0;
  let attention = 0;
  for (const r of rows) {
    if (isReallyBlocking(r)) blocking += 1;
    else if (r.state === "off") released += 1;
    else attention += 1;
  }
  return { total: rows.length, blocking, released, attention };
}

/** 非管理员时的前置提示（与后端 ADMIND_MSG 同义）。 */
export function adminNotice(list: AppNetList | null, fallback: string): string {
  if (!list) return "";
  return list.is_admin ? "" : fallback;
}

/** 从完整路径取文件名（表格里路径太长）。 */
export function baseName(program: string): string {
  const parts = program.split(/[\\/]/);
  return parts[parts.length - 1] || program;
}

/** 目录部分（展示用）。 */
export function dirName(program: string): string {
  const idx = Math.max(program.lastIndexOf("\\"), program.lastIndexOf("/"));
  return idx > 0 ? program.slice(0, idx) : "";
}

/** 确认文案：阻断一个程序的影响（避免用户以为"只是限速"）。 */
export function blockConfirmText(program: string): string {
  return (
    `确定禁止 ${baseName(program)} 联网吗？\n\n` +
    `会添加一条出站阻断的 Windows 防火墙规则；该程序的联网功能将不可用` +
    `（包括自动更新、云同步、登录）。可随时点「放行」或「移除」恢复。`
  );
}
