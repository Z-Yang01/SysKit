import { describe, expect, it } from "vitest";

import { batteryTone, deviceIcon, LOW_BATTERY } from "../src/DevicePanel";

/** 电量分档：低电量必须一眼可辨（提醒阈值与配色是两处，别各走各的）。 */
describe("batteryTone", () => {
  it("低于阈值判红", () => {
    expect(batteryTone(0)).toBe("err");
    expect(batteryTone(LOW_BATTERY - 1)).toBe("err");
  });

  it("临界值不判红（20% 仍算黄）", () => {
    expect(batteryTone(LOW_BATTERY)).toBe("warn");
    expect(batteryTone(39)).toBe("warn");
    expect(batteryTone(40)).toBe("ok");
    expect(batteryTone(100)).toBe("ok");
  });
});

describe("deviceIcon", () => {
  it("按后端粗分类给图标", () => {
    expect(deviceIcon("音频设备")).toBe("🎧");
    expect(deviceIcon("外设（鼠标/键盘）")).toBe("🖱");
    expect(deviceIcon("手机")).toBe("📱");
  });

  it("未知类型有兜底图标，不返回空", () => {
    expect(deviceIcon("蓝牙设备")).toBe("📶");
    expect(deviceIcon("")).toBe("📶");
  });
});
