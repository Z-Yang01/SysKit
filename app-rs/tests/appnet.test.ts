/**
 * 应用联网页纯逻辑测试。
 *
 * 最要紧的一组：**`drift` 绝不能被显示成"已禁止联网"**。用户把规则动作在 wf.msc 里
 * 改成"允许"后，Enabled 仍是 True；只看台账或只看 Enabled 都会把已经放行的程序
 * 报成"已拦住"——对安全控制而言这是实质性错报。
 */
import { describe, expect, it } from "vitest";

import {
  adminNotice,
  appNetStats,
  attentionNotice,
  baseName,
  blockConfirmText,
  dirName,
  isReallyBlocking,
  needsAttention,
  stateColor,
  stateLabel,
  type AppNetList,
  type AppNetRow,
  type RuleState,
} from "../src/appnet";

function row(over: Partial<AppNetRow> = {}): AppNetRow {
  return {
    program: "C:\\Program Files\\A\\a.exe",
    rule: "SysKit-AppNet-5bd6cecf63",
    blocked: true,
    added: "2026-01-01 10:00:00",
    state: "on",
    blocking: true,
    ...over,
  };
}

describe("三态文案", () => {
  it("on/off/drift/missing 四种都有明确文案", () => {
    expect(stateLabel("on")).toBe("已禁止联网");
    expect(stateLabel("off")).toContain("放行");
    expect(stateLabel("drift")).toContain("已失效");
    expect(stateLabel("missing")).toContain("不存在");
  });

  it("drift 的文案必须点明「并没有在拦」的意思，不能与 on 混淆", () => {
    const d = stateLabel("drift");
    expect(d).not.toBe(stateLabel("on"));
    expect(d).toMatch(/允许|失效/);
  });

  it("drift 用红色（需要用户注意）", () => {
    expect(stateColor("drift")).toBe("red");
    expect(stateColor("on")).toBe("green");
    expect(stateColor("missing")).toBe("orange");
    expect(stateColor("off")).toBe("default");
  });
});

describe("是否真的在阻断", () => {
  it("只有 on 算真的在拦", () => {
    expect(isReallyBlocking(row({ state: "on" }))).toBe(true);
    for (const s of ["off", "drift", "missing"] as RuleState[]) {
      expect(isReallyBlocking(row({ state: s }))).toBe(false);
    }
  });

  it("台账 blocked=true 但状态 drift 时不得算在拦", () => {
    // 这是核心用例：台账的"愿望值"不能覆盖实际状态
    const r = row({ blocked: true, state: "drift" });
    expect(isReallyBlocking(r)).toBe(false);
    expect(needsAttention(r)).toBe(true);
  });

  it("台账 blocked=false 但状态 on（外部启用了规则）时，实际在拦", () => {
    // 反向也不自欺：以系统实际状态为准
    expect(isReallyBlocking(row({ blocked: false, state: "on" }))).toBe(true);
  });
});

describe("needsAttention", () => {
  it("drift 与 missing 需要提醒", () => {
    expect(needsAttention(row({ state: "drift" }))).toBe(true);
    expect(needsAttention(row({ state: "missing" }))).toBe(true);
  });

  it("on / off 不需要提醒", () => {
    expect(needsAttention(row({ state: "on" }))).toBe(false);
    expect(needsAttention(row({ state: "off" }))).toBe(false);
  });
});

describe("attentionNotice", () => {
  it("分别说明 drift 与 missing，且 drift 强调「并没有在阻断」", () => {
    const rows = [
      row({ state: "drift" }),
      row({ state: "missing", rule: "r2" }),
      row({ state: "on", rule: "r3" }),
    ];
    const n = attentionNotice(rows);
    expect(n).toContain("1 条规则的动作被外部改成了「允许」");
    expect(n).toContain("并没有");
    expect(n).toContain("1 条规则已不在系统里");
  });

  it("只有 drift 时也要给完整说明（不能只报数量）", () => {
    const n = attentionNotice([row({ state: "drift" })]);
    expect(n).toContain("并没有");
    expect(n).toContain("重新应用或删除");
    expect(n).not.toContain("台账残留");
  });

  it("无异常时返回空串（界面不显示空警告框）", () => {
    expect(attentionNotice([row({ state: "on" }), row({ state: "off", rule: "r2" })])).toBe("");
    expect(attentionNotice([])).toBe("");
  });
});

describe("appNetStats", () => {
  it("按实际状态分类统计", () => {
    const rows = [
      row({ state: "on" }),
      row({ state: "on", rule: "r2" }),
      row({ state: "off", rule: "r3" }),
      row({ state: "drift", rule: "r4" }),
      row({ state: "missing", rule: "r5" }),
    ];
    expect(appNetStats(rows)).toEqual({
      total: 5,
      blocking: 2,
      released: 1,
      attention: 2,
    });
  });

  it("空表全 0", () => {
    expect(appNetStats([])).toEqual({ total: 0, blocking: 0, released: 0, attention: 0 });
  });
});

describe("adminNotice", () => {
  const fallback = "修改防火墙规则需要管理员权限。";
  const list = (isAdmin: boolean): AppNetList => ({ rows: [], count: 0, is_admin: isAdmin });

  it("非管理员时给出提示", () => {
    expect(adminNotice(list(false), fallback)).toBe(fallback);
  });

  it("管理员时为空（不打扰）", () => {
    expect(adminNotice(list(true), fallback)).toBe("");
  });

  it("列表未加载时为空", () => {
    expect(adminNotice(null, fallback)).toBe("");
  });
});

describe("路径展示", () => {
  it("baseName 取文件名", () => {
    expect(baseName("C:\\Program Files\\A\\a.exe")).toBe("a.exe");
    expect(baseName("C:/x/y/b.exe")).toBe("b.exe");
    expect(baseName("a.exe")).toBe("a.exe");
  });

  it("dirName 取目录部分", () => {
    expect(dirName("C:\\Program Files\\A\\a.exe")).toBe("C:\\Program Files\\A");
    expect(dirName("C:/x/y/b.exe")).toBe("C:/x/y");
    expect(dirName("a.exe")).toBe("");
  });
});

describe("blockConfirmText", () => {
  it("点明文件名并说明影响范围与可恢复性", () => {
    const t = blockConfirmText("C:\\Program Files\\A\\a.exe");
    expect(t).toContain("a.exe");
    expect(t).toContain("出站阻断");
    expect(t).toContain("不可用");
    expect(t).toContain("恢复");
    // 让用户明白不只是"限速"
    expect(t).toContain("更新");
  });
});
