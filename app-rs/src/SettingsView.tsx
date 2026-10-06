/**
 * 设置 / 日志页（M7；信息架构重构：双栏导航 + 吸顶工具条 + 失焦自动保存）。
 *
 * 设置项由后端 `describe_all()` 驱动（`defaults()` 是键清单的唯一来源），
 * 本页按类型渲染控件；**范围校验后端才是权威**，前端只做即时反馈。
 */
import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Alert,
  Button,
  Card,
  Empty,
  Input,
  InputNumber,
  Menu,
  Modal,
  Select,
  Space,
  Switch,
  Tabs,
  Tag,
  Tooltip,
  Typography,
  message,
  theme,
  type MenuProps,
} from "antd";

/** 热键回退事件里的键名 → 界面名称（与 settings.rs Decoration label 对齐）。 */
const HOTKEY_LABELS: Record<string, string> = {
  palette_hotkey: "命令面板热键",
  clip_paste_hotkey: "快捷粘贴面板热键",
  clip_quick_hotkey: "快速粘贴热键",
};

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

  // InputNumber 的受控值：null/空串必须显示为空——Number(null) 是 0，
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
    <Tooltip title={settingKeyTip(item)} mouseEnterDelay={0.3}>
      <tr>
        <td style={{ padding: "8px 8px", verticalAlign: "top", minWidth: 200, width: "40%" }}>
          <Space direction="vertical" size={0}>
            <Space size={6} wrap>
              <span>{item.label}</span>
              {isChanged(item) && <Tag color="orange">已改动</Tag>}
              {isUnwired(item) && (
                <Tooltip title={item.unwired_reason}>
                  <Tag color="default" style={{ borderStyle: "dashed" }}>
                    {unwiredBadge(item)}
                  </Tag>
                </Tooltip>
              )}
              {item.needs_restart && (
                <Tooltip title="该设置需要重启 SysKit 才生效">
                  <Tag>重启生效</Tag>
                </Tooltip>
              )}
            </Space>
            <span className="sk-hint" style={{ fontSize: 12 }}>
              {item.description}
            </span>
          </Space>
        </td>
        <td style={{ padding: "8px 8px", verticalAlign: "top", minWidth: 260 }}>
          <div style={{ display: "flex", alignItems: "center", gap: 8, flexWrap: "wrap" }}>
            {item.ty === "bool" ? (
              <Switch
                size="small"
                checked={Boolean(draft)}
                loading={busy}
                onChange={(v) => {
                  setDraft(v);
                  void commit(v);
                }}
              />
            ) : item.options ? (
              <Select
                size="small"
                style={{ width: 180 }}
                value={String(draft ?? "")}
                onChange={(v) => {
                  setDraft(v);
                  void commit(v);
                }}
                options={item.options.map((o) => ({ label: o, value: o }))}
              />
            ) : item.ty === "int" ? (
              <InputNumber
                size="small"
                style={{ width: 80 }}
                min={item.min}
                max={item.max}
                value={numValue}
                onChange={(v) => setDraft(v)}
                onBlur={commitOnBlur}
              />
            ) : (
              <Input
                size="small"
                style={{ width: 240 }}
                value={String(draft ?? "")}
                onChange={(e) => setDraft(e.target.value)}
                onBlur={commitOnBlur}
              />
            )}
          </div>
          {!check.ok && (
            <div style={{ fontSize: 12, marginTop: 4 }}>
              <Typography.Text type="danger">{check.reason}</Typography.Text>
            </div>
          )}
        </td>
      </tr>
    </Tooltip>
  );
}

function SettingsPanel() {
  // token 取当前主题色（darkAlgorithm 已接）：吸顶条/导航的底色随深浅主题走
  const { token } = theme.useToken();
  const [all, setAll] = useState<SettingsAll | null>(null);

  const load = useCallback(async () => {
    try {
      setAll(await invoke<SettingsAll>("sk_settings_all"));
    } catch (e) {
      message.error(`读取设置失败：${String(e)}`);
    }
  }, []);

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
          message.warning(
            e.payload.value
              ? `热键已被其他程序占用，「${name}」已回退为「${e.payload.value}」`
              : `热键已被其他程序占用，「${name}」已禁用（可在设置页改为其他组合）`,
          );
          void load();
        },
      );
    })();
    return () => unlisten?.();
  }, [load]);

  const save = useCallback(
    async (key: string, value: unknown, desc: string) => {
      try {
        await invoke("sk_settings_set", { key, value });
        message.success(`已自动保存：${desc}`);
        // 热键类设置已即时重挂（sk_settings_set → apply_hotkeys）：通知侧栏
        // 速查刷新，与注册链/设置页三方一致。注意这里不带值——保存的请求值
        // 可能因被占被后端回退，权威值走 sk://hotkey-reverted 或重读设置。
        if (isHotkeyKey(key)) {
          window.dispatchEvent(new CustomEvent("sk:hotkeys-changed"));
        }
        await load();
      } catch (e) {
        // 后端拒绝的原因原样展示（范围/枚举/未知键）
        message.error(String(e));
      }
    },
    [load],
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

  // AntD Menu 自带 hover/选中态与深浅主题适配，比自己糊按钮省心
  const menuItems: MenuProps["items"] = navItems.map((n) => {
    // 角标语义化（categoryBadge）：默认灰字总数；有已改动项才换红色「已改 N」，
    // 「4/8」这种分数读不出"改了几个"，两态文案一眼即懂。红色徽标上保留悬浮
    // 提示回看总数——语义化不等于丢信息。
    const badge = categoryBadge(n);
    return {
      key: n.key,
      label: (
        <span style={{ display: "flex", alignItems: "center", gap: 6 }}>
          <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
            {n.label}
          </span>
          <span style={{ marginLeft: "auto", display: "inline-flex", flexShrink: 0 }}>
            {badge.changed ? (
              <Tooltip title={`共 ${n.total} 项，其中 ${n.changed} 项与默认值不同`}>
                <Tag color="error" style={{ marginInlineEnd: 0, fontSize: 11, lineHeight: "18px" }}>
                  {badge.text}
                </Tag>
              </Tooltip>
            ) : (
              <span className="sk-hint" style={{ fontSize: 11 }}>
                {badge.text}
              </span>
            )}
          </span>
        </span>
      ),
    };
  });

  const reset = useCallback(() => {
    Modal.confirm({
      title: "恢复默认设置",
      content: (
        <Typography.Paragraph style={{ whiteSpace: "pre-wrap" }}>
          将把所有设置恢复为默认值（共 {all?.count ?? 0} 项）。
          {"\n\n"}
          动态键（如场景模式记录的进模式前音量）会保留，不受影响。
        </Typography.Paragraph>
      ),
      okText: "恢复默认",
      okButtonProps: { danger: true },
      cancelText: "取消",
      onOk: async () => {
        try {
          message.success(await invoke<string>("sk_settings_reset"));
          // 重置会改动热键三键且后端已即时重挂（sk_settings_reset →
          // apply_hotkeys）：通知侧栏速查刷新，与保存路径同一三方一致语义
          // （轮次37）。
          window.dispatchEvent(new CustomEvent("sk:hotkeys-changed"));
          await load();
        } catch (e) {
          message.error(String(e));
        }
      },
    });
  }, [all, load]);

  if (items.length === 0) {
    return <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="正在读取设置…" />;
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
          type="info"
          showIcon
          style={{ marginBottom: 10 }}
          message={`有 ${unwired} 项设置暂未生效`}
          description="这些项会被正常保存，但 Rust 版还没有对应的实现，改了不会有实际效果（点各项的「暂未生效」标签看原因）。保留它们是为了让设置文件与 Python 版保持同构，迁移期不会丢配置。"
        />
      )}

      {/* 工具条吸顶：滚动容器是 Layout.Content（overflow:auto），sticky 相对它生效；
          背景用不透明 token 色——内容从底下滚过时数字/按钮必须仍可读 */}
      <div
        style={{
          position: "sticky",
          top: 0,
          zIndex: 10,
          display: "flex",
          alignItems: "center",
          gap: 8,
          flexWrap: "wrap",
          padding: "8px 12px",
          marginBottom: 10,
          borderRadius: 8,
          border: `1px solid ${token.colorBorderSecondary}`,
          background: token.colorBgContainer,
        }}
      >
        <span className="sk-hint" style={{ whiteSpace: "nowrap" }}>
          共 {all?.count ?? 0} 项
        </span>
        {/* 已改动计数即过滤器：点亮后右侧只剩与默认值不同的项 */}
        {changed > 0 ? (
          <Tooltip title="点击只显示与默认值不同的设置项">
            <Button
              size="small"
              type={onlyChanged ? "primary" : "default"}
              onClick={() => setOnlyChanged((v) => !v)}
            >
              {changed} 项已改动
            </Button>
          </Tooltip>
        ) : (
          <span className="sk-hint" style={{ whiteSpace: "nowrap" }}>
            全部为默认值
          </span>
        )}
        <Input
          size="small"
          allowClear
          style={{ width: 220 }}
          placeholder="搜索设置：名称 / 键名 / 描述 / 值…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <Button size="small" onClick={() => void load()}>
          重新读取
        </Button>
        <Button size="small" danger onClick={reset}>
          恢复默认
        </Button>
      </div>

      {/* 双栏：左侧分类导航（稳定顺序 + 语义化角标：总数 / 红色「已改 N」），右侧只渲染选中分类 */}
      <div style={{ display: "flex", gap: 8, alignItems: "flex-start" }}>
        <Menu
          mode="inline"
          inlineIndent={12}
          selectedKeys={[category]}
          onClick={({ key }) => setCategory(key)}
          items={menuItems}
          style={{
            width: 168,
            flexShrink: 0,
            background: "transparent",
            borderInlineEnd: "none",
          }}
        />
        <div style={{ flex: 1, minWidth: 0 }}>
          {grouped.length === 0 ? (
            <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description={emptyText} />
          ) : (
            grouped.map((g) => (
              <Card
                key={g.group}
                size="small"
                className="sk-card"
                title={g.group}
                style={{ marginBottom: 10 }}
              >
                <table style={{ width: "100%", borderCollapse: "collapse" }}>
                  <tbody>
                    {g.items.map((it) => (
                      <SettingRow key={it.key} item={it} onSave={save} />
                    ))}
                  </tbody>
                </table>
              </Card>
            ))
          )}
        </div>
      </div>
    </div>
  );
}

function LogPanel() {
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
      message.error(`读取日志列表失败：${String(e)}`);
    }
  }, [current]);

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
      message.error(String(e));
    } finally {
      setBusy(false);
    }
  }, [current, lines]);

  useEffect(() => {
    void loadTail();
  }, [loadTail]);

  return (
    <Space direction="vertical" size={10} style={{ width: "100%" }}>
      <Space wrap>
        <Select
          size="small"
          style={{ width: 260 }}
          value={current || undefined}
          placeholder="选择日志文件"
          onChange={setCurrent}
          options={(list?.files ?? []).map((f) => ({
            label: `${f.name}（${humanLogSize(f.bytes)}）`,
            value: f.name,
          }))}
        />
        <span className="sk-hint">显示行数</span>
        <InputNumber size="small" min={50} max={5000} value={lines} onChange={(v) => setLines(Number(v ?? 500))} />
        <Button size="small" loading={busy} onClick={() => void loadTail()}>
          刷新
        </Button>
        <Button size="small" onClick={() => void loadList()}>
          重扫文件
        </Button>
        <span className="sk-hint">{list?.dir}</span>
      </Space>

      {(list?.files.length ?? 0) === 0 ? (
        <Alert
          type="info"
          showIcon
          message="没有日志文件"
          description={list ? `日志目录：${list.dir}（还没有写入过日志）` : "正在扫描日志目录…"}
        />
      ) : (
        <Card size="small" className="sk-card" title={tail ? tailSummary(tail) : "日志内容"}>
          {tail ? (
            <pre
              style={{
                maxHeight: 460,
                overflow: "auto",
                fontSize: 12,
                lineHeight: 1.5,
                margin: 0,
                fontFamily: "Consolas, monospace",
              }}
            >
              {tail.lines.map((l, i) => (
                <div
                  key={`${i}-${l.slice(0, 24)}`}
                  style={{
                    color:
                      logLevel(l) === "err"
                        ? "#f85149"
                        : logLevel(l) === "warn"
                          ? "#d29922"
                          : undefined,
                  }}
                >
                  {l || " "}
                </div>
              ))}
            </pre>
          ) : (
            <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="正在读取日志尾部…" />
          )}
          <Typography.Paragraph type="secondary" className="sk-hint" style={{ marginTop: 8, marginBottom: 0 }}>
            只读取文件尾部（按行数上限）——日志可能很大，整读会卡住界面。
          </Typography.Paragraph>
        </Card>
      )}
    </Space>
  );
}

export function SettingsView() {
  return (
    <Card size="small" className="sk-card">
      <Tabs
        items={[
          { key: "settings", label: "设置", children: <SettingsPanel /> },
          { key: "monitor", label: "后台监控", children: <MonitorPanel /> },
          { key: "logs", label: "日志", children: <LogPanel /> },
        ]}
      />
    </Card>
  );
}

/** 后台监控 / 自动修复的只读视图与"立即检查"。 */
function MonitorPanel() {
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
      message.error(`读取监控状态失败：${String(e)}`);
    }
  }, []);

  useEffect(() => {
    void load();
    const t = window.setInterval(() => void load(), 5000);
    return () => window.clearInterval(t);
  }, [load]);

  return (
    <Space direction="vertical" size={10} style={{ width: "100%" }}>
      <Space wrap>
        <Tag
          color={
            monitorNeedsAttention(state?.state ?? "")
              ? "orange"
              : state?.state === "ok"
                ? "green"
                : "default"
          }
        >
          {monitorStateLabel(state?.state ?? "")}
        </Tag>
        {state?.detail && <span className="sk-hint">{state.detail}</span>}
        <span className="sk-hint">{ladderSummary(preview)}</span>
        <span className="sk-hint">
          已执行自动修复 {state?.repair_count ?? 0} 次 · 连续失败 {state?.fail_streak ?? 0}
        </span>
        <Button size="small" onClick={() => void load()}>
          刷新
        </Button>
        <Button
          size="small"
          type="primary"
          onClick={() =>
            void invoke("sk_monitor_force_check").then(() => message.success("已请求立即检查"))
          }
        >
          立即检查
        </Button>
      </Space>

      <Alert
        type="info"
        showIcon
        message="自动修复只走固定白名单梯度"
        description="L1 刷 DNS → L2 + 起 NlaSvc / 续租 → L3 + 重启 WLAN / 断合网卡。L3 会短暂断网，因此受「三级修复冷却」限频，冷却内自动降级为 L2。其余改配置类动作只给建议、绝不自动执行。"
      />

      {preview?.vpn_up && (
        <Alert type="warning" showIcon message="检测到 VPN / 隧道在线" description={preview.vpn_note} />
      )}

      <Card size="small" title="当前梯度预览（不会执行）">
        <Space direction="vertical" size={6} style={{ width: "100%" }}>
          <Space wrap>
            <span className="sk-hint">上限 L{preview?.max_level ?? "—"}</span>
            <span className="sk-hint">间隔 {preview?.poll_interval_sec ?? "—"}s</span>
            <span className="sk-hint">阈值 {preview?.fail_threshold ?? "—"} 次</span>
            <span className="sk-hint">冷却 {preview?.cooldown_sec ?? "—"}s</span>
            {preview && !preview.enabled && <Tag color="red">自动修复已关闭</Tag>}
          </Space>
          <Space wrap>
            {(preview?.ladder ?? []).map((id) => (
              <Tag key={id} color="blue">
                {id}
              </Tag>
            ))}
            {(preview?.skipped_by_vpn ?? []).map((id) => (
              <Tooltip key={id} title="VPN/隧道在线，该项会切断隧道，已跳过">
                <Tag color="default" style={{ textDecoration: "line-through" }}>
                  {id}
                </Tag>
              </Tooltip>
            ))}
          </Space>
          {(preview?.suggestions?.length ?? 0) > 0 && (
            <div className="sk-hint">
              修复无效时只会给建议（不自动执行）：
              {preview?.suggestions.map((s) => s.id).join("、")}
            </div>
          )}
        </Space>
      </Card>
    </Space>
  );
}



