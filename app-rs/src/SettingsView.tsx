/**
 * 设置 / 日志页（M7；信息架构重构：双栏导航 + 吸顶工具条 + 失焦自动保存）。
 *
 * 设置项由后端 `describe_all()` 驱动（`defaults()` 是键清单的唯一来源），
 * 本页按类型渲染控件；**范围校验后端才是权威**，前端只做即时反馈。
 *
 * v1.4 UI 迁移：脱离 AntD，交互件来自 src/ui/（Toast/Confirm 由 ui 层承担）。
 */
import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import {
  ALL_CATEGORY,
  categoryBadge,
  categoryEntries,
  changedCount,
  filterSettings,
  groupSettings,
  humanLogSize,
  isChanged,
  isHotkeyKey,
  isUnwired,
  ladderSummary,
  monitorNeedsAttention,
  monitorStateLabel,
  logLevel,
  settingKeyTip,
  tailSummary,
  unwiredBadge,
  unwiredCount,
  validateValue,
  type LogList,
  type MonitorPreview,
  type MonitorState,
  type LogTail,
  type SettingItem,
  type SettingsAll,
} from "./settingsPage";
import {
  Badge,
  Button,
  Card,
  CardBody,
  CardHeader,
  CardTitle,
  Empty,
  Input,
  cx,
} from "./ui";
import { Alert } from "./ui/alert";
import { InputNumber } from "./ui/input-number";
import { Select } from "./ui/select";
import { Switch } from "./ui";
import { Tabs } from "./ui/tabs";
import { Tooltip } from "./ui/tooltip";
import { useToast } from "./ui/toast";
import { Modal } from "./ui/modal";

/** 热键回退事件里的键名 → 界面名称（与 settings.rs Decoration label 对齐）。 */
const HOTKEY_LABELS: Record<string, string> = {
  palette_hotkey: "命令面板热键",
  clip_paste_hotkey: "快捷粘贴面板热键",
  clip_quick_hotkey: "快速粘贴热键",
};

/**
 * 单行设置项：开关/下拉切换即存，文本/数字失焦即存——不再有逐行「保存」按钮。
 *
 * 为什么自动保存：设置项都是小粒度偏好，「改完还得找保存」是纯负担；blur 是
 * 「这一项我改完了」的自然信号。校验不过时不落盘，只给行内红字（不弹全局
 * 警告，避免打断输入过程）——后端仍是权威，这里只是即时反馈。
 */
function SettingRow({
  item,
  onSave,
}: {
  item: SettingItem;
  onSave: (key: string, value: unknown, desc: string) => Promise<void>;
}) {
  const [draft, setDraft] = useState<unknown>(item.current);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    setDraft(item.current);
  }, [item.current]);

  const check = validateValue(item, draft);
  const dirty = JSON.stringify(draft) !== JSON.stringify(item.current);

  const commit = useCallback(
    async (value: unknown) => {
      const v = validateValue(item, value);
      if (!v.ok) return; // 行内红字已常驻提示，无需再打断
      setBusy(true);
      try {
        await onSave(item.key, v.coerced, item.label);
      } finally {
        setBusy(false);
      }
    },
    [item, onSave],
  );

  // 文本/数字的提交时机 = 失焦。纯聚焦不改值不写盘（避免无谓写入），非法值不写盘。
  const commitOnBlur = useCallback(() => {
    if (dirty && check.ok) void commit(draft);
  }, [dirty, check.ok, commit, draft]);

  // 数字受控值：null/空串必须显示为空——Number(null) 是 0，
  // 直接换算会把「正在清空」瞬间变成 0（旧实现的小毛病，顺手修掉）。
  const numValue =
    typeof draft === "number"
      ? draft
      : draft === null || draft === undefined || draft === ""
        ? null
        : Number(draft);

  return (
    // 底层键名不再平铺在行尾（全局撤下，语义见 settingKeyTip）：悬浮整行才显示。
    // 0.3s 延迟是防骚扰阀门——快速划过多行去点控件时，Tooltip 不应连续弹出。
    <Tooltip title={settingKeyTip(item)}>
      <tr>
        <td className="min-w-[200px] max-w-[40%] p-2 align-top">
          <div className="flex flex-col gap-0.5">
            <div className="flex flex-wrap items-center gap-1.5">
              <span>{item.label}</span>
              {isChanged(item) && <Badge tone="warn">已改动</Badge>}
              {isUnwired(item) && (
                <Tooltip title={item.unwired_reason}>
                  <span>
                    <Badge tone="neutral" className="border-dashed">
                      {unwiredBadge(item)}
                    </Badge>
                  </span>
                </Tooltip>
              )}
              {item.needs_restart && (
                <Tooltip title="该设置需要重启 SysKit 才生效">
                  <span>
                    <Badge tone="neutral">重启生效</Badge>
                  </span>
                </Tooltip>
              )}
            </div>
            <span className="text-xs text-sk-sub">{item.description}</span>
          </div>
        </td>
        <td className="min-w-[260px] p-2 align-top">
          <div className="flex flex-wrap items-center gap-2">
            {item.ty === "bool" ? (
              <Switch
                aria-label={item.label}
                checked={Boolean(draft)}
                disabled={busy}
                onChange={(v) => {
                  setDraft(v);
                  void commit(v);
                }}
              />
            ) : item.options ? (
              <Select
                className="w-[180px]"
                value={String(draft ?? "")}
                onChange={(v) => {
                  setDraft(v);
                  void commit(v);
                }}
                options={item.options.map((o) => ({ label: o, value: o }))}
              />
            ) : item.ty === "int" ? (
              <InputNumber
                min={item.min}
                max={item.max}
                value={numValue}
                onChange={(v) => setDraft(v)}
                onBlur={commitOnBlur}
              />
            ) : (
              <Input
                className="w-[240px]"
                value={String(draft ?? "")}
                onChange={(e) => setDraft(e.target.value)}
                onBlur={commitOnBlur}
              />
            )}
          </div>
          {!check.ok && (
            <div className="mt-1 text-xs text-sk-err">{check.reason}</div>
          )}
        </td>
      </tr>
    </Tooltip>
  );
}

function SettingsPanel() {
  const toast = useToast();
  const [all, setAll] = useState<SettingsAll | null>(null);

  const load = useCallback(async () => {
    try {
      setAll(await invoke<SettingsAll>("sk_settings_all"));
    } catch (e) {
      toast.error(`读取设置失败：${String(e)}`);
    }
  }, [toast]);

  useEffect(() => {
    void load();
  }, [load]);

  // 热键被占回退：后端已持久化实际生效值，这里刷新界面并明示（对齐 Python 弹窗语义）
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    void (async () => {
      const { listen } = await import("@tauri-apps/api/event");
      unlisten = await listen<{ key: string; value: string }>(
        "sk://hotkey-reverted",
        (e) => {
          const name = HOTKEY_LABELS[e.payload.key] ?? e.payload.key;
          toast.info(
            e.payload.value
              ? `热键已被其他程序占用，「${name}」已回退为「${e.payload.value}」`
              : `热键已被其他程序占用，「${name}」已禁用（可在设置页改为其他组合）`,
          );
          void load();
        },
      );
    })();
    return () => unlisten?.();
  }, [load, toast]);

  const save = useCallback(
    async (key: string, value: unknown, desc: string) => {
      try {
        await invoke("sk_settings_set", { key, value });
        toast.success(`已自动保存：${desc}`);
        // 热键类设置已即时重挂（sk_settings_set → apply_hotkeys）：通知侧栏
        // 速查刷新，与注册链/设置页三方一致。注意这里不带值——保存的请求值
        // 可能因被占被后端回退，权威值走 sk://hotkey-reverted 或重读设置。
        if (isHotkeyKey(key)) {
          window.dispatchEvent(new CustomEvent("sk:hotkeys-changed"));
        }
        await load();
      } catch (e) {
        // 后端拒绝的原因原样展示（范围/枚举/未知键）
        toast.error(String(e));
      }
    },
    [load, toast],
  );

  const items = all?.items ?? [];

  // 工具条过滤三件套：搜索词 / 只看已改动 / 左侧选中分类
  const [query, setQuery] = useState("");
  const [onlyChanged, setOnlyChanged] = useState(false);
  const [category, setCategory] = useState<string>(ALL_CATEGORY);

  const grouped = useMemo(
    () => groupSettings(filterSettings(items, { category, onlyChanged, query })),
    [items, category, onlyChanged, query],
  );
  const changed = changedCount(items);
  const unwired = unwiredCount(items);

  // 左侧导航条目：计数基于**全部**设置算（不随右侧过滤变化），保持稳定锚点
  const navItems = useMemo(() => {
    const head = { key: ALL_CATEGORY, label: "全部", total: items.length, changed };
    return [
      head,
      ...categoryEntries(items).map((e) => ({
        key: e.group,
        label: e.group,
        total: e.total,
        changed: e.changed,
      })),
    ];
  }, [items, changed]);

  const [resetOpen, setResetOpen] = useState(false);
  const reset = useCallback(() => setResetOpen(true), []);

  const doReset = useCallback(async () => {
    setResetOpen(false);
    try {
      toast.success(await invoke<string>("sk_settings_reset"));
      // 重置会改动热键三键且后端已即时重挂（sk_settings_reset →
      // apply_hotkeys）：通知侧栏速查刷新，与保存路径同一三方一致语义
      // （轮次37）。
      window.dispatchEvent(new CustomEvent("sk:hotkeys-changed"));
      await load();
    } catch (e) {
      toast.error(String(e));
    }
  }, [load, toast]);

  if (items.length === 0) {
    return <Empty description="正在读取设置…" />;
  }

  // 空态文案要讲清是哪个过滤条件把列表清空的（组合过滤时按"卡人"的顺序说）
  const emptyText = query.trim()
    ? onlyChanged
      ? `没有匹配「${query.trim()}」的已改动项`
      : `没有匹配「${query.trim()}」的设置`
    : onlyChanged
      ? "没有已改动的设置项"
      : "该分类下没有设置项";

  return (
    <div>
      {unwired > 0 && (
        <Alert
          tone="info"
          className="mb-2.5"
          title={`有 ${unwired} 项设置暂未生效`}
        >
          这些项会被正常保存，但 Rust 版还没有对应的实现，改了不会有实际效果（点各项的「暂未生效」标签看原因）。保留它们是为了让设置文件与 Python 版保持同构，迁移期不会丢配置。
        </Alert>
      )}

      {/* 工具条吸顶：滚动容器是 Layout.Content（overflow:auto），sticky 相对它生效；
          背景用不透明 token 色——内容从底下滚过时数字/按钮必须仍可读 */}
      <div
        className="sticky top-0 z-10 mb-2.5 flex flex-wrap items-center gap-2 rounded-[8px] border border-sk-glass bg-sk-card px-3 py-2"
      >
        <span className="sk-hint whitespace-nowrap">共 {all?.count ?? 0} 项</span>
        {/* 已改动计数即过滤器：点亮后右侧只剩与默认值不同的项 */}
        {changed > 0 ? (
          <Tooltip title="点击只显示与默认值不同的设置项">
            <Button
              size="sm"
              variant={onlyChanged ? "primary" : "default"}
              onClick={() => setOnlyChanged((v) => !v)}
            >
              {changed} 项已改动
            </Button>
          </Tooltip>
        ) : (
          <span className="sk-hint whitespace-nowrap">全部为默认值</span>
        )}
        <Input
          className="w-[220px]"
          placeholder="搜索设置：名称 / 键名 / 描述 / 值…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <Button size="sm" onClick={() => void load()}>
          重新读取
        </Button>
        <Button size="sm" variant="danger" onClick={reset}>
          恢复默认
        </Button>
      </div>

      {/* 双栏：左侧分类导航（稳定顺序 + 语义化角标：总数 / 红色「已改 N」），右侧只渲染选中分类 */}
      <div className="flex items-start gap-2">
        <nav className="w-[168px] shrink-0">
          {navItems.map((n) => {
            const badge = categoryBadge(n);
            return (
              <button
                key={n.key}
                onClick={() => setCategory(n.key)}
                className={cx(
                  "flex w-full items-center gap-1.5 rounded-[8px] px-3 py-1.5 text-sm transition-colors duration-150",
                  n.key === category
                    ? "bg-sk-accent/12 font-medium text-sk-accent"
                    : "text-sk-text hover:bg-sk-hover",
                )}
              >
                <span className="min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap text-left">
                  {n.label}
                </span>
                {badge.changed ? (
                  <Tooltip title={`共 ${n.total} 项，其中 ${n.changed} 项与默认值不同`}>
                    <span>
                      <Badge tone="err" className="text-[11px]">
                        {badge.text}
                      </Badge>
                    </span>
                  </Tooltip>
                ) : (
                  <span className="text-[11px] text-sk-sub">{badge.text}</span>
                )}
              </button>
            );
          })}
        </nav>
        <div className="min-w-0 flex-1">
          {grouped.length === 0 ? (
            <Empty description={emptyText} />
          ) : (
            grouped.map((g) => (
              <Card key={g.group} className="mb-2.5">
                <CardHeader>
                  <CardTitle>{g.group}</CardTitle>
                </CardHeader>
                <CardBody>
                  <table className="w-full border-collapse">
                    <tbody>
                      {g.items.map((it) => (
                        <SettingRow key={it.key} item={it} onSave={save} />
                      ))}
                    </tbody>
                  </table>
                </CardBody>
              </Card>
            ))
          )}
        </div>
      </div>

      <Modal
        open={resetOpen}
        title="恢复默认设置"
        onClose={() => setResetOpen(false)}
        width={460}
        footer={
          <>
            <Button onClick={() => setResetOpen(false)}>取消</Button>
            <Button variant="danger" onClick={() => void doReset()}>
              恢复默认
            </Button>
          </>
        }
      >
        <div className="whitespace-pre-wrap">
          将把所有设置恢复为默认值（共 {all?.count ?? 0} 项）。
          {"\n\n"}
          动态键（如场景模式记录的进模式前音量）会保留，不受影响。
        </div>
      </Modal>
    </div>
  );
}


function LogPanel() {
  const toast = useToast();
  const [list, setList] = useState<LogList | null>(null);
  const [current, setCurrent] = useState<string>("");
  const [tail, setTail] = useState<LogTail | null>(null);
  const [lines, setLines] = useState(500);
  const [busy, setBusy] = useState(false);

  const loadList = useCallback(async () => {
    try {
      const l = await invoke<LogList>("sk_log_list");
      setList(l);
      if (!current && l.files.length > 0) setCurrent(l.files[0].name);
    } catch (e) {
      toast.error(`读取日志列表失败：${String(e)}`);
    }
  }, [current, toast]);

  useEffect(() => {
    void loadList();
  }, [loadList]);

  const loadTail = useCallback(async () => {
    if (!current) return;
    setBusy(true);
    try {
      setTail(await invoke<LogTail>("sk_log_tail", { name: current, lines }));
    } catch (e) {
      setTail(null);
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  }, [current, lines, toast]);

  useEffect(() => {
    void loadTail();
  }, [loadTail]);

  return (
    <div className="flex flex-col gap-2.5">
      <div className="flex flex-wrap items-center gap-2">
        <Select
          className="w-[260px]"
          value={current || ""}
          placeholder="选择日志文件"
          onChange={setCurrent}
          options={(list?.files ?? []).map((f) => ({
            label: `${f.name}（${humanLogSize(f.bytes)}）`,
            value: f.name,
          }))}
        />
        <span className="sk-hint">显示行数</span>
        <InputNumber min={50} max={5000} value={lines} onChange={(v) => setLines(Number(v ?? 500))} />
        <Button size="sm" loading={busy} onClick={() => void loadTail()}>
          刷新
        </Button>
        <Button size="sm" onClick={() => void loadList()}>
          重扫文件
        </Button>
        <span className="sk-hint">{list?.dir}</span>
      </div>

      {(list?.files.length ?? 0) === 0 ? (
        <Alert tone="info" title="没有日志文件">
          {list ? `日志目录：${list.dir}（还没有写入过日志）` : "正在扫描日志目录…"}
        </Alert>
      ) : (
        <Card>
          <CardHeader>
            <CardTitle>{tail ? tailSummary(tail) : "日志内容"}</CardTitle>
          </CardHeader>
          <CardBody>
            {tail ? (
              <pre className="m-0 max-h-[460px] overflow-auto font-mono text-xs leading-normal">
                {tail.lines.map((l, i) => (
                  <div
                    key={`${i}-${l.slice(0, 24)}`}
                    className={
                      // 语义 token：深浅主题各自调过对比度（写死 hex 在浅色下过浅）
                      logLevel(l) === "err"
                        ? "text-sk-err"
                        : logLevel(l) === "warn"
                          ? "text-sk-warn"
                          : "text-sk-sub"
                    }
                  >
                    {l || " "}
                  </div>
                ))}
              </pre>
            ) : (
              <Empty description="正在读取日志尾部…" />
            )}
            <div className="sk-hint mb-0 mt-2">按行数上限只读尾部。</div>
          </CardBody>
        </Card>
      )}
    </div>
  );
}

/** 设置页（设置 + 后台监控两个页签；日志 v1.4 起独立侧栏入口）。 */
export function SettingsView() {
  return (
    <Card>
      <Tabs
        items={[
          { key: "settings", label: "设置", children: <SettingsPanel /> },
          { key: "monitor", label: "后台监控", children: <MonitorPanel /> },
        ]}
      />
    </Card>
  );
}

/** 日志页（独立侧栏入口，v1.4 从「设置 / 日志」拆出）。
 *  LogPanel 自带一张内容卡，外层不再套卡——嵌套双卡是深色走查实锤的
 * 视觉噪音（双层玻璃描边 + 双层内边距）。 */
export function LogsView() {
  return <LogPanel />;
}

/** 后台监控 / 自动修复的只读视图与"立即检查"。 */
function MonitorPanel() {
  const toast = useToast();
  const [state, setState] = useState<MonitorState | null>(null);
  const [preview, setPreview] = useState<MonitorPreview | null>(null);

  const load = useCallback(async () => {
    try {
      const [s, p] = await Promise.all([
        invoke<MonitorState>("sk_monitor_state"),
        invoke<MonitorPreview>("sk_monitor_preview"),
      ]);
      setState(s);
      setPreview(p);
    } catch (e) {
      toast.error(`读取监控状态失败：${String(e)}`);
    }
  }, [toast]);

  useEffect(() => {
    void load();
    const t = window.setInterval(() => void load(), 5000);
    return () => window.clearInterval(t);
  }, [load]);

  const attention = monitorNeedsAttention(state?.state ?? "");
  const okState = state?.state === "ok";

  return (
    <div className="flex flex-col gap-2.5">
      <div className="flex flex-wrap items-center gap-2">
        <Badge tone={attention ? "warn" : okState ? "ok" : "neutral"}>
          {monitorStateLabel(state?.state ?? "")}
        </Badge>
        {state?.detail && <span className="sk-hint">{state.detail}</span>}
        <span className="sk-hint">{ladderSummary(preview)}</span>
        <span className="sk-hint">
          已执行自动修复 {state?.repair_count ?? 0} 次 · 连续失败 {state?.fail_streak ?? 0}
        </span>
        <Button size="sm" onClick={() => void load()}>
          刷新
        </Button>
        <Button
          size="sm"
          variant="primary"
          onClick={() =>
            void invoke("sk_monitor_force_check").then(() => toast.success("已请求立即检查"))
          }
        >
          立即检查
        </Button>
      </div>

      <Alert tone="info" title="自动修复只走固定白名单梯度">
        L1 刷 DNS → L2 + 起 NlaSvc / 续租 → L3 + 重启 WLAN / 断合网卡。L3 会短暂断网，因此受「三级修复冷却」限频，冷却内自动降级为 L2。其余改配置类动作只给建议、绝不自动执行。
      </Alert>

      {preview?.vpn_up && (
        <Alert tone="warning" title="检测到 VPN / 隧道在线">
          {preview.vpn_note}
        </Alert>
      )}

      <Card>
        <CardHeader>
          <CardTitle>当前梯度预览（不会执行）</CardTitle>
        </CardHeader>
        <CardBody>
          <div className="flex flex-col gap-1.5">
            <div className="flex flex-wrap items-center gap-2">
              <span className="sk-hint">上限 L{preview?.max_level ?? "—"}</span>
              <span className="sk-hint">间隔 {preview?.poll_interval_sec ?? "—"}s</span>
              <span className="sk-hint">阈值 {preview?.fail_threshold ?? "—"} 次</span>
              <span className="sk-hint">冷却 {preview?.cooldown_sec ?? "—"}s</span>
              {preview && !preview.enabled && <Badge tone="err">自动修复已关闭</Badge>}
            </div>
            <div className="flex flex-wrap items-center gap-1.5">
              {(preview?.ladder ?? []).map((id) => (
                <Badge key={id} tone="accent">
                  {id}
                </Badge>
              ))}
              {(preview?.skipped_by_vpn ?? []).map((id) => (
                <Tooltip key={id} title="VPN/隧道在线，该项会切断隧道，已跳过">
                  <span>
                    <Badge tone="neutral" className="line-through">
                      {id}
                    </Badge>
                  </span>
                </Tooltip>
              ))}
            </div>
            {(preview?.suggestions?.length ?? 0) > 0 && (
              <div className="sk-hint">
                修复无效时只会给建议（不自动执行）：
                {preview?.suggestions.map((s) => s.id).join("、")}
              </div>
            )}
          </div>
        </CardBody>
      </Card>
    </div>
  );
}
