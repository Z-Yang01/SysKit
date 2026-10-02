/**
 * 系统助手页纯逻辑测试。
 *
 * 重点是**断流守卫**：页面隐藏期间采样被跳过，若不清空旧序列，切回时会把
 * 10 分钟前的点和现在的点连成一条"平稳曲线"——用户会以为这段时间一直很稳，
 * 而实际是没有数据。这类"看起来正常的错误"正是本文件要锁住的。
 */
import { describe, expect, it } from "vitest";

import {
  SERIES_LEN,
  formatRate,
  humanBytes,
  isStale,
  procsSummary,
  procRows,
  procShare,
  pushSample,
  signalToken,
  usageToken,
  wifiSummary,
  type TopProcs,
  type WifiInfo,
} from "../src/system";

const wifi = (over: Partial<WifiInfo> = {}): WifiInfo => ({
  connected: true,
  ssid: "MyWiFi",
  signal: 80,
  dbm: -60,
  bssid: "aa:bb:cc:dd:ee:ff",
  rx_kbps: 144000,
  tx_kbps: 144000,
  label: "极佳",
  ...over,
});

describe("pushSample", () => {
  it("追加到末尾", () => {
    expect(pushSample([1, 2], 3, 5)).toEqual([1, 2, 3]);
  });

  it("超过上限时丢最旧的（保持窗口长度）", () => {
    expect(pushSample([1, 2, 3], 4, 3)).toEqual([2, 3, 4]);
  });

  it("不修改入参数组（React 状态不可变）", () => {
    const src = [1, 2, 3];
    const out = pushSample(src, 4, 3);
    expect(src).toEqual([1, 2, 3]);
    expect(out).not.toBe(src);
  });

  it("空数组也能推", () => {
    expect(pushSample([], 5)).toEqual([5]);
  });

  it("默认上限为 SERIES_LEN，且推入 SERIES_LEN+1 次后长度不变", () => {
    let s: number[] = [];
    for (let i = 0; i < SERIES_LEN + 1; i += 1) s = pushSample(s, i);
    expect(s).toHaveLength(SERIES_LEN);
    expect(s[s.length - 1]).toBe(SERIES_LEN);
    // 最旧的被丢弃后，首位应是第 2 个样本（vitest 的 toBe 不接受消息参数，故写成注释）
    expect(s[0]).toBe(1);
  });
});

describe("isStale 断流守卫", () => {
  it("从未有数据时不算断流（避免启动即清空）", () => {
    expect(isStale(null, 1_000_000)).toBe(false);
  });

  it("窗口内不算断流", () => {
    const now = 1_000_000_000;
    expect(isStale(now - 5_000, now)).toBe(false);
    expect(isStale(now - 599_000, now)).toBe(false);
  });

  it("超过 600 秒算断流", () => {
    const now = 1_000_000_000;
    expect(isStale(now - 601_000, now)).toBe(true);
    expect(isStale(now - 3_600_000, now)).toBe(true);
  });

  it("恰好 600 秒不算（边界用 >）", () => {
    const now = 1_000_000_000;
    expect(isStale(now - 600_000, now)).toBe(false);
  });

  it("支持自定义窗口", () => {
    const now = 1_000_000_000;
    expect(isStale(now - 31_000, now, 30)).toBe(true);
    expect(isStale(now - 29_000, now, 30)).toBe(false);
  });
});

describe("usageToken 分档", () => {
  it("三档边界", () => {
    expect(usageToken(0)).toBe("ok");
    expect(usageToken(69.9)).toBe("ok");
    expect(usageToken(70)).toBe("warn");
    expect(usageToken(89.9)).toBe("warn");
    expect(usageToken(90)).toBe("err");
    expect(usageToken(100)).toBe("err");
  });
});

describe("signalToken 分档", () => {
  it("无信号为中性（不是 err——可能只是没连 WiFi）", () => {
    expect(signalToken(null)).toBe("sub");
    expect(signalToken(undefined)).toBe("sub");
  });

  it("三档边界", () => {
    expect(signalToken(100)).toBe("ok");
    expect(signalToken(75)).toBe("ok");
    expect(signalToken(74)).toBe("warn");
    expect(signalToken(50)).toBe("warn");
    expect(signalToken(49)).toBe("err");
    expect(signalToken(0)).toBe("err");
  });
});

describe("wifiSummary", () => {
  it("已连接：SSID + 信号 + dBm + 分档", () => {
    const s = wifiSummary(wifi());
    expect(s).toContain("MyWiFi");
    expect(s).toContain("80%");
    expect(s).toContain("-60 dBm");
    expect(s).toContain("极佳");
  });

  it("隐藏网络（SSID 空）必须显示「隐藏网络」而不是空白", () => {
    // 否则用户看到"已连接 + 空名字"会以为程序坏了
    const s = wifiSummary(wifi({ ssid: "", signal: 60, dbm: -70 }));
    expect(s).toContain("隐藏网络");
    expect(s).not.toContain("· 信号 —");
  });

  it("未连接时明确说明", () => {
    expect(wifiSummary(wifi({ connected: false, ssid: "", signal: null, dbm: null }))).toContain(
      "未连接",
    );
  });

  it("未读取时为「未读取」", () => {
    expect(wifiSummary(null)).toBe("未读取");
  });

  it("信号为 null 但已连接时显示「—」而不是 0%", () => {
    const s = wifiSummary(wifi({ signal: null, dbm: null }));
    expect(s).toContain("信号 —");
    expect(s).not.toContain("0%");
  });
});

describe("Top 进程", () => {
  const data: TopProcs = {
    rows: [
      { pid: 75836, name: "chrome.exe", bytes: 900 },
      { pid: 3892, name: "Memory Compression", bytes: 100 },
    ],
    count: 120,
    total_bytes: 1000,
    limit: 15,
  };

  it("procShare 按进程合计算占比", () => {
    expect(procShare(900, 1000)).toBeCloseTo(90);
    expect(procShare(100, 1000)).toBeCloseTo(10);
  });

  it("合计为 0 时不除零", () => {
    expect(procShare(100, 0)).toBe(0);
    expect(procShare(0, 0)).toBe(0);
  });

  it("procRows 附加占比且不改原对象", () => {
    const rows = procRows(data);
    expect(rows).toHaveLength(2);
    expect(rows[0].share).toBeCloseTo(90);
    expect(rows[0].name).toBe("chrome.exe");
    expect(data.rows[0]).not.toHaveProperty("share");
  });

  it("null 输入返回空数组", () => {
    expect(procRows(null)).toEqual([]);
  });

  it("摘要含进程数/合计/显示条数", () => {
    const s = procsSummary(data);
    expect(s).toContain("120");
    expect(s).toContain("2");
    expect(s).toContain("显示前");
  });

  it("未采集时明确提示", () => {
    expect(procsSummary(null)).toBe("尚未采集");
  });
});

describe("formatRate", () => {
  it("kbps → Mbps", () => {
    expect(formatRate(144000)).toBe("144 Mbps");
    expect(formatRate(54000)).toBe("54 Mbps");
    // 0 表示取不到链路速率，不该显示成 "0 Mbps"
    expect(formatRate(0)).toBe("—");
  });
});

describe("humanBytes", () => {
  it("按 1024 进制换算", () => {
    expect(humanBytes(0)).toBe("0 B/s");
    expect(humanBytes(512)).toBe("512 B/s");
    expect(humanBytes(1024)).toBe("1.0 KB/s");
    expect(humanBytes(1024 * 1024)).toBe("1.0 MB/s");
    expect(humanBytes(1024 * 1024 * 1024)).toBe("1.0 GB/s");
  });

  it("大数值不显示多余小数", () => {
    expect(humanBytes(500 * 1024 * 1024)).toBe("500 MB/s");
  });

  it("负数保留符号（磁盘计数器回绕时可能出现）", () => {
    expect(humanBytes(-1024)).toBe("-1.0 KB/s");
  });
});
