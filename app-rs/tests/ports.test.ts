/**
 * 端口/进程与局域网页纯逻辑测试。
 *
 * 重点是筛选与「不能结束的进程」的判定——结束进程不可逆，
 * 受保护 PID 的判定错了会直接导致系统异常。
 */
import { describe, expect, it } from "vitest";

import {
  canKill,
  filterPorts,
  formatLatency,
  formatMac,
  killConfirmText,
  killConfirmTextFor,
  lanSummary,
  sortDevices,
  stateTagColor,
  stateToken,
  summarizePorts,
  type LanDevice,
  type LanResult,
  type PortRow,
} from "../src/ports";

function row(over: Partial<PortRow> = {}): PortRow {
  return {
    proto: "TCP",
    local: "0.0.0.0:135",
    remote: "0.0.0.0:0",
    state: "LISTENING",
    pid: 1234,
    process: "svchost.exe",
    listening: true,
    port: 135,
    ...over,
  };
}

const device = (over: Partial<LanDevice> = {}): LanDevice => ({
  ip: "192.168.1.1",
  mac: "aa-bb-cc-dd-ee-ff",
  kind: "动态",
  latency_ms: 3,
  ...over,
});

describe("状态展示", () => {
  it("LISTENING 用 ok 语义色，ESTABLISHED 用 accent", () => {
    expect(stateToken("LISTENING", true)).toBe("ok");
    expect(stateToken("ESTABLISHED", false)).toBe("accent");
    expect(stateToken("TIME_WAIT", false)).toBe("warn");
    expect(stateToken("CLOSE_WAIT", false)).toBe("warn");
    expect(stateToken("SYN_SENT", false)).toBe("sub");
  });

  it("Tag 颜色映射", () => {
    expect(stateTagColor("LISTENING", true)).toBe("green");
    expect(stateTagColor("ESTABLISHED", false)).toBe("blue");
    expect(stateTagColor("TIME_WAIT", false)).toBe("orange");
    expect(stateTagColor("SYN_SENT", false)).toBe("default");
  });

  it("UDP 无状态但 listening=true 时仍按监听配色", () => {
    expect(stateToken("", true)).toBe("ok");
    expect(stateTagColor("", true)).toBe("green");
  });
});

describe("filterPorts", () => {
  const rows = [
    row({ pid: 1234, process: "svchost.exe", port: 135 }),
    row({ proto: "TCP", local: "192.168.1.100:52344", state: "ESTABLISHED", listening: false, pid: 9876, process: "chrome.exe", port: 52344 }),
    row({ proto: "UDP", local: "0.0.0.0:5353", state: "", listening: true, pid: 5678, process: "mdns.exe", port: 5353 }),
  ];

  it("空关键字返回全部", () => {
    expect(filterPorts(rows, "", false)).toHaveLength(3);
  });

  it("按端口/PID/进程名/状态/IP 匹配且大小写不敏感", () => {
    expect(filterPorts(rows, "135", false)).toHaveLength(1);
    expect(filterPorts(rows, "9876", false)).toHaveLength(1);
    expect(filterPorts(rows, "CHROME", false)).toHaveLength(1);
    expect(filterPorts(rows, "established", false)).toHaveLength(1);
    expect(filterPorts(rows, "192.168.1", false)).toHaveLength(1);
    expect(filterPorts(rows, "udp", false)).toHaveLength(1);
  });

  it("只看监听时排除已建立连接", () => {
    const l = filterPorts(rows, "", true);
    expect(l).toHaveLength(2);
    expect(l.every((r) => r.listening)).toBe(true);
  });

  it("关键字与监听过滤叠加", () => {
    expect(filterPorts(rows, "chrome", true)).toHaveLength(0);
    expect(filterPorts(rows, "mdns", true)).toHaveLength(1);
  });

  it("无匹配返回空数组", () => {
    expect(filterPorts(rows, "不存在的进程", false)).toEqual([]);
  });

  it("port 为 null 时不影响匹配（不拼出 \"null\" 假命中）", () => {
    const r = [row({ port: null, local: "pipe", pid: 1, process: "" })];
    expect(filterPorts(r, "null", false)).toHaveLength(0);
  });
});

describe("summarizePorts", () => {
  it("统计总数/监听数/去重进程数", () => {
    const rows = [
      row({ pid: 1, listening: true }),
      row({ pid: 1, listening: false, state: "ESTABLISHED" }),
      row({ pid: 2, listening: true }),
    ];
    expect(summarizePorts(rows)).toEqual({ total: 3, listening: 2, processes: 2 });
  });

  it("空列表全 0", () => {
    expect(summarizePorts([])).toEqual({ total: 0, listening: 0, processes: 0 });
  });
});

describe("canKill 受保护进程", () => {
  it("拒绝 PID 0 与 4", () => {
    expect(canKill(0).ok).toBe(false);
    expect(canKill(0).reason).toContain("系统空闲");
    expect(canKill(4).ok).toBe(false);
    expect(canKill(4).reason).toContain("系统进程");
  });

  it("拒绝自身 PID（给出自身 PID 时）", () => {
    expect(canKill(9999, 9999).ok).toBe(false);
    expect(canKill(9999, 9999).reason).toContain("自身");
  });

  it("普通 PID 允许", () => {
    expect(canKill(1234).ok).toBe(true);
    expect(canKill(1234).reason).toBe("");
    expect(canKill(1234, 9999).ok).toBe(true);
  });
});

describe("killConfirmText", () => {
  it("点名进程与 PID，并说明后果", () => {
    const t = killConfirmText(row({ pid: 4321, process: "chrome.exe" }));
    expect(t).toContain("chrome.exe");
    expect(t).toContain("4321");
    expect(t).toContain("丢失");
  });

  it("无进程名时退回 PID 称呼", () => {
    const t = killConfirmText(row({ pid: 4321, process: "" }));
    expect(t).toContain("PID 4321");
  });
});

describe("局域网展示", () => {
  it("MAC 转大写", () => {
    expect(formatMac("aa-bb-cc-dd-ee-ff")).toBe("AA-BB-CC-DD-EE-FF");
  });

  it("延迟无值显示「—」而不是 0", () => {
    expect(formatLatency(null)).toBe("—");
    expect(formatLatency(0)).toBe("0 ms");
    expect(formatLatency(12)).toBe("12 ms");
  });

  it("按 IP 末段数值升序（不是字符串序）", () => {
    const list = [
      device({ ip: "192.168.1.100" }),
      device({ ip: "192.168.1.9" }),
      device({ ip: "192.168.1.20" }),
    ];
    expect(sortDevices(list).map((d) => d.ip)).toEqual([
      "192.168.1.9",
      "192.168.1.20",
      "192.168.1.100",
    ]);
  });

  it("sortDevices 不改原数组", () => {
    const list = [device({ ip: "192.168.1.9" }), device({ ip: "192.168.1.2" })];
    sortDevices(list);
    expect(list[0].ip).toBe("192.168.1.9");
  });

  it("摘要文案含网段/扫描数/设备数", () => {
    const r: LanResult = { prefix: "192.168.1", scanned: 254, devices: [device()], count: 1 };
    const s = lanSummary(r);
    expect(s).toContain("192.168.1.0/24");
    expect(s).toContain("254");
    expect(s).toContain("1 台");
  });

  it("未扫描时给明确提示", () => {
    expect(lanSummary(null)).toBe("尚未扫描");
  });
});

/** 端口页与性能监控页共用同一份结束进程文案——两处各写一份迟早说岔。 */
describe("killConfirmTextFor", () => {
  it("有进程名用进程名", () => {
    const t = killConfirmTextFor("chrome.exe", 4321);
    expect(t).toContain("chrome.exe");
    expect(t).toContain("4321");
    expect(t).toContain("未保存的数据会丢失");
  });

  it("进程名缺失退回 PID 兜底，不出「undefined」", () => {
    const t = killConfirmTextFor("", 99);
    expect(t).toContain("PID 99");
    expect(t).not.toContain("undefined");
  });

  it("行版转发与 name+pid 版一致", () => {
    expect(killConfirmText({ process: "nginx.exe", pid: 80 })).toBe(
      killConfirmTextFor("nginx.exe", 80),
    );
  });
});
