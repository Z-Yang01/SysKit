import React from "react";
import ReactDOM from "react-dom/client";
import { App } from "./App";
import { CommandPalette } from "./CommandPalette";
import { ClipPanel } from "./ClipPanel";
import { ErrorBoundary } from "./ErrorBoundary";
import { applyTheme } from "./theme";
import "./base.css";

/**
 * 三窗口共用一个前端入口（Tauri devUrl / frontendDist 只有一个）：
 * 托盘与热键会创建 label 为 `paste` / `palette` 的无边框窗口，它们各自的
 * WebviewWindow 标签决定渲染哪个组件；其余情况渲染主界面。
 *
 * 不能依赖 URL query：面板用 `WebviewUrl::App("index.html")` 打开，
 * 拿不到注入的启动参数，因此用窗口 label 判别（Tauri 2 的 __TAURI_INTERNALS__
 * 暴露了当前窗口 label）。
 */
function currentWindowLabel(): string {
  try {
    const internals = (
      window as unknown as {
        __TAURI_INTERNALS__?: { metadata?: { currentWindow?: { label?: string } } };
      }
    ).__TAURI_INTERNALS__;
    return internals?.metadata?.currentWindow?.label ?? "main";
  } catch {
    return "main";
  }
}

/** 面板窗口也要先把主题 CSS 变量写好，否则 --sk-* 取空值。
 *  M1 先用系统深色偏好做初值；真正跟随 settings.dark_mode 属 M7 设置页。 */
function bootTheme() {
  const dark = window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? false;
  applyTheme(dark);
}

bootTheme();

const label = currentWindowLabel();

const root =
  label === "paste" ? <ClipPanel /> : label === "palette" ? <CommandPalette /> : <App />;

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    {/* 三种窗口都兜：任一页面渲染抛错显示兜底界面+上报日志，而不是白屏 */}
    <ErrorBoundary>{root}</ErrorBoundary>
  </React.StrictMode>,
);
