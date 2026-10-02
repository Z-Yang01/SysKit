/**
 * 剪贴板内容规则动作的前端逻辑测试。
 *
 * 识别本身在后端（含 CJK 标点剥离、盘型预检等细节，由 Rust 单测覆盖），
 * 这里锁住**呈现与派发**：路由错了会执行错误操作，标注错了会让用户不知道
 * 该动作会发网络请求。
 */
import { describe, expect, it } from "vitest";

import {
  actionColor,
  actionHint,
  actionsSummary,
  formatColor,
  isLightColor,
  normalizeHex,
  routeAction,
  shouldShowActions,
  summarizeResult,
  type ClipAction,
  type DetectResult,
} from "../src/cliprules";

const action = (over: Partial<ClipAction> = {}): ClipAction => ({
  id: "url_open",
  label: "打开链接",
  arg: "https://example.com",
  online: false,
  ...over,
});

const detect = (actions: ClipAction[]): DetectResult => ({ actions, count: actions.length });

describe("routeAction", () => {
  it("需要渲染/额外查询的交给前端", () => {
    expect(routeAction("url_qr")).toBe("frontend");
    expect(routeAction("color_show")).toBe("frontend");
    expect(routeAction("ip_geo")).toBe("frontend");
  });

  it("系统操作交给后端", () => {
    for (const id of [
      "url_open",
      "file_open",
      "file_reveal",
      "json_format",
      "ip_ping",
      "domain_ping",
      "domain_dns",
    ]) {
      expect(routeAction(id), id).toBe("backend");
    }
  });

  it("未知动作明确不支持（不猜，猜错会执行错误操作）", () => {
    expect(routeAction("no_such_action")).toBe("unsupported");
    expect(routeAction("")).toBe("unsupported");
  });
});

describe("actionColor", () => {
  it("联网动作给提示色（用户要知道会发请求）", () => {
    expect(actionColor("url_open")).toBe("orange");
    expect(actionColor("ip_ping")).toBe("orange");
    expect(actionColor("domain_dns")).toBe("orange");
  });

  it("本地渲染类为蓝色", () => {
    expect(actionColor("url_qr")).toBe("blue");
    expect(actionColor("color_show")).toBe("blue");
  });

  it("未知动作标红", () => {
    expect(actionColor("weird")).toBe("red");
  });

  it("本地文件操作为中性", () => {
    expect(actionColor("file_open")).toBe("default");
    expect(actionColor("file_reveal")).toBe("default");
    expect(actionColor("json_format")).toBe("default");
  });
});

describe("actionHint", () => {
  it("联网动作明说需要联网", () => {
    expect(actionHint(action({ id: "domain_dns", online: true }))).toBe("需要联网");
    expect(actionHint(action({ id: "ip_ping", online: true }))).toBe("需要联网");
  });

  it("各动作给出可读说明", () => {
    expect(actionHint(action({ id: "file_reveal" }))).toContain("选中");
    expect(actionHint(action({ id: "json_format" }))).toContain("剪贴板");
    expect(actionHint(action({ id: "url_qr" }))).toContain("本地");
    // 无特别说明时为空串（不显示空 tooltip）
    expect(actionHint(action({ id: "file_open" }))).toBe("");
  });
});

describe("summarizeResult", () => {
  it("取首个非空行并截断", () => {
    expect(summarizeResult("\n\n第一行\n第二行")).toBe("第一行");
    const long = "x".repeat(500);
    const s = summarizeResult(long);
    expect(s.length).toBeLessThanOrEqual(201);
    expect(s.endsWith("…")).toBe(true);
  });

  it("空结果给出兜底文案", () => {
    expect(summarizeResult("")).toBe("已执行");
    expect(summarizeResult("   \n  ")).toBe("已执行");
  });

  it("保留中文不被截断破坏", () => {
    expect(summarizeResult("已在浏览器打开 https://a.com")).toContain("浏览器");
  });
});

describe("shouldShowActions", () => {
  it("有动作才显示（普通文本不显示占位，避免噪声）", () => {
    expect(shouldShowActions(detect([action()]))).toBe(true);
    expect(shouldShowActions(detect([]))).toBe(false);
    expect(shouldShowActions(null)).toBe(false);
  });
});

describe("actionsSummary", () => {
  it("含总数与联网项数", () => {
    const s = actionsSummary(detect([action(), action({ id: "ip_ping", online: true })]));
    expect(s).toContain("2 个动作");
    expect(s).toContain("1 个需联网");
  });

  it("无联网项时不提联网", () => {
    const s = actionsSummary(detect([action(), action({ id: "color_show" })]));
    expect(s).toContain("2 个动作");
    expect(s).not.toContain("联网");
  });

  it("空列表返回空串", () => {
    expect(actionsSummary(detect([]))).toBe("");
    expect(actionsSummary(null)).toBe("");
  });
});

describe("normalizeHex", () => {
  it("三位缩写展开为六位（否则某些渲染路径会显示空白）", () => {
    expect(normalizeHex("#abc")).toBe("#AABBCC");
    expect(normalizeHex("#f0a")).toBe("#FF00AA");
  });

  it("六位原样大写", () => {
    expect(normalizeHex("#a1b2c3")).toBe("#A1B2C3");
    expect(normalizeHex("#A1B2C3")).toBe("#A1B2C3");
  });

  it("非法值返回空串（界面据此显示兜底色）", () => {
    for (const bad of ["", "abc", "#gg0000", "#12345", "#1234567"]) {
      expect(normalizeHex(bad), bad).toBe("");
    }
  });
});

describe("isLightColor", () => {
  it("浅色判为亮（文字用黑）", () => {
    expect(isLightColor("#ffffff")).toBe(true);
    expect(isLightColor("#fff")).toBe(true);
    expect(isLightColor("#ffff00")).toBe(true);
  });

  it("深色判为暗（文字用白）", () => {
    expect(isLightColor("#000000")).toBe(false);
    expect(isLightColor("#333")).toBe(false);
    expect(isLightColor("#0000ff")).toBe(false);
  });

  it("非法值时保守返回 true", () => {
    expect(isLightColor("nope")).toBe(true);
  });
});

describe("formatColor", () => {
  it("HEX 与 RGB 两种格式", () => {
    expect(formatColor("#abc", "HEX")).toBe("#AABBCC");
    expect(formatColor("#ff8800", "RGB")).toBe("rgb(255, 136, 0)");
  });

  it("非法值原样返回（不编造）", () => {
    expect(formatColor("nope", "RGB")).toBe("nope");
  });
});
