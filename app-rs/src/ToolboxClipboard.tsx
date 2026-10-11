/**
 * 系统助手：剪贴板历史（对应 Python 侧 `ClipboardTab`）。
 *
 * 与快捷粘贴面板（`ClipPanel`）共用同一份历史与后端命令，但这里是完整管理界面：
 * 筛选、置顶、删除、清空、按天数清理、另存为。
 *
 * 布局（用户明确指令）：监听横幅一行化；工具栏 = 搜索 + 类型 Segmented（带计数）
 * + 刷新 + 清理下拉；列表点击整行即复制回剪贴板，行操作图标化，
 * 除 📋 复制外全部收进行尾「···」菜单，悬停行时才浮现（平时 25% 透明度）。
 *
 * v1.4 UI 迁移：脱离 AntD（List/Modal/Dropdown/Image 预览 → ui 件 + 本地灯箱；
 * 分页 → 页内切片；copyable → 本地 CopyInline）。
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

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
import { Badge, Button, Dropdown, Empty, Input, Segmented, type MenuItem } from "./ui";
import { InputNumber } from "./ui/input-number";
import { Modal } from "./ui/modal";
import { Tooltip as UiTooltip } from "./ui/tooltip";
import { Alert } from "./ui/alert";
import { confirm } from "./ui/confirm";
import { useToast } from "./ui/toast";

const PAGE_SIZE = 20;

/** kindColor 的 antd 色名 → Badge tone（紫归 accent，本项目 token 无紫色）。 */
function kindTone(kind: string): "accent" | "ok" | "warn" | "neutral" {
  switch (kindColor(kind)) {
    case "blue":
    case "purple":
      return "accent";
    case "green":
      return "ok";
    case "orange":
      return "warn";
    default:
      return "neutral";
  }
}

/** AntD Typography.copyable 的替代：值 + 点击复制（短暂 ✓ 反馈）。 */
function CopyInline({ text, children }: { text: string; children: React.ReactNode }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      type="button"
      className="inline-flex cursor-pointer items-center gap-1 border-0 bg-transparent p-0 text-left text-sk-sub hover:text-sk-text"
      onClick={() => {
        void invoke("sk_clip_copy_plain", { text }).then(() => {
          setCopied(true);
          window.setTimeout(() => setCopied(false), 1200);
        });
      }}
    >
      {children}
      <span aria-hidden className="text-xs">
        {copied ? "✓" : "⧉"}
      </span>
    </button>
  );
}

/** 图片条目缩略图：按需拉取后端 base64 缩略图；点击开本地灯箱放大（56px）。 */
function ClipImageThumb({ index }: { index: number }) {
  const [src, setSrc] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  const [zoom, setZoom] = useState(false);

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
    <>
      <img
        src={src}
        width={56}
        height={56}
        style={{ objectFit: "cover" }}
        className="cursor-zoom-in rounded-md"
        alt="剪贴板图片"
        onClick={() => setZoom(true)}
      />
      {zoom && (
        <div
          className="fixed inset-0 z-[2200] flex items-center justify-center bg-black/70 p-8"
          onClick={() => setZoom(false)}
        >
          <img src={src} className="max-h-full max-w-full rounded-md" alt="剪贴板图片（放大）" />
        </div>
      )}
    </>
  );
}

/** RowMenuItem（纯数据）→ ui MenuItem；分隔项转 divider，禁用项把原因拼进标签。 */
function toMenuItems(r: ClipRow): MenuItem[] {
  return rowMenuItems(r).map((it) =>
    it.key === "sep"
      ? { key: "sep", type: "divider" as const, label: "" }
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
  const toast = useToast();
  const [rows, setRows] = useState<ClipRow[]>([]);
  const [summary, setSummary] = useState<ClipSummary | null>(null);
  const [keyword, setKeyword] = useState("");
  // 防抖后的查询词：输入框即时回显，300ms 停顿才发后端（每键一发会连续打断）
  const [kind, setKind] = useState("");
  const [purgeDays, setPurgeDays] = useState(30);
  const [busy, setBusy] = useState(false);
  const [nowMs, setNowMs] = useState(() => Date.now());
  const [page, setPage] = useState(0);
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
      toast.error(`读取剪贴板历史失败：${String(e)}`);
    }
  }, [debouncedKeyword, kind, toast]);

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
        toast.error(String(e));
        setQrOf("");
      });
    return () => {
      alive = false;
    };
  }, [qrOf, toast]);

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
      toast.success("已保存 clip_qr.png");
    } catch (e) {
      toast.error(`二维码导出失败：${String(e)}`);
    }
  }, [qrSvg, toast]);

  // 监听开关三态：关→开启（确认框）；开且未暂停→暂停（直接）；已暂停→恢复（确认不补录）
  const watchToggleModal = useCallback(() => {
    const enabled = summary?.watch_enabled ?? false;
    const paused = summary?.watch_paused ?? false;
    const apply = async (target: boolean) => {
      try {
        const r = await invoke<{ message: string }>("sk_clip_watch_set", {
          enabled: target,
        });
        toast.success(r.message);
        await load();
      } catch (e) {
        toast.error(String(e));
      }
    };
    if (!enabled) {
      confirm({
        title: "开启剪贴板监听",
        width: 540,
        content: <div className="whitespace-pre-wrap">{watchEnableConfirmText()}</div>,
        okText: "开启",
        onOk: () => apply(true),
      });
    } else if (paused) {
      confirm({
        title: "恢复记录",
        content: watchResumeConfirmText(),
        okText: "恢复",
        cancelText: "保持暂停",
        onOk: () => apply(true),
      });
    } else {
      void apply(false);
      toast.info("已暂停记录（历史保留）");
    }
  }, [summary, load, toast]);

  useEffect(() => {
    void load();
  }, [load]);

  // 首次拦截提示：从 0 → N 只提醒一次（之后 Tag 常驻，不重复打扰）
  const lastSkipped = useRef(0);
  useEffect(() => {
    const n = summary?.skipped_sensitive ?? 0;
    if (lastSkipped.current === 0 && n > 0) {
      toast.info(`已拦截 ${n} 条疑似敏感内容（信用卡/身份证/密钥），未记入历史`);
    }
    lastSkipped.current = n;
  }, [summary?.skipped_sensitive, toast]);

  const act = useCallback(
    async (fn: () => Promise<unknown>, okMsg: string) => {
      setBusy(true);
      try {
        await fn();
        toast.success(okMsg);
        await load();
      } catch (e) {
        toast.error(String(e));
      } finally {
        setBusy(false);
      }
    },
    [load, toast],
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

  const saveAs = useCallback(
    async (row: ClipRow) => {
      try {
        const msg = await invoke<string>("sk_clip_save_as", { index: row.idx });
        if (msg) toast.success(msg);
        // 空串 = 用户取消，保持安静
      } catch (e) {
        toast.error(String(e));
      }
    },
    [toast],
  );

  /** 复制全文：走 write_back 单源（suppress + 系统排除格式），回填不被记成新复制。 */
  const copyFullText = useCallback(
    async (row: ClipRow) => {
      try {
        const ok = await invoke<boolean>("sk_clip_write_back", { index: row.idx });
        if (ok) toast.success("已复制");
        else toast.error("复制失败：条目内容不可用");
      } catch (e) {
        toast.error(String(e));
      }
    },
    [toast],
  );

  /** 检测行内文本的可执行动作（后端识别，前端呈现；结果条在列表下方）。 */
  const detect = useCallback(
    async (row: ClipRow) => {
      try {
        setDetected(await invoke<DetectResult>("sk_clip_detect", { text: row.text }));
        setDetectedFor(row.idx);
      } catch (e) {
        setDetected(null);
        toast.error(String(e));
      }
    },
    [toast],
  );

  /** 执行一个动作：前端路由的自行处理，其余交给后端。 */
  const runAction = useCallback(
    async (a: ClipAction) => {
      const route = routeAction(a.id);
      if (route === "unsupported") {
        toast.info(`该动作尚未接入界面：${a.id}`);
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
        toast.info("归属地查询需要联网查询第三方服务，本版不代查以免泄露你的 IP");
        return;
      }
      setRunning(a.id);
      try {
        toast.success(summarizeResult(await invoke<string>("sk_clip_action_run", { id: a.id, arg: a.arg })));
      } catch (e) {
        toast.error(String(e));
      } finally {
        setRunning("");
      }
    },
    [toast],
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
    confirm({
      title: "清空剪贴板历史",
      width: 520,
      content: <div className="whitespace-pre-wrap">{clearConfirmText(count, pinned)}</div>,
      okText: "清空",
      danger: true,
      onOk: () => act(() => invoke<boolean>("sk_clip_clear"), "历史已清空"),
    });
  }, [summary, rows, act]);

  const purge = useCallback(() => {
    confirm({
      title: `清理 ${purgeDays} 天前的历史`,
      content: <div className="whitespace-pre-wrap">{purgeConfirmText(purgeDays)}</div>,
      okText: "清理",
      danger: true,
      onOk: async () => {
        setBusy(true);
        try {
          const n = await invoke<number>("sk_clip_purge", { days: purgeDays });
          toast.success(n > 0 ? `已清理 ${n} 条` : "没有需要清理的条目");
          await load();
        } catch (e) {
          toast.error(String(e));
        } finally {
          setBusy(false);
        }
      },
    });
  }, [purgeDays, load, toast]);

  const evicting = useMemo(
    () => (summary ? evictionCandidates(rows, summary.max_items) : 0),
    [rows, summary],
  );

  const enabled = summary?.watch_enabled ?? false;
  const paused = summary?.watch_paused ?? false;

  const pageCount = Math.max(1, Math.ceil(rows.length / PAGE_SIZE));
  const cur = Math.min(page, pageCount - 1);
  const pageRows = rows.slice(cur * PAGE_SIZE, cur * PAGE_SIZE + PAGE_SIZE);

  return (
    <>
      {/* 行操作浮现样式：base.css 属并行改动域，故在组件内注入一条作用域限定的
          小样式；:focus-within 让键盘 Tab 到行时操作同样可见。 */}
      <style>{`
        .sk-clip-row .sk-clip-row-actions { opacity: 0.25; transition: opacity 0.15s ease; }
        .sk-clip-row:hover .sk-clip-row-actions,
        .sk-clip-row:focus-within .sk-clip-row-actions { opacity: 1; }
      `}</style>

      <div className="flex flex-col gap-2.5">
        {/* 监听状态一行化：原大横幅独占一屏头部，状态+开关其实一行就够。
            pauseHint 的语义保留在行内文案里（暂停时说明"不补录"）。 */}
        <div className="flex flex-wrap items-center gap-2">
          <span className="sk-hint">
            {enabled
              ? paused
                ? pauseHint(true, true)
                : `监听中 · ${pauseHint(false, true)}`
              : "监听已关闭"}
          </span>
          {(summary?.skipped_sensitive ?? 0) > 0 && (
            <Badge tone="warn">🛡 已拦截 {summary!.skipped_sensitive} 条敏感内容</Badge>
          )}
          <Button
            size="sm"
            variant={enabled ? "default" : "primary"}
            onClick={watchToggleModal}
          >
            {enabled && !paused ? "⏸" : "▶"} {watchToggleLabel(enabled, paused)}
          </Button>
        </div>

        {/* 工具栏：搜索 + 类型 Segmented（计数直接取 summary，前端不重复统计）
            + 刷新 + 清理天数/清理下拉 */}
        <div className="flex flex-wrap items-center gap-x-2 gap-y-1.5">
          <Input
            className="h-7 w-[220px] text-xs"
            placeholder="搜索历史…"
            value={keyword}
            onChange={(e) => setKeyword(e.target.value)}
          />
          <Segmented
            size="sm"
            value={kind}
            onChange={setKind}
            options={kindFilterOptions(summary).map((k) => ({ label: k.label, value: k.key }))}
          />
          <Button size="sm" onClick={() => void load()}>
            🔄 刷新
          </Button>
          <span className="inline-flex items-center gap-1.5">
            <span className="sk-hint">清理天数</span>
            <InputNumber
              min={1}
              max={3650}
              value={purgeDays}
              onChange={(v) => setPurgeDays(Number(v ?? 30))}
            />
          </span>
          <Dropdown
            align="left"
            trigger={
              <Button size="sm" loading={busy}>
                🧹 清理
              </Button>
            }
            items={[
              { key: "purge", label: "清理更早的历史", danger: true },
              { key: "clear", label: "清空历史", danger: true },
            ]}
            onItemClick={(key) => {
              if (key === "purge") purge();
              else clearAll();
            }}
          />
          <span className="sk-hint">{clipSummaryText(summary)}</span>
          {evicting > 0 && (
            <UiTooltip title="超出上限的非置顶条目会在新内容进来时被淘汰；置顶条目不受影响">
              <Badge tone="warn">将有 {evicting} 条被淘汰</Badge>
            </UiTooltip>
          )}
        </div>

        {rows.length === 0 ? (
          <Empty description={keyword || kind ? "没有匹配的条目" : "剪贴板历史为空"} />
        ) : (
          <>
            <div className="flex flex-col">
              {pageRows.map((r) => (
                // 点击整行 = 复制回剪贴板（sk_clip_write_back：写回不进历史）
                <div
                  key={r.idx}
                  className="sk-clip-row flex cursor-pointer items-start gap-2.5 rounded-lg px-2.5 py-2 transition-colors hover:bg-sk-hover"
                  onClick={() => copyBack(r)}
                >
                  {/* 左列：相对时间在上，类型 Tag 紧随其下 */}
                  <div className="flex w-[78px] shrink-0 flex-col items-start gap-1">
                    <UiTooltip title={formatTs(r.ts)}>
                      <span className="sk-hint text-xs">{relativeTime(r.ts, nowMs)}</span>
                    </UiTooltip>
                    <Badge tone={kindTone(r.kind)}>{kindLabel(r.kind)}</Badge>
                  </div>

                  {/* 中列：内容。文本 line-clamp 2 行省略（悬停看前 500 字）；
                      图片 56px 缩略图，点击放大——stopPropagation 防止误触整行复制 */}
                  <div className="min-w-0 flex-1">
                    {r.pinned && (
                      <UiTooltip title="已置顶——不会被自动淘汰">
                        <Badge tone="warn" className="mb-0.5">
                          📌 已置顶
                        </Badge>
                      </UiTooltip>
                    )}
                    {r.kind === "image" ? (
                      <span onClick={(e) => e.stopPropagation()}>
                        <ClipImageThumb index={r.idx} />
                      </span>
                    ) : (
                      <span
                        title={r.text.slice(0, 500)}
                        className="break-all"
                        style={{
                          display: "-webkit-box",
                          WebkitBoxOrient: "vertical",
                          WebkitLineClamp: 2,
                          overflow: "hidden",
                        }}
                      >
                        {r.preview || r.text || "—"}
                      </span>
                    )}
                  </div>

                  {/* 右列：只留 📋 复制 + ··· 菜单（置顶/另存为/删除/动作全收进去）。
                      平时 25% 透明度、行悬停浮现（见顶部 style）；stopPropagation
                      防止点按钮时冒泡成"整行复制"（一次点击触发两次复制语义混乱）。 */}
                  <span
                    className="sk-clip-row-actions flex shrink-0"
                    onClick={(e) => e.stopPropagation()}
                  >
                    <UiTooltip title="复制">
                      <Button variant="ghost" size="sm" aria-label="复制" onClick={() => copyBack(r)}>
                        📋
                      </Button>
                    </UiTooltip>
                    <Dropdown
                      trigger={
                        <Button variant="ghost" size="sm" aria-label="更多操作">
                          ···
                        </Button>
                      }
                      items={toMenuItems(r)}
                      onItemClick={(key) => onRowMenu(key, r)}
                    />
                  </span>
                </div>
              ))}
            </div>
            {pageCount > 1 && (
              <div className="flex items-center justify-end gap-2 text-xs text-sk-sub">
                <span>
                  共 {rows.length} 条 · 第 {cur + 1}/{pageCount} 页
                </span>
                <Button size="sm" disabled={cur === 0} onClick={() => setPage(cur - 1)}>
                  上一页
                </Button>
                <Button size="sm" disabled={cur >= pageCount - 1} onClick={() => setPage(cur + 1)}>
                  下一页
                </Button>
              </div>
            )}
          </>
        )}

        {detected && shouldShowActions(detected) && (
          <Alert tone="info" title={`可执行动作 · ${actionsSummary(detected)}`}>
            <div className="flex flex-wrap items-center gap-1.5">
              {detected.actions.map((a) => (
                <UiTooltip key={a.id} title={actionHint(a)}>
                  <Button
                    size="sm"
                    variant={
                      actionColor(a.id) === "red"
                        ? "danger"
                        : actionColor(a.id) === "default"
                          ? "default"
                          : "primary"
                    }
                    loading={running === a.id}
                    disabled={detectedFor !== -1 && !rows.some((x) => x.idx === detectedFor)}
                    onClick={() => void runAction(a)}
                  >
                    {a.label}
                  </Button>
                </UiTooltip>
              ))}
              <Button variant="ghost" size="sm" onClick={() => setDetected(null)}>
                收起
              </Button>
            </div>
          </Alert>
        )}

        {qrOf && (
          <Alert tone="success" title="二维码">
            <div className="flex flex-col gap-1.5">
              {/* 本地渲染（后端 qrcode crate），不调用任何第三方二维码服务：
                  你复制的链接不出本机。白底是扫码可靠性的硬要求（深色主题下
                  静区必须仍是白的）。 */}
              <CopyInline text={qrOf}>
                <span className="break-all text-sk-text">{qrOf}</span>
              </CopyInline>
              {qrSvg ? (
                <div className="w-[296px] bg-white p-2">
                  <img src={svgDataUrl(qrSvg)} width={280} height={280} alt="二维码" />
                </div>
              ) : (
                <span className="sk-hint">生成中…</span>
              )}
              <div className="flex items-center gap-2">
                <Button size="sm" onClick={() => void saveQrPng()} disabled={!qrSvg}>
                  保存图片
                </Button>
                <Button size="sm" onClick={() => setQrOf("")}>
                  收起
                </Button>
              </div>
            </div>
          </Alert>
        )}

        {colorOf && (
          <Alert tone="success" title="色块">
            <div className="flex flex-col gap-1.5">
              <div
                className="flex h-16 w-full items-center justify-center rounded-md font-mono"
                style={{
                  background: normalizeHex(colorOf) || "#000",
                  border: "1px solid #8884",
                  color: isLightColor(colorOf) ? "#000" : "#fff",
                }}
              >
                {normalizeHex(colorOf) || colorOf}
              </div>
              <div className="flex flex-wrap items-center gap-2.5">
                <CopyInline text={normalizeHex(colorOf)}>
                  <span>HEX: {normalizeHex(colorOf)}</span>
                </CopyInline>
                <CopyInline text={formatColor(colorOf, "RGB")}>
                  <span>RGB: {formatColor(colorOf, "RGB")}</span>
                </CopyInline>
                <Button size="sm" onClick={() => setColorOf("")}>
                  收起
                </Button>
              </div>
            </div>
          </Alert>
        )}

        {/* 全文预览（对应 Python _show_text_preview）：只读文本 + 复制全文 + 关闭 */}
        <Modal
          open={previewRow !== null}
          title="剪贴板内容预览"
          onClose={() => setPreviewRow(null)}
          width={640}
          footer={
            <>
              <Button
                size="sm"
                onClick={() => previewRow && void copyFullText(previewRow)}
              >
                复制全文
              </Button>
              <Button size="sm" onClick={() => setPreviewRow(null)}>
                关闭
              </Button>
            </>
          }
        >
          <div
            className="mb-4 max-h-[420px] overflow-y-auto break-all whitespace-pre-wrap"
          >
            {previewRow?.text}
          </div>
        </Modal>

        {/* 图片条目识别二维码：复用工具箱的结果卡（WiFi 拆解/复制密码/打开链接） */}
        <Modal
          open={qrDecodeIndex >= 0}
          title="识别二维码"
          onClose={() => setQrDecodeIndex(-1)}
          width={560}
        >
          {qrDecodeIndex >= 0 && (
            <ToolboxQr decodeClipItem={qrDecodeIndex} onDone={() => setQrDecodeIndex(-1)} />
          )}
        </Modal>

        {rows.length > 0 && <div className="sk-hint">点击行复制；悬停行浮现操作。</div>}
      </div>
    </>
  );
}
