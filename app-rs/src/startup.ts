/**
 * 启动项管理纯逻辑（无 React/DOM，vitest 覆盖）。对应 Python 侧 StartupTab。
 *
 * 禁用机制决定了展示上的两个要点：
 * 1. 已禁用项必须显示**原名** + 禁用态（后端已剥 `.disabled` 后缀），否则启用按钮
 *    永远点不到——它只在禁用态生效；
 * 2. HKLM 项需要管理员，非管理员时应提前禁用按钮并说明原因，
 *    而不是让用户点了才弹"失败"。
 */

/** 后端 `sk_startup_list` 的一行。 */
export interface StartupItem {
  location: string;
  name: string;
  command: string;
  enabled: boolean;
}

export interface StartupList {
  rows: StartupItem[];
  count: number;
  enabled: number;
  disabled: number;
  is_admin: boolean;
}

/** 位置 → 是否需要管理员（只有 HKLM 需要；启动文件夹在 %APPDATA% 下）。 */
export function needsAdmin(location: string): boolean {
  return location === "HKLM-Run";
}

/**
 * 能否切换该行。非管理员改 HKLM 必失败（PermissionError），
 * 所以提前禁用并把原因说清楚——不管当前是启用还是禁用态，判定都一样。
 */
export function canToggle(item: StartupItem, isAdmin: boolean): { ok: boolean; reason: string } {
  if (needsAdmin(item.location) && !isAdmin) {
    return { ok: false, reason: "该启动项在 HKLM 下，需要管理员权限才能修改" };
  }
  return { ok: true, reason: "" };
}

/** 位置标签 → 展示色（HKLM 更"重"，用不同色提示影响范围）。 */
export function locationColor(location: string): string {
  switch (location) {
    case "HKLM-Run":
      return "red";
    case "HKCU-Run":
      return "blue";
    case "启动文件夹":
      return "green";
    default:
      return "default";
  }
}

/** 位置说明（帮用户理解作用范围差异）。 */
export function locationHint(location: string): string {
  switch (location) {
    case "HKLM-Run":
      return "所有用户 · 需管理员";
    case "HKCU-Run":
      return "仅当前用户";
    case "启动文件夹":
      return "%APPDATA% 下的启动快捷方式";
    default:
      return location;
  }
}

/** 统计：启用/禁用。 */
export function startupStats(rows: StartupItem[]): { total: number; enabled: number; disabled: number } {
  let enabled = 0;
  for (const r of rows) if (r.enabled) enabled += 1;
  return { total: rows.length, enabled, disabled: rows.length - enabled };
}

/** 状态筛选档位（all 之外的两档服务"找出自启项/找回被禁项"两个高频意图）。 */
export type StartupStatusFilter = "all" | "enabled" | "disabled";

/**
 * 关键字 + 状态双重筛选。
 * status 用可选参数保持旧签名兼容（只按关键字筛）。
 */
/**
 * 排序维度（下拉框用）。
 *
 * 为什么需要它：启动项常有 50+ 条、每页 15 行，默认顺序是"注册表遍历顺序"，
 * 想集中看某个位置（比如 HKLM 全局项）或先处理已启用的，只能一页页翻。
 */
export type StartupSort = "default" | "name" | "location";

export const STARTUP_SORTS: { value: StartupSort; label: string }[] = [
  { value: "default", label: "默认顺序" },
  { value: "location", label: "按位置" },
  { value: "name", label: "按名称" },
];

/**
 * 排序（不改输入数组）。
 *
 * `default` 原样返回；「按位置」是主要用途——把 HKLM（全局、需管理员）与
 * HKCU 的项各自聚在一起，用户一眼看出哪些能动。
 * 用 `localeCompare(.., "zh-CN")` 而不是码点比较：`%APPDATA%` 这类 ASCII 项
 * 与中文名混排时，码点序会把中文全排到最后，读起来像"根本没排"。
 */
export function sortStartup(rows: StartupItem[], sort: StartupSort = "default"): StartupItem[] {
  if (sort === "default") return rows;
  const byName = (a: StartupItem, b: StartupItem) => a.name.localeCompare(b.name, "zh-CN");
  const out = [...rows];
  if (sort === "name") out.sort(byName);
  else
    out.sort(
      (a, b) => a.location.localeCompare(b.location, "zh-CN") || byName(a, b),
    );
  return out;
}

export function filterStartup(
  rows: StartupItem[],
  keyword: string,
  status: StartupStatusFilter = "all",
): StartupItem[] {
  let out = rows;
  if (status === "enabled") out = out.filter((r) => r.enabled);
  else if (status === "disabled") out = out.filter((r) => !r.enabled);
  const kw = keyword.trim().toLowerCase();
  if (!kw) return out;
  return out.filter((r) =>
    `${r.name} ${r.command} ${r.location}`.toLowerCase().includes(kw),
  );
}

/**
 * 解析命令行里的可执行文件路径（展示用）。
 *
 * 启动项命令常见四种形态，直接显示整串很长，截出程序名更好认：
 * `"C:\a b\x.exe" -flag` / `C:\x.exe -flag` / `cmd /c ...` / `rundll32 ...`。
 * 解析不出时返回原串——**不要猜**，猜错会让用户认不出自己的启动项。
 */
export function commandProgram(command: string): string {
  const t = command.trim();
  if (!t) return "";
  if (t.startsWith('"')) {
    const end = t.indexOf('"', 1);
    if (end > 1) return t.slice(1, end);
  }
  // 无引号：第一个空格前的部分（路径含空格时必然有引号，所以这里够用）
  const sp = t.indexOf(" ");
  return sp > 0 ? t.slice(0, sp) : t;
}

/** 禁用确认文案（说明可恢复，降低用户顾虑）。 */
export function toggleConfirmText(item: StartupItem, enable: boolean): string {
  if (enable) {
    return `重新启用 ${item.name}？下次登录时会随系统启动。`;
  }
  return (
    `禁用 ${item.name}？\n\n` +
    `不会删除程序本身，只是让它下次登录时不自启；随时可以在本页重新启用。\n` +
    `位置：${item.location}（${locationHint(item.location)}）`
  );
}

/**
 * 切换失败的完整文案。后端错误串本身已可读（如"需要管理员权限：请以管理员
 * 身份重启 SysKit 后重试"），但要补上"是禁用还是启用失败了"这个动作上下文——
 * 否则用户只看到一句权限报错，不知道刚才哪步没做成。
 */
export function toggleErrorText(enable: boolean, e: unknown): string {
  return `${enable ? "启用" : "禁用"}失败：${String(e)}`;
}
