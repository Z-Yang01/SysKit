/**
 * 二维码识别结果的纯逻辑（无 React/DOM）：WIFI: 串解析与结果分类。
 * 与 Rust 侧 qrdecode.rs::parse_wifi_qr 同口径（那边的单测是 round-trip 锁，
 * 这边锁前端展示分支用不到后端细节时的独立行为——两层各自有测试）。
 */

export interface WifiQrInfo {
  auth: string;
  ssid: string;
  password: string;
  hidden: boolean;
}

/** 解析 `WIFI:T:..;S:..;P:..;H:..;;`；非 WiFi 码返回 null。 */
export function parseWifiQr(text: string): WifiQrInfo | null {
  if (!text.startsWith("WIFI:")) return null;
  const out: WifiQrInfo = { auth: "nopass", ssid: "", password: "", hidden: false };
  // 按非转义分号切分；字段内 \; \, \: \" 先还原（与生成侧转义对偶）
  const fields: string[] = [];
  let cur = "";
  let escaped = false;
  for (const c of text.slice(5)) {
    if (escaped) {
      cur += c;
      escaped = false;
    } else if (c === "\\") {
      cur += c; // 保留反斜杠，统一在字段级还原
      escaped = true;
    } else if (c === ";") {
      fields.push(cur);
      cur = "";
    } else {
      cur += c;
    }
  }
  fields.push(cur);
  for (const field of fields) {
    const f = field.trim();
    if (!f) continue;
    const i = f.indexOf(":");
    if (i < 0) return null;
    const key = f.slice(0, i).toUpperCase();
    const value = unescapeWifi(f.slice(i + 1));
    if (key === "T") out.auth = value.toUpperCase();
    else if (key === "S") out.ssid = value;
    else if (key === "P") out.password = value;
    else if (key === "H") out.hidden = ["TRUE", "YES", "1"].includes(value.toUpperCase());
  }
  return out.ssid ? out : null;
}

function unescapeWifi(s: string): string {
  return s.replace(/\\(.)/g, "$1");
}

/** http/https 链接判定（「打开链接」动作只对这两类协议出现）。 */
export function isHttpUrl(text: string): boolean {
  return /^https?:\/\//i.test(text.trim());
}
