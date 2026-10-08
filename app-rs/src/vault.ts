/**
 * 密码库页纯逻辑（无 React/DOM，vitest 覆盖）。
 *
 * M6 数据层已实现：本页渲染真实条目（sites/accounts），复制走后端
 * `sk_vault_copy_secret`（明文不过 JS，三重敏感防护后端完成）。
 */

/** 后端 `sk_vault_status`。 */
export interface VaultStatus {
  path: string;
  provisioned: boolean;
  unlocked: boolean;
  has_master_password: boolean;
  mode: string;
  /** KDF 算法（v1.3+ 返回 "pbkdf2" | "argon2id"，旧后端缺省 undefined） */
  kdf?: "pbkdf2" | "argon2id" | null;
  /** 免重复输入窗口剩余秒数（undefined/0 = 无窗口）。 */
  trusted_until?: number;
  capabilities: {
    key_management: boolean;
    password_storage: boolean;
    password_storage_reason?: string;
  };
}

/** 界面阶段（决定显示哪些操作）。 */
export type VaultPhase = "unprovisioned" | "locked" | "unlocked";

/** 由状态推出界面阶段。 */
export function vaultPhase(s: VaultStatus | null): VaultPhase {
  if (!s || !s.provisioned) return "unprovisioned";
  return s.unlocked ? "unlocked" : "locked";
}

/** 模式展示名。 */
export function modeLabel(mode: string): string {
  switch (mode) {
    case "password":
      return "主密码模式";
    case "dpapi":
      return "免密模式（DPAPI 绑定当前用户）";
    case "":
      return "尚未初始化";
    default:
      return mode;
  }
}

/** 状态摘要。 */
export function statusSummary(s: VaultStatus | null): string {
  if (!s) return "尚未读取";
  if (!s.provisioned) return "尚未创建密码库";
  const lock = s.unlocked ? "已解锁" : "已锁定";
  return `${modeLabel(s.mode)} · ${lock}`;
}

/**
 * 数据层是否可用。**没实现时必须为 false**，界面据此隐藏条目列表并显示说明，
 * 而不是渲染一个永远为空的表格（那看起来像"还没有条目"）。
 */
export function storageAvailable(s: VaultStatus | null): boolean {
  return s?.capabilities?.password_storage === true;
}

/** 数据层不可用时的说明文案（不得为空，否则界面会显示空白）。 */
export function storageNotice(s: VaultStatus | null): string {
  if (!s) return "正在读取密码库状态…";
  if (storageAvailable(s)) return "";
  return (
    s.capabilities?.password_storage_reason ||
    "密码条目存取尚未实现（数据层未引入），当前无法保存或读取密码。"
  );
}

// ---------------- M6 数据层（后端 vault_store 薄封装的界面模型） ----------------

/** 后端 `sk_vault_sites` 行。 */
export interface SiteRow {
  id: number;
  name: string;
  domain: string;
  url: string;
  aliases: string;
  tags: string;
  created_at?: string;
  updated_at?: string;
  n_accounts: number;
}

/** 后端 `sk_vault_site_accounts` 行（无任何明文 secrets）。 */
export interface AccountRow {
  id: number;
  site_id: number;
  label: string;
  username: string;
  is_favorite: boolean;
  last_used_at?: string;
  has_totp: boolean;
  has_notes: boolean;
}

/** 账号行的展示标题：label 优先，退用户名，再退「账号 #id」。 */
export function accountTitle(a: AccountRow): string {
  if (a.label.trim()) return a.label.trim();
  if (a.username.trim()) return a.username.trim();
  return `账号 #${a.id}`;
}

/** 密文型字段在列表中的掩码（长度不外泄——固定 8 位圆点）。 */
export function maskedSecret(): string {
  return "••••••••";
}

/** TOTP 剩余秒数（30s 窗口），供倒计时环；[0,30)。 */
export function totpRemainingSecs(nowMs = Date.now()): number {
  return 30 - Math.floor(nowMs / 1000) % 30;
}

/**
 * 「添加密码」按钮是否可点。
 *
 * **先判数据层**：功能根本没有时，说明原因就该说"存取未实现"，而不是"请先解锁"
 * ——后者会让用户以为解锁后就能存密码，解完锁发现还是不行（首版按解锁优先判定，
 * 测试指出这个顺序会误导）。
 */
export function canAddPassword(s: VaultStatus | null): { ok: boolean; reason: string } {
  if (!storageAvailable(s)) {
    return { ok: false, reason: storageNotice(s) };
  }
  if (vaultPhase(s) !== "unlocked") {
    return { ok: false, reason: "请先解锁密码库" };
  }
  return { ok: true, reason: "" };
}

/** 主密码设置是否可用（需要已解锁，且当前是免密模式）。 */
export function canSetMasterPassword(s: VaultStatus | null): { ok: boolean; reason: string } {
  if (vaultPhase(s) !== "unlocked") {
    return { ok: false, reason: "请先解锁密码库" };
  }
  if (s?.has_master_password) {
    return { ok: false, reason: "当前已是主密码模式；如需更换请用「修改主密码」" };
  }
  return { ok: true, reason: "" };
}

/**
 * 主密码强度提示（**只提示，不阻断**）。
 *
 * 判定先把"长度"和"字符种类"分开看：
 * - 长度 < 8 → weak（绝对偏短）；
 * - 长度 ≥ 12 且种类 ≥ 3 → good；
 * - 长度 ≥ 12 → fair：**12 位纯小写也明显强于 8 位混合**，不该因为"种类少"
 *   就被打成 weak（首版按种类主导，把 `abcdefghijkl` 判成 weak，测试指出这不合理）；
 * - 其余（8~11 位或种类单一）→ fair / weak 按种类分。
 */
export function passwordStrength(pw: string): { level: "weak" | "fair" | "good"; hint: string } {
  if (pw.length < 8) {
    return {
      level: "weak",
      hint: "少于 8 位，建议加长：主密码是解开整个密码库的唯一凭据，忘了无法找回。",
    };
  }
  const hasUpper = /[A-Z]/.test(pw);
  const hasLower = /[a-z]/.test(pw);
  const hasDigit = /\d/.test(pw);
  const hasSymbol = /[^A-Za-z0-9]/.test(pw);
  const kinds = [hasUpper, hasLower, hasDigit, hasSymbol].filter(Boolean).length;

  if (pw.length >= 12) {
    return kinds >= 3
      ? { level: "good", hint: "长度与字符种类都不错。" }
      : { level: "fair", hint: "长度够了；再混合大小写、数字或符号会更稳。" };
  }
  return kinds >= 2
    ? { level: "fair", hint: "可以用，但加长到 12 位以上或混合更多字符类型会更稳。" }
    : { level: "weak", hint: "偏短且字符种类少，建议混合大小写、数字与符号并加长。" };
}

/** 创建/解锁的确认文案。 */
export function setupConfirmText(mode: "dpapi" | "password"): string {
  if (mode === "dpapi") {
    return (
      "将创建免密密码库：\n" +
      "· 数据密钥由 Windows DPAPI 保护，绑定**当前 Windows 用户**；\n" +
      "· 换用户或换机器后无法解开（这是 DPAPI 的设计）；\n" +
      "· 任何人都能用你的 Windows 账户直接打开密码库。\n\n" +
      "需要跨机器/更强保护，请改选主密码模式。"
    );
  }
  return (
    "将创建主密码密码库：\n" +
    "· 数据密钥由主密码派生的密钥包裹（PBKDF2-HMAC-SHA256，60 万次迭代）；\n" +
    "· **主密码无法找回**——忘记后密码库内容无法解密；\n" +
    "· 主密码只存在你脑子里，不落盘。"
  );
}

// ---------------- 密码生成器（纯逻辑；生成核心在 Rust pwgen.rs） ----------------

/** 后端 `sk_vault_generate_password` 的参数。 */
export interface PwgenOptions {
  length: number;
  upper: boolean;
  lower: boolean;
  digits: boolean;
  symbols: boolean;
  exclude_ambiguous: boolean;
}

/** 生成器默认长度（与 pwgen.rs DEFAULT_LENGTH 一致，parity 守卫锁定）。 */
export const PWGEN_DEFAULT_LENGTH = 16;
/** 长度区间（与 pwgen.rs MIN/MAX_LENGTH 一致）。 */
export const PWGEN_MIN_LENGTH = 8;
export const PWGEN_MAX_LENGTH = 64;

/**
 * 四类字符集与易混淆集：**与 pwgen.rs 的 b"..." 字面量逐字对齐**。
 * tests/vault.test.ts 的 parity 守卫直接读 Rust 源里的字面量与本文件比对，
 * 任何一侧单独改动都会红——熵按字母表大小算，双端不同步就会撒谎。
 */
export const PWGEN_UPPER = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
export const PWGEN_LOWER = "abcdefghijklmnopqrstuvwxyz";
export const PWGEN_DIGITS = "0123456789";
export const PWGEN_SYMBOLS = "!@#$%^&*()-_=+[]{};:,.<>/?~\\|'\"`";
/** 易混淆字符（跨类）：0/O、1/l/I、竖线、反引号、单双引号。 */
export const PWGEN_AMBIGUOUS = "0O1lI|`'\"";

/** 排除易混淆后的单类字符集。 */
function filterAmbiguous(set: string, excludeAmbiguous: boolean): string {
  if (!excludeAmbiguous) return set;
  return [...set].filter((c) => !PWGEN_AMBIGUOUS.includes(c)).join("");
}

/** 当前选项下可用字符总数（熵公式的真数）。全部关闭时为 0。 */
export function pwgenAlphabetSize(o: PwgenOptions): number {
  let n = 0;
  if (o.upper) n += filterAmbiguous(PWGEN_UPPER, o.exclude_ambiguous).length;
  if (o.lower) n += filterAmbiguous(PWGEN_LOWER, o.exclude_ambiguous).length;
  if (o.digits) n += filterAmbiguous(PWGEN_DIGITS, o.exclude_ambiguous).length;
  if (o.symbols) n += filterAmbiguous(PWGEN_SYMBOLS, o.exclude_ambiguous).length;
  return n;
}

/** 熵位数 = 长度 × log2(字符集大小)。字母表为空（全类关闭）时为 0。 */
export function pwgenEntropyBits(o: PwgenOptions): number {
  const size = pwgenAlphabetSize(o);
  return size > 1 ? o.length * Math.log2(size) : 0;
}

/**
 * 熵分档（弱 / 中 / 强）——只提示，不阻断。
 *
 * 阈值口径：<50 弱（8 位全字符集 52 勉强过线，8 位纯小写 37.6 直接弱）；
 * 50~79 中（12 位全字符集 78.7）；≥80 强（16 位全字符集 104.9、
 * 12 位大小写数字 71 中档）。上限 128 位封顶显示——超过后再长对在线猜测
 * 已无区分度，进度条顶格即可，不必把 400+ 位画成天梯。
 */
export function pwgenTier(bits: number): {
  level: "weak" | "fair" | "strong";
  label: string;
  color: string;
} {
  if (bits < 50) return { level: "weak", label: "弱", color: "red" };
  if (bits < 80) return { level: "fair", label: "中", color: "orange" };
  return { level: "strong", label: "强", color: "green" };
}

/** 强度条显示百分比：熵 128 位封顶（见 pwgenTier 注），避免 400 位画满千格。 */
export function pwgenMeterPercent(bits: number): number {
  return Math.max(0, Math.min(100, Math.round((bits / 128) * 100)));
}

/**
 * 生成参数可满足性（与后端 validate 同口径的前端预检，双端各自成立）：
 * 至少一类；长度在区间内。不满足时给出可直接展示的原因。
 */
export function pwgenUsable(o: PwgenOptions): { ok: boolean; reason: string } {
  if (!o.upper && !o.lower && !o.digits && !o.symbols) {
    return { ok: false, reason: "至少勾选一类字符（大写/小写/数字/符号）" };
  }
  if (o.length < PWGEN_MIN_LENGTH || o.length > PWGEN_MAX_LENGTH) {
    return { ok: false, reason: `长度须在 ${PWGEN_MIN_LENGTH}~${PWGEN_MAX_LENGTH} 之间` };
  }
  return { ok: true, reason: "" };
}
