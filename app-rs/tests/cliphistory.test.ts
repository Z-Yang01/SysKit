/**
 * 剪贴板历史管理页纯逻辑测试。
 *
 * 重点是"用户能看到的真相"：置顶是否真的免于淘汰、清空会连带删掉什么、
 * 隐私暂停恢复后会不会补录。这些弄错会让用户对隐私与数据产生错误预期。
 */
import { describe, expect, it } from "vitest";

import {
  KIND_FILTERS,
  canSaveAs,
  clearConfirmText,
  clipSummaryText,
  evictionCandidates,
  formatTs,
  kindColor,
  kindLabel,
  pauseHint,
  purgeConfirmText,
  relativeTime,
  type ClipRow,
  type ClipSummary,
} from "../src/cliphistory";

const row = (over: Partial<ClipRow> = {}): ClipRow => ({
  idx: 0,
  ts: 1_700_000_000,
  text: "hello",
  kind: "text",
  pinned: false,
  data: "",
  preview: "hello",
  ...over,
});

const summary = (over: Partial<ClipSummary> = {}): ClipSummary => ({
  count: 10,
  pinned: 2,
  max_items: 50,
  watch_enabled: true,
  watch_paused: false,
  by_kind: { text: 8, image: 2 },
  ...over,
});

describe("类型映射", () => {
  it("筛选项与 Python 同口径（首项为全部）", () => {
    expect(KIND_FILTERS[0]).toEqual({ key: "", label: "全部类型" });
    expect(KIND_FILTERS.map((k) => k.key)).toEqual(["", "text", "image", "file", "html"]);
  });

  it("标签与颜色", () => {
    expect(kindLabel("text")).toBe("文本");
    expect(kindLabel("image")).toBe("图片");
    expect(kindLabel("file")).toBe("文件");
    expect(kindLabel("html")).toBe("HTML");
    expect(kindLabel("weird")).toBe("weird");
    expect(kindLabel("")).toBe("未知");
    expect(kindColor("image")).toBe("purple");
    expect(kindColor("nope")).toBe("default");
  });
});

describe("时间展示", () => {
  it("formatTs 输出本地时间", () => {
    const s = formatTs(1_700_000_000);
    expect(s).toMatch(/^\d{4}-\d{2}-\d{2} \d{2}:\d{2}$/);
  });

  it("非法时间戳显示「—」而不是 1970", () => {
    expect(formatTs(0)).toBe("—");
    expect(formatTs(-1)).toBe("—");
    expect(formatTs(Number.NaN)).toBe("—");
  });

  it("relativeTime 分档", () => {
    const now = 1_700_000_000_000;
    expect(relativeTime(1_700_000_000, now)).toBe("刚刚");
    expect(relativeTime(1_700_000_000 - 300, now)).toBe("5 分钟前");
    expect(relativeTime(1_700_000_000 - 7200, now)).toBe("2 小时前");
    expect(relativeTime(1_700_000_000 - 172800, now)).toBe("2 天前");
  });

  it("未来时间戳不显示负数（时钟回拨）", () => {
    const now = 1_700_000_000_000;
    expect(relativeTime(1_700_000_000 + 999, now)).toBe("刚刚");
  });
});

describe("evictionCandidates 置顶保护", () => {
  it("只统计非置顶条目超出上限的部分", () => {
    const rows = [
      row({ idx: 0, pinned: true }),
      row({ idx: 1 }),
      row({ idx: 2 }),
      row({ idx: 3 }),
    ];
    // 3 条非置顶、上限 2 → 1 条将被淘汰
    expect(evictionCandidates(rows, 2)).toBe(1);
  });

  it("非置顶未超上限时为 0（置顶不参与计数）", () => {
    const rows = [row({ pinned: true }), row({ pinned: true }), row()];
    expect(evictionCandidates(rows, 2)).toBe(0);
  });

  it("空列表与 0 上限", () => {
    expect(evictionCandidates([], 50)).toBe(0);
    expect(evictionCandidates([row()], 0)).toBe(1);
  });
});

describe("clipSummaryText", () => {
  it("含总数/上限/置顶/分类", () => {
    const s = clipSummaryText(summary());
    expect(s).toContain("10/50");
    expect(s).toContain("置顶 2");
    expect(s).toContain("文本 8");
    expect(s).toContain("图片 2");
  });

  it("跳过计数为 0 的类型（避免噪声）", () => {
    const s = clipSummaryText(summary({ by_kind: { text: 3, image: 0 } }));
    expect(s).toContain("文本 3");
    expect(s).not.toContain("图片");
  });

  it("监听关闭 / 隐私暂停各自说明", () => {
    expect(clipSummaryText(summary({ watch_enabled: false }))).toContain("监听已关闭");
    expect(
      clipSummaryText(summary({ watch_enabled: true, watch_paused: true })),
    ).toContain("隐私暂停");
  });

  it("null 时明确提示", () => {
    expect(clipSummaryText(null)).toBe("尚未读取");
  });
});

describe("清空/清理确认文案", () => {
  it("清空时必须说明置顶也会被删", () => {
    const t = clearConfirmText(10, 3);
    expect(t).toContain("10");
    expect(t).toContain("不可撤销");
    expect(t).toContain("置顶");
    expect(t).toContain("3 条");
  });

  it("没有置顶时不提置顶（避免噪声）", () => {
    const t = clearConfirmText(10, 0);
    expect(t).not.toContain("已置顶");
  });

  it("清空说明图片文件也会删", () => {
    expect(clearConfirmText(1, 0)).toContain("PNG");
  });

  it("按天数清理说明保留置顶", () => {
    const t = purgeConfirmText(30);
    expect(t).toContain("30 天前");
    expect(t).toContain("置顶");
  });
});

describe("pauseHint 隐私说明", () => {
  it("监听关闭时说明无「暂停」可言", () => {
    expect(pauseHint(false, false)).toContain("监听已关闭");
  });

  it("暂停时必须说明恢复后不补录（隐私预期）", () => {
    const t = pauseHint(true, true);
    expect(t).toContain("不会");
    expect(t).toContain("补录");
  });

  it("未暂停时也说明暂停的后果", () => {
    const t = pauseHint(false, true);
    expect(t).toContain("补录");
  });
});

describe("canSaveAs", () => {
  it("文本条目可保存", () => {
    expect(canSaveAs(row({ kind: "text", text: "abc" })).ok).toBe(true);
  });

  it("图片条目需要 data 里的文件名", () => {
    expect(canSaveAs(row({ kind: "image", data: "a.png", text: "[图片]" })).ok).toBe(true);
    const noFile = canSaveAs(row({ kind: "image", data: "", text: "" }));
    expect(noFile.ok).toBe(false);
    expect(noFile.reason).toContain("没有对应文件");
  });

  it("空文本条目不可保存并说明原因", () => {
    const r = canSaveAs(row({ kind: "text", text: "" }));
    expect(r.ok).toBe(false);
    expect(r.reason).toContain("为空");
  });
});
