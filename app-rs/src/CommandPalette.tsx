/**
 * 命令面板（无边框弹出窗，label `palette`，默认热键 Ctrl+Alt+K）。
 *
 * M1 阶段只交付与热键/托盘闭环相关的部分：输入即搜剪贴板历史、回车把条目
 * **复制**回剪贴板（不注键，与快捷粘贴面板的分工一致——面板负责闭环，命令面板
 * 只放回内容）。计算器 / 页面跳转 / 密码库搜索（Python palette.py 的
 * `_dynamic_actions`）属 M7，本组件按「provider 表」结构预留，接入时只加条目。
 */
import { useCallback, useEffect, useMemo, useReducer, useRef } from "react";
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
  }, []);

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
    () => state.notice || "↑↓ 选择 · Enter 放回剪贴板 · Esc 关闭（M7 接入页面跳转/计算器）",
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
