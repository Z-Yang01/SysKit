/**
 * TOTP 二维码导入的纯逻辑：解析 `otpauth://totp/…` URI。
 *
 * 标准 RFC 6238 迁移 URI：otpauth://totp/<Issuer:account>?secret=&issuer=&
 * algorithm=&digits=&period=。schema 现状只存 secret（SHA1/6/30 隐含默认），
 * **非默认参数导入时必须警示**（nonDefaultParams）——这是"看起来能用实际
 * 动态码不对"的坑（评审 7）。纯函数 + vitest，与 qrdecode.ts 同模式。
 */

export interface OtpauthInfo {
  secret: string;
  issuer: string;
  label: string;
  algorithm: string;
  digits: number;
  period: number;
}

function urlDecode(s: string): string {
  try {
    return decodeURIComponent(s.replace(/\+/g, " "));
  } catch {
    return s; // 畸形编码按原样用（调用方有 secret 校验兜底）
  }
}

/** 解析 otpauth URI；非 otpauth:/缺 secret 返回 null。 */
export function parseOtpauth(uri: string): OtpauthInfo | null {
  if (!uri.startsWith("otpauth://")) return null;
  const rest = uri.slice("otpauth://".length);
  const slash = rest.indexOf("/");
  if (slash < 0) return null;
  const type = rest.slice(0, slash).toLowerCase();
  if (type !== "totp") return null; // hotp 不支持（计数器语义不同）
  const qmark = rest.indexOf("?");
  const rawLabel =
    qmark < 0 ? rest.slice(slash + 1) : rest.slice(slash + 1, qmark);
  const label = urlDecode(rawLabel);
  const params = new URLSearchParams(qmark < 0 ? "" : rest.slice(qmark + 1));

  const secret = params.get("secret")?.replace(/[\s-]/g, "").toUpperCase() ?? "";
  if (!secret) return null;
  const issuer = urlDecode(params.get("issuer") ?? "");
  // label 常形如 "Issuer:account"——issuer 参数缺失时从 label 前缀取
  const labelIssuer = label.includes(":") ? label.split(":")[0].trim() : "";
  return {
    secret,
    issuer: issuer || labelIssuer,
    label,
    algorithm: (params.get("algorithm") ?? "SHA1").toUpperCase(),
    digits: Number(params.get("digits") ?? 6) || 6,
    period: Number(params.get("period") ?? 30) || 30,
  };
}

/** 是否含当前 schema 不支持的参数（SHA1/6/30 之外）——导入时必须警示。 */
export function nonDefaultParams(info: OtpauthInfo): string[] {
  const out: string[] = [];
  if (info.algorithm !== "SHA1") out.push(`算法 ${info.algorithm}`);
  if (info.digits !== 6) out.push(`位数 ${info.digits}`);
  if (info.period !== 30) out.push(`周期 ${info.period}s`);
  return out;
}
