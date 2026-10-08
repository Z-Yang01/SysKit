/**
 * 系统助手：剪贴板历史（对应 Python 侧 `ClipboardTab`）。
 *
 * 与快捷粘贴面板（`ClipPanel`）共用同一份历史与后端命令，但这里是完整管理界面：
 * 筛选、置顶、删除、清空、按天数清理、另存为。
 *
 * 布局（用户明确指令）：监听横幅一行化；工具栏 = 搜索 + 类型 Segmented（带计数）
 * + 刷新 + 清理下拉；列表用 AntD List——点击整行即复制回剪贴板，行操作图标化，
 * 除 📋 复制外全部收进行尾「···」菜单，悬停行时才浮现（平时 25% 透明度）。
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Alert,
  Button,
  Dropdown,
  Empty,
  Image as AntImage,
  Input,
  InputNumber,
  List,
  Modal,
  Segmented,
  Space,
  Tag,
  Tooltip,
  Typography,
  message,
  type MenuProps,
} from "antd";

import {
  clearConfirmText,
  clipSummaryText,
  evictionCandidates,
  formatTs,
  kindColor,
  kindFilterOptions,
  kindLabel,
  pauseHint,
  purgeConfirmText,
  relativeTime,
  rowMenuItems,
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
import { svgDataUrl } from "./qr";
import { ToolboxQr } from "./ToolboxQr";

/** 图片条目缩略图：按需拉取后端 base64 缩略图；AntD Image 点击放大（56px，规格放大自 44px）。 */
function ClipImageThumb({ index }: { index: number }) {
  const [src, setSrc] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let alive = true;
    invoke<string>("sk_clip_image_thumb", { index: index })
      .then((url) => {
        if (alive) setSrc(url);
      })
      .catch(() => {
        if (alive) setFailed(true);
      });
    return () => {
      alive = false;
    };
  }, [index]);

  if (failed) return <span className="sk-hint">图片缺失</span>;
  if (!src) return <span className="sk-hint">…</span>;
  return (
    <AntImage
      src={src}
      width={56}
      height={56}
      style={{ objectFit: "cover", borderRadius: 6 }}
      preview={{ mask: "预览" }}
    />
  );
}

/** RowMenuItem（纯数据）→ antd Menu items；分隔项转 divider，禁用项把原因拼进标签。 */
function toMenuItems(r: ClipRow): MenuProps["items"] {
  return rowMenuItems(r).map((it) =>
    it.key === "sep"
      ? { type: "divider" as const }
      : {
          key: it.key,
          label:
            it.disabled && it.disabledReason ? `${it.label}（${it.disabledReason}）` : it.label,
          danger: it.danger,
          disabled: it.disabled,
        },
  );
}

export function ClipboardHistoryPanel() {
  const [rows, setRows] = useState<ClipRow[]>([]);
  const [summary, setSummary] = useState<ClipSummary | null>(null);
  const [keyword, setKeyword] = useState("");
  // 防抖后的查询词：输入框即时回显，300ms 停顿才发后端（每键一发会连续打断）
  const [kind, setKind] = useState("");
  const [purgeDays, setPurgeDays] = useState(30);
  const [busy, setBusy] = useState(false);
  const [nowMs, setNowMs] = useState(() => Date.now());
  // 清理下拉的开关：受控是为了点「清理更早/清空历史」时先收起，再弹确认框，
  // 否则确认框取消后下拉仍悬在原处，像一次没生效的点击
  const [cleanupOpen, setCleanupOpen] = useState(false);
  // 内容规则动作：detected 是"当前展示的那一条"的检测结果
  const [detected, setDetected] = useState<DetectResult | null>(null);
  const [detectedFor, setDetectedFor] = useState(-1);
  const [running, setRunning] = useState("");
  const [qrOf, setQrOf] = useState("");
  const [qrSvg, setQrSvg] = useState("");
  // 图片条目「识别二维码」：>=-1 时弹结果卡（复用 ToolboxQr 的展示与动作）
  const [qrDecodeIndex, setQrDecodeIndex] = useState(-1);
  const [colorOf, setColorOf] = useState("");
  const [debouncedKeyword, setDebouncedKeyword] = useState("");
  // 全文预览：列表只显示两行折叠，长条目从这里看全（Python「预览」同职责）
  const [previewRow, setPreviewRow] = useState<ClipRow | null>(null);

  useEffect(() => {
    const t = setTimeout(() => setDebouncedKeyword(keyword), 300);
    return () => clearTimeout(t);
  }, [keyword]);

  const load = useCallback(async () => {
    try {
      const [list, sum] = await Promise.all([
        invoke<ClipRow[]>("sk_clip_list", {
          keyword: debouncedKeyword.trim() || null,
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
  }, [debouncedKeyword, kind]);

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
    img.src = svgDataUrl(qrSvg);
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

  // 首次拦截提示：从 0 → N 只提醒一次（之后 Tag 常驻，不重复打扰）
  const lastSkipped = useRef(0);
  useEffect(() => {
    const n = summary?.skipped_sensitive ?? 0;
    if (lastSkipped.current === 0 && n > 0) {
      message.info(`已拦截 ${n} 条疑似敏感内容（信用卡/身份证/密钥），未记入历史`);
    }
    lastSkipped.current = n;
  }, [summary?.skipped_sensitive]);

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
        "已复制",
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

  /** 复制全文：走 write_back 单源（suppress + 系统排除格式），回填不被记成新复制。 */
  const copyFullText = useCallback(async (row: ClipRow) => {
    try {
      const ok = await invoke<boolean>("sk_clip_write_back", { index: row.idx });
      if (ok) message.success("已复制");
      else message.error("复制失败：条目内容不可用");
    } catch (e) {
      message.error(String(e));
    }
  }, []);

  /** 检测行内文本的可执行动作（后端识别，前端呈现；结果条在列表下方）。 */
  const detect = useCallback(async (row: ClipRow) => {
    try {
      setDetected(await invoke<DetectResult>("sk_clip_detect", { text: row.text }));
      setDetectedFor(row.idx);
    } catch (e) {
      setDetected(null);
      message.error(String(e));
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

  /** 「···」菜单派发：key 与 cliphistory.rowMenuItems 一一对应。 */
  const onRowMenu = useCallback(
    (key: string, r: ClipRow) => {
      switch (key) {
        case "preview":
          setPreviewRow(r);
          break;
        case "pin":
          togglePin(r);
          break;
        case "save":
          void saveAs(r);
          break;
        case "actions":
          void detect(r);
          break;
        case "qrdecode":
          setQrDecodeIndex(r.idx);
          break;
        case "remove":
          remove(r);
          break;
        default:
          break;
      }
    },
    [togglePin, saveAs, detect, remove],
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

  const enabled = summary?.watch_enabled ?? false;
  const paused = summary?.watch_paused ?? false;

  return (
    <>
      {/* 行操作浮现样式：base.css 属并行改动域（本任务只许改 3 个文件），故在组件内
          注入一条作用域限定的小样式；:focus-within 让键盘 Tab 到行时操作同样可见。 */}
      <style>{`
        .sk-clip-row .sk-clip-row-actions { opacity: 0.25; transition: opacity 0.15s ease; }
        .sk-clip-row:hover .sk-clip-row-actions,
        .sk-clip-row:focus-within .sk-clip-row-actions { opacity: 1; }
      `}</style>

      <Space direction="vertical" size={10} style={{ width: "100%" }}>
        {/* 监听状态一行化：原大横幅独占一屏头部，状态+开关其实一行就够。
            pauseHint 的语义保留在行内文案里（暂停时说明"不补录"）。 */}
        <Space size={8} wrap align="center">
          <span className="sk-hint">
            {enabled
              ? paused
                ? pauseHint(true, true)
                : `监听中 · ${pauseHint(false, true)}`
              : "监听已关闭"}
          </span>
          {(summary?.skipped_sensitive ?? 0) > 0 && (
            <Tag color="orange">🛡 已拦截 {summary!.skipped_sensitive} 条敏感内容</Tag>
          )}
          <Button size="small" type={enabled ? "default" : "primary"} onClick={watchToggleModal}>
            {enabled && !paused ? "⏸" : "▶"} {watchToggleLabel(enabled, paused)}
          </Button>
        </Space>

        {/* 工具栏：搜索 + 类型 Segmented（计数直接取 summary，前端不重复统计）
            + 刷新 + 清理下拉（清理天数/清理更早/清空历史全收进来） */}
        <Space wrap size={[8, 6]} align="center">
          <Input
            size="small"
            style={{ width: 220 }}
            allowClear
            placeholder="搜索历史…"
            value={keyword}
            onChange={(e) => setKeyword(e.target.value)}
          />
          <Segmented
            size="small"
            value={kind}
            onChange={(v) => setKind(String(v))}
            options={kindFilterOptions(summary).map((k) => ({ label: k.label, value: k.key }))}
          />
          <Button size="small" onClick={() => void load()}>
            🔄 刷新
          </Button>
          <Dropdown
            trigger={["click"]}
            open={cleanupOpen}
            onOpenChange={setCleanupOpen}
            dropdownRender={() => (
              <div
                style={{
                  padding: 12,
                  width: 300,
                  background: "var(--sk-card)",
                  border: "1px solid var(--sk-border)",
                  borderRadius: 8,
                  display: "flex",
                  flexDirection: "column",
                  gap: 10,
                }}
              >
                <Space size={6} align="center">
                  <span className="sk-hint">清理天数</span>
                  <InputNumber
                    size="small"
                    min={1}
                    max={3650}
                    value={purgeDays}
                    onChange={(v) => setPurgeDays(Number(v ?? 30))}
                  />
                </Space>
                <Space size={6} wrap>
                  <Button
                    size="small"
                    danger
                    loading={busy}
                    onClick={() => {
                      setCleanupOpen(false);
                      purge();
                    }}
                  >
                    清理更早的历史
                  </Button>
                  <Button
                    size="small"
                    danger
                    loading={busy}
                    onClick={() => {
                      setCleanupOpen(false);
                      clearAll();
                    }}
                  >
                    清空历史
                  </Button>
                </Space>
                <Typography.Text type="secondary" className="sk-hint" style={{ fontSize: 12 }}>
                  两个操作都会先弹确认框再执行。
                </Typography.Text>
              </div>
            )}
          >
            <Button size="small">🧹 清理</Button>
          </Dropdown>
          <span className="sk-hint">{clipSummaryText(summary)}</span>
          {evicting > 0 && (
            <Tooltip title="超出上限的非置顶条目会在新内容进来时被淘汰；置顶条目不受影响">
              <Tag color="orange">将有 {evicting} 条被淘汰</Tag>
            </Tooltip>
          )}
        </Space>

        {rows.length === 0 ? (
          <Empty
            image={Empty.PRESENTED_IMAGE_SIMPLE}
            description={keyword || kind ? "没有匹配的条目" : "剪贴板历史为空"}
          />
        ) : (
          <List<ClipRow>
            rowKey="idx"
            size="small"
            pagination={{ pageSize: 20, size: "small", showSizeChanger: false }}
            dataSource={rows}
            renderItem={(r) => (
              <List.Item
                className="sk-clip-row"
                style={{
                  cursor: "pointer",
                  padding: "8px 10px",
                  gap: 10,
                  alignItems: "flex-start",
                  borderRadius: 8,
                }}
                // 点击整行 = 复制回剪贴板（sk_clip_write_back：写回不进历史）
                onClick={() => copyBack(r)}
              >
                {/* 左列：相对时间在上，类型 Tag 紧随其下 */}
                <div
                  style={{
                    width: 78,
                    flexShrink: 0,
                    display: "flex",
                    flexDirection: "column",
                    alignItems: "flex-start",
                    gap: 4,
                  }}
                >
                  <Tooltip title={formatTs(r.ts)}>
                    <span className="sk-hint" style={{ fontSize: 12 }}>
                      {relativeTime(r.ts, nowMs)}
                    </span>
                  </Tooltip>
                  <Tag color={kindColor(r.kind)} style={{ marginRight: 0 }}>
                    {kindLabel(r.kind)}
                  </Tag>
                </div>

                {/* 中列：内容。文本 line-clamp 2 行省略（悬停看前 500 字）；
                    图片 56px 缩略图，点击放大——stopPropagation 防止误触整行复制 */}
                <div style={{ flex: 1, minWidth: 0 }}>
                  {r.pinned && (
                    <Tooltip title="已置顶——不会被自动淘汰">
                      <Tag color="gold" style={{ marginBottom: 2 }}>
                        📌 已置顶
                      </Tag>
                    </Tooltip>
                  )}
                  {r.kind === "image" ? (
                    <span onClick={(e) => e.stopPropagation()}>
                      <ClipImageThumb index={r.idx} />
                    </span>
                  ) : (
                    <span
                      title={r.text.slice(0, 500)}
                      style={{
                        display: "-webkit-box",
                        WebkitBoxOrient: "vertical",
                        WebkitLineClamp: 2,
                        overflow: "hidden",
                        wordBreak: "break-all",
                      }}
                    >
                      {r.preview || r.text || "—"}
                    </span>
                  )}
                </div>

                {/* 右列：只留 📋 复制 + ··· 菜单（置顶/另存为/删除/动作全收进去）。
                    平时 25% 透明度、行悬停浮现（见顶部 style）；stopPropagation
                    防止点按钮时冒泡成"整行复制"（一次点击触发两次复制语义混乱）。 */}
                <Space
                  size={0}
                  className="sk-clip-row-actions"
                  style={{ flexShrink: 0 }}
                  onClick={(e) => e.stopPropagation()}
                >
                  <Tooltip title="复制">
                    <Button
                      size="small"
                      type="text"
                      aria-label="复制"
                      onClick={() => copyBack(r)}
                    >
                      📋
                    </Button>
                  </Tooltip>
                  <Dropdown
                    trigger={["click"]}
                    menu={{ items: toMenuItems(r), onClick: ({ key }) => onRowMenu(key, r) }}
                  >
                    <Button size="small" type="text" aria-label="更多操作">
                      ···
                    </Button>
                  </Dropdown>
                </Space>
              </List.Item>
            )}
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
                      src={svgDataUrl(qrSvg)}
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

        {/* 全文预览（对应 Python _show_text_preview）：只读文本 + 复制全文 + 关闭 */}
        <Modal
          open={previewRow !== null}
          title="剪贴板内容预览"
          footer={null}
          onCancel={() => setPreviewRow(null)}
          width={640}
        >
          <Typography.Paragraph
            style={{
              whiteSpace: "pre-wrap",
              wordBreak: "break-all",
              maxHeight: 420,
              overflowY: "auto",
              marginBottom: 16,
            }}
          >
            {previewRow?.text}
          </Typography.Paragraph>
          <Space style={{ display: "flex", justifyContent: "flex-end" }}>
            <Button size="small" onClick={() => previewRow && void copyFullText(previewRow)}>
              复制全文
            </Button>
            <Button size="small" onClick={() => setPreviewRow(null)}>
              关闭
            </Button>
          </Space>
        </Modal>

        {/* 图片条目识别二维码：复用工具箱的结果卡（WiFi 拆解/复制密码/打开链接） */}
        <Modal
          open={qrDecodeIndex >= 0}
          title="识别二维码"
          footer={null}
          onCancel={() => setQrDecodeIndex(-1)}
          width={560}
        >
          {qrDecodeIndex >= 0 && (
            <ToolboxQr decodeClipItem={qrDecodeIndex} onDone={() => setQrDecodeIndex(-1)} />
          )}
        </Modal>

        <Typography.Paragraph type="secondary" className="sk-hint" style={{ marginBottom: 0 }}>
          点击行即可复制回剪贴板（写回不会记进历史）；操作按钮在行悬停时浮现。置顶条目不会被自动淘汰，且相同文本再次复制时不会新增条目。
        </Typography.Paragraph>
      </Space>
    </>
  );
}
