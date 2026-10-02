/**
 * 命令面板纯逻辑测试。
 *
 * 重点是**输入分类与提示时机**：用户搜东西时不该被"表达式不合法"打扰，
 * 而输入算式时又该立刻看到结果。
 */
import { describe, expect, it } from "vitest";

import {
  calcLevel,
  calcText,
  classifyInput,
  defaultIndex,
  digitIndex,
  moveIndex,
  parseCustomCommands,
  resultSummary,
  serializeCustomCommands,
  sourceColor,
  type CalcResult,
  type PaletteActions,
} from "../src/palette";

const calc = (over: Partial<CalcResult> = {}): CalcResult => ({
  is_math: true,
  ok: true,
  display: "2",
  error: "",
  ...over,
});

const actions = (over: Partial<PaletteActions> = {}): PaletteActions => ({
  rows: [],
  count: 0,
  total: 10,
  custom_count: 0,
  ...over,
});

describe("classifyInput", () => {
  it("空输入 → empty", () => {
    expect(classifyInput("", null, 5)).toBe("empty");
    expect(classifyInput("   ", calc(), 5)).toBe("empty");
  });

  it("算式优先于动作", () => {
    // 输入 1+1 就是想算数，而不是搜名为 "1+1" 的页面
    expect(classifyInput("1+1", calc(), 3)).toBe("calc");
  });

  it("非算式且无匹配 → empty", () => {
    expect(classifyInput("zzz", null, 0)).toBe("empty");
  });

  it("非算式有匹配 → actions", () => {
    expect(classifyInput("诊断", null, 2)).toBe("actions");
  });

  it("像算式但求值失败时仍走 calc（要显示错误）", () => {
    const bad = calc({ ok: false, display: "", error: "除数为 0" });
    expect(classifyInput("1/0", bad, 0)).toBe("calc");
  });
});

describe("calcLevel 提示时机", () => {
  it("不是算式时**不提示**（用户只是在搜东西，弹错误是噪声）", () => {
    expect(calcLevel(null)).toBe("none");
    expect(calcLevel({ is_math: false, ok: false, display: "", error: "" })).toBe("none");
  });

  it("是算式且成功 → success", () => {
    expect(calcLevel(calc())).toBe("success");
  });

  it("是算式但失败 → error", () => {
    expect(calcLevel(calc({ ok: false, error: "除数为 0" }))).toBe("error");
  });
});

describe("calcText", () => {
  it("成功时带等号（一眼看出是算出来的）", () => {
    expect(calcText(calc({ display: "42" }))).toBe("= 42");
  });

  it("失败时显示原因", () => {
    expect(calcText(calc({ ok: false, error: "除数为 0" }))).toBe("除数为 0");
  });

  it("不是算式时为空", () => {
    expect(calcText(null)).toBe("");
    expect(calcText({ is_math: false, ok: false, display: "", error: "" })).toBe("");
  });
});

describe("resultSummary", () => {
  it("空查询报总数", () => {
    expect(resultSummary(actions(), "")).toContain("10 个动作");
  });

  it("有查询报匹配数", () => {
    expect(resultSummary(actions({ count: 3 }), "诊断")).toContain("3 / 10");
  });

  it("无匹配时明确说明", () => {
    expect(resultSummary(actions({ count: 0 }), "zzz")).toContain("没有匹配");
  });

  it("未加载时为空", () => {
    expect(resultSummary(null, "x")).toBe("");
  });
});

describe("defaultIndex", () => {
  it("有结果时选第一项", () => {
    expect(defaultIndex(3)).toBe(0);
  });

  it("无结果时为 -1（不越界）", () => {
    expect(defaultIndex(0)).toBe(-1);
  });
});

describe("moveIndex 循环移动", () => {
  it("向下到底回到开头", () => {
    expect(moveIndex(2, 1, 3)).toBe(0);
  });

  it("向上到顶回到末尾", () => {
    expect(moveIndex(0, -1, 3)).toBe(2);
  });

  it("常规移动", () => {
    expect(moveIndex(0, 1, 3)).toBe(1);
    expect(moveIndex(1, 1, 3)).toBe(2);
  });

  it("空列表返回 -1（不越界）", () => {
    expect(moveIndex(0, 1, 0)).toBe(-1);
  });

  it("从 -1（未选中）向上时落到末尾", () => {
    expect(moveIndex(-1, -1, 3)).toBe(2);
    expect(moveIndex(-1, 1, 3)).toBe(0);
  });

  it("大跨度移动仍然落在范围内", () => {
    for (const delta of [-100, -7, 7, 100]) {
      const i = moveIndex(1, delta, 5);
      expect(i).toBeGreaterThanOrEqual(0);
      expect(i).toBeLessThan(5);
    }
  });
});

describe("digitIndex 数字键直选", () => {
  it("1-9 映射到 0-8", () => {
    expect(digitIndex(1, 5)).toBe(0);
    expect(digitIndex(9, 9)).toBe(8);
  });

  it("超出结果数返回 -1", () => {
    expect(digitIndex(5, 3)).toBe(-1);
  });

  it("0 与越界数字无效", () => {
    expect(digitIndex(0, 5)).toBe(-1);
    expect(digitIndex(10, 20)).toBe(-1);
  });
});

describe("sourceColor", () => {
  it("按来源区分", () => {
    expect(sourceColor("自定义")).toBe("purple");
    expect(sourceColor("内置")).toBe("blue");
    expect(sourceColor("剪贴板")).toBe("green");
    expect(sourceColor("密码库")).toBe("gold");
    expect(sourceColor("其他")).toBe("default");
  });
});

describe("自定义命令解析/序列化", () => {
  it("解析合法 JSON", () => {
    const c = parseCustomCommands('[{"name":"内网","type":"url","value":"https://x"}]');
    expect(c).toHaveLength(1);
    expect(c[0]).toEqual({ name: "内网", kind: "url", value: "https://x" });
  });

  it("坏输入返回空数组（不抛异常）", () => {
    expect(parseCustomCommands("")).toEqual([]);
    expect(parseCustomCommands("不是 json")).toEqual([]);
    expect(parseCustomCommands("{}")).toEqual([]);
    expect(parseCustomCommands("[1,2]")).toEqual([]);
    expect(parseCustomCommands('[{"name":"x"}]')).toEqual([]);
  });

  it("缺 type 默认 url；未知 type 收敛成 url（保守方向）", () => {
    expect(parseCustomCommands('[{"name":"a","value":"v"}]')[0].kind).toBe("url");
    expect(parseCustomCommands('[{"name":"a","value":"v","type":"weird"}]')[0].kind).toBe("url");
    expect(parseCustomCommands('[{"name":"a","value":"v","type":"cmd"}]')[0].kind).toBe("cmd");
  });

  it("序列化后可再解析（往返一致）", () => {
    const cmds = [
      { name: "内网", kind: "url" as const, value: "https://x" },
      { name: "清理", kind: "cmd" as const, value: "cleanmgr" },
    ];
    const json = serializeCustomCommands(cmds);
    expect(parseCustomCommands(json)).toEqual(cmds);
  });

  it("空列表序列化成空串（写回设置的值与后端口径一致）", () => {
    expect(serializeCustomCommands([])).toBe("");
    expect(parseCustomCommands("")).toEqual([]);
  });
});
