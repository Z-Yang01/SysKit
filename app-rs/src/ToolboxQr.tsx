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
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Alert,
  Button,
  Descriptions,
  Space,
  Spin,
  Tag,
  Typography,
  message,
} from "antd";

import { isHttpUrl, parseWifiQr, type WifiQrInfo } from "./qrdecode";
import { SENSITIVE_COPY_COMMAND } from "./wifipassword";

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

export function ToolboxQr({ initialText, decodeClipItem, onDone }: Props) {
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

  const copyPlain = useCallback(async (t: string, label: string) => {
    try {
      await invoke("sk_clip_copy_plain", { text: t });
      message.success(`${label}已复制`);
    } catch (e) {
      message.error(String(e));
    }
  }, []);

  const copyPassword = useCallback(async () => {
    if (!wifi?.password) return;
    try {
      // 纪律同 WiFi 密钥：密码走敏感通道（不进历史 / 不进 Win+V / 到期清空）
      await invoke(SENSITIVE_COPY_COMMAND, { text: wifi.password });
      message.success("密码已复制（敏感通道，到期自动清空）");
    } catch (e) {
      message.error(String(e));
    }
  }, [wifi]);

  const openUrl = useCallback(async (u: string) => {
    try {
      await invoke("sk_open_url", { url: u });
    } catch (e) {
      message.error(String(e));
    }
  }, []);

  return (
    <Space direction="vertical" size={10} style={{ width: "100%" }}>
      <Space wrap>
        <Button
          type="primary"
          size="small"
          loading={busy}
          onClick={() => fileRef.current?.click()}
        >
          选择图片…
        </Button>
        <Button size="small" loading={busy} onClick={() => void decodeClipboard()}>
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
        <Typography.Text type="secondary" className="sk-hint">
          支持截图 / 图片文件里的二维码；识别在本机完成，不联网
        </Typography.Text>
      </Space>

      {busy && (
        <div style={{ textAlign: "center", padding: "18px 0" }}>
          <Spin tip="正在识别…" />
        </div>
      )}

      {err && <Alert type="warning" showIcon message="识别失败" description={err} />}

      {text !== null && !busy && (
        <>
          {wifi ? (
            <Alert
              type="success"
              showIcon
              message="识别到 WiFi 二维码"
              description={
                <Space direction="vertical" size={8} style={{ width: "100%" }}>
                  <Descriptions size="small" column={1}>
                    <Descriptions.Item label="网络名（SSID）">
                      {wifi.ssid}
                    </Descriptions.Item>
                    <Descriptions.Item label="认证方式">
                      {wifi.auth.toUpperCase()}
                    </Descriptions.Item>
                    <Descriptions.Item label="密码">
                      {wifi.password ? (
                        <Space size={8}>
                          <Typography.Text code>{wifi.password}</Typography.Text>
                          <Button size="small" type="link" onClick={() => void copyPassword()}>
                            复制密码
                          </Button>
                        </Space>
                      ) : (
                        <Typography.Text type="secondary">（无密码，开放网络）</Typography.Text>
                      )}
                    </Descriptions.Item>
                    {wifi.hidden && (
                      <Descriptions.Item label="隐藏网络">是</Descriptions.Item>
                    )}
                  </Descriptions>
                  {ssids.includes(wifi.ssid) && (
                    <Typography.Text type="secondary" className="sk-hint">
                      该网络已在本机已存列表——密钥可在「系统助手 → WiFi 密码 → 已存密码」查看
                    </Typography.Text>
                  )}
                </Space>
              }
            />
          ) : (
            <Alert
              type="success"
              showIcon
              message={
                <Space size={6}>
                  识别结果
                  {url ? <Tag color="blue">链接</Tag> : <Tag>文本</Tag>}
                </Space>
              }
              description={
                <Typography.Paragraph
                  copyable
                  style={{ marginBottom: 0, wordBreak: "break-all", whiteSpace: "pre-wrap" }}
                >
                  {text}
                </Typography.Paragraph>
              }
            />
          )}
          <Space wrap>
            <Button size="small" onClick={() => void copyPlain(text, "内容")}>
              复制内容
            </Button>
            {url && (
              <Button size="small" type="primary" onClick={() => void openUrl(url)}>
                打开链接
              </Button>
            )}
            {onDone && <Button size="small" onClick={onDone}>关闭</Button>}
          </Space>
        </>
      )}
    </Space>
  );
}
