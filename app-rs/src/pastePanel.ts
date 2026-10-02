/**
 * 快捷粘贴面板的纯逻辑（无 React / 无 DOM，便于 vitest 全量覆盖）。
 *
 * 与 Python 侧 ui/clip_paste.py::ClipPastePanel 的行为逐条对齐：
 * - 呼出时取一次快照，此后按键**只做内存过滤**（不再读盘/解密）；
 * - ↑↓ 只在**可见行**之间移动；1-9 直选当前可见序的第 N 行（Ditto 同语义）；
 * - Enter 执行当前可见行；Esc 收起；
 * - 点击某行 → 选中；双击 / 命令执行 → 粘贴到呼出前记录的目标窗口。
 *
 * 把它做成 reducer 而不是散在组件里的 useState：兜底分支（空列表、过滤无命中、
 * 越界数字键、陈旧请求）都是状态机的一部分，可以逐条断言。
 */

/** 后端 `sk_clip_list` / `sk_panel_init` 返回的条目行。 */
export interface ClipRow {
  idx: number;
  ts: number;
  text: string;
  kind: string;
  pinned: boolean;
  data: string;
  preview: string;
}

export type PanelLabel = "paste" | "palette";

export interface PanelState {
  /** 本次呼出的快照（顺序 = 可见顺序，置顶在前）。 */
  all: ClipRow[];
  /** 过滤后的可见行（与 all 共享同一批对象）。 */
  visible: ClipRow[];
  query: string;
  /** 当前选中行在 `visible` 中的下标；空列表为 -1。 */
  cursor: number;
  /** 本次呼出的粘贴链序号（后端 `sk_paste` 用它防陈旧请求）。 */
  seq: number;
  /** 是否正在等待后端快照。 */
  loading: boolean;
  /** 最近一次操作结果文案（粘贴降级提示等）。 */
  notice: string;
}

export const initialState: PanelState = {
  all: [],
  visible: [],
  query: "",
  cursor: -1,
  seq: 0,
  loading: true,
  notice: "",
};

export type PanelAction =
  | { type: "loaded"; rows: ClipRow[]; seq: number }
  | { type: "query"; value: string }
  | { type: "move"; delta: number }
  | { type: "select"; idx: number }
  | { type: "selectVisible"; position: number }
  | { type: "togglePin"; idx: number }
  | { type: "removed"; idx: number }
  | { type: "cleared" }
  | { type: "notice"; value: string }
  | { type: "seq"; value: number };

/** 单行文本的过滤判定（大小写不敏感子串；空查询全通过）。 */
export function matches(text: string, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  return text.toLowerCase().includes(q);
}

/** 可见行 = 快照中命中查询的行（保持快照顺序）。 */
export function filterRows(all: ClipRow[], query: string): ClipRow[] {
  return all.filter((r) => matches(r.text, query));
}

/** 当前可见行上是否存在可见的选中项（过滤后不强制跳回首行，与 Python 一致）。 */
function cursorStillVisible(visible: ClipRow[], cursor: number): boolean {
  return cursor >= 0 && cursor < visible.length;
}

/** 找第一条可见行（过滤后若无选中则落到首行）。 */
function firstVisibleIndex(visible: ClipRow[]): number {
  return visible.length > 0 ? 0 : -1;
}

/** ↑↓ 跨过不可见行移动（此处所有 visible 都可见，等价于普通移动 + 端点停住）。 */
export function moveCursor(visible: ClipRow[], cursor: number, delta: number): number {
  const n = visible.length;
  if (n === 0) return -1;
  const step = delta > 0 ? 1 : -1;
  if (cursor < 0) return step > 0 ? 0 : n - 1;
  const next = cursor + step;
  if (next < 0 || next >= n) return cursor; // 已在可见端点：保持不动
  return next;
}

/** 当前选中的行（无选中/空列表返回 null）。 */
export function currentRow(state: PanelState): ClipRow | null {
  if (state.cursor < 0 || state.cursor >= state.visible.length) return null;
  return state.visible[state.cursor];
}

export function reducer(state: PanelState, action: PanelAction): PanelState {
  switch (action.type) {
    case "loaded": {
      // 呼出时重置查询与游标（与 Python popup() 的 clear + _refresh 同语义）
      const visible = filterRows(action.rows, "");
      return {
        ...state,
        all: action.rows,
        visible,
        query: "",
        cursor: firstVisibleIndex(visible),
        seq: action.seq,
        loading: false,
      };
    }
    case "query": {
      const visible = filterRows(state.all, action.value);
      // 过滤后原选中仍可见就不打扰（Python 的 currentItem 保持逻辑）
      const cursor = cursorStillVisible(visible, state.cursor)
        ? state.cursor
        : firstVisibleIndex(visible);
      return { ...state, query: action.value, visible, cursor };
    }
    case "move": {
      return { ...state, cursor: moveCursor(state.visible, state.cursor, action.delta) };
    }
    case "select": {
      const pos = state.visible.findIndex((r) => r.idx === action.idx);
      if (pos < 0) return state; // 过滤态下不可见的行不可被选中
      return { ...state, cursor: pos };
    }
    case "selectVisible": {
      // 数字键直选当前可见序的第 N 行；越界 → 原样返回（按键回落到输入框）
      if (action.position < 0 || action.position >= state.visible.length) return state;
      return { ...state, cursor: action.position };
    }
    case "togglePin": {
      const all = state.all.map((r) =>
        r.idx === action.idx ? { ...r, pinned: !r.pinned } : r,
      );
      // 置顶块整体前移；同组内相对顺序不构成契约（JS sort 对等键不保证稳定，
      // 后端已按 ts/置顶做权威排序，前端只做即时视觉反馈）
      all.sort((a, b) => Number(b.pinned) - Number(a.pinned));
      const visible = filterRows(all, state.query);
      return {
        ...state,
        all,
        visible,
        cursor: cursorStillVisible(visible, state.cursor)
          ? state.cursor
          : firstVisibleIndex(visible),
      };
    }
    case "removed": {
      const all = state.all.filter((r) => r.idx !== action.idx);
      const visible = filterRows(all, state.query);
      return {
        ...state,
        all,
        visible,
        cursor: cursorStillVisible(visible, state.cursor)
          ? state.cursor
          : firstVisibleIndex(visible),
      };
    }
    case "cleared": {
      return { ...state, all: [], visible: [], cursor: -1 };
    }
    case "notice":
      return { ...state, notice: action.value };
    case "seq":
      return { ...state, seq: action.value };
    default:
      return state;
  }
}

/** 键盘事件 → 动作（组件只负责把原生事件转成这个描述）。 */
export type KeyIntent =
  | { kind: "move"; delta: number }
  | { kind: "execute" }
  | { kind: "direct"; position: number }
  | { kind: "close" }
  | { kind: "none" };

export function keyIntent(key: string): KeyIntent {
  switch (key) {
    case "ArrowUp":
      return { kind: "move", delta: -1 };
    case "ArrowDown":
      return { kind: "move", delta: 1 };
    case "Enter":
      return { kind: "execute" };
    case "Escape":
      return { kind: "close" };
    default:
      break;
  }
  // 数字 1-9 直选（顶层数字键；小键盘在浏览器里同为 "1".."9"）
  if (/^[1-9]$/.test(key)) {
    return { kind: "direct", position: Number(key) - 1 };
  }
  return { kind: "none" };
}

/** 条目类型前缀（与 Python _KIND_EMOJI 对齐）。 */
export function kindPrefix(row: ClipRow): string {
  switch (row.kind) {
    case "image":
      return "🖼 ";
    case "file":
      return "📁 ";
    case "html":
      return "🌐 ";
    default:
      return "";
  }
}

/** 行显示文本：置顶前缀 + 类型前缀 + 预览。 */
export function rowLabel(row: ClipRow): string {
  const pin = row.pinned ? "📌 " : "";
  const kind = kindPrefix(row);
  const text = row.preview || row.text;
  // 图片条目预览已是「[图片 W×H]」形态，不再叠类型 emoji（Python 同规则：
  // 有缩略图时用图标替位，载荷丢失才回退 emoji）
  if (row.kind === "image" && text.startsWith("[图片")) return pin + text;
  return pin + kind + text;
}

/** 降级提示是否应当提示用户手按 Ctrl+V（供组件选文案样式）。 */
export function isFallbackNotice(notice: string): boolean {
  return notice.includes("Ctrl+V") || notice.includes("已取消");
}
