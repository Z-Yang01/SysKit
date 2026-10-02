/**
 * 电池页纯逻辑测试。
 *
 * 重点是"没有数据时不能编造"：台式机很常见，健康度读不到时必须说"未读取到"
 * 而不是 0%；低电量提醒不能在插着电时骚扰用户。
 */
import { describe, expect, it } from "vitest";

import {
  capacityText,
  chargingLabel,
  healthColor,
  healthText,
  lowBatteryWarning,
  percentText,
  percentToken,
  type BatteryHealth,
  type PowerStatus,
} from "../src/battery";

const power = (over: Partial<PowerStatus> = {}): PowerStatus => ({
  ac_online: false,
  has_battery: true,
  percent: 50,
  charging: false,
  seconds_left: 3600,
  remaining_human: "1 小时 0 分",
  summary: "",
  ...over,
});

const health = (over: Partial<BatteryHealth> = {}): BatteryHealth => ({
  design_mwh: 50000,
  full_mwh: 45000,
  health_percent: 90,
  token: "ok",
  available: true,
  note: "",
  ...over,
});

describe("healthColor", () => {
  it("三档映射", () => {
    expect(healthColor("ok")).toBe("green");
    expect(healthColor("warn")).toBe("orange");
    expect(healthColor("err")).toBe("red");
    expect(healthColor("sub")).toBe("default");
  });
});

describe("healthText 诚实性", () => {
  it("可用时给出百分比", () => {
    expect(healthText(health())).toBe("90.0%");
    expect(healthText(health({ health_percent: 82.35 }))).toBe("82.3%");
  });

  it("不可用时说「未读取到」，不能显示 0%", () => {
    const t = healthText(health({ available: false, health_percent: null }));
    expect(t).toBe("未读取到");
    expect(t).not.toContain("0");
  });

  it("未查询时为「尚未查询」", () => {
    expect(healthText(null)).toBe("尚未查询");
  });
});

describe("capacityText", () => {
  it("正常容量带单位", () => {
    expect(capacityText(50000)).toBe("50000 mWh");
  });

  it("null 与 0 都显示「—」（WMI 无电池时给 0，不是真实容量）", () => {
    expect(capacityText(null)).toBe("—");
    // 0 mWh 不是有效容量，不该显示成 0
    expect(capacityText(0)).toBe("—");
  });
});

describe("percentText", () => {
  it("有电池时给百分比", () => {
    expect(percentText(power({ percent: 42 }))).toBe("42%");
  });

  it("无电池时明确说「无电池」", () => {
    expect(percentText(power({ has_battery: false, percent: null }))).toBe("无电池");
  });

  it("有电池但读不到电量时说「未知」而不是 0%", () => {
    const t = percentText(power({ percent: null }));
    expect(t).toBe("未知");
    expect(t).not.toContain("0");
  });

  it("null 输入为「—」", () => {
    expect(percentText(null)).toBe("—");
  });
});

describe("percentToken", () => {
  it("接着交流电时不因电量低告警（正在充电）", () => {
    expect(percentToken(power({ ac_online: true, percent: 5 }))).toBe("ok");
  });

  it("电池供电时低电量分档", () => {
    expect(percentToken(power({ percent: 50 }))).toBe("ok");
    expect(percentToken(power({ percent: 20 }))).toBe("warn");
    expect(percentToken(power({ percent: 10 }))).toBe("err");
  });

  it("无电池或无电量时中性", () => {
    expect(percentToken(power({ has_battery: false }))).toBe("sub");
    expect(percentToken(power({ percent: null }))).toBe("sub");
    expect(percentToken(null)).toBe("sub");
  });
});

describe("lowBatteryWarning", () => {
  it("插着电时不提醒（否则是骚扰）", () => {
    expect(lowBatteryWarning(power({ ac_online: true, percent: 3 }))).toBe("");
  });

  it("电量充足时不提醒", () => {
    expect(lowBatteryWarning(power({ percent: 80 }))).toBe("");
    expect(lowBatteryWarning(power({ percent: 21 }))).toBe("");
  });

  it("≤20% 提醒，≤10% 升级为立即接入电源", () => {
    expect(lowBatteryWarning(power({ percent: 20 }))).toContain("注意");
    expect(lowBatteryWarning(power({ percent: 10 }))).toContain("立即");
    expect(lowBatteryWarning(power({ percent: 5 }))).toContain("5%");
  });

  it("无电池/无电量时不提醒", () => {
    expect(lowBatteryWarning(power({ has_battery: false, percent: 5 }))).toBe("");
    expect(lowBatteryWarning(power({ percent: null }))).toBe("");
    expect(lowBatteryWarning(null)).toBe("");
  });
});

describe("chargingLabel", () => {
  it("充电中 / 已接电源 / 电池供电", () => {
    expect(chargingLabel(power({ charging: true, ac_online: true }))).toBe("充电中");
    expect(chargingLabel(power({ charging: false, ac_online: true }))).toBe("已接电源");
    expect(chargingLabel(power({ charging: false, ac_online: false }))).toBe("电池供电");
  });

  it("无电池时为空（不显示供电方式）", () => {
    expect(chargingLabel(power({ has_battery: false }))).toBe("");
    expect(chargingLabel(null)).toBe("");
  });
});
