/**
 * WiFi 工具页的纯逻辑（无 React/DOM，vitest 覆盖）。
 *
 * **核心纪律：密码只能经敏感路径进剪贴板。**
 * 直接调用普通"复制"会漏掉三重保护（不进本应用历史 / 不进 Win+V 与云剪贴板 /
 * 到期清空），所以这里把"该走哪条路"做成显式判定，并由测试锁住。
 *
 * 周边扫描 / 信道聚合 / 二维码分享的展示侧纯逻辑也在这里：netsh 原文解析
 * 归后端 `wifi::parse_networks`（双语样例锁在 Rust 单测里），前端不重复
 * 维护第二份解析器。
 */

/** 后端 `sk_wifi_profile_password`。 */
export interface ProfileCredential {
  name: string;
  auth: string;
  password: string;
  has_password: boolean;
}

export interface ProfilesResult {
  profiles: string[];
  count: number;
}

/** 后端 `sk_clip_copy_sensitive`。 */
export interface SensitiveCopyResult {
  /** 0 表示不自动清空（但仍不进历史与系统剪贴板历史） */
  ttl: number;
  note: string;
}

/**
 * 复制密码必须走的命令名。
 *
 * 单独导出成常量而不是散落的字面量：守卫测试与实现引用同一个来源，
 * 改错了会被测试拦住。
 */
export const SENSITIVE_COPY_COMMAND = "sk_clip_copy_sensitive";

/**
 * 该内容是否必须以敏感路径复制。
 *
 * 判定按"内容是否属于凭据"而不是"在哪个页面"：将来新增页面（VPN 密钥、
 * 数据库口令）只要复用这个判定就不会漏保护。
 */
export function requiresSensitiveCopy(kind: "password" | "plain"): boolean {
  return kind === "password";
}

/** 密码强度（仅作提示，不阻断——用户可能有历史遗留的弱密码）。 */
export interface StrengthHint {
  label: string;
  /** 0-4 */
  score: number;
  advice: string;
}

/**
 * 弱密码提示。
 *
 * **只在本地做长度/字符类判断，绝不联网查泄露库**——那等于把密码发出去。
 */
export function strengthHint(pwd: string): StrengthHint {
  if (!pwd) return { label: "无密码", score: 0, advice: "开放网络或未保存密钥" };
  let score = 0;
  if (pwd.length >= 8) score += 1;
  if (pwd.length >= 12) score += 1;
  if (/[a-z]/.test(pwd) && /[A-Z]/.test(pwd)) score += 1;
  if (/\d/.test(pwd)) score += 1;
  if (/[^A-Za-z0-9]/.test(pwd)) score += 1;
  score = Math.min(4, score);

  const label = ["很弱", "弱", "一般", "较强", "很强"][score];
  let advice = "";
  if (pwd.length < 8) advice = "长度不足 8 位，建议加长";
  else if (score <= 2) advice = "建议混用大小写、数字与符号";
  else if (score < 4) advice = "尚可；再长一些更安全";
  else advice = "强度良好";
  return { label, score, advice };
}

/** 是否显示"需要管理员"的提示。 */
export function needsAdminHint(err: string): boolean {
  return err.includes("管理员");
}

/** 读取失败时的可读提示（区分"没权限"与"读不到"）。 */
export function passwordErrorHint(err: string): string {
  if (needsAdminHint(err)) {
    return "读取明文密钥需要管理员权限。请以管理员身份重新启动 SysKit 后再试（这是 Windows netsh key=clear 接口的限制，不是程序缺陷）。";
  }
  if (err.includes("找不到")) {
    return "系统里找不到这个 WLAN 配置文件的记录，可能已被删除。";
  }
  return err || "读取失败";
}

/**
 * 复制结果的提示文案。
 *
 * 必须**如实转述后端给出的保护强度**：用户看到"25s 后自动清空"才知道
 * 密码不会一直留在剪贴板里。
 */
export function copyResultMessage(r: SensitiveCopyResult): string {
  return r.note || (r.ttl > 0 ? `${r.ttl}s 后自动清空剪贴板` : "已复制到剪贴板");
}

/** 密码遮罩显示（默认不直接显示明文）。 */
export function maskPassword(pwd: string, shown: boolean): string {
  if (!pwd) return "（无密钥）";
  if (shown) return pwd;
  return "•".repeat(Math.min(16, Math.max(8, pwd.length)));
}

// ---------------- 周边 AP 扫描（netsh 解析在后端 wifi.rs，这里只做展示侧纯逻辑） ----------------

/** 后端 `sk_wifi_scan` 返回的单个 AP（BSSID 级）。 */
export interface ScannedAp {
  ssid: string;
  bssid: string;
  /** 信号质量 0~100（netsh 未给出时为 0） */
  signal: number;
  /** 信道号（0 = 未知，不编造） */
  channel: number;
  radio: string;
  auth: string;
  band: "2.4G" | "5G" | "?";
}

/** 后端 `sk_wifi_scan` 返回结构。 */
export interface ScanResult {
  aps: ScannedAp[];
  count: number;
}

/** 自动刷新周期（毫秒）。 */
export const WIFI_SCAN_INTERVAL_MS = 10_000;

/** 频段判定（与后端 band_of_channel 同口径：1~14 → 2.4G，>14 → 5G）。 */
export function bandOf(channel: number): ScannedAp["band"] {
  if (channel <= 0) return "?";
  return channel <= 14 ? "2.4G" : "5G";
}

/** 单个信道上的 AP 占用。 */
export interface ChannelUsage {
  channel: number;
  /** 该信道上的 BSSID 数 */
  count: number;
}

/**
 * 扫描结果按信道聚合：2.4G 只看 1~13（与 Python 图表口径一致，信道 14 是
 * 日本 legacy 极少见），5G 列出扫到的全部信道。未知信道无处安放，跳过。
 */
export function aggregateChannels(aps: ScannedAp[]): { c24: ChannelUsage[]; c5: ChannelUsage[] } {
  const count24 = new Map<number, number>();
  const count5 = new Map<number, number>();
  for (const ap of aps) {
    if (!ap.channel) continue;
    const bucket = bandOf(ap.channel) === "2.4G" ? count24 : count5;
    bucket.set(ap.channel, (bucket.get(ap.channel) ?? 0) + 1);
  }
  const toRows = (m: Map<number, number>, keep: (ch: number) => boolean): ChannelUsage[] =>
    [...m.entries()]
      .filter(([ch]) => keep(ch))
      .sort((a, b) => a[0] - b[0])
      .map(([channel, count]) => ({ channel, count }));
  return {
    c24: toRows(count24, (ch) => ch >= 1 && ch <= 13),
    c5: toRows(count5, () => true),
  };
}

/** 占用分档 Tag 色（Python bars 的阈值：≤2 绿、≤4 橙、更多红）。 */
export function channelLoadTag(count: number): "green" | "orange" | "red" {
  return count <= 2 ? "green" : count <= 4 ? "orange" : "red";
}

/** 信号分档 Tag 色（Python _signal_color 的阈值：≥60 绿、≥40 橙、其余红）。 */
export function signalTag(pct: number): "green" | "orange" | "red" {
  return pct >= 60 ? "green" : pct >= 40 ? "orange" : "red";
}

// ---------------- 推荐信道（syskit/wifi.py recommend_channels 的等价移植） ----------------

// 2.4G 候选 1/6/11（经典非重叠三信道）
const CH24_CANDIDATES = [1, 6, 11];

// |Δch| = 0..4 的重叠能量权重（802.11 频谱掩码近似值，d≥5 无明显重叠）
const SPECTRUM_MASK = [1.0, 0.53, 0.22, 0.08, 0.02];

// 5G 80MHz 块 → 主信道锚点（165 非标准 80M 块，不推荐）
const CH5_BLOCKS = [
  [36, 40, 44, 48],
  [52, 56, 60, 64],
  [100, 104, 108, 112],
  [116, 120, 124, 128],
  [132, 136, 140, 144],
  [149, 153, 157, 161],
];

// 52~144 为 DFS 雷达信道：部分客户端不扫、须雷达检测静默期，加罚 1.0
// （约合一个满强 AP）：非 DFS 明显不空时才推荐 DFS
const DFS_CHANNELS = (() => {
  const s = new Set<number>();
  for (let c = 52; c <= 144; c++) s.add(c);
  return s;
})();
const DFS_PENALTY = 1.0;

/** 推荐信道结果（对应 Python recommend_channels 的返回）。 */
export interface ChannelAdvice {
  ch24: number;
  /** 同频 AP 数 */
  same24: number;
  /** ±4 内邻频 AP 数 */
  adj24: number;
  /** 5G 锚点信道；无 5G AP 时为 null */
  ch5: number | null;
  /** 该 80M 块内 AP 数 */
  block5: number;
}

/** netsh 信号 %（0~100）→ 0~1 强弱因子，越界钳位。 */
function signalWeight(signal: number): number {
  return Math.min(100, Math.max(0, signal)) / 100;
}

/** 候选信道的干扰分：Σ 频谱权重(Δch) × 信号因子。越低越好。 */
function score24(aps: ScannedAp[], channel: number): number {
  let total = 0;
  for (const ap of aps) {
    const d = Math.abs(ap.channel - channel);
    if (d < SPECTRUM_MASK.length) total += SPECTRUM_MASK[d] * signalWeight(ap.signal);
  }
  return total;
}

/**
 * 推荐信道：2.4G 计邻频重叠 + 信号强弱（不再只数同频 AP）；5G 按 80MHz 块
 * 挑最空锚点并避开 DFS。平分时取先到者（2.4G 从 1 起荐、5G 取低信道）。
 */
export function recommendChannels(aps: ScannedAp[]): ChannelAdvice {
  const a24 = aps.filter((a) => a.channel && bandOf(a.channel) === "2.4G");
  const a5 = aps.filter((a) => a.channel && bandOf(a.channel) === "5G");

  let best24 = CH24_CANDIDATES[0];
  let bestScore = Infinity;
  for (const c of CH24_CANDIDATES) {
    const s = score24(a24, c);
    if (s < bestScore) {
      bestScore = s;
      best24 = c;
    }
  }
  const same24 = a24.filter((a) => a.channel === best24).length;
  const adj24 = a24.filter((a) => a.channel !== best24 && Math.abs(a.channel - best24) <= 4).length;

  let ch5: number | null = null;
  let block5 = 0;
  if (a5.length > 0) {
    let best5Score = Infinity;
    for (const block of CH5_BLOCKS) {
      const anchor = block[0];
      const inBlock = a5.filter((a) => block.includes(a.channel));
      let s = inBlock.reduce((sum, a) => sum + signalWeight(a.signal), 0);
      if (DFS_CHANNELS.has(anchor)) s += DFS_PENALTY;
      if (s < best5Score) {
        best5Score = s;
        ch5 = anchor;
        block5 = inBlock.length;
      }
    }
  }
  return { ch24: best24, same24, adj24, ch5, block5 };
}

/** 推荐信道的一句话文案（对齐 Python lbl_advice）。 */
export function channelAdviceText(a: ChannelAdvice): string {
  const b24 = `2.4G 用信道 ${a.ch24}（同频 ${a.same24} / 邻频 ${a.adj24} 个 AP）`;
  if (a.ch5 === null) return `推荐：${b24}；未发现 5G 网络`;
  return `推荐：${b24}；5G 用信道 ${a.ch5}（80M 块内 ${a.block5} 个 AP）`;
}

// ---------------- 扫描状态与二维码分享辅助 ----------------

/** 扫描状态行文案（对齐 Python `共发现 N 个 BSSID　HH:MM:SS`，中间是全角空格）。 */
export function scanSummaryText(count: number, at: Date): string {
  const p = (n: number) => String(n).padStart(2, "0");
  return `共发现 ${count} 个 BSSID　${p(at.getHours())}:${p(at.getMinutes())}:${p(at.getSeconds())}`;
}

/** 扫描失败提示：位置服务的错误后端已给好人话，其余给不空的兜底。 */
export function scanFailedHint(err: string): string {
  if (err.includes("位置") || /location/i.test(err)) return err;
  return err || "扫描失败（netsh 无法执行或无线网卡不可用）";
}

/** WIFI QR 的 T 字段取值集合（手输模式直接选；已存配置由后端从 netsh 认证映射）。 */
export const QR_AUTH_OPTIONS = [
  { value: "WPA", label: "WPA/WPA2（个人）" },
  { value: "SAE", label: "WPA3（SAE）" },
  { value: "WEP", label: "WEP（不安全）" },
  { value: "nopass", label: "开放网络（无密码）" },
] as const;

/** T 值 → 人话标签（二维码下方展示）。 */
export function qrAuthLabel(t: string): string {
  return QR_AUTH_OPTIONS.find((o) => o.value === t)?.label ?? t;
}

/** 手输模式校验：SSID 必填；非 nopass 必须有密码。返回错误文案，空串 = 通过。 */
export function validateWifiQrInput(ssid: string, password: string, auth: string): string {
  if (!ssid.trim()) return "SSID 不能为空";
  if (auth !== "nopass" && !password) return "该认证类型需要填写密码";
  return "";
}
