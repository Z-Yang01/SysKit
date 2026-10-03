/**
 * 全局错误边界：任一页面渲染抛错时显示兜底界面，而不是整个 webview 白屏。
 *
 * Python/Qt 版没有这个失败模式（异常有日志、窗口还在）；React 里一个未捕获
 * 的渲染异常会卸掉整棵树。兜底给出错误摘要 + 重载按钮——数据都在后端文件里，
 * 重载即可恢复。错误内容同时上报后端落文件日志（sk_frontend_error），
 * 让"前端崩了"从 devtools 独白变成可诊断的日志事件。
 */
import { Component, type ErrorInfo, type ReactNode } from "react";

import { invoke } from "@tauri-apps/api/core";

interface Props {
  children: ReactNode;
}

interface State {
  error: Error | null;
}

export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    // 上报后端落日志（fire-and-forget：上报失败不能再抛，兜底界面照常显示）
    const where = info.componentStack?.trim().split("\n")[0]?.trim() ?? "未知组件树";
    invoke("sk_frontend_error", {
      message: `${error.name}: ${error.message} @ ${where}`,
    }).catch(() => {});
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    return (
      <div
        style={{
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          justifyContent: "center",
          gap: 12,
          minHeight: "100vh",
          padding: 24,
        }}
      >
        <h2>界面出了问题，但数据没事</h2>
        <p style={{ color: "var(--sk-err, #d94040)", maxWidth: 640, wordBreak: "break-all" }}>
          {error.name}: {error.message}
        </p>
        <p style={{ color: "var(--sk-hint, #888)" }}>
          错误已写入日志（logs/syskit.log）。点「重新加载」即可恢复——所有数据都保存在
          后端，不会因此丢失。
        </p>
        <button onClick={() => location.reload()}>重新加载</button>
      </div>
    );
  }
}
