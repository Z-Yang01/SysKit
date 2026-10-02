/**
 * Hosts 编辑页（对应 Python 侧 ui/toolbox_page.py::HostsTab）。
 *
 * 数据流：`sk_hosts_read` 取全量 → 内存增删切换 → `sk_hosts_write` 写回（自动备份）。
 * 后端在写回前会做编码护栏（UTF-16/非 UTF-8 一律拒绝），错误直接展示给用户。
 */
import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Alert,
  Button,
  Card,
  Empty,
  Input,
  Modal,
  Space,
  Switch,
  Table,
  Tag,
  Typography,
  message,
} from "antd";

import {
  hostsStats,
  mappingRows,
  parseAliases,
  rowText,
  saveWarnings,
  summarizeChanges,
  validateNewEntry,
  type HostEntry,
  type HostRow,
  type HostsInfo,
} from "./hosts";

export function ToolboxHosts() {
  const [entries, setEntries] = useState<HostEntry[]>([]);
  const [original, setOriginal] = useState<HostEntry[]>([]);
  const [info, setInfo] = useState<HostsInfo | null>(null);
  const [busy, setBusy] = useState(false);
  const [showComments, setShowComments] = useState(false);

  // 新增表单
  const [newIp, setNewIp] = useState("127.0.0.1");
  const [newHost, setNewHost] = useState("");
  const [newAliases, setNewAliases] = useState("");
  const [newComment, setNewComment] = useState("");

  const load = useCallback(async () => {
    try {
      const [list, meta] = await Promise.all([
        invoke<HostEntry[]>("sk_hosts_read"),
        invoke<HostsInfo>("sk_hosts_info"),
      ]);
      setEntries(list);
      setOriginal(list);
      setInfo(meta);
    } catch (e) {
      message.error(`读取 hosts 失败：${String(e)}`);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const stats = useMemo(() => hostsStats(entries), [entries]);
  const rows = useMemo(
    () => (showComments ? entries.map((e, idx) => ({ ...e, idx })) : mappingRows(entries)),
    [entries, showComments],
  );
  const changes = useMemo(() => summarizeChanges(original, entries), [original, entries]);
  const warnings = useMemo(() => saveWarnings(info), [info]);

  const dirty = changes.added + changes.removed + changes.toggled > 0;

  /** 新增（仅内存）。 */
  const add = useCallback(() => {
    const aliases = parseAliases(newAliases);
    const v = validateNewEntry(newIp, newHost, aliases);
    if (!v.ok) {
      v.errors.forEach((e) => message.error(e));
      return;
    }
    setEntries((prev) => [
      ...prev,
      {
        raw: "",
        kind: "entry",
        ip: newIp.trim(),
        host: newHost.trim(),
        aliases,
        comment: newComment.trim(),
        enabled: true,
        had_content: true,
      },
    ]);
    setNewHost("");
    setNewAliases("");
    setNewComment("");
  }, [newIp, newHost, newAliases, newComment]);

  const remove = useCallback((idx: number) => {
    setEntries((prev) => prev.filter((_, i) => i !== idx));
  }, []);

  const toggle = useCallback((idx: number) => {
    setEntries((prev) =>
      prev.map((e, i) =>
        i === idx && (e.kind === "entry" || e.kind === "disabled")
          ? {
              ...e,
              enabled: !e.enabled,
              kind: e.enabled ? ("disabled" as const) : ("entry" as const),
            }
          : e,
      ),
    );
  }, []);

  /** 写回（自动备份）。 */
  const save = useCallback(async () => {
    const c = summarizeChanges(original, entries);
    Modal.confirm({
      title: "写回 hosts 文件",
      width: 560,
      content: (
        <Space direction="vertical" size={6}>
          <Typography.Text>
            新增 {c.added} 条 · 删除 {c.removed} 条 · 切换 {c.toggled} 条
          </Typography.Text>
          <Typography.Text type="secondary" className="sk-hint">
            写入前会自动把原文件备份到备份目录；若检测到编码异常（UTF-16/非 UTF-8）会
            <b>拒绝写入</b>并保留原文件。
          </Typography.Text>
          {warnings.map((w) => (
            <Typography.Text key={w} type="warning" className="sk-hint">
              ⚠ {w}
            </Typography.Text>
          ))}
        </Space>
      ),
      okText: "写入",
      cancelText: "取消",
      onOk: async () => {
        setBusy(true);
        try {
          const r = await invoke<{ ok: boolean; backup: string; message: string }>(
            "sk_hosts_write",
            { entries, backup: true },
          );
          message.success(r.message);
          await load();
        } catch (e) {
          message.error(`写入失败（原文件已保留）：${String(e)}`);
        } finally {
          setBusy(false);
        }
      },
    });
  }, [entries, original, warnings, load]);

  const backupOnly = useCallback(async () => {
    try {
      const p = await invoke<string>("sk_hosts_backup");
      message.success(p ? `已备份: ${p}` : "hosts 文件不存在，无需备份");
    } catch (e) {
      message.error(`备份失败：${String(e)}`);
    }
  }, []);

  return (
    <Space direction="vertical" size={12} style={{ width: "100%" }}>
      {warnings.map((w) => (
        <Alert key={w} type="warning" showIcon message={w} />
      ))}

      <Card
        size="small"
        className="sk-card"
        title="hosts 文件"
        extra={
          <Space>
            <span className="sk-hint">{info?.path ?? "—"}</span>
            <Button size="small" onClick={() => void load()}>
              重新读取
            </Button>
            <Button size="small" onClick={() => void backupOnly()}>
              只备份
            </Button>
          </Space>
        }
      >
        <Space>
          <Tag color="green">启用 {stats.enabled}</Tag>
          <Tag>禁用 {stats.disabled}</Tag>
          <Tag color="default">注释/空行 {stats.comments}</Tag>
          <span className="sk-hint">共 {stats.total} 行</span>
          {dirty && <Tag color="orange">有未保存改动</Tag>}
          <span className="sk-hint">显示注释行</span>
          <Switch size="small" checked={showComments} onChange={setShowComments} />
          <Button type="primary" size="small" disabled={!dirty} loading={busy} onClick={() => void save()}>
            写回文件
          </Button>
        </Space>
      </Card>

      <Card size="small" className="sk-card" title="新增映射">
        <Space wrap>
          <Input
            size="small"
            style={{ width: 150 }}
            placeholder="IP（如 127.0.0.1）"
            value={newIp}
            onChange={(e) => setNewIp(e.target.value)}
          />
          <Input
            size="small"
            style={{ width: 190 }}
            placeholder="主机名（如 dev.local）"
            value={newHost}
            onChange={(e) => setNewHost(e.target.value)}
          />
          <Input
            size="small"
            style={{ width: 190 }}
            placeholder="别名（逗号分隔，可选）"
            value={newAliases}
            onChange={(e) => setNewAliases(e.target.value)}
          />
          <Input
            size="small"
            style={{ width: 160 }}
            placeholder="备注（可选）"
            value={newComment}
            onChange={(e) => setNewComment(e.target.value)}
          />
          <Button size="small" type="primary" onClick={add}>
            添加
          </Button>
        </Space>
      </Card>

      <Card size="small" className="sk-card" title="条目">
        {rows.length === 0 ? (
          <Empty
            image={Empty.PRESENTED_IMAGE_SIMPLE}
            description={info?.exists ? "hosts 里没有主机映射（可能全是注释）" : "hosts 文件不存在"}
          />
        ) : (
          <Table<HostRow>
            size="small"
            rowKey="idx"
            pagination={{ pageSize: 15 }}
            dataSource={rows}
            columns={[
              {
                title: "状态",
                dataIndex: "enabled",
                width: 90,
                render: (v: boolean, r) =>
                  r.kind === "comment" ? (
                    <Tag color="default">注释</Tag>
                  ) : v ? (
                    <Tag color="green">启用</Tag>
                  ) : (
                    <Tag color="orange">禁用</Tag>
                  ),
              },
              {
                title: "内容",
                render: (_: unknown, r) =>
                  r.kind === "comment" ? (
                    <span className="sk-hint">{r.raw || "#"}</span>
                  ) : (
                    <Space size={6}>
                      <span>{rowText(r)}</span>
                      {r.comment && <span className="sk-hint"># {r.comment}</span>}
                    </Space>
                  ),
              },
              {
                title: "操作",
                width: 150,
                render: (_: unknown, r) =>
                  r.kind === "comment" ? null : (
                    <Space size={4}>
                      <Button size="small" type="link" onClick={() => toggle(r.idx)}>
                        {r.enabled ? "禁用" : "启用"}
                      </Button>
                      <Button size="small" type="link" danger onClick={() => remove(r.idx)}>
                        删除
                      </Button>
                    </Space>
                  ),
              },
            ]}
          />
        )}
      </Card>
    </Space>
  );
}
