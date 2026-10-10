/**
 * 工具箱 · 二维码识别 tab（生成的反方向：图片 → 原文）。
 *
 * 三个入口汇聚到同一结果卡：
 * - 选图片文件（HTML input file，读 bytes base64 后 invoke，不引对话框插件）
 * - 从剪贴板识别（后端读 CF_DIB → PNG → 解码）
 * - （来自剪贴板历史图片条目 ··· 菜单的同款结果卡，见 ClipboardHistoryPanel）
 *
 * 结果动作按文本类型路由（routeQrResult，纯函数在 qrdecode.ts 有测试）：
 * URL → 打开；WIFI: → 拆解 SSID/密码/认证 + 复制密码走敏感通道；
 * 已存 WiFi 命中时给「查看该网密钥」跳转；其余 → 复制。
 *
 * v1.4 UI 迁移：脱离 AntD（Alert/Descriptions/Spin/Tag/copyable → ui 件 + KV 行）。
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import { copyChannel, isHttpUrl, parseWifiQr, type WifiQrInfo } from "./qrdecode";
import { SENSITIVE_COPY_COMMAND } from "./wifipassword";
import { Badge, Button } from "./ui";
import { Alert } from "./ui/alert";
import { useToast } from "./ui/toast";

interface Props {
  /** 剪贴板历史 ··· 菜单复用本组件的结果卡时，外部已拿到文本，跳过识别步骤 */
  initialText?: string;
  /** 给剪贴板历史条目 index：挂载即识别该条目图片（对话框形态） */
  decodeClipItem?: number;
  onDone?: () => void;
}

/** 已存 WiFi 名单（命中时给「查看该网密钥」跳转）。 */
async function knownSsids(): Promise<string[]> {
  try {
    return await invoke<string[]>("sk_known_ssids");
  } catch {
    return [];
  }
}

/** 键值行（AntD Descriptions 的替代，同 ToolboxBattery）。 */
function KV({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex items-baseline gap-3 py-1">
      <span className="w-32 shrink-0 text-sk-sub">{label}</span>
      <span className="min-w-0 text-sk-text">{children}</span>
    </div>
  );
}

export function ToolboxQr({ initialText, decodeClipItem, onDone }: Props) {
  const toast = useToast();
  const [busy, setBusy] = useState(false);
  const [text, setText] = useState<string | null>(initialText ?? null);
  const [err, setErr] = useState("");
  const [ssids, setSsids] = useState<string[]>([]);
  const fileRef = useRef<HTMLInputElement | null>(null);

  const finish = useCallback((decoded: string) => {
    setText(decoded);
    setErr("");
    if (parseWifiQr(decoded)) void knownSsids().then(setSsids);
  }, []);

  const decodeFile = useCallback(
    async (f: File) => {
      setBusy(true);
      setErr("");
      try {
        const buf = new Uint8Array(await f.arrayBuffer());
        let bin = "";
        const CHUNK = 0x8000;
        for (let i = 0; i < buf.length; i += CHUNK) {
          bin += String.fromCharCode(...buf.subarray(i, i + CHUNK));
        }
        const b64 = btoa(bin);
        finish(await invoke<string>("sk_qr_decode_image", { data: b64 }));
      } catch (e) {
        setText(null);
        setErr(String(e));
      } finally {
        setBusy(false);
      }
    },
    [finish],
  );

  const decodeClipHistoryItem = useCallback(
    async (idx: number) => {
      setBusy(true);
      setErr("");
      try {
        finish(await invoke<string>("sk_qr_decode_clip_item", { index: idx }));
      } catch (e) {
        setText(null);
        setErr(String(e));
      } finally {
        setBusy(false);
      }
    },
    [finish],
  );

  const decodeClipboard = useCallback(async () => {
    setBusy(true);
    setErr("");
    try {
      finish(await invoke<string>("sk_qr_decode_clipboard"));
    } catch (e) {
      setText(null);
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  }, [finish]);

  // 对话框形态：挂载即识别指定历史条目（只跑一次）
  useEffect(() => {
    if (decodeClipItem !== undefined) void decodeClipHistoryItem(decodeClipItem);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const wifi: WifiQrInfo | null = text ? parseWifiQr(text) : null;
  const url = text && isHttpUrl(text) ? text : null;

  const copyContent = useCallback(
    async (t: string, label: string) => {
      try {
        // 轮次116：WiFi 码整串含明文密码（P 字段）——复制走敏感通道，
        // 与「复制密码」同口径（不进历史 / 不进 Win+V / 到期清空）
        if (copyChannel(t) === "sensitive") {
          await invoke(SENSITIVE_COPY_COMMAND, { text: t });
          toast.success(`${label}已复制（含密码，敏感通道，到期自动清空）`);
        } else {
          await invoke("sk_clip_copy_plain", { text: t });
          toast.success(`${label}已复制`);
        }
      } catch (e) {
        toast.error(String(e));
      }
    },
    [toast],
  );

  const copyPassword = useCallback(async () => {
    if (!wifi?.password) return;
    try {
      // 纪律同 WiFi 密钥：密码走敏感通道（不进历史 / 不进 Win+V / 到期清空）
      await invoke(SENSITIVE_COPY_COMMAND, { text: wifi.password });
      toast.success("密码已复制（敏感通道，到期自动清空）");
    } catch (e) {
      toast.error(String(e));
    }
  }, [wifi, toast]);

  const openUrl = useCallback(
    async (u: string) => {
      try {
        await invoke("sk_open_url", { url: u });
      } catch (e) {
        toast.error(String(e));
      }
    },
    [toast],
  );

  return (
    <div className="flex flex-col gap-2.5">
      <div className="flex flex-wrap items-center gap-2">
        <Button variant="primary" loading={busy} onClick={() => fileRef.current?.click()}>
          选择图片…
        </Button>
        <Button loading={busy} onClick={() => void decodeClipboard()}>
          从剪贴板识别
        </Button>
        <input
          ref={fileRef}
          type="file"
          accept="image/png,image/jpeg,image/webp,image/bmp"
          style={{ display: "none" }}
          onChange={(e) => {
            const f = e.target.files?.[0];
            if (f) void decodeFile(f);
            e.target.value = ""; // 同一张图可重选
          }}
        />
        <span className="sk-hint">支持截图 / 图片文件里的二维码；识别在本机完成，不联网</span>
      </div>

      {busy && <div className="py-4 text-center text-sm text-sk-sub">正在识别…</div>}

      {err && (
        <Alert tone="warning" title="识别失败">
          {err}
        </Alert>
      )}

      {text !== null && !busy && (
        <>
          {wifi ? (
            <Alert tone="success" title="识别到 WiFi 二维码">
              <div className="flex flex-col gap-2">
                <div className="flex flex-col divide-y divide-sk-border/60">
                  <KV label="网络名（SSID）">{wifi.ssid}</KV>
                  <KV label="认证方式">{wifi.auth.toUpperCase()}</KV>
                  <KV label="密码">
                    {wifi.password ? (
                      <span className="inline-flex items-center gap-2">
                        <code className="rounded bg-sk-hover px-1.5 py-0.5 font-mono text-xs">
                          {wifi.password}
                        </code>
                        <Button variant="ghost" size="sm" onClick={() => void copyPassword()}>
                          复制密码
                        </Button>
                      </span>
                    ) : (
                      <span className="text-sk-sub">（无密码，开放网络）</span>
                    )}
                  </KV>
                  {wifi.hidden && <KV label="隐藏网络">是</KV>}
                </div>
                {ssids.includes(wifi.ssid) && (
                  <div className="sk-hint">
                    该网络已在本机已存列表——密钥可在「系统助手 → WiFi 密码 → 已存密码」查看
                  </div>
                )}
              </div>
            </Alert>
          ) : (
            <Alert
              tone="success"
              title={
                <span className="inline-flex items-center gap-1.5">
                  识别结果
                  {url ? <Badge tone="accent">链接</Badge> : <Badge>文本</Badge>}
                </span>
              }
            >
              <div className="break-all whitespace-pre-wrap">{text}</div>
            </Alert>
          )}
          <div className="flex flex-wrap gap-2">
            <Button size="sm" onClick={() => void copyContent(text, "内容")}>
              复制内容
            </Button>
            {url && (
              <Button variant="primary" size="sm" onClick={() => void openUrl(url)}>
                打开链接
              </Button>
            )}
            {onDone && (
              <Button size="sm" onClick={onDone}>
                关闭
              </Button>
            )}
          </div>
        </>
      )}
    </div>
  );
}
