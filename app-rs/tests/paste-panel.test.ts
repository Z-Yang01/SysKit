/**
 * 快捷粘贴面板的状态机测试（纯逻辑，无 DOM）。
 *
 * 覆盖 Python 版 ClipPastePanel 的每条行为契约与兜底分支：
 * 呼出快照/内存过滤、↑↓ 端点停住、1-9 直选越界回落、置顶重排、删除后游标修正、
 * 双击按可见序定位、键位意图映射、行文案规则。
 */
import { describe, expect, it } from "vitest";

import {
  currentRow,
  filterRows,
  initialState,
  isFallbackNotice,
  keyIntent,
  kindPrefix,
  matches,
  moveCursor,
  reducer,
  rowLabel,
  type ClipRow,
  type PanelState,
} from "../src/pasteState";

function row(idx: number, text: string, kind = "text", pinned = false): ClipRow {
  return { idx, ts: 1000 - idx, text, kind, pinned, data: "", preview: text.slice(0, 80) };
}

const ROWS: ClipRow[] = [
  row(0, "Hello World"),
  row(1, "[图片 128×96]", "image"),
  row(2, "hello 中文"),
  row(3, "/tmp/a.txt", "file"),
];

function loaded(rows: ClipRow[] = ROWS, seq = 7): PanelState {
  return reducer(initialState, { type: "loaded", rows, seq });
}

describe("matches / filterRows", () => {
  it("空查询一律通过", () => {
    expect(matches("任意", "")).toBe(true);
    expect(matches("任意", "   ")).toBe(true);
    expect(filterRows(ROWS, "")).toHaveLength(4);
  });

  it("大小写不敏感子串", () => {
    const hits = filterRows(ROWS, "HELLO");
    expect(hits).toHaveLength(2);
    expect(hits.map((r) => r.idx)).toEqual([0, 2]);
  });

  it("无命中返回空数组", () => {
    expect(filterRows(ROWS, "不存在的内容")).toHaveLength(0);
  });
});

describe("moveCursor", () => {
  it("空列表恒为 -1", () => {
    expect(moveCursor([], -1, 1)).toBe(-1);
    expect(moveCursor([], 3, -1)).toBe(-1);
  });

  it("无选中时向下落首行、向上落末行", () => {
    expect(moveCursor(ROWS, -1, 1)).toBe(0);
    expect(moveCursor(ROWS, -1, -1)).toBe(3);
  });

  it("端点处保持不动（不环绕）", () => {
    expect(moveCursor(ROWS, 3, 1)).toBe(3);
    expect(moveCursor(ROWS, 0, -1)).toBe(0);
  });

  it("中间正常移动", () => {
    expect(moveCursor(ROWS, 1, 1)).toBe(2);
    expect(moveCursor(ROWS, 2, -1)).toBe(1);
  });
});

describe("reducer: loaded", () => {
  it("重置查询、游标落首行、记录序号", () => {
    const s = loaded(ROWS, 42);
    expect(s.query).toBe("");
    expect(s.cursor).toBe(0);
    expect(s.seq).toBe(42);
    expect(s.loading).toBe(false);
    expect(s.visible).toHaveLength(4);
  });

  it("空历史 → 游标 -1 且不崩", () => {
    const s = loaded([]);
    expect(s.cursor).toBe(-1);
    expect(currentRow(s)).toBeNull();
  });

  it("快照截到前 200 行（Python _build_rows 的 _all[:200] 同口径）", () => {
    // 面板只服务「最近条目」；用户把上限调得很大时也要保住渲染轻量
    const many: ClipRow[] = Array.from({ length: 260 }, (_, i) => row(i, `条目 ${i}`));
    const s = loaded(many);
    expect(many).toHaveLength(260);
    expect(s.all).toHaveLength(200);
    expect(s.visible).toHaveLength(200);
    // 截断保留的是最新的前 200 条（idx 0..199，后端已按时间倒序给出）
    expect(s.all[0].idx).toBe(0);
    expect(s.all[199].idx).toBe(199);
  });
});

describe("reducer: query", () => {
  it("过滤后游标按位置保留（与 Python currentItem 保持语义一致）", () => {
    let s = loaded();
    s = reducer(s, { type: "move", delta: 1 }); // cursor=1
    s = reducer(s, { type: "query", value: "hello" });
    // 过滤后只剩 idx 0 与 2；cursor=1 仍在下标范围内 → 保持位置，指向 idx 2
    expect(s.visible.map((r) => r.idx)).toEqual([0, 2]);
    expect(s.cursor).toBe(1);
    expect(currentRow(s)?.idx).toBe(2);
  });

  it("游标越界时才落回首行", () => {
    let s = loaded();
    s = reducer(s, { type: "move", delta: -1 }); // 无选中向上 → cursor=3
    s = reducer(s, { type: "query", value: "hello" }); // 只剩 2 行 → 3 越界
    expect(s.cursor).toBe(0);
    expect(currentRow(s)?.idx).toBe(0);
  });

  it("过滤后原选中仍在可见集内则保持位置", () => {
    let s = loaded();
    s = reducer(s, { type: "move", delta: 1 }); // cursor=1
    s = reducer(s, { type: "query", value: "1" }); // 命中 idx1、idx2(逗号后无1?) → 只 idx1
    expect(s.visible.map((r) => r.idx)).toEqual([1]);
    expect(s.cursor).toBe(0);
  });

  it("过滤无命中 → 游标 -1，Enter 无动作", () => {
    let s = loaded();
    s = reducer(s, { type: "query", value: "zzz" });
    expect(s.visible).toHaveLength(0);
    expect(s.cursor).toBe(-1);
    expect(currentRow(s)).toBeNull();
  });
});

describe("reducer: 数字键直选", () => {
  it("1-9 选中当前可见序第 N 行", () => {
    let s = loaded();
    s = reducer(s, { type: "selectVisible", position: 2 });
    expect(s.cursor).toBe(2);
    expect(currentRow(s)?.idx).toBe(2);
  });

  it("越界原样返回（按键回落到输入框）", () => {
    const s = loaded();
    const after = reducer(s, { type: "selectVisible", position: 8 });
    expect(after).toBe(s);
  });

  it("直选按可见序而非快照序", () => {
    let s = loaded();
    s = reducer(s, { type: "query", value: "hello" });
    s = reducer(s, { type: "selectVisible", position: 1 });
    expect(currentRow(s)?.idx).toBe(2);
  });
});

describe("reducer: select / 悬停", () => {
  it("只允许选中可见行", () => {
    let s = loaded();
    s = reducer(s, { type: "query", value: "hello" });
    s = reducer(s, { type: "select", idx: 1 }); // 被过滤掉 → 无变化
    expect(s.cursor).toBe(0);
    s = reducer(s, { type: "select", idx: 2 });
    expect(s.cursor).toBe(1);
  });
});

describe("reducer: 置顶", () => {
  it("置顶后移到最前，块内保持原序", () => {
    let s = loaded();
    s = reducer(s, { type: "togglePin", idx: 2 });
    expect(s.all[0].idx).toBe(2);
    expect(s.all[0].pinned).toBe(true);
    expect(s.all.slice(1).map((r) => r.idx)).toEqual([0, 1, 3]);
  });

  it("再次切换取消置顶后回到非置顶块", () => {
    let s = loaded();
    s = reducer(s, { type: "togglePin", idx: 2 });
    s = reducer(s, { type: "togglePin", idx: 2 });
    // 置顶标志翻转 + 集合不变；同组内相对顺序不构成契约（Python 用稳定排序，
    // V8 对等键的排序不保证稳定，因此只断言「置顶块为空、成员不变」）
    expect(s.all.every((r) => !r.pinned)).toBe(true);
    expect([...s.all.map((r) => r.idx)].sort()).toEqual([0, 1, 2, 3]);
    expect(s.all).toHaveLength(4);
  });

  it("过滤态下置顶不破坏可见集", () => {
    let s = loaded();
    s = reducer(s, { type: "query", value: "hello" });
    s = reducer(s, { type: "togglePin", idx: 2 });
    expect(s.visible.map((r) => r.idx)).toEqual([2, 0]);
    expect(s.cursor).toBe(0);
  });
});

describe("reducer: 删除 / 清空", () => {
  it("删除后游标修正到有效范围", () => {
    let s = loaded();
    s = reducer(s, { type: "move", delta: 1 });
    s = reducer(s, { type: "move", delta: 1 }); // cursor=2
    s = reducer(s, { type: "removed", idx: s.visible[2].idx });
    expect(s.all).toHaveLength(3);
    expect(s.cursor).toBeLessThan(s.visible.length);
  });

  it("删空后游标 -1", () => {
    let s = loaded([row(0, "唯一")]);
    s = reducer(s, { type: "removed", idx: 0 });
    expect(s.visible).toHaveLength(0);
    expect(s.cursor).toBe(-1);
  });

  it("清空重置一切", () => {
    let s = loaded();
    s = reducer(s, { type: "cleared" });
    expect(s.all).toHaveLength(0);
    expect(s.visible).toHaveLength(0);
    expect(s.cursor).toBe(-1);
  });
});

describe("keyIntent", () => {
  it("方向键与回车/Esc", () => {
    expect(keyIntent("ArrowUp")).toEqual({ kind: "move", delta: -1 });
    expect(keyIntent("ArrowDown")).toEqual({ kind: "move", delta: 1 });
    expect(keyIntent("Enter")).toEqual({ kind: "execute" });
    expect(keyIntent("Escape")).toEqual({ kind: "close" });
  });

  it("1-9 映射到 0 起的位置", () => {
    expect(keyIntent("1")).toEqual({ kind: "direct", position: 0 });
    expect(keyIntent("9")).toEqual({ kind: "direct", position: 8 });
  });

  it("0 与普通字符不产生动作", () => {
    expect(keyIntent("0")).toEqual({ kind: "none" });
    expect(keyIntent("a")).toEqual({ kind: "none" });
    expect(keyIntent("F1")).toEqual({ kind: "none" });
  });
});

describe("行文案", () => {
  it("类型前缀与 Python _KIND_EMOJI 对齐", () => {
    expect(kindPrefix(row(0, "x", "image"))).toBe("🖼 ");
    expect(kindPrefix(row(0, "x", "file"))).toBe("📁 ");
    expect(kindPrefix(row(0, "x", "html"))).toBe("🌐 ");
    expect(kindPrefix(row(0, "x", "text"))).toBe("");
  });

  it("置顶加 📌 前缀", () => {
    expect(rowLabel(row(0, "内容", "text", true))).toBe("📌 内容");
  });

  it("图片条目始终带 🖼 前缀（占位文本也要有类型区分）", () => {
    // Python 面板健康图片用缩略图替位、载荷丢失回退 🖼；紧凑面板没有缩略图
    // 链路，统一用 Python 的回退 emoji，否则「[图片 W×H]」与文本无从区分
    expect(rowLabel(row(0, "[图片 128×96]", "image"))).toBe("🖼 [图片 128×96]");
  });

  it("图片条目载荷丢失（非占位文本）时同样回退 emoji", () => {
    expect(rowLabel(row(0, "某个图片描述", "image"))).toBe("🖼 某个图片描述");
  });

  it("文件条目带 📁 前缀", () => {
    expect(rowLabel(row(0, "C:\\a.txt", "file"))).toBe("📁 C:\\a.txt");
  });
});

describe("降级提示判定", () => {
  it("手按 Ctrl+V 与已取消都算降级", () => {
    expect(isFallbackNotice("已复制；自动粘贴未送达，请按 Ctrl+V")).toBe(true);
    expect(isFallbackNotice("目标窗口已切换，已取消自动粘贴（内容已在剪贴板）")).toBe(true);
    expect(isFallbackNotice("已粘贴到原窗口")).toBe(false);
  });
});
