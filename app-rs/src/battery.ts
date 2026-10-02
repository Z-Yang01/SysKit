/**
 * 电池页纯逻辑（无 React/DOM，vitest 覆盖）。对应 Python 侧 BatteryTab。
 *
 * 核心是**没有电池时必须给出明确说明**（台式机很常见），不能显示空白或 0%。
 * 健康度不可用时也不能编造数值——那会让用户以为电池状态良好。
 */

/** 后端 `sk_power_status`。 */
export interface PowerStatus {
  ac_online: boolean;
  has_battery: boolean;
  percent: number | null;
  charging: boolean;
  seconds_left: number | null;
  remaining_human: string;
  summary: string;
}

/** 后端 `sk_battery_health`。 */
export interface BatteryHealth {
  design_mwh: number | null;
  full_mwh: number | null;
  health_percent: number | null;
  token: string;
  available: boolean;
  note: string;
}

/** 健康度 → AntD Tag 颜色。 */
export function healthColor(token: string): string {
  switch (token) {
    case "ok":
      return "green";
    case "warn":
      return "orange";
    case "err":
      return "red";
    default:
      return "default";
  }
}

/** 健康度文案：不可用时明确说"未读取到"，不显示 0%。 */
export function healthText(h: BatteryHealth | null): string {
  if (!h) return "尚未查询";
  if (!h.available || h.health_percent === null) return "未读取到";
  return `${h.health_percent.toFixed(1)}%`;
}

/** 容量展示（mWh）。 */
export function capacityText(mwh: number | null): string {
  if (mwh === null || mwh === undefined || mwh === 0) return "—";
  return `${mwh} mWh`;
}

/** 电量文案：未知时说"未知"而不是 0%。 */
export function percentText(p: PowerStatus | null): string {
  if (!p) return "—";
  if (!p.has_battery) return "无电池";
  if (p.percent === null) return "未知";
  return `${p.percent}%`;
}

/** 电量 → 语义色（低电量给提醒色）。 */
export function percentToken(p: PowerStatus | null): string {
  if (!p || !p.has_battery || p.percent === null) return "sub";
  // 接着交流电时不因电量低而告警（正在充）
  if (p.ac_online) return "ok";
  if (p.percent <= 10) return "err";
  if (p.percent <= 20) return "warn";
  return "ok";
}

/**
 * 低电量提醒文案（桌面通知用）。
 *
 * 接着交流电时不提醒：那会变成"插着电还让我充电"的骚扰。
 */
export function lowBatteryWarning(p: PowerStatus | null): string {
  if (!p || !p.has_battery || p.percent === null) return "";
  if (p.ac_online) return "";
  if (p.percent > 20) return "";
  return p.percent <= 10
    ? `电量仅剩 ${p.percent}%，建议立即接入电源`
    : `电量剩余 ${p.percent}%，注意准备电源`;
}

/** 是否应该显示"充电中"。 */
export function chargingLabel(p: PowerStatus | null): string {
  if (!p || !p.has_battery) return "";
  if (p.charging) return "充电中";
  return p.ac_online ? "已接电源" : "电池供电";
}
