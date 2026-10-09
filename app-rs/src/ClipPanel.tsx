/**
 * 快捷粘贴面板（无边框弹出窗，label `paste`）。
 *
 * 生命周期：全局热键 / 托盘呼出 → 后端先记录前台目标窗口 → 显示本面板 →
 * 输入即搜（按键只做内存过滤）→ Enter/1-9/双击 → 写回剪贴板 + 焦点守卫注键。
 * 失焦（点其他窗口）或 Esc 即收起；执行后由后端负责隐藏窗口。
 *
 * 业务规则全部在 `pastePanel.ts` 的 reducer 里（无 DOM 依赖，可被 vitest 全覆盖），
 * 本组件只负责把原生事件翻译成动作、调用后端并渲染。
 *
 * v1.4 UI 迁移：本组件是**首个脱离 AntD 的页面**——全部交互件来自 src/ui/
 * （shadcn 风格自建组件），作为后续逐页迁移的样板与组件库完备性验证。
 */
import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

import {
  initialState,
  reducer,
  currentRow,
  keyIntent,
  rowLabel,
  type ClipRow,
} from "./pastePanel";
import { Button, Empty, Input, cx } from "./ui";
import { Modal } from "./ui/modal";

/** 后端 `sk_panel_init` 的返回。 */
interface PanelInit {
  rows: ClipRow[];
  max_items: number;
  paste_hotkey: string;
  paste_hotkey_display: string;
  quick_hotkey: string;
  quick_hotkey_display: string;
  watch_enabled: boolean;
}

/** 后端 `sk_paste` / `sk_quick_paste` 的返回。 */
interface PasteResult {
  ok: boolean;
  message: string;
  written: boolean;
}

/** 文本长于该长度时提供全文预览（Python 用 80 字折叠 + 对话框看全文）。 */
const PREVIEW_THRESHOLD = 120;

/** 面板内图片条目缩略图：与历史页同源 sk_clip_image_thumb（96px）。 */
function PanelThumb({ index }: { index: number }) {
  const [src, setSrc] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    let alive = true;
    invoke<string>("sk_clip_image_thumb", { index: index })
      .then((url) => {
        if (alive) setSrc(url);
      })
      .catch(() => {
        if (alive) setFailed(true);
      });
    return () => {
      alive = false;
    };
  }, [index]);
  if (failed) return null; // 图片文件缺失时退回 emoji 标记
  if (!src) return <span className="sk-hint">…</span>;
  return (
    <img
      src={src}
      width={36}
      height={36}
      alt=""
      className="shrink-0 rounded object-cover"
    />
  );
}

export function ClipPanel() {
  const [state, dispatch] = useReducer(reducer, initialState);
  const [init, setInit] = useState<PanelInit | null>(null);
  const [preview, setPreview] = useState<ClipRow | null>(null);
  const inputRef = useRef<HTMLInputElement | null>(null);

  /** 呼出/重呼出：取一次快照，重置查询，聚焦输入框。 */
  const reload = useCallback(async () => {
    try {
      const data = await invoke<PanelInit>("sk_panel_init");
      setInit(data);
      const seq = await invoke<number>("sk_paste_seq");
      dispatch({ type: "loaded", rows: data.rows, seq });
    } catch (e) {
      dispatch({ type: "notice", value: `读取剪贴板历史失败：${String(e)}` });
    }
    inputRef.current?.focus();
  }, []);

  useEffect(() => {
    void reload();
    // 后端每次呼出都广播 panel-shown：重呼出必须换一份快照并作废旧粘贴链
    const unlisten = getCurrentWindow().listen("sk://panel-shown", (ev) => {
      const payload = ev.payload as { panel?: string } | undefined;
      if (!payload || payload.panel === "paste") void reload();
    });
    return () => {
      void unlisten.then((f) => f());
    };
  }, [reload]);

  // 焦点离开即收起（Ditto / Win+V 同语义）
  useEffect(() => {
    const unlisten = getCurrentWindow().onFocusChanged(({ payload: focused }) => {
      if (!focused) void invoke("sk_hide_panel", { label: "paste" });
    });
    return () => {
      void unlisten.then((f) => f());
    };
  }, []);

  useEffect(() => {
    if (state.notice) {
      const t = window.setTimeout(() => dispatch({ type: "notice", value: "" }), 2600);
      return () => window.clearTimeout(t);
    }
    return undefined;
  }, [state.notice]);

  /** 执行当前选中行（或指定可见序位置）→ 写回 + 守卫注键。 */
  const execute = useCallback(
    async (position?: number) => {
      if (position !== undefined) dispatch({ type: "selectVisible", position });
      const row = position !== undefined ? state.visible[position] : currentRow(state);
      if (!row) {
        dispatch({ type: "notice", value: "没有可粘贴的条目" });
        return;
      }
      dispatch({ type: "notice", value: "" });
      try {
        const r = await invoke<PasteResult>("sk_paste", {
          req: { index: row.idx, seq: state.seq },
        });
        if (r.message) dispatch({ type: "notice", value: r.message });
        // 注键成功 → 后端已隐藏面板；失败时保留面板让用户手动 Ctrl+V
      } catch (e) {
        dispatch({ type: "notice", value: `粘贴失败：${String(e)}` });
      }
    },
    [state],
  );

  const onKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLInputElement>) => {
      const intent = keyIntent(e.key);
      switch (intent.kind) {
        case "move":
          e.preventDefault();
          dispatch({ type: "move", delta: intent.delta });
          break;
        case "execute":
          e.preventDefault();
          void execute();
          break;
        case "direct":
          // 越界时 reducer 原样返回 → 按键回落成文本输入
          e.preventDefault();
          void execute(intent.position);
          break;
        case "close":
          e.preventDefault();
          void invoke("sk_hide_panel", { label: "paste" });
          break;
        default:
          break;
      }
    },
    [execute],
  );

  const togglePin = useCallback(async (row: ClipRow) => {
    await invoke("sk_clip_toggle_pin", { index: row.idx });
    dispatch({ type: "togglePin", idx: row.idx });
  }, []);

  const remove = useCallback(async (row: ClipRow) => {
    await invoke("sk_clip_remove", { index: row.idx });
    dispatch({ type: "removed", idx: row.idx });
  }, []);

  const quickPaste = useCallback(async () => {
    const r = await invoke<PasteResult>("sk_quick_paste");
    if (r.message) dispatch({ type: "notice", value: r.message });
  }, []);

  const hint = useMemo(() => {
    const paste = init?.paste_hotkey_display ? `热键 ${init.paste_hotkey_display}` : "无热键";
    return `↑↓ 选择 · Enter 粘贴 · 1-9 直选 · Esc 取消（${paste}）`;
  }, [init]);

  const visible = state.visible;

  return (
    <div className="flex h-screen flex-col overflow-hidden rounded-[10px] border border-sk-border bg-sk-card text-sk-text">
      <div className="px-2.5 pb-1.5 pt-2.5">
        <Input
          ref={inputRef}
          autoFocus
          value={state.query}
          onChange={(e) => dispatch({ type: "query", value: e.target.value })}
          onKeyDown={onKeyDown}
          placeholder="搜索剪贴板历史…　↑↓ 选择 · Enter/1-9 直接粘贴到原窗口 · Esc 取消"
        />
      </div>

      <div className="flex-1 overflow-y-auto px-2.5">
        {state.loading ? (
          <div className="text-sk-sub">正在读取历史…</div>
        ) : visible.length === 0 ? (
          <div className="px-1 py-4">
            {state.all.length === 0 && init && !init.watch_enabled ? (
              <>
                <div className="text-sk-sub">剪贴板监听未开启，还没有可粘贴的历史。</div>
                <div className="mt-1.5 text-xs text-sk-sub">
                  到主窗口「剪贴板历史」页点「开启监听」——之后复制的文本会自动出现在这里。
                </div>
              </>
            ) : (
              <Empty
                description={state.all.length === 0 ? "剪贴板历史为空" : "没有匹配的条目"}
              />
            )}
          </div>
        ) : (
          visible.map((row, i) => (
            <div
              key={row.idx}
              onMouseEnter={() => dispatch({ type: "select", idx: row.idx })}
              onDoubleClick={() => void execute(i)}
              className={cx(
                "flex cursor-pointer items-center gap-2 rounded-md px-2 py-1",
                i === state.cursor ? "bg-sk-hover" : "",
              )}
            >
              {row.kind === "image" && <PanelThumb index={row.idx} />}
              <span className="flex-1 overflow-hidden text-ellipsis whitespace-nowrap" title={row.text}>
                {row.kind === "image" ? row.preview || rowLabel(row) : rowLabel(row)}
              </span>
              {row.text.length > PREVIEW_THRESHOLD && (
                <Button
                  size="sm"
                  variant="ghost"
                  onClick={(e) => {
                    e.stopPropagation();
                    setPreview(row);
                  }}
                >
                  全文
                </Button>
              )}
              <Button size="sm" variant="ghost" onClick={() => void togglePin(row)}>
                {row.pinned ? "取消置顶" : "置顶"}
              </Button>
              <Button size="sm" variant="danger" onClick={() => void remove(row)}>
                删除
              </Button>
            </div>
          ))
        )}
      </div>

      <div className="flex items-center gap-2.5 border-t border-sk-border px-3 py-1.5">
        <span className="sk-hint flex-1">{state.notice || hint}</span>
        <span className="sk-hint">
          共 {state.all.length} 条 / 上限 {init?.max_items ?? "—"}
        </span>
        <Button size="sm" onClick={() => void quickPaste()}>
          直贴最新
        </Button>
      </div>

      <Modal open={preview !== null} title="条目全文" onClose={() => setPreview(null)} width={640}>
        <div className="max-h-[60vh] overflow-auto whitespace-pre-wrap break-all">
          {preview?.text}
        </div>
      </Modal>
    </div>
  );
}
