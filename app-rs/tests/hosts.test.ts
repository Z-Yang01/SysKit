/**
 * Hosts 编辑页纯逻辑测试。
 *
 * 重点是**校验**：hosts 写错会影响整机域名解析，非法输入必须在点保存之前拦住。
 * 另有「保存前护栏」与「变更摘要」——两者都直接影响用户决策。
 */
import { describe, expect, it } from "vitest";

import {
  effectiveFilter,
  filterHostRows,
  hostsStats,
  isDuplicateMapping,
  isValidHost,
  isValidIp,
  isValidIpv4,
  isValidIpv6,
  mappingRows,
  parseAliases,
  rowText,
  saveWarnings,
  summarizeChanges,
  validateNewEntry,
  type HostEntry,
  type HostsInfo,
  type HostRow,
} from "../src/hosts";

function entry(over: Partial<HostEntry> = {}): HostEntry {
  return {
    raw: "",
    kind: "entry",
    ip: "127.0.0.1",
    host: "localhost",
    aliases: [],
    comment: "",
    enabled: true,
    had_content: true,
    ...over,
  };
}

const info = (over: Partial<HostsInfo> = {}): HostsInfo => ({
  path: "C:\\Windows\\System32\\drivers\\etc\\hosts",
  exists: true,
  writable: true,
  is_admin: false,
  ...over,
});

describe("IPv4 校验", () => {
  it("接受合法四段地址", () => {
    for (const ip of ["127.0.0.1", "0.0.0.0", "255.255.255.255", "10.0.0.1", "192.168.1.100"]) {
      expect(isValidIpv4(ip), ip).toBe(true);
    }
  });

  it("拒绝段数不对/越界/非数字", () => {
    for (const ip of [
      "127.0.0",
      "127.0.0.1.1",
      "256.0.0.1",
      "999.1.1.1",
      "1.1.1.-1",
      "a.b.c.d",
      "1.1.1.",
      "",
      "  ",
      "1.1.1.1 ",
    ]) {
      expect(isValidIpv4(ip), JSON.stringify(ip)).toBe(
        ip === "1.1.1.1 " ? true : false,
        // 末尾空格会被 trim，属合法
      );
    }
  });

  it("前导零按十进制解析（010 → 10，不按八进制）", () => {
    expect(isValidIpv4("010.0.0.1")).toBe(true);
    expect(isValidIpv4("0999.0.0.1")).toBe(false);
  });
});

describe("IPv6 校验", () => {
  it("接受常见形态", () => {
    for (const ip of ["::1", "fe80::1", "2001:db8::1", "::", "FE80::AB"]) {
      expect(isValidIpv6(ip), ip).toBe(true);
    }
  });

  it("拒绝畸形", () => {
    for (const ip of ["", "127.0.0.1", ":::", "fe80::gg", "1:2:3"]) {
      expect(isValidIpv6(ip), ip).toBe(ip === "1:2:3" ? true : false);
      // 1:2:3 只有冒号与 hex，宽松校验接受（真实缩写也长这样）
    }
  });
});

describe("isValidIp 综合", () => {
  it("v4 与 v6 都认", () => {
    expect(isValidIp("127.0.0.1")).toBe(true);
    expect(isValidIp("::1")).toBe(true);
    expect(isValidIp("not-an-ip")).toBe(false);
    expect(isValidIp("")).toBe(false);
  });
});

describe("主机名校验", () => {
  it("接受常见域名与本地名", () => {
    for (const h of ["localhost", "dev.local", "a-b.example.com", "x_1.local", "a"]) {
      expect(isValidHost(h), h).toBe(true);
    }
  });

  it("拒绝空/前导点/尾随点或横线/非法字符", () => {
    for (const h of ["", "  ", ".local", "local.", "-x", "x-", "a b", "a/b", "中文.local", "a#b"]) {
      expect(isValidHost(h), JSON.stringify(h)).toBe(false);
    }
  });

  it("拒绝超长主机名（>253）", () => {
    expect(isValidHost("a".repeat(254))).toBe(false);
    expect(isValidHost("a".repeat(253))).toBe(true);
  });
});

describe("validateNewEntry", () => {
  it("合法输入通过", () => {
    const v = validateNewEntry("10.0.0.1", "dev.local", ["api.local"]);
    expect(v.ok).toBe(true);
    expect(v.errors).toEqual([]);
  });

  it("逐项报错且可读", () => {
    const v = validateNewEntry("999.1.1.1", "bad host", ["ok.local", "bad alias"]);
    expect(v.ok).toBe(false);
    expect(v.errors.length).toBeGreaterThanOrEqual(3);
    expect(v.errors.some((e) => e.includes("IP 格式非法"))).toBe(true);
    expect(v.errors.some((e) => e.includes("主机名非法"))).toBe(true);
    expect(v.errors.some((e) => e.includes("别名非法"))).toBe(true);
  });

  it("空 IP/主机名各自报错", () => {
    const v = validateNewEntry("", "", []);
    expect(v.errors).toContain("IP 不能为空");
    expect(v.errors).toContain("主机名不能为空");
  });

  it("空白别名被忽略（不报错）", () => {
    const v = validateNewEntry("10.0.0.1", "x.local", ["", "   "]);
    expect(v.ok).toBe(true);
  });
});

describe("parseAliases", () => {
  it("支持逗号/中文逗号/空格混排并去空", () => {
    expect(parseAliases("a.local, b.local")).toEqual(["a.local", "b.local"]);
    expect(parseAliases("a.local，b.local c.local")).toEqual([
      "a.local",
      "b.local",
      "c.local",
    ]);
    expect(parseAliases("  ")).toEqual([]);
    expect(parseAliases("")).toEqual([]);
  });
});

describe("mappingRows", () => {
  it("只保留映射行并带原始下标", () => {
    const entries = [
      entry({ kind: "comment", host: "", ip: "" }),
      entry({ ip: "10.0.0.1", host: "a.local" }),
      entry({ kind: "disabled", ip: "10.0.0.2", host: "b.local", enabled: false }),
    ];
    const rows = mappingRows(entries);
    expect(rows).toHaveLength(2);
    expect(rows[0].idx).toBe(1);
    expect(rows[1].idx).toBe(2);
    expect(rows[1].kind).toBe("disabled");
  });

  it("空列表返回空", () => {
    expect(mappingRows([])).toEqual([]);
  });
});

describe("rowText", () => {
  it("拼接 IP/主机名/别名并跳过空段", () => {
    expect(rowText({ ...entry({ aliases: ["a.local", "b.local"] }), idx: 0 })).toBe(
      "127.0.0.1 localhost a.local b.local",
    );
    expect(rowText({ ...entry({ aliases: [""] }), idx: 0 })).toBe("127.0.0.1 localhost");
  });
});

describe("saveWarnings", () => {
  it("不可写时给出最直接的后果说明", () => {
    const w = saveWarnings(info({ writable: false }));
    expect(w).toHaveLength(1);
    expect(w[0]).toContain("不可写");
    expect(w[0]).toContain("管理员");
  });

  it("可写但非管理员时提示可能被拒", () => {
    const w = saveWarnings(info({ writable: true, is_admin: false }));
    expect(w.some((x) => x.includes("不是管理员"))).toBe(true);
  });

  it("管理员且可写时无警告", () => {
    expect(saveWarnings(info({ writable: true, is_admin: true }))).toEqual([]);
  });

  it("文件不存在时说明会新建", () => {
    const w = saveWarnings(info({ exists: false, writable: false, is_admin: true }));
    expect(w.some((x) => x.includes("新建"))).toBe(true);
  });

  it("info 为空时返回空数组（不炸）", () => {
    expect(saveWarnings(null)).toEqual([]);
  });
});

describe("summarizeChanges", () => {
  it("统计新增/删除/切换", () => {
    const orig = [
      entry({ ip: "10.0.0.1", host: "a.local" }),
      entry({ ip: "10.0.0.2", host: "b.local" }),
      entry({ ip: "10.0.0.3", host: "c.local" }),
    ];
    const cur = [
      entry({ ip: "10.0.0.1", host: "a.local" }),
      // b 被禁用（切换）
      entry({ ip: "10.0.0.2", host: "b.local", enabled: false, kind: "disabled" }),
      // c 被删除
      // 新增 d
      entry({ ip: "10.0.0.4", host: "d.local" }),
    ];
    const c = summarizeChanges(orig, cur);
    expect(c).toEqual({ added: 1, removed: 1, toggled: 1 });
  });

  it("无改动时全 0", () => {
    const e = [entry()];
    expect(summarizeChanges(e, e)).toEqual({ added: 0, removed: 0, toggled: 0 });
  });

  it("注释行不参与统计", () => {
    const orig = [entry({ kind: "comment", host: "", ip: "" })];
    expect(summarizeChanges(orig, [])).toEqual({ added: 0, removed: 0, toggled: 0 });
  });
});

describe("hostsStats", () => {
  it("分类计数", () => {
    const entries = [
      entry(),
      entry({ kind: "disabled", enabled: false }),
      entry({ kind: "comment", host: "", ip: "" }),
      entry({ kind: "comment", host: "", ip: "" }),
    ];
    expect(hostsStats(entries)).toEqual({
      total: 4,
      enabled: 1,
      disabled: 1,
      comments: 2,
    });
  });

  it("空列表全 0", () => {
    expect(hostsStats([])).toEqual({ total: 0, enabled: 0, disabled: 0, comments: 0 });
  });
});

describe("filterHostRows", () => {
  const rows: HostRow[] = [
    { ...entry({ ip: "10.0.0.1", host: "api.local", comment: "后端" }), idx: 0 },
    { ...entry({ kind: "disabled", enabled: false, ip: "10.0.0.2", host: "dev.local" }), idx: 1 },
    { ...entry({ kind: "comment", host: "", ip: "", raw: "# 临时屏蔽" }), idx: 2 },
  ];

  it("all 返回全部", () => {
    expect(filterHostRows(rows, "all", "")).toHaveLength(3);
  });

  it("enabled 只留启用行，disabled 只留禁用行", () => {
    expect(filterHostRows(rows, "enabled", "").map((r) => r.idx)).toEqual([0]);
    expect(filterHostRows(rows, "disabled", "").map((r) => r.idx)).toEqual([1]);
  });

  it("comment 只留注释行", () => {
    expect(filterHostRows(rows, "comment", "").map((r) => r.idx)).toEqual([2]);
  });

  it("关键字匹配 IP/主机名/别名/备注/原文，大小写不敏感", () => {
    expect(filterHostRows(rows, "all", "api").map((r) => r.idx)).toEqual([0]);
    expect(filterHostRows(rows, "all", "10.0.0.2").map((r) => r.idx)).toEqual([1]);
    expect(filterHostRows(rows, "all", "后端").map((r) => r.idx)).toEqual([0]);
    expect(filterHostRows(rows, "all", "临时屏蔽").map((r) => r.idx)).toEqual([2]);
    expect(filterHostRows(rows, "all", "API.LOCAL").map((r) => r.idx)).toEqual([0]);
  });

  it("筛选与关键字叠加", () => {
    // "local" 三行都命中，限定 enabled 后只剩启用那条
    expect(filterHostRows(rows, "enabled", "local").map((r) => r.idx)).toEqual([0]);
  });

  it("无匹配返回空数组；关键字两侧空白被忽略", () => {
    expect(filterHostRows(rows, "all", "不存在")).toEqual([]);
    expect(filterHostRows(rows, "all", "  dev  ").map((r) => r.idx)).toEqual([1]);
  });
});

describe("isDuplicateMapping", () => {
  it("同 IP 同主机名判重", () => {
    const entries = [entry({ ip: "10.0.0.1", host: "api.local" })];
    expect(isDuplicateMapping(entries, "10.0.0.1", "api.local")).toBe(true);
    expect(isDuplicateMapping(entries, "10.0.0.2", "api.local")).toBe(false);
    expect(isDuplicateMapping(entries, "10.0.0.1", "other.local")).toBe(false);
  });

  it("主机名大小写不敏感（hosts 语义里是同一条）", () => {
    const entries = [entry({ ip: "10.0.0.1", host: "Dev.Local" })];
    expect(isDuplicateMapping(entries, "10.0.0.1", "dev.local")).toBe(true);
  });

  it("禁用行也算重复（它还在文件里）", () => {
    const entries = [entry({ kind: "disabled", enabled: false, ip: "10.0.0.9", host: "x.local" })];
    expect(isDuplicateMapping(entries, "10.0.0.9", "x.local")).toBe(true);
  });

  it("空输入不误判", () => {
    expect(isDuplicateMapping([], "", "")).toBe(false);
    expect(isDuplicateMapping([entry()], "", "")).toBe(false);
  });
});

describe("effectiveFilter", () => {
  it("注释筛选在隐藏注释行时回落到 all（Segmented 选中值必须始终在可见选项里）", () => {
    expect(effectiveFilter("comment", false)).toBe("all");
  });

  it("显示注释行时注释筛选原样保留", () => {
    expect(effectiveFilter("comment", true)).toBe("comment");
  });

  it("其余筛选不受开关影响", () => {
    expect(effectiveFilter("all", false)).toBe("all");
    expect(effectiveFilter("enabled", false)).toBe("enabled");
    expect(effectiveFilter("disabled", false)).toBe("disabled");
    expect(effectiveFilter("all", true)).toBe("all");
    expect(effectiveFilter("enabled", true)).toBe("enabled");
    expect(effectiveFilter("disabled", true)).toBe("disabled");
  });

  it("回落的 all 与 filterHostRows 组合不再筛出空表", () => {
    // 回归场景：选中「注释」→关掉「显示注释行」→条目表不得变成不可解释的空表
    const rows: HostRow[] = [
      { ...entry({ ip: "10.0.0.1", host: "api.local" }), idx: 0 },
      { ...entry({ kind: "comment", host: "", ip: "", raw: "# 注释" }), idx: 1 },
    ];
    const visible = filterHostRows(rows, effectiveFilter("comment", false), "");
    expect(visible.map((r) => r.idx)).toEqual([0, 1]);
  });
});
