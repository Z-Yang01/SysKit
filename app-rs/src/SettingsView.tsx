/**
 * 设置 / 日志页（M7）。
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
  Modal,
  Select,
  Space,
  Switch,
  Tabs,
  Tag,
  Tooltip,
  Typography,
  message,
} from "antd";

/** 热键回退事件里的键名 → 界面名称（与 settings.rs Decoration label 对齐）。 */
const HOTKEY_LABELS: Record<string, string> = {
  palette_hotkey: "命令面板热键",
  clip_paste_hotkey: "快捷粘贴面板热键",
  clip_quick_hotkey: "快速粘贴热键",
};

import {
  changedCount,
  displayValue,
  groupSettings,
  humanLogSize,
  isChanged,
  isHotkeyKey,
  isUnwired,
  groupKey,
  ladderSummary,
  monitorNeedsAttention,
  monitorStateLabel,
  logLevel,
  restartNote,
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

function SettingRow({
  item,
  onSave,
}: {
  item: SettingItem;
  onSave: (key: string, value: unknown) => Promise<void>;
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
      if (!v.ok) {
        message.warning(v.reason);
        return;
      }
      setBusy(true);
      try {
        await onSave(item.key, v.coerced);
      } finally {
        setBusy(false);
      }
    },
    [item, onSave],
  );

  return (
    <tr>
      <td style={{ padding: "6px 8px", verticalAlign: "top", minWidth: 180 }}>
        <Space direction="vertical" size={0}>
          <Space size={6}>
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
          <code className="sk-hint" style={{ fontSize: 11 }}>
            {item.key}
          </code>
        </Space>
      </td>
      <td style={{ padding: "6px 8px", verticalAlign: "top", minWidth: 220 }}>
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
          <Space size={6}>
            <InputNumber
              size="small"
              style={{ width: 120 }}
              min={item.min}
              max={item.max}
              value={typeof draft === "number" ? draft : Number(draft)}
              onChange={(v) => setDraft(v)}
            />
            <Button
              size="small"
              type="primary"
              disabled={!dirty || !check.ok}
              loading={busy}
              onClick={() => void commit(draft)}
            >
              保存
            </Button>
          </Space>
        ) : (
          <Space size={6}>
            <Input
              size="small"
              style={{ width: 260 }}
              value={String(draft ?? "")}
              onChange={(e) => setDraft(e.target.value)}
            />
            <Button
              size="small"
              type="primary"
              disabled={!dirty || !check.ok}
              loading={busy}
              onClick={() => void commit(draft)}
            >
              保存
            </Button>
          </Space>
        )}
        {!check.ok && (
          <div className="sk-hint" style={{ fontSize: 12, marginTop: 4 }}>
            <Typography.Text type="danger">{check.reason}</Typography.Text>
          </div>
        )}
        {item.ty === "bool" && (
          <div className="sk-hint" style={{ fontSize: 12 }}>
            当前：{displayValue(item)}
            {restartNote(item) ? ` · ${restartNote(item)}` : ""}
          </div>
        )}
      </td>
    </tr>
  );
}

function SettingsPanel() {
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
    async (key: string, value: unknown) => {
      try {
        await invoke("sk_settings_set", { key, value });
        message.success("已保存");
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
  // 设置搜索：24 项平铺难找——键名/描述/分组/当前值文本全参与匹配（大小写不敏感）
  const [query, setQuery] = useState("");
  const grouped = useMemo(() => {
    const q = query.trim().toLowerCase();
    const filtered = q
      ? items.filter(
          (it) =>
            it.key.toLowerCase().includes(q) ||
            (it.description ?? "").toLowerCase().includes(q) ||
            groupKey(it.key).toLowerCase().includes(q) ||
            String(displayValue(it)).toLowerCase().includes(q),
        )
      : items;
    return groupSettings(filtered);
  }, [items, query]);
  const changed = changedCount(items);
  const unwired = unwiredCount(items);

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

  return (
    <Space direction="vertical" size={10} style={{ width: "100%" }}>
      {unwired > 0 && (
        <Alert
          type="info"
          showIcon
          message={`有 ${unwired} 项设置暂未生效`}
          description="这些项会被正常保存，但 Rust 版还没有对应的实现，改了不会有实际效果（点各项的「暂未生效」标签看原因）。保留它们是为了让设置文件与 Python 版保持同构，迁移期不会丢配置。"
        />
      )}

      <Space>
        <span className="sk-hint">
          共 {all?.count ?? 0} 项
          {changed > 0 ? ` · ${changed} 项已改动` : " · 全部为默认值"}
        </span>
        <Input
          size="small"
          allowClear
          style={{ width: 220 }}
          placeholder="搜索设置：键名 / 描述 / 值…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <Button size="small" onClick={() => void load()}>
          重新读取
        </Button>
        <Button size="small" danger onClick={reset}>
          恢复默认
        </Button>
      </Space>

      {grouped.length === 0 && query.trim() && (
        <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description={`没有匹配「${query.trim()}」的设置`} />
      )}

      {grouped.map((g) => (
        <Card key={g.group} size="small" className="sk-card" title={g.group}>
          <table style={{ width: "100%", borderCollapse: "collapse" }}>
            <tbody>
              {g.items.map((it) => (
                <SettingRow key={it.key} item={it} onSave={save} />
              ))}
            </tbody>
          </table>
        </Card>
      ))}
    </Space>
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



