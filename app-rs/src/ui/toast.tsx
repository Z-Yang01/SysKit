/**
 * 轻量 Toast（AntD message 的替代）：无依赖、portal 渲染、2.6s 自动消退。
 *
 * 用法：`const toast = useToast(); toast.success("已复制");`
 * 需在应用根部挂一次 `<ToastProvider />`（main.tsx 已挂）。
 */
import {
  createContext,
  useCallback,
  useContext,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";
import { cx } from "./index";

type ToastKind = "success" | "error" | "info";

interface ToastItem {
  id: number;
  kind: ToastKind;
  text: string;
}

const ToastCtx = createContext<{
  success: (text: string) => void;
  error: (text: string) => void;
  info: (text: string) => void;
}>({ success: () => {}, error: () => {}, info: () => {} });

export function useToast() {
  return useContext(ToastCtx);
}

/** 展示时长：悬停会暂停（剩余时长续走），点击立即关闭。 */
const TOAST_MS = 2600;

const KIND_STYLE: Record<ToastKind, string> = {
  success: "border-sk-ok/45 text-sk-ok",
  error: "border-sk-err/45 text-sk-err",
  info: "border-sk-accent/45 text-sk-accent",
};

const KIND_MARK: Record<ToastKind, string> = {
  success: "✓",
  error: "✕",
  info: "ℹ",
};

let NEXT_ID = 1;

export function ToastProvider({ children }: { children: ReactNode }) {
  const [items, setItems] = useState<ToastItem[]>([]);
  const timers = useRef<Map<number, number>>(new Map());
  const remainMs = useRef<Map<number, number>>(new Map());
  const shownAt = useRef<Map<number, number>>(new Map());

  const dismiss = useCallback((id: number) => {
    const t = timers.current.get(id);
    if (t !== undefined) window.clearTimeout(t);
    timers.current.delete(id);
    remainMs.current.delete(id);
    shownAt.current.delete(id);
    setItems((list) => list.filter((i) => i.id !== id));
  }, []);

  /** 悬停暂停：清掉计时器但记下剩余时长，移开鼠标后按剩余时长续走。 */
  const pause = useCallback((id: number) => {
    const t = timers.current.get(id);
    if (t === undefined) return;
    window.clearTimeout(t);
    timers.current.delete(id);
    const shown = shownAt.current.get(id) ?? 0;
    remainMs.current.set(id, Math.max(TOAST_MS - (Date.now() - shown), 400));
  }, []);

  const resume = useCallback(
    (id: number) => {
      if (timers.current.has(id)) return;
      const ms = remainMs.current.get(id) ?? 400;
      const t = window.setTimeout(() => dismiss(id), ms);
      timers.current.set(id, t);
    },
    [dismiss],
  );

  const push = useCallback((kind: ToastKind, text: string) => {
    const id = NEXT_ID++;
    setItems((list) => [...list.slice(-4), { id, kind, text }]);
    shownAt.current.set(id, Date.now());
    const t = window.setTimeout(() => dismiss(id), TOAST_MS);
    timers.current.set(id, t);
  }, [dismiss]);

  const api = useMemo(
    () => ({
      success: (text: string) => push("success", text),
      error: (text: string) => push("error", text),
      info: (text: string) => push("info", text),
    }),
    [push],
  );

  // 卸载时清计时器（开发热重载防泄漏）
  const onUnmount = useCallback(() => {
    for (const t of timers.current.values()) window.clearTimeout(t);
    timers.current.clear();
    remainMs.current.clear();
    shownAt.current.clear();
  }, []);
  useRef(onUnmount);

  return (
    <ToastCtx.Provider value={api}>
      {children}
      {createPortal(
        <div className="pointer-events-none fixed left-1/2 top-[18px] z-[3000] flex -translate-x-1/2 flex-col items-center gap-2">
          {items.map((i) => (
            <div
              key={i.id}
              role="status"
              title="点击关闭"
              onMouseEnter={() => pause(i.id)}
              onMouseLeave={() => resume(i.id)}
              onClick={() => dismiss(i.id)}
              className={cx(
                "pointer-events-auto flex cursor-pointer items-center gap-2 rounded-[10px] border px-3 py-1.5 text-sm",
                "bg-sk-card shadow-[0_4px_16px_var(--sk-shadow_color)]",
                KIND_STYLE[i.kind],
              )}
            >
              <span className="font-bold">{KIND_MARK[i.kind]}</span>
              <span className="text-sk-text">{i.text}</span>
            </div>
          ))}
        </div>,
        document.body,
      )}
    </ToastCtx.Provider>
  );
}
