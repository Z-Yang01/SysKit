/**
 * 场景模式纯逻辑（无 React/DOM，vitest 覆盖）。对应 Python 侧 ModesTab。
 *
 * 两个诚实性要求：
 * 1. 结果必须**如实列出失败步骤**——只说"已进入游戏模式"会让用户以为全部生效，
 *    而"暂停更新需要管理员"这类步骤其实没做；
 * 2. "暂停更新"的文案不得暗示到期自动恢复（`NoAutoUpdate` 是持久策略）。
 */

/** 后端 `sk_modes_run` 的单步结果。 */
export interface ModeStep {
  name: string;
  ok: boolean;
  detail: string;
}

export interface ModeOutcome {
  mode: string;
  steps: ModeStep[];
  ok_count: number;
  fail_count: number;
}

export interface ModeRunResult {
  outcome: ModeOutcome;
  summary: string;
}

/** 后端 `sk_modes_info`。 */
export interface ModesInfo {
  common_apps: { name: string; target: string }[];
  office_default_apps: string[];
  whitelist: string[];
  is_admin: boolean;
  notes: Record<string, string>;
}

export type ModeKey = "office" | "office_exit" | "game" | "game_exit";

/** 模式 → 按钮文案。 */
export function modeLabel(key: ModeKey): string {
  switch (key) {
    case "office":
      return "进入办公模式";
    case "office_exit":
      return "退出办公模式";
    case "game":
      return "进入游戏模式";
    case "game_exit":
      return "退出游戏模式";
  }
}

/** 该模式是否"进入"（用于确认框语气：进入会改系统状态，退出是恢复）。 */
export function isEntering(key: ModeKey): boolean {
  return key === "office" || key === "game";
}

/** 结果整体级别：全成 → success，部分失败 → warning（不是 error，因为多数步骤生效了）。 */
export function outcomeLevel(o: ModeOutcome | null): "success" | "warning" | "error" {
  if (!o) return "success";
  if (o.fail_count === 0) return "success";
  if (o.ok_count > 0) return "warning";
  return "error";
}

/** 汇总文案：**必须点名失败步骤**。 */
export function outcomeSummary(o: ModeOutcome | null): string {
  if (!o) return "尚未执行模式";
  if (o.fail_count === 0) {
    return `${o.mode}：${o.steps.length} 步全部完成`;
  }
  const failed = o.steps.filter((s) => !s.ok).map((s) => s.name);
  return `${o.mode}：${o.ok_count}/${o.steps.length} 步成功；失败：${failed.join("、")}`;
}

/** 失败步骤的详情（供展开显示）。 */
export function failedSteps(o: ModeOutcome | null): ModeStep[] {
  if (!o) return [];
  return o.steps.filter((s) => !s.ok);
}

/** 进入模式的确认文案（说明会改哪些系统状态）。 */
export function enterConfirmText(key: ModeKey, opts: { openApps: boolean; clearBg: boolean }): string {
  if (key === "office") {
    const extra = opts.openApps
      ? "\n· 打开常用软件（记事本 / 计算器 / 资源管理器 / 画图）"
      : "\n· （不自动打开软件）";
    return (
      "进入办公模式会做：\n" +
      "· 关闭系统通知（勿扰）\n" +
      "· 系统静音（退出时会恢复进模式前的音量）\n" +
      "· 备份当前配置" +
      extra +
      "\n\n随时可点「退出办公模式」恢复通知与音量。"
    );
  }
  const extra = opts.clearBg
    ? "\n· 结束后台白名单应用（仅限 OneDrive / 音乐 / 游戏启动器等，不碰系统进程）"
    : "\n· （不清理后台）";
  return (
    "进入游戏模式会做：\n" +
    "· 关闭系统通知\n" +
    "· 切换到高性能电源计划\n" +
    "· 暂停 Windows 更新（持久策略，需退出模式或手动恢复）" +
    extra +
    "\n\n退出模式会恢复通知、平衡电源与更新策略。"
  );
}

/** 更新策略提示：**不得**暗示到期自动恢复。 */
export function updatePolicyNote(info: ModesInfo | null): string {
  return (
    info?.notes?.update_policy ??
    "「暂停更新」是持久策略（不是暂停 N 天），需退出游戏模式或手动恢复。"
  );
}

/** 无管理员时的提示。 */
export function adminNote(info: ModesInfo | null): string {
  if (!info || info.is_admin) return "";
  return info.notes?.admin ?? "暂停更新与部分电源操作需要管理员权限。";
}
