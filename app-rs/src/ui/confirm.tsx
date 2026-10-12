/**
 * 轻量 Confirm（AntD Modal.confirm 替代）：命令式单发确认框。
 *
 * 用法与 AntD 同形：`confirm({ title, content, okText, danger, onOk })`，
 * onOk 返回 Promise 时按钮进 loading、弹窗保持打开直到 resolve——
 * reject 保持打开（与 AntD 语义一致），resolve 关闭。
 *
 * 实现为模块级状态 + 事件通知，由应用根部挂的 `<ConfirmHost />` 渲染
 * （main.tsx 已挂）。**密码输入场景**传 input: "password" 直接渲染受控
 * 密码框（AntD 时代的静态快照/非受控 DOM hack 不再需要）。
 */
import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { Button, Input } from "./index";

interface ConfirmOptions {
  title: string;
  content?: React.ReactNode;
  okText?: string;
  cancelText?: string;
  danger?: boolean;
  width?: number;
  /** 渲染一个密码输入框（解锁/更改主密码等），值为 onOk 的入参 */
  input?: "password" | "text";
  inputPlaceholder?: string;
  /** input 存在时必填校验文案；为空不校验 */
  inputRequiredMsg?: string;
  onOk: (inputValue: string) => Promise<void> | void;
}

interface ActiveConfirm extends ConfirmOptions {
  id: number;
}

let LISTENER: ((c: ActiveConfirm) => void) | null = null;
let NEXT_ID = 1;

export function confirm(opts: ConfirmOptions): void {
  const c: ActiveConfirm = { ...opts, id: NEXT_ID++ };
  LISTENER?.(c);
}

/** 应用根部挂一次：渲染当前确认框（同刻只显示一个）。 */
export function ConfirmHost() {
  const [active, setActive] = useState<ActiveConfirm | null>(null);
  const [busy, setBusy] = useState(false);
  const [inputValue, setInputValue] = useState("");
  const [inputError, setInputError] = useState("");

  useEffect(() => {
    LISTENER = (c) => {
      setActive(c);
      setBusy(false);
      setInputValue("");
      setInputError("");
    };
    return () => {
      LISTENER = null;
    };
  }, []);

  if (!active) return null;

  const close = () => setActive(null);

  const runOk = async () => {
    if (active.input && active.inputRequiredMsg && !inputValue) {
      setInputError(active.inputRequiredMsg);
      return;
    }
    setBusy(true);
    try {
      await active.onOk(inputValue);
      close();
    } catch {
      // onOk reject = 保持打开（语义与 AntD 一致，错误信息由 onOk 自己展示）
    } finally {
      setBusy(false);
    }
  };

  return createPortal(
    <div
      className="fixed inset-0 z-[2100] flex items-start justify-center bg-black/45 pt-[14vh]"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget && !busy) close();
      }}
    >
      <div
        className="max-h-[76vh] overflow-auto rounded-[14px] border border-sk-border bg-sk-card shadow-[0_8px_24px_var(--sk-shadow_color)]"
        style={{ width: active.width ?? 440 }}
        role="alertdialog"
        aria-modal
      >
        <div className="px-4 pb-1 pt-3.5 text-[15px] font-semibold text-sk-text">{active.title}</div>
        <div className="px-4 pb-3 pt-1 text-sm text-sk-text">
          {typeof active.content === "string" ? (
            <div className="whitespace-pre-wrap">{active.content}</div>
          ) : (
            active.content
          )}
          {active.input && (
            <div className="mt-3">
              <Input
                autoFocus
                type={active.input === "password" ? "password" : "text"}
                placeholder={active.inputPlaceholder ?? ""}
                value={inputValue}
                onChange={(e) => {
                  setInputValue(e.target.value);
                  setInputError("");
                }}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && !busy) void runOk();
                }}
              />
              {inputError && <div className="mt-1 text-xs text-sk-err">{inputError}</div>}
            </div>
          )}
        </div>
        <div className="flex justify-end gap-2 px-4 pb-3.5">
          <Button onClick={close} disabled={busy}>
            {active.cancelText ?? "取消"}
          </Button>
          <Button variant={active.danger ? "danger" : "primary"} loading={busy} onClick={() => void runOk()}>
            {active.okText ?? "确定"}
          </Button>
        </div>
      </div>
    </div>,
    document.body,
  );
}
