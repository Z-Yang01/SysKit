/**
 * 剪贴板内容规则动作的前端逻辑（无 React/DOM，vitest 覆盖）。
 *
 * 后端 `cliprules::detect` 负责**识别**（含 CJK 标点剥离、盘型预检等细节），
 * 这里负责**呈现与派发**：
 * - 哪些动作由前端自己渲染（二维码/色块/归属地需要渲染或联网查询）；
 * - 哪些交给后端执行（打开链接/文件/目录、ping、DNS）；
 * - 联网动作要标注（用户可能不想让工具发网络请求）。
 */

/** 后端 `sk_clip_detect` 的一个动作。 */
export interface ClipAction {
  id: string;
  label: string;
  arg: string;
  online: boolean;
}

export interface DetectResult {
  actions: ClipAction[];
  count: number;
}

/** 该动作由前端处理还是要走后端执行。 */
export type ActionRoute = "frontend" | "backend" | "unsupported";

/**
 * 动作路由。
 *
 * `url_qr` / `color_show` / `ip_geo` 需要渲染或额外查询，由前端接；
 * 其余是明确的系统操作，交给后端（后端已有 shell_open / ping / DNS）。
 */
export function routeAction(id: string): ActionRoute {
  switch (id) {
    case "url_qr":
    case "color_show":
    case "ip_geo":
      return "frontend";
    case "url_open":
    case "file_open":
    case "file_reveal":
    case "json_format":
    case "ip_ping":
    case "domain_ping":
    case "domain_dns":
      return "backend";
    default:
      // 未知动作**明确不支持**，不猜——猜错会执行错误操作
      return "unsupported";
  }
}

/** 动作语气色（打开/查看类中性，联网类提示，未知类警示）。 */
export function actionColor(id: string): string {
  const route = routeAction(id);
  if (route === "unsupported") return "red";
  if (route === "frontend") return "blue";
  // 打开文件/目录是中性操作；ping/DNS/打开链接会发网络请求
  return id === "ip_ping" || id === "domain_ping" || id === "domain_dns" || id === "url_open"
    ? "orange"
    : "default";
}

/** 按钮提示文案：联网动作要明说。 */
export function actionHint(a: ClipAction): string {
  if (a.online) return "需要联网";
  if (a.id === "file_reveal") return "打开所在目录并选中该文件";
  if (a.id === "json_format") return "格式化后复制回剪贴板";
  if (a.id === "url_qr") return "生成二维码（本地渲染）";
  return "";
}

/** 动作执行结果的一句话摘要（截断以适配提示条）。 */
export function summarizeResult(text: string, limit = 200): string {
  const t = text.trim();
  if (!t) return "已执行";
  const firstLine = t.split(/\r?\n/).find((l) => l.trim()) ?? t;
  return firstLine.length > limit ? `${firstLine.slice(0, limit)}…` : firstLine;
}

/**
 * 是否显示动作栏。
 *
 * 空列表**不显示任何占位**：剪贴板里绝大多数内容是普通文本，天天显示
 * "暂无可执行动作"只会变成噪声。
 */
export function shouldShowActions(r: DetectResult | null): boolean {
  return (r?.actions.length ?? 0) > 0;
}

/** 动作栏摘要（给标题行用）。 */
export function actionsSummary(r: DetectResult | null): string {
  const n = r?.actions.length ?? 0;
  if (n === 0) return "";
  const online = r?.actions.filter((a) => a.online).length ?? 0;
  return online > 0 ? `${n} 个动作（${online} 个需联网）` : `${n} 个动作`;
}

/**
 * 颜色值规整：`#abc` → `#aabbcc`（色块渲染用）。
 *
 * 三位缩写要展开，否则某些渲染路径会把它当成非法值而显示空白。
 */
export function normalizeHex(color: string): string {
  const c = color.trim().toUpperCase();
  if (!/^#[0-9A-F]{3}$|^#[0-9A-F]{6}$/.test(c)) return "";
  if (c.length === 4) {
    const [r, g, b] = [c[1], c[2], c[3]];
    return `#${r}${r}${g}${g}${b}${b}`;
  }
  return c;
}

/** 该十六进制色的亮度是否偏亮（决定文字用黑还是白，保证可读）。 */
export function isLightColor(hex: string): boolean {
  const c = normalizeHex(hex);
  if (!c) return true;
  const r = parseInt(c.slice(1, 3), 16);
  const g = parseInt(c.slice(3, 5), 16);
  const b = parseInt(c.slice(5, 7), 16);
  // ITU-R BT.601 亮度
  return (r * 299 + g * 587 + b * 114) / 1000 > 140;
}

/** 颜色格式化：`HEX` 或 `RGB(r, g, b)`。 */
export function formatColor(hex: string, fmt: "HEX" | "RGB"): string {
  const c = normalizeHex(hex);
  if (!c) return hex;
  if (fmt === "HEX") return c;
  const r = parseInt(c.slice(1, 3), 16);
  const g = parseInt(c.slice(3, 5), 16);
  const b = parseInt(c.slice(5, 7), 16);
  return `rgb(${r}, ${g}, ${b})`;
}
