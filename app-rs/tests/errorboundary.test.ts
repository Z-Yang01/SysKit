/**
 * ErrorBoundary 的纯逻辑部分（无需 DOM 的可测子集）。
 *
 * getDerivedStateFromError 是静态纯函数：输入异常 → 输出兜底 state。
 * 渲染部分（兜底 UI）无 jsdom 基建，不硬造组件测试——组件本身仅 30 行，
 * 真实验证靠 nightly 启动自检与 ErrorBoundary 的日志出口（frontend_error
 * 落 syskit.log 的 Rust 侧测试已锁）。
 */
import { describe, expect, it } from "vitest";

import { ErrorBoundary } from "../src/ErrorBoundary";

describe("ErrorBoundary.getDerivedStateFromError", () => {
  it("把异常放进 state（渲染分支据此切换兜底 UI）", () => {
    const err = new Error("boom");
    expect(ErrorBoundary.getDerivedStateFromError(err)).toEqual({ error: err });
  });
});
