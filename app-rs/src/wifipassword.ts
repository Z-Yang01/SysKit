/**
 * WiFi 密码查看的纯逻辑（无 React/DOM，vitest 覆盖）。
 *
 * **核心纪律：密码只能经敏感路径进剪贴板。**
 * 直接调用普通"复制"会漏掉三重保护（不进本应用历史 / 不进 Win+V 与云剪贴板 /
 * 到期清空），所以这里把"该走哪条路"做成显式判定，并由测试锁住。
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
