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
  actionableFixes,
  confirmText,
  formatCheckForCopy,
  groupByRisk,
  needsConfirm,
  outcomeLevel,
  outcomeTag,
  riskLabel,
  riskToken,
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
  type RepairAction,
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
