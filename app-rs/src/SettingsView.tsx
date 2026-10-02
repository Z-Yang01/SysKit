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

import {
  changedCount,
  displayValue,
  groupSettings,
  humanLogSize,
  isChanged,
  isUnwired,
  logLevel,
  restartNote,
  tailSummary,
  unwiredBadge,
  unwiredCount,
  validateValue,
  type LogList,
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

  const save = useCallback(
    async (key: string, value: unknown) => {
      try {
        await invoke("sk_settings_set", { key, value });
        message.success("已保存");
        await load();
      } catch (e) {
        // 后端拒绝的原因原样展示（范围/枚举/未知键）
        message.error(String(e));
      }
    },
    [load],
  );

  const items = all?.items ?? [];
  const grouped = useMemo(() => groupSettings(items), [items]);
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
        <Button size="small" onClick={() => void load()}>
          重新读取
        </Button>
        <Button size="small" danger onClick={reset}>
          恢复默认
        </Button>
      </Space>

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
          { key: "logs", label: "日志", children: <LogPanel /> },
        ]}
      />
    </Card>
  );
}


