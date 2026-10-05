/**
 * 命令面板（无边框弹出窗，label `palette`，默认热键 Ctrl+Alt+K）。
 *
 * M1 阶段只交付与热键/托盘闭环相关的部分：输入即搜剪贴板历史、回车把条目
 * **复制**回剪贴板（不注键，与快捷粘贴面板的分工一致——面板负责闭环，命令面板
 * 也放回内容）。计算器 / 页面跳转 / 快捷动作（Python palette.py 的
 * `_dynamic_actions`）经 `sk_palette_query` 已接入：动作区在前端执行
 * （nav: 跳页 / tab: 跳页+子tab / quick: 触发既有按钮或命令）。
 */
import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Input, List, Typography } from "antd";
import type { InputRef } from "antd";

import {
  initialState,
  reducer,
  currentRow,
  keyIntent,
  rowLabel,
  type ClipRow,
} from "./pastePanel";

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
  const inputRef = useRef<InputRef | null>(null);

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
    <div
      style={{
        height: "100vh",
        display: "flex",
        flexDirection: "column",
        background: "var(--sk-card)",
        color: "var(--sk-text)",
        border: "1px solid var(--sk-border)",
        borderRadius: 10,
        overflow: "hidden",
      }}
    >
      <div style={{ padding: 10, paddingBottom: 6 }}>
        <Input
          ref={inputRef}
          autoFocus
          value={state.query}
          onChange={(e) => dispatch({ type: "query", value: e.target.value })}
          onKeyDown={onKeyDown}
          placeholder="搜索剪贴板条目…"
          allowClear
        />
      </div>
      <div style={{ flex: 1, overflowY: "auto", padding: "0 10px" }}>
        {actionsOpen && actions.length > 0 && (
          <>
            <div className="sk-hint" style={{ padding: "6px 4px 2px" }}>
              动作（Enter 执行第一个；剪贴板结果见下方）
            </div>
            <List
              size="small"
              split={false}
              dataSource={actions.slice(0, 8)}
              renderItem={(a) => (
                <List.Item
                  onClick={() => void runAction(a.id)}
                  style={{ cursor: "pointer", padding: "3px 8px", borderRadius: 6 }}
                >
                  <span style={{ flex: 1, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                    {a.label}
                    <span className="sk-hint" style={{ marginLeft: 6 }}>{a.hint}</span>
                  </span>
                </List.Item>
              )}
            />
            <div style={{ borderTop: "1px solid var(--sk-border)", margin: "4px 0" }} />
          </>
        )}
        {state.visible.length === 0 ? (
          <Typography.Text type="secondary">
            {state.all.length === 0 ? "剪贴板历史为空" : "没有匹配的条目"}
          </Typography.Text>
        ) : (
          <List
            size="small"
            split={false}
            dataSource={state.visible}
            renderItem={(row, i) => (
              <List.Item
                onMouseEnter={() => dispatch({ type: "select", idx: row.idx })}
                onDoubleClick={() => void execute(i)}
                style={{
                  cursor: "pointer",
                  padding: "4px 8px",
                  borderRadius: 6,
                  background: i === state.cursor ? "var(--sk-sel_bg)" : undefined,
                  color: i === state.cursor ? "var(--sk-sel_text)" : undefined,
                }}
              >
                <span
                  style={{
                    flex: 1,
                    overflow: "hidden",
                    textOverflow: "ellipsis",
                    whiteSpace: "nowrap",
                  }}
                  title={row.text}
                >
                  {rowLabel(row)}
                </span>
              </List.Item>
            )}
          />
        )}
      </div>
      <div
        style={{
          padding: "6px 12px",
          borderTop: "1px solid var(--sk-border)",
          display: "flex",
          alignItems: "center",
          gap: 10,
        }}
      >
        <span className="sk-hint" style={{ flex: 1 }}>
          {footer}
        </span>
        <span className="sk-hint">共 {state.all.length} 条</span>
      </div>
    </div>
  );
}
