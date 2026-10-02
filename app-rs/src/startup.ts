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

/** 关键字筛选（名称/命令/位置）。 */
export function filterStartup(rows: StartupItem[], keyword: string): StartupItem[] {
  const kw = keyword.trim().toLowerCase();
  if (!kw) return rows;
  return rows.filter((r) =>
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
