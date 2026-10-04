/**
 * 启动项管理纯逻辑测试。
 *
 * 重点是两个"用户体验会直接崩"的场景：
 * 1. 非管理员面对 HKLM 项时，按钮必须提前禁用并说明原因——否则用户点了才失败；
 * 2. 已禁用项必须以**原名**显示，否则启用按钮永远点不到。
 */
import { describe, expect, it } from "vitest";

import {
  canToggle,
  commandProgram,
  filterStartup,
  locationColor,
  locationHint,
  needsAdmin,
  startupStats,
  toggleConfirmText,
  toggleErrorText,
  type StartupItem,
} from "../src/startup";

const item = (over: Partial<StartupItem> = {}): StartupItem => ({
  location: "HKCU-Run",
  name: "OneDrive",
  command: "C:\\Users\\x\\OneDrive.exe /background",
  enabled: true,
  ...over,
});

describe("needsAdmin", () => {
  it("只有 HKLM 需要管理员", () => {
    expect(needsAdmin("HKLM-Run")).toBe(true);
    expect(needsAdmin("HKCU-Run")).toBe(false);
    expect(needsAdmin("启动文件夹")).toBe(false);
    expect(needsAdmin("未知")).toBe(false);
  });
});

describe("canToggle", () => {
  it("非管理员不能改 HKLM（不管当前是启用还是禁用）", () => {
    const on = canToggle(item({ location: "HKLM-Run", enabled: true }), false);
    expect(on.ok).toBe(false);
    expect(on.reason).toContain("管理员");
    const off = canToggle(item({ location: "HKLM-Run", enabled: false }), false);
    expect(off.ok).toBe(false);
  });

  it("管理员可以改 HKLM", () => {
    expect(canToggle(item({ location: "HKLM-Run" }), true).ok).toBe(true);
  });

  it("非管理员可以改 HKCU 与启动文件夹", () => {
    expect(canToggle(item({ location: "HKCU-Run" }), false).ok).toBe(true);
    expect(canToggle(item({ location: "启动文件夹" }), false).ok).toBe(true);
  });
});

describe("位置展示", () => {
  it("三种位置颜色不同（HKLM 用红色提示影响范围更大）", () => {
    expect(locationColor("HKLM-Run")).toBe("red");
    expect(locationColor("HKCU-Run")).toBe("blue");
    expect(locationColor("启动文件夹")).toBe("green");
    expect(locationColor("未知")).toBe("default");
  });

  it("提示文案说明作用范围", () => {
    expect(locationHint("HKLM-Run")).toContain("所有用户");
    expect(locationHint("HKCU-Run")).toContain("当前用户");
    expect(locationHint("启动文件夹")).toContain("APPDATA");
  });
});

describe("startupStats", () => {
  it("统计启用与禁用", () => {
    const rows = [
      item({ name: "a", enabled: true }),
      item({ name: "b", enabled: false }),
      item({ name: "c", enabled: false }),
    ];
    expect(startupStats(rows)).toEqual({ total: 3, enabled: 1, disabled: 2 });
  });

  it("空表全 0", () => {
    expect(startupStats([])).toEqual({ total: 0, enabled: 0, disabled: 0 });
  });
});

describe("filterStartup", () => {
  const rows = [
    item({ name: "OneDrive", command: "C:\\OneDrive.exe", location: "HKCU-Run" }),
    item({ name: "SecurityHealth", command: "C:\\sec.exe", location: "HKLM-Run" }),
    item({ name: "WeChat", command: "D:\\WeChat\\WeChat.exe", location: "启动文件夹" }),
  ];

  it("空关键字返回全部", () => {
    expect(filterStartup(rows, "")).toHaveLength(3);
    expect(filterStartup(rows, "   ")).toHaveLength(3);
  });

  it("按名称/命令/位置匹配且大小写不敏感", () => {
    expect(filterStartup(rows, "onedrive")).toHaveLength(1);
    expect(filterStartup(rows, "wechat")).toHaveLength(1);
    expect(filterStartup(rows, "hklm")).toHaveLength(1);
    expect(filterStartup(rows, "D:\\")).toHaveLength(1);
  });

  it("无匹配返回空", () => {
    expect(filterStartup(rows, "不存在")).toEqual([]);
  });
});

describe("commandProgram", () => {
  it("带引号的长路径（含空格）", () => {
    expect(commandProgram('"C:\\Program Files\\App\\app.exe" --flag')).toBe(
      "C:\\Program Files\\App\\app.exe",
    );
  });

  it("无引号的路径", () => {
    expect(commandProgram("C:\\Windows\\notepad.exe")).toBe("C:\\Windows\\notepad.exe");
    expect(commandProgram("C:\\a.exe -x")).toBe("C:\\a.exe");
  });

  it("rundll32 / cmd 形态取第一个 token", () => {
    expect(commandProgram("rundll32.exe shell32.dll,Control_RunDLL")).toBe("rundll32.exe");
    expect(commandProgram("cmd /c start app")).toBe("cmd");
  });

  it("空串返回空（不猜）", () => {
    expect(commandProgram("")).toBe("");
    expect(commandProgram("   ")).toBe("");
  });

  it("解析不出时返回原串而不是编造", () => {
    expect(commandProgram("some weird command line")).toBe("some");
  });
});

describe("toggleConfirmText", () => {
  it("禁用时说明可恢复且不删程序", () => {
    const t = toggleConfirmText(item(), false);
    expect(t).toContain("OneDrive");
    expect(t).toContain("不会删除");
    expect(t).toContain("重新启用");
    expect(t).toContain("HKCU-Run");
  });

  it("启用时说明下次登录会自启", () => {
    const t = toggleConfirmText(item({ enabled: false }), true);
    expect(t).toContain("重新启用");
    expect(t).toContain("下次登录");
  });
});

describe("filterStartup 状态档", () => {
  const rows = [
    item({ name: "OneDrive", enabled: true }),
    item({ name: "SecurityHealth", enabled: false }),
    item({ name: "WeChat", enabled: false }),
  ];

  it("默认 all 不筛状态（旧签名兼容）", () => {
    expect(filterStartup(rows, "")).toHaveLength(3);
  });

  it("enabled / disabled 档分别过滤", () => {
    expect(filterStartup(rows, "", "enabled").map((r) => r.name)).toEqual(["OneDrive"]);
    expect(filterStartup(rows, "", "disabled")).toHaveLength(2);
  });

  it("状态档与关键字叠加", () => {
    expect(filterStartup(rows, "security", "disabled")).toHaveLength(1);
    expect(filterStartup(rows, "security", "enabled")).toEqual([]);
  });
});

describe("toggleErrorText", () => {
  it("补上动作上下文（是禁用还是启用失败了）", () => {
    expect(toggleErrorText(false, "需要管理员权限")).toBe("禁用失败：需要管理员权限");
    expect(toggleErrorText(true, "boom")).toBe("启用失败：boom");
  });
});
