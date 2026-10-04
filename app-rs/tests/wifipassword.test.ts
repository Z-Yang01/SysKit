/**
 * WiFi 工具页逻辑测试。
 *
 * 重点是**纪律**：密码必须经敏感路径复制；以及提示文案必须如实转述保护强度
 * （用户看到"25s 后自动清空"才知道密码不会一直留在剪贴板里）。
 * 另含周边扫描的信道聚合/推荐与二维码手输校验——fixture 里的 SSID/认证值取自
 * 中英文 netsh 的真实输出写法（netsh 原文解析本身归后端 wifi.rs，双语样例锁在
 * Rust 单测里，这里锁展示侧口径）。
 */
import { describe, expect, it } from "vitest";

import {
  aggregateChannels,
  channelAdviceText,
  channelLoadTag,
  copyResultMessage,
  maskPassword,
  needsAdminHint,
  passwordErrorHint,
  QR_AUTH_OPTIONS,
  qrAuthLabel,
  recommendChannels,
  requiresSensitiveCopy,
  SENSITIVE_COPY_COMMAND,
  scanFailedHint,
  scanSummaryText,
  signalTag,
  strengthHint,
  validateWifiQrInput,
  WIFI_SCAN_INTERVAL_MS,
  bandOf,
  type ScannedAp,
} from "../src/wifipassword";

/** 造一个 AP（默认值取自 netsh 中文样例的写法）。 */
const ap = (over: Partial<ScannedAp> = {}): ScannedAp => ({
  ssid: "MyHome",
  bssid: "aa:bb:cc:dd:ee:01",
  signal: 70,
  channel: 6,
  radio: "802.11ax",
  auth: "WPA2-Personal",
  band: "2.4G",
  ...over,
});

describe("敏感复制纪律", () => {
  it("密码必须以敏感路径复制", () => {
    expect(requiresSensitiveCopy("password")).toBe(true);
  });

  it("普通文本走普通路径", () => {
    expect(requiresSensitiveCopy("plain")).toBe(false);
  });

  it("命令名集中定义（守卫测试与实现引用同一来源）", () => {
    // 拼错命令名会静默走到普通复制，密码就漏了三重保护
    expect(SENSITIVE_COPY_COMMAND).toBe("sk_clip_copy_sensitive");
  });
});

describe("strengthHint 本地判断", () => {
  it("无密码", () => {
    const h = strengthHint("");
    expect(h.score).toBe(0);
    expect(h.advice).toContain("开放网络");
  });

  it("短密码判弱并给长度建议", () => {
    const h = strengthHint("abc123");
    expect(h.score).toBeLessThanOrEqual(2);
    expect(h.advice).toContain("8 位");
  });

  it("长且混合的密码判强", () => {
    const h = strengthHint("Tr0ub4dor&3xyz");
    expect(h.score).toBe(4);
    expect(h.label).toBe("很强");
  });

  it("只有长度没有字符类是中等", () => {
    const h = strengthHint("aaaaaaaaaaaa");
    expect(h.score).toBe(2);
    expect(h.advice).toContain("大小写");
  });

  it("中文密码按长度与字符类照常评估（不崩溃）", () => {
    const h = strengthHint("我的密码很长的中文密码");
    expect(h.score).toBeGreaterThanOrEqual(0);
    expect(h.label).toBeTruthy();
  });

  it("分数上限为 4", () => {
    expect(strengthHint("Aa1!Bb2@Cc3#Dd4$Ee5%").score).toBe(4);
  });
});

describe("错误提示", () => {
  it("识别管理员权限问题并给出可操作建议", () => {
    const err = "读取明文密钥失败——通常需要以管理员身份运行（netsh key=clear 接口限制）";
    expect(needsAdminHint(err)).toBe(true);
    const hint = passwordErrorHint(err);
    expect(hint).toContain("管理员");
    expect(hint).toContain("不是程序缺陷");
  });

  it("找不到配置文件时说明可能已删除", () => {
    expect(passwordErrorHint("找不到配置文件 MyWiFi")).toContain("已被删除");
  });

  it("其它错误原样转述（不吞掉信息）", () => {
    expect(passwordErrorHint("netsh 无法执行: 超时")).toBe("netsh 无法执行: 超时");
  });

  it("空错误给兜底文案", () => {
    expect(passwordErrorHint("")).toBe("读取失败");
  });
});

describe("copyResultMessage", () => {
  it("如实转述清空时间（用户据此判断保护强度）", () => {
    expect(copyResultMessage({ ttl: 25, note: "25s 后自动清空剪贴板" })).toContain("25s");
  });

  it("不清空时也要给出说明", () => {
    const m = copyResultMessage({ ttl: 0, note: "不会自动清空（敏感内容仍不进历史与系统剪贴板历史）" });
    expect(m).toContain("不会自动清空");
  });

  it("后端没给文案时按 TTL 兜底", () => {
    expect(copyResultMessage({ ttl: 30, note: "" })).toContain("30s");
    expect(copyResultMessage({ ttl: 0, note: "" })).toContain("已复制");
  });
});

describe("maskPassword", () => {
  it("默认遮罩，不直接显示明文", () => {
    const m = maskPassword("secret123", false);
    expect(m).not.toContain("secret");
    expect(m).toContain("•");
  });

  it("长度影响遮罩长度但有上下限", () => {
    expect(maskPassword("ab", false).length).toBe(8);
    expect(maskPassword("x".repeat(100), false).length).toBe(16);
  });

  it("显式请求时显示明文", () => {
    expect(maskPassword("secret123", true)).toBe("secret123");
  });

  it("空密码给出明确文案", () => {
    expect(maskPassword("", false)).toBe("（无密钥）");
  });
});

describe("频段与信号展示口径", () => {
  it("bandOf 与后端 band_of_channel 同口径（1~14 → 2.4G）", () => {
    expect(bandOf(0)).toBe("?");
    expect(bandOf(1)).toBe("2.4G");
    expect(bandOf(14)).toBe("2.4G");
    expect(bandOf(36)).toBe("5G");
    expect(bandOf(165)).toBe("5G");
  });

  it("信号分档取 Python _signal_color 的阈值（中英样例同判）", () => {
    // 82% 来自中文 netsh 样例（信号 : 82%），40% 来自英文样例（Signal : 40%）
    expect(signalTag(82)).toBe("green");
    expect(signalTag(60)).toBe("green");
    expect(signalTag(55)).toBe("orange");
    expect(signalTag(40)).toBe("orange");
    expect(signalTag(30)).toBe("red");
  });
});

describe("aggregateChannels 按信道聚合", () => {
  it("中英混合样例：双频 AP 各归各的信道与频段", () => {
    // fixture 即「中文 netsh + 英文 netsh」扫到混合环境：中文 SSID + 两种认证写法
    const aps = [
      ap({ ssid: "MyHome", channel: 6, auth: "WPA2-Personal", signal: 82 }),
      ap({ ssid: "MyHome", bssid: "aa:bb:cc:dd:ee:02", channel: 149, band: "5G", signal: 40 }),
      ap({ ssid: "前台WiFi", bssid: "dd:ee:ff:00:11:22", channel: 11, auth: "WPA2 - 个人", signal: 55 }),
      ap({ ssid: "(隐藏)", bssid: "cc:dd:ee:ff:00:11", channel: 1, auth: "开放式", signal: 30 }),
      ap({ ssid: "Office-5G", bssid: "bb:cc:dd:ee:ff:03", channel: 44, auth: "WPA3-SAE", band: "5G", signal: 60 }),
    ];
    const { c24, c5 } = aggregateChannels(aps);
    expect(c24).toEqual([
      { channel: 1, count: 1 },
      { channel: 6, count: 1 },
      { channel: 11, count: 1 },
    ]);
    expect(c5).toEqual([
      { channel: 44, count: 1 },
      { channel: 149, count: 1 },
    ]);
  });

  it("同信道多 BSSID 计数累加，2.4G 信道 14 不进图表（与 Python 口径一致）", () => {
    const aps = [
      ap({ channel: 6 }),
      ap({ bssid: "11:11:11:11:11:11", channel: 6 }),
      ap({ bssid: "22:22:22:22:22:22", channel: 14 }),
    ];
    const { c24 } = aggregateChannels(aps);
    expect(c24).toEqual([{ channel: 6, count: 2 }]);
  });

  it("未知信道（0）无处安放，跳过不崩溃", () => {
    const { c24, c5 } = aggregateChannels([ap({ channel: 0, band: "?" })]);
    expect(c24).toEqual([]);
    expect(c5).toEqual([]);
  });

  it("占用分档：≤2 绿、≤4 橙、更多红", () => {
    expect(channelLoadTag(1)).toBe("green");
    expect(channelLoadTag(2)).toBe("green");
    expect(channelLoadTag(4)).toBe("orange");
    expect(channelLoadTag(5)).toBe("red");
  });
});

describe("recommendChannels 推荐信道（syskit/wifi.py 移植）", () => {
  it("无数据：2.4G 从 1 起荐、5G 为 null", () => {
    const a = recommendChannels([]);
    expect(a.ch24).toBe(1);
    expect(a.same24).toBe(0);
    expect(a.adj24).toBe(0);
    expect(a.ch5).toBeNull();
    expect(channelAdviceText(a)).toContain("未发现 5G 网络");
  });

  it("邻频重叠计入强弱：信道 1 上的强 AP 把推荐推向 11", () => {
    // ch1 强（82%）+ ch3 弱（30%）：6 距 1/3 都近，11 最远
    const a = recommendChannels([
      ap({ channel: 1, signal: 82 }),
      ap({ channel: 3, signal: 30 }),
    ]);
    expect(a.ch24).toBe(11);
  });

  it("同频与邻频计数（±4 算邻频，同频不算）", () => {
    // 1/6 都空 → 平分取先到的 1
    const a = recommendChannels([
      ap({ channel: 11, signal: 80 }),
      ap({ channel: 7, signal: 50 }),
      ap({ channel: 11, signal: 40 }),
    ]);
    expect(a.ch24).toBe(1);
    expect(a.same24).toBe(0);
    expect(a.adj24).toBe(0);
    // 1/6 也被强信号占用时，只有一个远端弱邻频的 11 最空（完全空置的信道得 0 分，
    // 永远优先——所以 1/6 必须先被占上）
    const b = recommendChannels([
      ap({ channel: 1, signal: 90 }),
      ap({ channel: 6, signal: 90 }),
      ap({ channel: 7, signal: 20 }),
    ]);
    expect(b.ch24).toBe(11);
    expect(b.adj24).toBe(1);
    // 1/6 被强信号占死、11 上有两个弱 AP + 一个邻频：荐 11（同频 2 / 邻频 1）
    const c = recommendChannels([
      ap({ channel: 1, signal: 90 }),
      ap({ channel: 6, signal: 90 }),
      ap({ channel: 11, signal: 10 }),
      ap({ channel: 11, signal: 10 }),
      ap({ channel: 7, signal: 20 }),
    ]);
    expect(c.ch24).toBe(11);
    expect(c.same24).toBe(2);
    expect(c.adj24).toBe(1);
  });

  it("5G 按 80MHz 块评分：非 DFS 明显不空时才荐 DFS", () => {
    // 只有 36 块被占用（强信号）→ 149 块完全空闲，直接胜出
    const a = recommendChannels([
      ap({ channel: 36, signal: 90, band: "5G" }),
      ap({ channel: 44, signal: 90, band: "5G" }),
    ]);
    expect(a.ch5).toBe(149);
    expect(a.block5).toBe(0);
    expect(channelAdviceText(a)).toContain("5G 用信道 149");
    // 两个非 DFS 块都被占死（1.8 / 1.85），DFS 块空但罚 1.0 仍更低：
    // 平分取最低 DFS 锚点 52（与 Python 的严格小于语义一致）
    const b = recommendChannels([
      ap({ channel: 36, signal: 90, band: "5G" }),
      ap({ channel: 44, signal: 90, band: "5G" }),
      ap({ channel: 149, signal: 95, band: "5G" }),
      ap({ channel: 161, signal: 90, band: "5G" }),
    ]);
    expect(b.ch5).toBe(52);
    expect(b.block5).toBe(0);
  });

  it("5G 块内计数与文案（80M 块内 AP 数）", () => {
    // 36 块占 1.0；其余 DFS 块罚分后也是 1.0、149 块占 1.8：
    // 36 与 DFS 平分，严格小于语义取先到的 36
    const a = recommendChannels([
      ap({ channel: 40, signal: 50, band: "5G" }),
      ap({ channel: 48, signal: 50, band: "5G" }),
      ap({ channel: 149, signal: 90, band: "5G" }),
      ap({ channel: 153, signal: 90, band: "5G" }),
    ]);
    expect(a.ch5).toBe(36);
    expect(a.block5).toBe(2);
    expect(channelAdviceText(a)).toContain("（80M 块内 2 个 AP）");
  });
});

describe("扫描状态与失败提示", () => {
  it("scanSummaryText 对齐 Python 状态行（全角空格 + HH:MM:SS）", () => {
    const s = scanSummaryText(5, new Date(2026, 9, 3, 8, 5, 9));
    expect(s).toBe("共发现 5 个 BSSID　08:05:09");
  });

  it("自动刷新周期为 8 秒（对齐 Python tick）", () => {
    expect(WIFI_SCAN_INTERVAL_MS).toBe(8_000);
  });

  it("位置服务错误原样转述（后端已给可操作指引）", () => {
    const err = "需要开启系统「位置」服务才能扫描 WiFi（设置→隐私和安全性→位置）";
    expect(scanFailedHint(err)).toBe(err);
  });

  it("空错误给不空的兜底文案", () => {
    expect(scanFailedHint("")).toContain("扫描失败");
    expect(scanFailedHint("netsh 无法执行: 超时")).toContain("超时");
  });
});

describe("二维码分享（手输模式）", () => {
  it("T 值选项齐全且 qrAuthLabel 可读", () => {
    const values = QR_AUTH_OPTIONS.map((o) => o.value);
    expect(values).toEqual(["WPA", "SAE", "WEP", "nopass"]);
    expect(qrAuthLabel("SAE")).toContain("WPA3");
    expect(qrAuthLabel("nopass")).toContain("开放");
    // 未知值原样回显（不吞信息）
    expect(qrAuthLabel("mystery")).toBe("mystery");
  });

  it("校验：SSID 必填、非 nopass 必须有密码", () => {
    expect(validateWifiQrInput("MyNet", "pw", "WPA")).toBe("");
    expect(validateWifiQrInput("   ", "pw", "WPA")).toContain("SSID");
    expect(validateWifiQrInput("MyNet", "", "WPA")).toContain("密码");
    // 开放网络可以没有密码
    expect(validateWifiQrInput("Cafe", "", "nopass")).toBe("");
  });
});
