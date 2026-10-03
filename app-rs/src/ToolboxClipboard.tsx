/**
 * 系统助手：剪贴板历史（对应 Python 侧 `ClipboardTab`）。
 *
 * 与快捷粘贴面板（`ClipPanel`）共用同一份历史与后端命令，但这里是完整管理界面：
 * 筛选、置顶、删除、清空、按天数清理、另存为。
 */
import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Alert,
  Button,
  Empty,
  Input,
  InputNumber,
  Modal,
  Select,
  Space,
  Table,
  Tag,
  Tooltip,
  Typography,
  message,
} from "antd";

import {
  KIND_FILTERS,
  canSaveAs,
  clearConfirmText,
  clipSummaryText,
  evictionCandidates,
  formatTs,
  kindColor,
  kindLabel,
  pauseHint,
  purgeConfirmText,
  relativeTime,
  watchEnableConfirmText,
  watchResumeConfirmText,
  watchToggleLabel,
  type ClipRow,
  type ClipSummary,
} from "./cliphistory";
import {
  actionColor,
  actionHint,
  actionsSummary,
  formatColor,
  isLightColor,
  normalizeHex,
  routeAction,
  shouldShowActions,
  summarizeResult,
  type ClipAction,
  type DetectResult,
} from "./cliprules";

export function ClipboardHistoryPanel() {
  const [rows, setRows] = useState<ClipRow[]>([]);
  const [summary, setSummary] = useState<ClipSummary | null>(null);
  const [keyword, setKeyword] = useState("");
  const [kind, setKind] = useState("");
  const [purgeDays, setPurgeDays] = useState(30);
  const [busy, setBusy] = useState(false);
  const [nowMs, setNowMs] = useState(() => Date.now());
  // 内容规则动作：detected 是"当前展示的那一条"的检测结果
  const [detecting, setDetecting] = useState(-1);
  const [detected, setDetected] = useState<DetectResult | null>(null);
  const [detectedFor, setDetectedFor] = useState(-1);
  const [running, setRunning] = useState("");
  const [qrOf, setQrOf] = useState("");
  const [qrSvg, setQrSvg] = useState("");
  const [colorOf, setColorOf] = useState("");

  const load = useCallback(async () => {
    try {
      const [list, sum] = await Promise.all([
        invoke<ClipRow[]>("sk_clip_list", {
          keyword: keyword.trim() || null,
          kind: kind || null,
        }),
        invoke<ClipSummary>("sk_clip_summary"),
      ]);
      setRows(list);
      setSummary(sum);
      setNowMs(Date.now());
      // 列表刷新后旧的检测结果已不对应任何行，必须清掉——
      // 否则用户会对着 A 条目的按钮操作 B 的内容
      setDetected(null);
      setDetectedFor(-1);
    } catch (e) {
      message.error(`读取剪贴板历史失败：${String(e)}`);
    }
  }, [keyword, kind]);

  // 二维码：qrOf 是要编码的文本，SVG 由后端本地渲染（qrcode crate）。
  // 失败必须把整个二维码区收起——不能留着"复制地址"让用户以为图还在。
  useEffect(() => {
    if (!qrOf) {
      setQrSvg("");
      return;
    }
    let alive = true;
    invoke<string>("sk_clip_qr_svg", { data: qrOf })
      .then((svg) => {
        if (alive) setQrSvg(svg);
      })
      .catch((e) => {
        if (!alive) return;
        message.error(String(e));
        setQrOf("");
      });
    return () => {
      alive = false;
    };
  }, [qrOf]);

  // 导出 PNG（对应 Python 版"保存图片"，文件名同为 clip_qr.png）：
  // SVG 画上白底后转位图。data-URL 的 SVG 不含外部资源，画布不会被污染。
  const saveQrPng = useCallback(async () => {
    if (!qrSvg) return;
    const img = new Image();
    img.src = "data:image/svg+xml;charset=utf-8," + encodeURIComponent(qrSvg);
    try {
      await new Promise<void>((res, rej) => {
        img.onload = () => res();
        img.onerror = () => rej(new Error("SVG 加载失败"));
      });
      const side = 560;
      const canvas = document.createElement("canvas");
      canvas.width = side;
      canvas.height = side;
      const ctx = canvas.getContext("2d");
      if (!ctx) throw new Error("画布不可用");
      ctx.fillStyle = "#fff";
      ctx.fillRect(0, 0, side, side);
      ctx.drawImage(img, 0, 0, side, side);
      const a = document.createElement("a");
      a.href = canvas.toDataURL("image/png");
      a.download = "clip_qr.png";
      a.click();
      message.success("已保存 clip_qr.png");
    } catch (e) {
      message.error(`二维码导出失败：${String(e)}`);
    }
  }, [qrSvg]);

  // 监听开关三态：关→开启（确认框）；开且未暂停→暂停（直接）；已暂停→恢复（确认不补录）
  const watchToggleModal = useCallback(() => {
    const enabled = summary?.watch_enabled ?? false;
    const paused = summary?.watch_paused ?? false;
    const apply = async (target: boolean) => {
      try {
        const r = await invoke<{ message: string }>("sk_clip_watch_set", {
          enabled: target,
        });
        message.success(r.message);
        await load();
      } catch (e) {
        message.error(String(e));
      }
    };
    if (!enabled) {
      Modal.confirm({
        title: "开启剪贴板监听",
        width: 540,
        content: (
          <Typography.Paragraph style={{ whiteSpace: "pre-wrap", marginBottom: 0 }}>
            {watchEnableConfirmText()}
          </Typography.Paragraph>
        ),
        okText: "开启",
        cancelText: "取消",
        onOk: () => apply(true),
      });
    } else if (paused) {
      Modal.confirm({
        title: "恢复记录",
        content: watchResumeConfirmText(),
        okText: "恢复",
        cancelText: "保持暂停",
        onOk: () => apply(true),
      });
    } else {
      void apply(false);
      message.info("已暂停记录（历史保留）");
    }
  }, [summary, load]);

  useEffect(() => {
    void load();
  }, [load]);

  const act = useCallback(
    async (fn: () => Promise<unknown>, okMsg: string) => {
      setBusy(true);
      try {
        await fn();
        message.success(okMsg);
        await load();
      } catch (e) {
        message.error(String(e));
      } finally {
        setBusy(false);
      }
    },
    [load],
  );

  const copyBack = useCallback(
    (row: ClipRow) => {
      void act(
        () => invoke<boolean>("sk_clip_write_back", { index: row.idx }),
        "已复制到剪贴板",
      );
    },
    [act],
  );

  const togglePin = useCallback(
    (row: ClipRow) => {
      void act(
        () => invoke<boolean>("sk_clip_toggle_pin", { index: row.idx }),
        row.pinned ? "已取消置顶" : "已置顶（不会被自动淘汰）",
      );
    },
    [act],
  );

  const remove = useCallback(
    (row: ClipRow) => {
      void act(() => invoke<boolean>("sk_clip_remove", { index: row.idx }), "已删除");
    },
    [act],
  );

  const saveAs = useCallback(async (row: ClipRow) => {
    try {
      const msg = await invoke<string>("sk_clip_save_as", { index: row.idx });
      if (msg) message.success(msg);
      // 空串 = 用户取消，保持安静
    } catch (e) {
      message.error(String(e));
    }
  }, []);

  /** 检测选中条目的可执行动作（后端识别，前端呈现）。 */
  const detect = useCallback(async (row: ClipRow) => {
    setDetecting(row.idx);
    try {
      setDetected(await invoke<DetectResult>("sk_clip_detect", { text: row.text }));
      setDetectedFor(row.idx);
    } catch (e) {
      setDetected(null);
      message.error(String(e));
    } finally {
      setDetecting(-1);
    }
  }, []);

  /** 执行一个动作：前端路由的自行处理，其余交给后端。 */
  const runAction = useCallback(
    async (a: ClipAction) => {
      const route = routeAction(a.id);
      if (route === "unsupported") {
        message.warning(`该动作尚未接入界面：${a.id}`);
        return;
      }
      if (route === "frontend") {
        if (a.id === "url_qr") {
          setQrOf(a.arg);
          return;
        }
        if (a.id === "color_show") {
          setColorOf(a.arg);
          return;
        }
        // ip_geo 需要外部服务，本版不代查（避免静默把 IP 发给第三方）
        message.info("归属地查询需要联网查询第三方服务，本版不代查以免泄露你的 IP");
        return;
      }
      setRunning(a.id);
      try {
        message.success(summarizeResult(await invoke<string>("sk_clip_action_run", { id: a.id, arg: a.arg })));
      } catch (e) {
        message.error(String(e));
      } finally {
        setRunning("");
      }
    },
    [],
  );

  const clearAll = useCallback(() => {
    const count = summary?.count ?? rows.length;
    const pinned = summary?.pinned ?? rows.filter((r) => r.pinned).length;
    Modal.confirm({
      title: "清空剪贴板历史",
      width: 520,
      content: (
        <Typography.Paragraph style={{ whiteSpace: "pre-wrap" }}>
          {clearConfirmText(count, pinned)}
        </Typography.Paragraph>
      ),
      okText: "清空",
      okButtonProps: { danger: true },
      cancelText: "取消",
      onOk: () => act(() => invoke<boolean>("sk_clip_clear"), "历史已清空"),
    });
  }, [summary, rows, act]);

  const purge = useCallback(() => {
    Modal.confirm({
      title: `清理 ${purgeDays} 天前的历史`,
      content: (
        <Typography.Paragraph style={{ whiteSpace: "pre-wrap" }}>
          {purgeConfirmText(purgeDays)}
        </Typography.Paragraph>
      ),
      okText: "清理",
      okButtonProps: { danger: true },
      cancelText: "取消",
      onOk: async () => {
        setBusy(true);
        try {
          const n = await invoke<number>("sk_clip_purge", { days: purgeDays });
          message.success(n > 0 ? `已清理 ${n} 条` : "没有需要清理的条目");
          await load();
        } catch (e) {
          message.error(String(e));
        } finally {
          setBusy(false);
        }
      },
    });
  }, [purgeDays, load]);

  const evicting = useMemo(
    () => (summary ? evictionCandidates(rows, summary.max_items) : 0),
    [rows, summary],
  );

  return (
    <Space direction="vertical" size={10} style={{ width: "100%" }}>
      <Alert
        type="info"
        showIcon
        message="剪贴板监听"
        description={pauseHint(summary?.watch_paused ?? false, summary?.watch_enabled ?? false)}
        action={
          <Button
            size="small"
            type={summary?.watch_enabled ? "default" : "primary"}
            onClick={watchToggleModal}
          >
            {watchToggleLabel(summary?.watch_enabled ?? false, summary?.watch_paused ?? false)}
          </Button>
        }
      />

      <Space wrap>
        <Input
          size="small"
          style={{ width: 240 }}
          allowClear
          placeholder="搜索历史…"
          value={keyword}
          onChange={(e) => setKeyword(e.target.value)}
        />
        <Select
          size="small"
          style={{ width: 120 }}
          value={kind}
          onChange={setKind}
          options={KIND_FILTERS.map((k) => ({ label: k.label, value: k.key }))}
        />
        <Button size="small" onClick={() => void load()}>
          刷新
        </Button>
        <span className="sk-hint">{clipSummaryText(summary)}</span>
        {evicting > 0 && (
          <Tooltip title="超出上限的非置顶条目会在新内容进来时被淘汰；置顶条目不受影响">
            <Tag color="orange">将有 {evicting} 条被淘汰</Tag>
          </Tooltip>
        )}
      </Space>

      <Space wrap>
        <span className="sk-hint">清理天数</span>
        <InputNumber
          size="small"
          min={1}
          max={3650}
          value={purgeDays}
          onChange={(v) => setPurgeDays(Number(v ?? 30))}
        />
        <Button size="small" danger loading={busy} onClick={purge}>
          清理更早的历史
        </Button>
        <Button size="small" danger loading={busy} onClick={clearAll}>
          清空历史
        </Button>
      </Space>

      {rows.length === 0 ? (
        <Empty
          image={Empty.PRESENTED_IMAGE_SIMPLE}
          description={keyword || kind ? "没有匹配的条目" : "剪贴板历史为空"}
        />
      ) : (
        <Table<ClipRow>
          size="small"
          rowKey="idx"
          pagination={{ pageSize: 20, showSizeChanger: false }}
          dataSource={rows}
          onRow={(r) => ({ onDoubleClick: () => copyBack(r) })}
          columns={[
            {
              title: "时间",
              dataIndex: "ts",
              width: 150,
              render: (v: number) => (
                <Tooltip title={formatTs(v)}>
                  <span className="sk-hint">{relativeTime(v, nowMs)}</span>
                </Tooltip>
              ),
            },
            {
              title: "类型",
              dataIndex: "kind",
              width: 90,
              render: (v: string) => <Tag color={kindColor(v)}>{kindLabel(v)}</Tag>,
            },
            {
              title: "内容",
              dataIndex: "preview",
              ellipsis: true,
              render: (v: string, r) => (
                <Space size={6}>
                  {r.pinned && <Tag color="gold">置顶</Tag>}
                  <span>{v || r.text || (r.kind === "image" ? "[图片]" : "—")}</span>
                </Space>
              ),
            },
            {
              title: "操作",
              width: 250,
              render: (_: unknown, r) => {
                const save = canSaveAs(r);
                return (
                  <Space size={4}>
                    <Button size="small" type="link" onClick={() => copyBack(r)}>
                      复制
                    </Button>
                    <Button size="small" type="link" onClick={() => togglePin(r)}>
                      {r.pinned ? "取消置顶" : "置顶"}
                    </Button>
                    {save.ok ? (
                      <Button size="small" type="link" onClick={() => void saveAs(r)}>
                        另存为
                      </Button>
                    ) : (
                      <Tooltip title={save.reason}>
                        <span className="sk-hint">另存为</span>
                      </Tooltip>
                    )}
                    <Button size="small" type="link" danger onClick={() => remove(r)}>
                      删除
                    </Button>
                    {/* 内容规则动作：识别这段文本能做什么（URL/JSON/颜色/路径/IP/域名） */}
                    <Button
                      size="small"
                      type="link"
                      loading={detecting === r.idx}
                      onClick={() => void detect(r)}
                    >
                      动作
                    </Button>
                  </Space>
                );
              },
            },
          ]}
        />
      )}

      {detected && shouldShowActions(detected) && (
        <Alert
          type="info"
          showIcon
          message={`可执行动作 · ${actionsSummary(detected)}`}
          description={
            <Space wrap size={6}>
              {detected.actions.map((a) => (
                <Tooltip key={a.id} title={actionHint(a)}>
                  <Button
                    size="small"
                    danger={actionColor(a.id) === "red"}
                    type={actionColor(a.id) === "default" ? "default" : "primary"}
                    loading={running === a.id}
                    disabled={detectedFor !== -1 && !rows.some((x) => x.idx === detectedFor)}
                    onClick={() => void runAction(a)}
                  >
                    {a.label}
                  </Button>
                </Tooltip>
              ))}
              <Button size="small" type="text" onClick={() => setDetected(null)}>
                收起
              </Button>
            </Space>
          }
        />
      )}

      {qrOf && (
        <Alert
          type="success"
          showIcon
          message="二维码"
          description={
            <Space direction="vertical" size={6}>
              {/* 本地渲染（后端 qrcode crate），不调用任何第三方二维码服务：
                  你复制的链接不出本机。白底是扫码可靠性的硬要求（深色主题下
                  静区必须仍是白的）。 */}
              <Typography.Text copyable={{ text: qrOf }}>{qrOf}</Typography.Text>
              {qrSvg ? (
                <div style={{ background: "#fff", padding: 8, width: 296 }}>
                  <img
                    src={"data:image/svg+xml;charset=utf-8," + encodeURIComponent(qrSvg)}
                    width={280}
                    height={280}
                    alt="二维码"
                  />
                </div>
              ) : (
                <Typography.Text type="secondary" className="sk-hint">
                  生成中…
                </Typography.Text>
              )}
              <Space size={8}>
                <Button size="small" onClick={saveQrPng} disabled={!qrSvg}>
                  保存图片
                </Button>
                <Button size="small" onClick={() => setQrOf("")}>
                  收起
                </Button>
              </Space>
            </Space>
          }
        />
      )}

      {colorOf && (
        <Alert
          type="success"
          showIcon
          message="色块"
          description={
            <Space direction="vertical" size={6}>
              <div
                style={{
                  width: "100%",
                  height: 64,
                  borderRadius: 6,
                  background: normalizeHex(colorOf) || "#000",
                  border: "1px solid #8884",
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "center",
                  color: isLightColor(colorOf) ? "#000" : "#fff",
                  fontFamily: "Consolas, monospace",
                }}
              >
                {normalizeHex(colorOf) || colorOf}
              </div>
              <Space size={10} wrap>
                <Typography.Text copyable={{ text: normalizeHex(colorOf) }} className="sk-hint">
                  HEX: {normalizeHex(colorOf)}
                </Typography.Text>
                <Typography.Text copyable={{ text: formatColor(colorOf, "RGB") }} className="sk-hint">
                  RGB: {formatColor(colorOf, "RGB")}
                </Typography.Text>
                <Button size="small" onClick={() => setColorOf("")}>
                  收起
                </Button>
              </Space>
            </Space>
          }
        />
      )}

      <Typography.Paragraph type="secondary" className="sk-hint" style={{ marginBottom: 0 }}>
        双击行可快速复制回剪贴板。置顶条目不会被自动淘汰，且相同文本再次复制时不会新增条目。
      </Typography.Paragraph>
    </Space>
  );
}



