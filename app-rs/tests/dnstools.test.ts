/**
 * DNS / 子网 / ping·路由 页纯逻辑测试。
 *
 * 重点是展示格式化与「失败级别」的判定——把超时显示成 error 会让用户以为
 * 域名不存在，而其实只是网络慢。
 */
import { describe, expect, it } from "vitest";

import {
  PREFIX_MAX,
  PREFIX_MIN,
  dnsLevel,
  dnsSummary,
  formatCount,
  formatHopHost,
  formatHopLatencies,
  hopTimedOut,
  isValidPrefix,
  lossToken,
  pingSummary,
  subnetRows,
  traceSummary,
  type DnsAnswer,
  type PingResult,
  type SubnetCalcResult,
  type TraceResult,
} from "../src/dnstools";

const answer = (over: Partial<DnsAnswer> = {}): DnsAnswer => ({
  ok: true,
  domain: "www.baidu.com",
  server: "223.5.5.5",
  ips: ["1.2.3.4"],
  ms: 12,
  message: "解析成功，用时 12 ms",
  ...over,
});

describe("前缀范围", () => {
  it("0~32 合法", () => {
    expect(PREFIX_MIN).toBe(0);
    expect(PREFIX_MAX).toBe(32);
    expect(isValidPrefix(0)).toBe(true);
    expect(isValidPrefix(24)).toBe(true);
    expect(isValidPrefix(32)).toBe(true);
    expect(isValidPrefix(-1)).toBe(false);
    expect(isValidPrefix(33)).toBe(false);
    expect(isValidPrefix(24.5)).toBe(false);
  });
});

describe("subnetRows", () => {
  it("优先用后端给的 lines", () => {
    const r: SubnetCalcResult = {
      info: {
        prefix: 24,
        mask: "255.255.255.0",
        wildcard: "0.0.0.255",
        network: "192.168.1.0/24",
        broadcast: "192.168.1.255",
        first_host: "192.168.1.1",
        last_host: "192.168.1.254",
        usable_hosts: 254,
        total_addresses: 256,
        is_private: true,
      },
      lines: [
        ["网络地址", "192.168.1.0/24"],
        ["可用主机数", "254"],
      ],
      presets: [],
    };
    expect(subnetRows(r)).toEqual([
      ["网络地址", "192.168.1.0/24"],
      ["可用主机数", "254"],
    ]);
  });

  it("lines 缺失时按 info 兜底重建（八行，顺序固定）", () => {
    const r: SubnetCalcResult = {
      info: {
        prefix: 30,
        mask: "255.255.255.252",
        wildcard: "0.0.0.3",
        network: "10.0.0.4/30",
        broadcast: "10.0.0.7",
        first_host: "10.0.0.5",
        last_host: "10.0.0.6",
        usable_hosts: 2,
        total_addresses: 4,
        is_private: true,
      },
      lines: [],
      presets: [],
    };
    const rows = subnetRows(r);
    expect(rows).toHaveLength(8);
    expect(rows[0][0]).toBe("网络地址");
    expect(rows[4][1]).toBe("10.0.0.5 ~ 10.0.0.6");
    expect(rows[7][1]).toBe("私有地址");
  });

  it("空结果返回空数组", () => {
    expect(subnetRows(null)).toEqual([]);
  });
});

describe("formatCount", () => {
  it("千分位可读（/0 的 4294967296 不该是一串数字）", () => {
    expect(formatCount(254)).toBe("254");
    expect(formatCount(4294967296)).toBe("4,294,967,296");
  });
});

describe("DNS 结果级别", () => {
  it("成功 → success", () => {
    expect(dnsLevel(answer())).toBe("success");
  });

  it("超时/无应答 → warning（不是用户输入问题）", () => {
    expect(dnsLevel(answer({ ok: false, message: "DNS 无应答（超时）" }))).toBe("warning");
    expect(dnsLevel(answer({ ok: false, message: "DNS 响应异常：无应答记录" }))).toBe("warning");
  });

  it("其他失败 → error", () => {
    expect(dnsLevel(answer({ ok: false, message: "DNS 返回错误码 3" }))).toBe("error");
  });
});

describe("dnsSummary", () => {
  it("成功时给出域名/结果/服务器/耗时", () => {
    const s = dnsSummary(answer({ ips: ["1.1.1.1", "2.2.2.2"] }));
    expect(s).toContain("www.baidu.com");
    expect(s).toContain("1.1.1.1, 2.2.2.2");
    expect(s).toContain("223.5.5.5");
    expect(s).toContain("12 ms");
  });

  it("失败时给出原因", () => {
    const s = dnsSummary(answer({ ok: false, ips: [], message: "无应答" }));
    expect(s).toContain("失败");
    expect(s).toContain("无应答");
  });

  it("未查询时明确提示", () => {
    expect(dnsSummary(null)).toBe("尚未查询");
  });
});

describe("ping 展示", () => {
  it("丢包率配色三档", () => {
    expect(lossToken(0)).toBe("ok");
    expect(lossToken(25)).toBe("warn");
    expect(lossToken(100)).toBe("err");
    expect(lossToken(null)).toBe("sub");
  });

  it("摘要含丢包与延迟，未知值显示 ?", () => {
    const r: PingResult = {
      ok: true,
      host: "1.1.1.1",
      loss_percent: 0,
      latency_ms: 12,
      raw: "",
    };
    expect(pingSummary(r)).toBe("1.1.1.1：丢包 0% · 延迟 12 ms");
    expect(
      pingSummary({ ...r, loss_percent: null, latency_ms: null }),
    ).toContain("丢包 ?%");
  });

  it("未 ping 时明确提示", () => {
    expect(pingSummary(null)).toBe("尚未 ping");
  });
});

describe("路由跳点展示", () => {
  it("延迟列把超时显示成 *", () => {
    expect(formatHopLatencies([1, 2, 3])).toBe("1 ms  2 ms  3 ms");
    expect(formatHopLatencies([12, null, 11])).toBe("12 ms  *  11 ms");
    expect(formatHopLatencies([null, null, null])).toBe("*  *  *");
  });

  it("主机为空时显示「请求超时」", () => {
    expect(formatHopHost({ hop: 2, latencies: [null, null, null], host: "" })).toBe("请求超时");
    expect(formatHopHost({ hop: 1, latencies: [1, 1, 1], host: "1.2.3.4" })).toBe("1.2.3.4");
  });

  it("hopTimedOut 只在三次全超时时为真", () => {
    expect(hopTimedOut({ hop: 1, latencies: [null, null, null], host: "" })).toBe(true);
    expect(hopTimedOut({ hop: 2, latencies: [10, null, null], host: "x" })).toBe(false);
    expect(hopTimedOut({ hop: 3, latencies: [10, 11, 12], host: "x" })).toBe(false);
  });

  it("路由摘要含主机与跳数", () => {
    const r: TraceResult = { ok: true, host: "a.com", hops: [], count: 7, raw: "" };
    expect(traceSummary(r)).toBe("a.com：共 7 跳");
    expect(traceSummary(null)).toBe("尚未跟踪");
  });
});
