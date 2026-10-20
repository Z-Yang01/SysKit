/**
 * 修复动作库展示逻辑测试。
 *
 * 重点：风险分级标签/配色、非管理员跳过标注、确认框触发条件、组合计划摘要、
 * 结果 Tag 收口、复制详情文本、体检建议的动作入口过滤。
 * 这些都是「用户点之前就该看清楚」的信息，错了会让用户误执行高危动作。
 */
import { describe, expect, it } from "vitest";

import {
  RISK_ORDER,
  REPORT_STORAGE_KEY,
  actionableFixes,
  clipText,
  confirmText,
  formatCheckForCopy,
  formatMonitorEvent,
  formatReportForCopy,
  groupByRisk,
  hhmmss,
  loadReportFrom,
  maskSensitive,
  needsConfirm,
  outcomeLevel,
  outcomeTag,
  isReportStale,
  loadRecordsFrom,
  pushRecord,
  RECORDS_STORAGE_KEY,
  saveRecordsTo,
  riskLabel,
  riskToken,
  saveReportTo,
  skipReason,
  sortBySeverity,
  statusLabel,
  statusMark,
  statusTagColor,
  statusToken,
  summarizeDiag,
  summarizePlan,
  verdictColor,
  willBeSkipped,
  type CheckResult,
  type ComboStep,
  type DiagReport,
  type MonitorEvent,
  type RepairAction,
  type RepairRecord,
} from "../src/repair";

function action(over: Partial<RepairAction> = {}): RepairAction {
  return {
    id: "flush_dns",
    title: "刷新 DNS 解析缓存",
    risk: "safe",
    risk_label: "安全",
    description: "清空系统 DNS 缓存。无副作用，可反复执行。",
    restore_hint: "无需还原（缓存会自动重建）",
    brief: "清空系统 DNS 缓存。",
    need_reboot: false,
    params: [],
    ...over,
  };
}

describe("风险分级", () => {
  it("顺序是 安全→改配置→强力（严重度递增）", () => {
    expect(RISK_ORDER).toEqual(["safe", "config", "aggressive"]);
  });

  it("标签与 Python RISK_LABEL 逐字一致", () => {
    expect(riskLabel("safe")).toBe("安全");
    expect(riskLabel("config")).toBe("改配置");
    expect(riskLabel("aggressive")).toBe("强力(需重启)");
  });

  it("配色 token 复用主题语义色", () => {
    expect(riskToken("safe")).toBe("ok");
    expect(riskToken("config")).toBe("warn");
    expect(riskToken("aggressive")).toBe("err");
  });

  it("只有安全类免确认", () => {
    expect(needsConfirm("safe")).toBe(false);
    expect(needsConfirm("config")).toBe(true);
    expect(needsConfirm("aggressive")).toBe(true);
  });
});

describe("按风险分组", () => {
  it("组内保持后端顺序，空组被剔除", () => {
    const list = [
      action({ id: "a", risk: "safe" }),
      action({ id: "b", risk: "config" }),
      action({ id: "c", risk: "safe" }),
    ];
    const g = groupByRisk(list);
    expect(g.map((x) => x.risk)).toEqual(["safe", "config"]);
    expect(g[0].items.map((i) => i.id)).toEqual(["a", "c"]);
    expect(g[1].items.map((i) => i.id)).toEqual(["b"]);
  });

  it("空清单返回空分组（不炸）", () => {
    expect(groupByRisk([])).toEqual([]);
  });
});

describe("非管理员门禁", () => {
  it("管理员下没有任何动作被跳过", () => {
    for (const r of RISK_ORDER) {
      expect(willBeSkipped(action({ risk: r }), true)).toBe(false);
      expect(skipReason(action({ risk: r }), true)).toBeNull();
    }
  });

  it("非管理员下只有安全类可执行", () => {
    expect(willBeSkipped(action({ risk: "safe" }), false)).toBe(false);
    expect(willBeSkipped(action({ risk: "config" }), false)).toBe(true);
    expect(willBeSkipped(action({ risk: "aggressive" }), false)).toBe(true);
  });

  it("跳过原因要说明是权限问题并能区分动作类别", () => {
    expect(skipReason(action({ risk: "config" }), false)).toContain("改配置类");
    expect(skipReason(action({ risk: "aggressive" }), false)).toContain("强力类");
    expect(skipReason(action({ risk: "config" }), false)).toContain("管理员");
  });
});

describe("确认文案", () => {
  it("包含描述与还原方式", () => {
    const t = confirmText(action({ risk: "config", restore_hint: "运行 dns_auto 恢复" }));
    expect(t).toContain("清空系统 DNS 缓存");
    expect(t).toContain("还原方式：运行 dns_auto 恢复");
  });

  it("还原方式为「无」时不写还原行", () => {
    const t = confirmText(action({ restore_hint: "无" }));
    expect(t).not.toContain("还原方式");
  });

  it("需重启的动作必须显著提示", () => {
    const t = confirmText(action({ need_reboot: true, risk: "aggressive" }));
    expect(t).toContain("需要重启");
    expect(t).toContain("⚠");
  });
});

describe("结果级别", () => {
  it("成功 → success", () => {
    expect(outcomeLevel({ success: true, message: "完成", need_reboot: false, output: "" })).toBe(
      "success",
    );
  });

  it("「尚未接入」归为 warning 而不是 error（不是用户的操作失败）", () => {
    expect(
      outcomeLevel({
        success: false,
        message: "restart_wlan 尚未在 Rust 版接入（需管理员且会断网数秒）",
        need_reboot: false,
        output: "",
      }),
    ).toBe("warning");
  });

  it("真实失败 → error", () => {
    expect(
      outcomeLevel({ success: false, message: "sc config 失败", need_reboot: false, output: "" }),
    ).toBe("error");
  });
});

describe("结果 Tag 收口（outcomeTag）", () => {
  it("三档结果对应 绿已成功 / 橙未接入 / 红失败", () => {
    expect(outcomeTag({ success: true, message: "完成", need_reboot: false, output: "" })).toEqual({
      color: "green",
      text: "已成功",
    });
    expect(
      outcomeTag({
        success: false,
        message: "restart_wlan 尚未在 Rust 版接入",
        need_reboot: false,
        output: "",
      }),
    ).toEqual({ color: "orange", text: "未接入" });
    expect(
      outcomeTag({ success: false, message: "sc config 失败", need_reboot: false, output: "" }),
    ).toEqual({ color: "red", text: "失败" });
  });
});

describe("组合计划摘要", () => {
  const step = (id: string, risk: ComboStep["risk"], skipped: string | null = null): ComboStep => ({
    id,
    title: id,
    risk,
    skipped_reason: skipped,
  });

  it("统计执行/跳过，并标记是否含需重启动作", () => {
    const s = summarizePlan([
      step("restore_point", "safe"),
      step("winsock_reset", "aggressive"),
      step("dns_auto", "config", "非管理员：改配置类动作已跳过"),
    ]);
    expect(s.run).toBe(2);
    expect(s.skipped).toBe(1);
    expect(s.needsReboot).toBe(true);
  });

  it("无强力动作时不标需重启", () => {
    expect(summarizePlan([step("flush_dns", "safe")]).needsReboot).toBe(false);
  });

  it("空计划全 0", () => {
    expect(summarizePlan([])).toEqual({ run: 0, skipped: 0, needsReboot: false });
  });
});

// ---------------- 诊断结果展示 ----------------

function check(over: Partial<CheckResult> = {}): CheckResult {
  return {
    id: "real_conn",
    name: "真实连通性",
    status: "pass",
    detail: "204探测:OK",
    fix_ids: [],
    ...over,
  };
}

function report(over: Partial<DiagReport> = {}): DiagReport {
  return {
    results: [check()],
    verdict: "normal",
    verdict_text: "网络正常",
    counts: { pass: 1 },
    quick: false,
    ...over,
  };
}

describe("诊断状态展示", () => {
  it("状态符号与 Python STATUS_MARK 逐字一致", () => {
    expect(statusMark("pass")).toBe("√");
    expect(statusMark("warn")).toBe("!");
    expect(statusMark("fail")).toBe("×");
    expect(statusMark("skip")).toBe("-");
    expect(statusMark("info")).toBe("·");
  });

  it("状态标签与配色", () => {
    expect(statusLabel("pass")).toBe("通过");
    expect(statusLabel("skip")).toBe("跳过");
    expect(statusToken("fail")).toBe("err");
    expect(statusTagColor("pass")).toBe("green");
    expect(statusTagColor("skip")).toBe("default");
  });

  it("综合判定配色区分三档", () => {
    expect(verdictColor("normal")).toBe("green");
    expect(verdictColor("ncsi_false_alarm")).toBe("orange");
    expect(verdictColor("dns_broken")).toBe("orange");
    expect(verdictColor("no_internet")).toBe("red");
    expect(verdictColor("其他")).toBe("default");
  });
});

describe("诊断摘要", () => {
  it("固定顺序输出五档，缺失记 0（0 也要显示）", () => {
    const s = summarizeDiag(report({ counts: { pass: 3, fail: 1 } }));
    expect(s.map((x) => x.status)).toEqual(["pass", "warn", "fail", "skip", "info"]);
    expect(s.find((x) => x.status === "pass")?.count).toBe(3);
    expect(s.find((x) => x.status === "fail")?.count).toBe(1);
    expect(s.find((x) => x.status === "skip")?.count).toBe(0);
  });

  it("空报告全 0 且不炸", () => {
    const s = summarizeDiag(null);
    expect(s).toHaveLength(5);
    expect(s.every((x) => x.count === 0)).toBe(true);
  });
});

describe("按严重度排序", () => {
  it("失败 → 警告 → 提示 → 跳过 → 通过", () => {
    const sorted = sortBySeverity([
      check({ id: "a", status: "pass" }),
      check({ id: "b", status: "skip" }),
      check({ id: "c", status: "fail" }),
      check({ id: "d", status: "info" }),
      check({ id: "e", status: "warn" }),
    ]);
    expect(sorted.map((r) => r.status)).toEqual(["fail", "warn", "info", "skip", "pass"]);
  });

  it("是纯函数：不改原数组", () => {
    const orig = [check({ id: "a", status: "pass" }), check({ id: "b", status: "fail" })];
    sortBySeverity(orig);
    expect(orig.map((r) => r.id)).toEqual(["a", "b"]);
  });
});

describe("可执行修复建议", () => {
  it("汇总去重后返回", () => {
    const r = report({
      results: [
        check({ id: "a", status: "fail", fix_ids: ["flush_dns", "dns_auto"] }),
        check({ id: "b", status: "warn", fix_ids: ["flush_dns"] }),
        check({ id: "c", status: "pass", fix_ids: [] }),
      ],
    });
    const fixes = actionableFixes(r);
    expect(fixes).toHaveLength(2);
    expect(fixes).toContain("flush_dns");
    expect(fixes).toContain("dns_auto");
  });

  it("传已知动作时过滤掉没入口的 id（体检可能指向未移植动作）", () => {
    const r = report({
      results: [check({ status: "fail", fix_ids: ["flush_dns", "restart_wlan"] })],
    });
    expect(actionableFixes(r, ["flush_dns", "dns_auto"])).toEqual(["flush_dns"]);
  });

  it("不传已知动作时保持原行为（全量去重）", () => {
    const r = report({
      results: [check({ status: "fail", fix_ids: ["flush_dns", "restart_wlan"] })],
    });
    expect(actionableFixes(r)).toHaveLength(2);
  });

  it("空报告返回空数组", () => {
    expect(actionableFixes(null)).toEqual([]);
    expect(actionableFixes(report({ results: [] }))).toEqual([]);
  });
});

describe("复制详情文本（formatCheckForCopy）", () => {
  it("带状态标签与详情，自包含成一段可贴出去的信息", () => {
    const t = formatCheckForCopy(
      check({ status: "fail", name: "真实连通性", detail: "204探测:全部超时", fix_ids: [] }),
    );
    expect(t).toBe("【失败】真实连通性：204探测:全部超时");
  });

  it("有建议动作时追加一行", () => {
    const t = formatCheckForCopy(
      check({ status: "warn", detail: "静态 DNS 与 DHCP 下发不一致", fix_ids: ["dns_auto", "flush_dns"] }),
    );
    expect(t).toBe("【警告】真实连通性：静态 DNS 与 DHCP 下发不一致\n建议动作：dns_auto、flush_dns");
  });
});

// ---------------- 报告全文复制（对齐 Python _report_lines） ----------------

describe("报告全文复制（formatReportForCopy）", () => {
  it("格式与 Python 逐行同构：报告头 / [符号] 名称 / 缩进详情 / 结论", () => {
    const r = report({
      results: [
        check({ status: "pass", name: "真实连通性", detail: "204探测:OK" }),
        check({ status: "fail", name: "持久路由", detail: "存在持久默认路由 → 172.16.2.1" }),
      ],
      verdict_text: "外网不可达；失败项: 1 个",
    });
    // full=true：格式同构锁用完整信息；脱敏行为由下面的独立断言覆盖
    const text = formatReportForCopy(r, new Date(2026, 9, 3, 14, 5), true);
    const lines = text.split("\n");
    expect(lines[0]).toBe("SysKit 诊断报告  2026-10-03 14:05");
    expect(lines[1]).toBe("-".repeat(50));
    expect(lines[2]).toBe("[√] 真实连通性");
    expect(lines[3]).toBe("      204探测:OK");
    expect(lines[4]).toBe("[×] 持久路由");
    expect(lines[5]).toBe("      存在持久默认路由 → 172.16.2.1");
    expect(lines[6]).toBe("-".repeat(50));
    expect(lines[7]).toBe("结论：外网不可达；失败项: 1 个");

    // 默认（full=false）：内网 IP 保留前两段
    const masked = formatReportForCopy(r, new Date(2026, 9, 3, 14, 5));
    expect(masked.split("\n")[5]).toBe("      存在持久默认路由 → 172.16.*.*");
  });

  it("时间戳两位数补零（凌晨/早盘不出现 9:5 这种歧义时间）", () => {
    const r = report();
    const text = formatReportForCopy(r, new Date(2026, 0, 2, 3, 4));
    expect(text).toContain("2026-01-02 03:04");
  });

  it("空报告也能出完整骨架（结论行来自 verdict_text）", () => {
    const text = formatReportForCopy(report({ results: [] }), new Date(2026, 0, 1));
    expect(text).toContain("结论：网络正常");
  });
});

// ---------------- 修复记录 / 后台监控事件 ----------------

describe("监控事件格式化（formatMonitorEvent）", () => {
  const ev = (over: Partial<MonitorEvent>): MonitorEvent => ({
    kind: "action",
    id: "flush_dns",
    title: "刷新 DNS 解析缓存",
    ok: true,
    message: "已刷新",
    ...over,
  });

  it("成功/失败动作分别打 √/×，正文为「标题 — 摘要」", () => {
    expect(formatMonitorEvent(ev({ ok: true }))).toEqual({
      mark: "√",
      text: "刷新 DNS 解析缓存 — 已刷新",
    });
    expect(formatMonitorEvent(ev({ ok: false, message: "需要管理员权限" }))).toEqual({
      mark: "×",
      text: "刷新 DNS 解析缓存 — 需要管理员权限",
    });
  });

  it("建议事件打 ! 并注明「建议手动修复」（绝不与已执行动作混淆）", () => {
    const out = formatMonitorEvent(
      ev({ kind: "suggest", id: "dns_auto", title: "DNS 恢复为 DHCP 自动获取", ok: false, message: "多次自动修复无效" }),
    );
    expect(out.mark).toBe("!");
    expect(out.text).toContain("（建议手动修复）");
    expect(out.text).toContain("多次自动修复无效");
  });

  it("恢复事件打 √，直接展示恢复详情（后台自动修复成功的可见反馈）", () => {
    const out = formatMonitorEvent(
      ev({ kind: "recovered", id: "", title: "", ok: true, message: "自动修复成功（重启 WLAN 服务并重连 WiFi）" }),
    );
    expect(out.mark).toBe("√");
    expect(out.text).toContain("自动修复成功");
  });

  it("长摘要按字符截到 90（对齐 Python message[:90]，中文按 1 字符计）", () => {
    const long = "长".repeat(120);
    const out = formatMonitorEvent(ev({ message: long }));
    expect(out.text.endsWith("长")).toBe(true);
    expect([...out.text].length).toBeLessThanOrEqual("刷新 DNS 解析缓存 — ".length + 90);
  });
});

describe("记录列表（pushRecord / hhmmss）", () => {
  const rec = (t: string): RepairRecord => ({ time: t, mark: "√", text: t });

  it("新的在前（对齐 Python insertItem(0, …)）", () => {
    const out = pushRecord([rec("a")], rec("b"));
    expect(out.map((r) => r.time)).toEqual(["b", "a"]);
  });

  it("默认上限 60 条（挂机几天也不撑爆列表）", () => {
    let list: RepairRecord[] = [];
    for (let i = 0; i < 70; i++) {
      list = pushRecord(list, rec(`t${i}`));
    }
    expect(list).toHaveLength(60);
    expect(list[0].time).toBe("t69");
  });

  it("自定义上限生效（截断发生在最新在前之后）", () => {
    // ["c","a","b"] 截到 2 → ["c","a"]：b 是最旧的，先被淘汰
    expect(pushRecord([rec("a"), rec("b")], rec("c"), 2).map((r) => r.time)).toEqual(["c", "a"]);
  });

  it("hhmmss 两位数补零", () => {
    expect(hhmmss(new Date(2026, 0, 1, 3, 4, 5))).toBe("03:04:05");
    expect(hhmmss(new Date(2026, 0, 1, 23, 59, 59))).toBe("23:59:59");
  });
});

describe("clipText", () => {
  it("按字符截断（Python [:n] 语义）", () => {
    expect(clipText("你好世界", 2)).toBe("你好");
    expect(clipText("abc", 10)).toBe("abc");
    expect(clipText("", 5)).toBe("");
  });
});

// ---------------- 「上次结果」的会话保存 ----------------

describe("诊断报告会话保存（saveReportTo / loadReportFrom）", () => {
  it("写入后能原样读回（切页/重挂载后恢复上次结论）", () => {
    const store = new Map<string, string>();
    const r = report({ verdict: "no_internet", verdict_text: "外网不可达" });
    saveReportTo(r, (k, v) => void store.set(k, v));
    expect(store.has(REPORT_STORAGE_KEY)).toBe(true);
    const back = loadReportFrom((k) => store.get(k) ?? null);
    expect(back?.verdict).toBe("no_internet");
    expect(back?.results).toHaveLength(1);
  });

  it("没存过返回 null", () => {
    expect(loadReportFrom(() => null)).toBeNull();
  });

  it("损坏的 JSON / 缺字段一律返回 null（绝不把脏数据当报告）", () => {
    expect(loadReportFrom(() => "not-json{")).toBeNull();
    expect(loadReportFrom(() => JSON.stringify({ verdict: "x" }))).toBeNull();
    expect(loadReportFrom(() => JSON.stringify({ results: "nope", verdict: "x" }))).toBeNull();
  });

  it("writer 抛错被吞掉（存储不可用不拖垮诊断流程）", () => {
    expect(() =>
      saveReportTo(report(), () => {
        throw new Error("quota");
      }),
    ).not.toThrow();
  });
});

describe("maskSensitive（诊断报告脱敏 v1.3）", () => {
  it("内网 IPv4 保留前两段，公网全打", () => {
    expect(maskSensitive("网关 192.168.1.100 不可达")).toBe("网关 192.168.*.* 不可达");
    expect(maskSensitive("DNS 10.0.0.2 超时")).toBe("DNS 10.0.*.* 超时");
    expect(maskSensitive("公网 223.5.5.5 解析正常")).toBe("公网 *.*.*.* 解析正常");
    expect(maskSensitive("链路本地 169.254.3.4")).toBe("链路本地 169.254.*.*");
    expect(maskSensitive("回环 127.0.0.1")).toBe("回环 127.0.*.*");
  });

  it("IPv6 保留前两组其余打码", () => {
    const out = maskSensitive("地址 2408:8207:1851:01a2:b1c2:d3e4:f506:a7b8 可用");
    expect(out).toContain("2408:8207:****");
    expect(out).not.toContain("b1c2");
  });

  it("MAC 保留 OUI 前缀（冒号与连字符两种分隔）", () => {
    expect(maskSensitive("MAC AA-BB-CC-DD-EE-FF 异常")).toBe("MAC AA-BB-CC-**-**-** 异常");
    const out = maskSensitive("mac 11:22:33:44:55:66");
    expect(out).toContain("11:22:33");
    expect(out).not.toContain("44:55:66");
  });

  it("代理 URL 凭据打码", () => {
    expect(maskSensitive("代理 http://user:secret@proxy.corp.com:8080 已设")).toBe(
      "代理 http://***:***@proxy.corp.com:8080 已设",
    );
  });

  it("SSID 打码（已连接/SSID 两种形态）", () => {
    expect(maskSensitive("已连接: ziroom402_5G（信号 90%）")).toBe(
      "已连接: «SSID»（信号 90%）",
    );
    expect(maskSensitive("SSID: home-wifi; 信号弱")).toBe("SSID: «SSID»; 信号弱");
  });

  it("无敏感内容的文本原样返回", () => {
    expect(maskSensitive("NlaSvc 服务正在运行，启动类型: 自动")).toBe(
      "NlaSvc 服务正在运行，启动类型: 自动",
    );
  });

  it("port 数字不被误当 IPv4 打码", () => {
    expect(maskSensitive("端口 8080 与版本 1.2.3.4 混排")).toBe("端口 8080 与版本 *.*.*.* 混排");
  });
});

/**
 * 修复记录的会话存储：记录此前只存组件 state，刷新即丢——而排查恰恰发生在
 * 「刚才那下坏了」之后。与诊断报告同一策略（sessionStorage，关窗即清）。
 */
describe("修复记录会话存储", () => {
  const rec = (over: Partial<RepairRecord> = {}): RepairRecord => ({
    time: "10:00:00",
    mark: "√",
    text: "刷新 DNS 缓存 — 完成",
    ...over,
  });

  it("写入后可原样恢复", () => {
    const store = new Map<string, string>();
    const list = [rec(), rec({ mark: "×", text: "改静态 DNS — 拒绝访问" })];
    saveRecordsTo(list, (k, v) => store.set(k, v));
    expect(loadRecordsFrom((k) => store.get(k) ?? null)).toEqual(list);
  });

  it("空存储返回空数组而不是报错", () => {
    expect(loadRecordsFrom(() => null)).toEqual([]);
  });

  it("脏数据一律回空：数组外结构、字段缺失都不当记录用", () => {
    const store = new Map<string, string>([
      ["k", JSON.stringify({ time: "10:00:00" })], // 对象不是数组
      ["k2", JSON.stringify([{ time: "10:00:00" }])], // 缺 mark/text
      ["k3", "{broken"],
    ]);
    const get = (k: string) => store.get(k) ?? null;
    expect(loadRecordsFrom(get)).toEqual([]);
    expect(loadRecordsFrom((k) => (k === "k2" ? store.get(k)! : null))).toEqual([]);
    expect(loadRecordsFrom((k) => (k === "k3" ? store.get(k)! : null))).toEqual([]);
  });

  it("部分合法部分脏：合法的留下，脏的剔除", () => {
    const store = new Map<string, string>();
    // 必须存在真实键下：load 按 RECORDS_STORAGE_KEY 查，存错键等于没存
    store.set(
      RECORDS_STORAGE_KEY,
      JSON.stringify([rec(), { nope: 1 }, rec({ mark: "×" })]),
    );
    const got = loadRecordsFrom((k) => store.get(k) ?? null);
    expect(got).toHaveLength(2);
    expect(got[1].mark).toBe("×");
  });
});

/** 报告时效：旧结论会误导下一步判断，但没有证据不能乱报警。 */
describe("isReportStale", () => {
  const now = 1_700_000_000_000;

  it("缺时间戳（旧会话遗留）不报警", () => {
    expect(isReportStale(undefined, now)).toBe(false);
    expect(isReportStale(NaN, now)).toBe(false);
  });

  it("30 分钟内新鲜，超过判定过期", () => {
    expect(isReportStale(now - 29 * 60_000, now)).toBe(false);
    expect(isReportStale(now - 30 * 60_000, now)).toBe(false);
    expect(isReportStale(now - 31 * 60_000, now)).toBe(true);
  });

  it("时间戳在将来（时钟回拨）不算过期", () => {
    expect(isReportStale(now + 60_000, now)).toBe(false);
  });
});
