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
  dayBuckets,
  dayKey,
  dayLabel,
  evictionCandidates,
  filterByDay,
  groupRowsByDay,
  formatTs,
  kindColor,
  kindFilterOptions,
  kindLabel,
  pauseHint,
  purgeConfirmText,
  relativeTime,
  rowMenuItems,
  watchEnableConfirmText,
  watchResumeConfirmText,
  watchToggleLabel,
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
  it("筛选项与 Python 同口径（首项为全部，html 叫「网页」）", () => {
    expect(KIND_FILTERS[0]).toEqual({ key: "", label: "全部类型" });
    expect(KIND_FILTERS.map((k) => k.key)).toEqual(["", "text", "image", "file", "html"]);
    expect(KIND_FILTERS.find((k) => k.key === "html")?.label).toBe("网页");
  });

  it("标签与颜色", () => {
    expect(kindLabel("text")).toBe("文本");
    expect(kindLabel("image")).toBe("图片");
    expect(kindLabel("file")).toBe("文件");
    expect(kindLabel("html")).toBe("网页");
    expect(kindLabel("weird")).toBe("weird");
    expect(kindLabel("")).toBe("未知");
    expect(kindColor("image")).toBe("purple");
    expect(kindColor("nope")).toBe("default");
  });
});

describe("时间展示", () => {
  it("formatTs 输出本地时间（含秒，对齐 Python %H:%M:%S 的分辨力）", () => {
    const s = formatTs(1_700_000_000);
    expect(s).toMatch(/^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/);
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

  it("按天数清理必须如实说明置顶也会被删", () => {
    // Python 语义：置顶条目同样过期——按天是时间语义；置顶只豁免数量淘汰。
    // 文案若许诺「置顶保留」，用户拿置顶当永久收藏会被静默清掉。
    const t = purgeConfirmText(30);
    expect(t).toContain("30 天前");
    expect(t).toContain("置顶");
    expect(t).toContain("一并删除");
    expect(t).not.toContain("保留");
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

describe("类型筛选计数（顶部 Segmented）", () => {
  it("全部用 summary.count，其余用 by_kind；html 显示「网页」不写 HTML", () => {
    const opts = kindFilterOptions(summary());
    expect(opts.map((o) => o.key)).toEqual(["", "text", "image", "file", "html"]);
    expect(opts[0].label).toBe("全部(10)");
    expect(opts.find((o) => o.key === "text")?.label).toBe("文本(8)");
    expect(opts.find((o) => o.key === "image")?.label).toBe("图片(2)");
    expect(opts.find((o) => o.key === "file")?.label).toBe("文件(0)");
    expect(opts.find((o) => o.key === "html")?.label).toBe("网页(0)");
  });

  it("summary 未读到时退化为纯标签（无计数，不猜 0）", () => {
    const opts = kindFilterOptions(null);
    expect(opts[0]).toEqual({ key: "", label: "全部" });
    expect(opts.find((o) => o.key === "text")?.label).toBe("文本");
  });

  it("全部的计数不等于 by_kind 求和也能对上（count 是后端唯一真源）", () => {
    // by_kind 漏记的类型不能让「全部」缩水
    const opts = kindFilterOptions(summary({ count: 99, by_kind: { text: 3 } }));
    expect(opts[0].label).toBe("全部(99)");
    expect(opts.find((o) => o.key === "text")?.label).toBe("文本(3)");
  });
});

describe("行内「···」菜单项", () => {
  it("文本行：全文/置顶/另存为/动作/分隔/删除（danger）", () => {
    const items = rowMenuItems(row({ kind: "text", text: "abc" }));
    expect(items.map((i) => i.key)).toEqual(["preview", "pin", "save", "actions", "sep", "remove"]);
    expect(items.find((i) => i.key === "pin")?.label).toBe("📌 置顶");
    expect(items.find((i) => i.key === "save")?.disabled).toBe(false);
    expect(items.find((i) => i.key === "remove")?.danger).toBe(true);
  });

  it("已置顶行：置顶项变为「取消置顶」", () => {
    expect(rowMenuItems(row({ pinned: true })).find((i) => i.key === "pin")?.label).toBe(
      "📌 取消置顶",
    );
  });

  it("图片缺文件：另存为禁用并带原因；图片行不出「全文」项", () => {
    const items = rowMenuItems(row({ kind: "image", text: "", data: "" }));
    expect(items.some((i) => i.key === "preview")).toBe(false);
    const save = items.find((i) => i.key === "save");
    expect(save?.disabled).toBe(true);
    expect(save?.disabledReason).toContain("没有对应文件");
  });

  it("图片有文件：另存为可用", () => {
    const save = rowMenuItems(row({ kind: "image", data: "a.png", text: "[图片]" })).find(
      (i) => i.key === "save",
    );
    expect(save?.disabled).toBe(false);
  });
});

describe("监听开关（M6 后补的界面入口）", () => {
  it("三态按钮标签", () => {
    expect(watchToggleLabel(false, false)).toBe("开启监听");
    expect(watchToggleLabel(true, false)).toBe("暂停记录");
    expect(watchToggleLabel(true, true)).toBe("恢复记录");
  });

  it("开启确认文案覆盖记录范围与黑名单豁免", () => {
    const t = watchEnableConfirmText();
    expect(t).toContain("文本 / 图片 / 文件路径 / HTML");
    expect(t).toContain("不记录");
    expect(t).toContain("到期自动清空");
  });

  it("恢复确认文案必须说明不补录", () => {
    expect(watchResumeConfirmText()).toContain("不会");
    expect(watchResumeConfirmText()).toContain("补录");
  });
});

/**
 * 按天查看。
 *
 * 最容易错的不是分组，而是**时区**：用 toISOString() 取日期会把东八区
 * 00:00–08:00 复制的内容归到昨天。用例用本地时间构造时间戳，锁住这一点。
 */
describe("按天查看", () => {
  /** 本地时间的某天某点 → unix 秒（避免 UTC 偏移导致的假红）。 */
  const local = (y: number, m: number, d: number, h = 12, min = 0) =>
    Math.floor(new Date(y, m - 1, d, h, min, 0).getTime() / 1000);

  it("dayKey 按本地时区取日期（不是 UTC）", () => {
    // 本地 00:30 必须仍属于当天——toISOString() 在东八区会返回前一天
    expect(dayKey(local(2026, 10, 19, 0, 30))).toBe("2026-10-19");
    expect(dayKey(local(2026, 10, 19, 23, 59))).toBe("2026-10-19");
    expect(dayKey(0)).toBe("");
    expect(dayKey(-1)).toBe("");
  });

  it("dayLabel 区分今天/昨天/更早", () => {
    const now = Date.now();
    const today = dayKey(Math.floor(now / 1000));
    const y = new Date(now);
    y.setDate(y.getDate() - 1);
    const yesterday = dayKey(Math.floor(y.getTime() / 1000));
    expect(dayLabel(today, now)).toBe("今天");
    expect(dayLabel(yesterday, now)).toBe("昨天");
    expect(dayLabel("2020-01-02", now)).toBe("2020-01-02");
    expect(dayLabel("")).toBe("未知日期");
  });

  it("dayBuckets 计数正确，且同一天只出现一次（置顶穿插也不切段）", () => {
    const rows = [
      row({ idx: 0, ts: local(2026, 10, 15), pinned: true }),
      row({ idx: 1, ts: local(2026, 10, 19, 9) }),
      row({ idx: 2, ts: local(2026, 10, 15), pinned: true }),
      row({ idx: 3, ts: local(2026, 10, 19, 10) }),
    ];
    const b = dayBuckets(rows, local(2026, 10, 19) * 1000);
    expect(b.map((x) => x.key)).toEqual(["2026-10-15", "2026-10-19"]);
    expect(b[0].count).toBe(2);
    expect(b[1].count).toBe(2);
    expect(b[1].label).toBe("今天");
  });

  it("filterByDay 只留当天；空 key = 原样返回", () => {
    const rows = [
      row({ idx: 0, ts: local(2026, 10, 18) }),
      row({ idx: 1, ts: local(2026, 10, 19) }),
      row({ idx: 2, ts: local(2026, 10, 19) }),
    ];
    expect(filterByDay(rows, "2026-10-19").map((r) => r.idx)).toEqual([1, 2]);
    expect(filterByDay(rows, "")).toBe(rows);
  });

  it("groupRowsByDay 保持组内顺序，且跨日的同一天合并", () => {
    const rows = [
      row({ idx: 0, ts: local(2026, 10, 15) }),
      row({ idx: 1, ts: local(2026, 10, 19) }),
      row({ idx: 2, ts: local(2026, 10, 15) }),
    ];
    const g = groupRowsByDay(rows);
    expect(g.map((x) => x.key)).toEqual(["2026-10-15", "2026-10-19"]);
    expect(g[0].rows.map((r) => r.idx)).toEqual([0, 2]);
    expect(g[1].rows.map((r) => r.idx)).toEqual([1]);
  });
});
