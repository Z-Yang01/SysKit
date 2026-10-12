/**
 * 命令面板（无边框弹出窗，label `palette`，默认热键 Ctrl+Alt+K）。
 *
 * M1 阶段只交付与热键/托盘闭环相关的部分：输入即搜剪贴板历史、回车把条目
 * **复制**回剪贴板（不注键，与快捷粘贴面板的分工一致——面板负责闭环，命令面板
 * 也放回内容）。计算器 / 页面跳转 / 快捷动作（Python palette.py 的
 * `_dynamic_actions`）经 `sk_palette_query` 已接入：动作区在前端执行
 * （nav: 跳页 / tab: 跳页+子tab / quick: 触发既有按钮或命令）。
 *
 * v1.4 UI 迁移：脱离 AntD，交互件来自 src/ui/（与 ClipPanel 同批样板）。
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
} from "./pasteState";
import { Empty, Input, cx } from "./ui";

/** 命令面板条目：与剪贴板条目同形，外加动作名以便后续接 provider。 */
interface PaletteEntry extends ClipRow {
  action: "copy_back";
}

interface PanelInit {
  rows: ClipRow[];
  max_items: number;
}

export function CommandPalette() {
  const [state, dispatch] = useReducer(reducer, initialState);
  /** 动作区结果（sk_palette_query；输入停顿 200ms 才查） */
  const [actions, setActions] = useState<{ id: string; label: string; hint: string }[]>([]);
  const [actionsOpen, setActionsOpen] = useState(false);
  const inputRef = useRef<HTMLInputElement | null>(null);

  const reload = useCallback(async () => {
    try {
      const data = await invoke<PanelInit>("sk_panel_init");
      const entries: PaletteEntry[] = data.rows.map((r) => ({ ...r, action: "copy_back" }));
      dispatch({ type: "loaded", rows: entries, seq: 0 });
    } catch (e) {
      dispatch({ type: "notice", value: `读取失败：${String(e)}` });
    }
    inputRef.current?.focus();
    // 呼出即拉动作区（空查询 = 全量动作列表）
    invoke<{ id: string; label: string; hint: string }[]>("sk_palette_query", { q: "" })
      .then((r) => {
        setActions(r);
        setActionsOpen(true);
      })
      .catch(() => setActionsOpen(false));
  }, []);

  // 输入防抖 200ms → 动作区查询（剪贴板过滤仍是内存即时）
  useEffect(() => {
    const t = setTimeout(() => {
      invoke<{ id: string; label: string; hint: string }[]>("sk_palette_query", {
        q: state.query,
      })
        .then((r) => {
          setActions(r);
          setActionsOpen(true);
        })
        .catch(() => setActionsOpen(false));
    }, 200);
    return () => clearTimeout(t);
  }, [state.query]);

  useEffect(() => {
    void reload();
    const unlisten = getCurrentWindow().listen("sk://panel-shown", (ev) => {
      const payload = ev.payload as { panel?: string } | undefined;
      if (!payload || payload.panel === "palette") void reload();
    });
    return () => {
      void unlisten.then((f) => f());
    };
  }, [reload]);

  useEffect(() => {
    const unlisten = getCurrentWindow().onFocusChanged(({ payload: focused }) => {
      if (!focused) void invoke("sk_hide_panel", { label: "palette" });
    });
    return () => {
      void unlisten.then((f) => f());
    };
  }, []);

  /** 执行：把选中条目放回系统剪贴板（不注键）。 */
  const execute = useCallback(
    async (position?: number) => {
      const row = position !== undefined ? state.visible[position] : currentRow(state);
      if (!row) return;
      try {
        const ok = await invoke<boolean>("sk_clip_write_back", { index: row.idx });
        dispatch({
          type: "notice",
          value: ok ? "内容已放回剪贴板" : "放回失败：内容不可用",
        });
      } catch (e) {
        dispatch({ type: "notice", value: `放回失败：${String(e)}` });
      }
      void invoke("sk_hide_panel", { label: "palette" });
    },
    [state],
  );

  /** 执行一个动作（nav/tab 跳页 + 子tab；quick 触发主窗既有按钮或后端命令）。 */
  const runAction = useCallback(
    async (id: string) => {
      void invoke("sk_hide_panel", { label: "palette" });
      const { emit } = await import("@tauri-apps/api/event");
      if (id.startsWith("nav:")) {
        const page = id.slice(4);
        await emit("sk://palette-nav", { page });
        return;
      }
      if (id.startsWith("tab:")) {
        const [, page, tab] = id.split(":");
        await emit("sk://palette-nav", { page, tab });
        return;
      }
      switch (id) {
        case "quick:diag":
        case "quick:deep":
          await emit("sk://palette-nav", { page: "diagnose", quick: id === "quick:diag" });
          return;
        case "quick:flush":
          await invoke("sk_clip_action_run", { id: "flush_dns", arg: "" }).catch(() => {});
          return;
        case "quick:wifi":
          await emit("sk://palette-nav", { page: "system", tab: "wifipwd" });
          return;
        case "quick:show":
          await emit("sk://palette-nav", { page: "overview" });
          return;
        case "quick:clip_paste":
          await invoke("sk_toggle_panel", { label: "paste" }).catch(() => {});
          return;
        default:
          if (id.startsWith("custom:")) {
            // 自定义命令的执行需要 value：sk_palette_query 的动作列表补个
            // value 字段更直接——但为省一次往返，这里从 settings_all 读
            const all = await invoke<{ items: { key: string; current: unknown }[] }>(
              "sk_settings_all",
            ).catch(() => null);
            const raw = all?.items?.find((i) => i.key === "palette_commands")?.current;
            const cmds =
              typeof raw === "string"
                ? (JSON.parse(raw) as { name: string; type: string; value: string }[])
                : ((raw as { name: string; type: string; value: string }[]) ?? []);
            const i = Number(id.slice(7));
            const cmd = cmds[i];
            if (!cmd) return;
            if (cmd.type === "cmd") {
              // cmd 类：写进剪贴板交给用户执行更安全（Rust 端不静默跑任意命令）
              await invoke("sk_clip_copy_plain", { text: cmd.value });
            } else {
              await invoke("sk_clip_action_run", { id: "url_open", arg: cmd.value });
            }
          }
      }
    },
    [],
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
          e.preventDefault();
          void execute(intent.position);
          break;
        case "close":
          e.preventDefault();
          void invoke("sk_hide_panel", { label: "palette" });
          break;
        default:
          break;
      }
    },
    [execute],
  );

  const footer = useMemo(
    () => state.notice || "↑↓ 选择 · Enter 放回剪贴板 · Esc 关闭 · 点上方动作直达页面",
    [state.notice],
  );

  return (
    <div className="flex h-screen flex-col overflow-hidden rounded-[10px] border border-sk-border bg-sk-card text-sk-text">
      <div className="px-2.5 pb-1.5 pt-2.5">
        <Input
          ref={inputRef}
          autoFocus
          value={state.query}
          onChange={(e) => dispatch({ type: "query", value: e.target.value })}
          onKeyDown={onKeyDown}
          placeholder="搜索剪贴板条目…"
        />
      </div>
      <div className="flex-1 overflow-y-auto px-2.5">
        {actionsOpen && actions.length > 0 && (
          <>
            <div className="sk-hint px-1 pb-0.5 pt-1.5">
              动作（Enter 执行第一个；剪贴板结果见下方）
            </div>
            {actions.slice(0, 8).map((a) => (
              <div
                key={a.id}
                onClick={() => void runAction(a.id)}
                className="cursor-pointer rounded-md px-2 py-[3px] hover:bg-sk-hover"
              >
                <span className="block flex-1 overflow-hidden text-ellipsis whitespace-nowrap">
                  {a.label}
                  <span className="sk-hint ml-1.5">{a.hint}</span>
                </span>
              </div>
            ))}
            <div className="my-1 border-t border-sk-border" />
          </>
        )}
        {state.visible.length === 0 ? (
          <Empty description={state.all.length === 0 ? "剪贴板历史为空" : "没有匹配的条目"} />
        ) : (
          state.visible.map((row, i) => (
            <div
              key={row.idx}
              onMouseEnter={() => dispatch({ type: "select", idx: row.idx })}
              onDoubleClick={() => void execute(i)}
              className={cx(
                "cursor-pointer rounded-md px-2 py-1",
                i === state.cursor ? "bg-sk-hover" : "",
              )}
            >
              <span className="block flex-1 overflow-hidden text-ellipsis whitespace-nowrap" title={row.text}>
                {rowLabel(row)}
              </span>
            </div>
          ))
        )}
      </div>
      <div className="flex items-center gap-2.5 border-t border-sk-border px-3 py-1.5">
        <span className="sk-hint flex-1">{footer}</span>
        <span className="sk-hint">共 {state.all.length} 条</span>
      </div>
    </div>
  );
}
