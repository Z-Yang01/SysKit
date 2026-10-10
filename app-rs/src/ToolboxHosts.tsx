/**
 * Hosts 编辑页（对应 Python 侧 ui/toolbox_page.py::HostsTab）。
 *
 * 数据流：`sk_hosts_read` 取全量 → 内存增删切换 → `sk_hosts_write` 写回（自动备份）。
 * 后端在写回前会做编码护栏（UTF-16/非 UTF-8 一律拒绝），错误直接展示给用户。
 *
 * v1.4 UI 迁移：脱离 AntD（Table 分页→页内切片；Tag/Modal.confirm/message → ui 件）。
 */
import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import {
  hostsStats,
  effectiveFilter,
  filterHostRows,
  isDuplicateMapping,
  mappingRows,
  parseAliases,
  rowText,
  saveWarnings,
  summarizeChanges,
  validateNewEntry,
  type HostEntry,
  type HostFilter,
  type HostRow,
  type HostsInfo,
} from "./hosts";
import { Badge, Button, Card, CardBody, CardHeader, CardTitle, Empty, Input, Segmented, Switch, Table, type Column } from "./ui";
import { Alert } from "./ui/alert";
import { confirm } from "./ui/confirm";
import { useToast } from "./ui/toast";

const PAGE_SIZE = 15;

export function ToolboxHosts() {
  const toast = useToast();
  const [entries, setEntries] = useState<HostEntry[]>([]);
  const [original, setOriginal] = useState<HostEntry[]>([]);
  const [info, setInfo] = useState<HostsInfo | null>(null);
  const [busy, setBusy] = useState(false);
  const [showComments, setShowComments] = useState(false);
  // 条目筛选：全部/启用/禁用（+注释行）与关键字。hosts 大文件上百行，没有筛选基本不可用
  const [filter, setFilter] = useState<HostFilter>("all");
  const [keyword, setKeyword] = useState("");
  const [page, setPage] = useState(0);

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
      toast.error(`读取 hosts 失败：${String(e)}`);
    }
  }, [toast]);

  useEffect(() => {
    void load();
  }, [load]);

  const stats = useMemo(() => hostsStats(entries), [entries]);
  const allRows = useMemo(
    () => (showComments ? entries.map((e, idx) => ({ ...e, idx })) : mappingRows(entries)),
    [entries, showComments],
  );
  // 实际生效的筛选：选中「注释」后关掉显示注释行，状态残留会让 Segmented
  // 无选中态且必然空表——回落到 all（重新打开时所选筛选还会回来）
  const activeFilter = effectiveFilter(filter, showComments);
  const rows = useMemo(
    () => filterHostRows(allRows, activeFilter, keyword),
    [allRows, activeFilter, keyword],
  );
  const changes = useMemo(() => summarizeChanges(original, entries), [original, entries]);
  const warnings = useMemo(() => saveWarnings(info), [info]);

  const dirty = changes.added + changes.removed + changes.toggled > 0;

  const pageCount = Math.max(1, Math.ceil(rows.length / PAGE_SIZE));
  const cur = Math.min(page, pageCount - 1);
  const pageRows = rows.slice(cur * PAGE_SIZE, cur * PAGE_SIZE + PAGE_SIZE);

  /** 新增（仅内存）。 */
  const add = useCallback(() => {
    const aliases = parseAliases(newAliases);
    const v = validateNewEntry(newIp, newHost, aliases);
    if (!v.ok) {
      v.errors.forEach((e) => toast.error(e));
      return;
    }
    // 重复映射不报错但语义混乱（同名两条时解析顺序难推断），在录入时拦下
    if (isDuplicateMapping(entries, newIp, newHost)) {
      toast.info(`映射已存在：${newIp.trim()} ${newHost.trim()}（禁用行也算）`);
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
  }, [newIp, newHost, newAliases, newComment, entries, toast]);

  const remove = useCallback((idx: number) => {
    setEntries((prev) => prev.filter((_, i) => i !== idx));
  }, []);

  /**
   * 删除也要确认：虽然只是内存改动、写回前仍有一次确认，但列表里翻了几页后
   * 误删一条想恢复，只能「重新读取」丢掉全部未保存改动——确认框成本低得多。
   */
  const removeWithConfirm = useCallback(
    (r: HostRow) => {
      confirm({
        title: "删除该映射",
        content: (
          <div className="whitespace-pre-wrap">
            {`${rowText(r)}${r.comment ? `  # ${r.comment}` : ""}\n\n仅从列表移除，点「写回文件」后才会真正改动 hosts。`}
          </div>
        ),
        okText: "删除",
        danger: true,
        onOk: () => remove(r.idx),
      });
    },
    [remove],
  );

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
    confirm({
      title: "写回 hosts 文件",
      width: 560,
      content: (
        <div className="flex flex-col gap-1.5">
          <div>
            新增 {c.added} 条 · 删除 {c.removed} 条 · 切换 {c.toggled} 条
          </div>
          <div className="sk-hint">
            写入前会自动把原文件备份到备份目录；若检测到编码异常（UTF-16/非 UTF-8）会
            <b>拒绝写入</b>并保留原文件。
          </div>
          {warnings.map((w) => (
            <div key={w} className="sk-hint text-sk-warn">
              ⚠ {w}
            </div>
          ))}
        </div>
      ),
      okText: "写入",
      onOk: async () => {
        setBusy(true);
        try {
          const r = await invoke<{ ok: boolean; backup: string; message: string }>(
            "sk_hosts_write",
            { entries, backup: true },
          );
          // 备份路径单独展示：写坏 hosts 是大事故，用户需要知道备份在哪
          toast.success(r.backup ? `${r.message}（备份：${r.backup}）` : r.message);
          await load();
        } catch (e) {
          toast.error(`写入失败（原文件已保留）：${String(e)}`);
        } finally {
          setBusy(false);
        }
      },
    });
  }, [entries, original, warnings, load, toast]);

  const backupOnly = useCallback(async () => {
    try {
      const p = await invoke<string>("sk_hosts_backup");
      toast.success(p ? `已备份: ${p}` : "hosts 文件不存在，无需备份");
    } catch (e) {
      toast.error(`备份失败：${String(e)}`);
    }
  }, [toast]);

  const columns: Column<HostRow>[] = [
    {
      title: "状态",
      dataIndex: "enabled",
      width: 90,
      render: (v: boolean, r) =>
        r.kind === "comment" ? (
          <Badge>注释</Badge>
        ) : v ? (
          <Badge tone="ok">启用</Badge>
        ) : (
          <Badge tone="warn">禁用</Badge>
        ),
    },
    {
      title: "内容",
      dataIndex: "idx",
      render: (_: number, r) =>
        r.kind === "comment" ? (
          <span className="sk-hint">{r.raw || "#"}</span>
        ) : (
          <span className="inline-flex items-center gap-1.5">
            <span>{rowText(r)}</span>
            {r.comment && <span className="sk-hint"># {r.comment}</span>}
          </span>
        ),
    },
    {
      title: "操作",
      width: 150,
      render: (_: unknown, r) =>
        r.kind === "comment" ? null : (
          <span className="flex gap-1">
            <Button variant="ghost" size="sm" onClick={() => toggle(r.idx)}>
              {r.enabled ? "禁用" : "启用"}
            </Button>
            <Button variant="danger" size="sm" onClick={() => removeWithConfirm(r)}>
              删除
            </Button>
          </span>
        ),
    },
  ];

  return (
    <div className="flex flex-col gap-3">
      {warnings.map((w) => (
        <Alert key={w} tone="warning" title={w} />
      ))}

      <Card>
        <CardHeader>
          <CardTitle>hosts 文件</CardTitle>
          <div className="flex items-center gap-2">
            <span className="sk-hint">{info?.path ?? "—"}</span>
            <Button size="sm" onClick={() => void load()}>
              重新读取
            </Button>
            <Button size="sm" onClick={() => void backupOnly()}>
              只备份
            </Button>
          </div>
        </CardHeader>
        <CardBody>
          <div className="flex flex-wrap items-center gap-2">
            <Badge tone="ok">启用 {stats.enabled}</Badge>
            <Badge>禁用 {stats.disabled}</Badge>
            <Badge>注释/空行 {stats.comments}</Badge>
            <span className="sk-hint">共 {stats.total} 行</span>
            {dirty && <Badge tone="warn">有未保存改动</Badge>}
            <span className="sk-hint">显示注释行</span>
            <Switch checked={showComments} onChange={setShowComments} aria-label="显示注释行" />
            <Button
              variant="primary"
              size="sm"
              disabled={!dirty}
              loading={busy}
              onClick={() => void save()}
            >
              写回文件
            </Button>
          </div>
        </CardBody>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>新增映射</CardTitle>
        </CardHeader>
        <CardBody>
          <div className="flex flex-wrap gap-2">
            <Input
              className="h-7 w-[150px] text-xs"
              placeholder="IP（如 127.0.0.1）"
              value={newIp}
              onChange={(e) => setNewIp(e.target.value)}
            />
            <Input
              className="h-7 w-[190px] text-xs"
              placeholder="主机名（如 dev.local）"
              value={newHost}
              onChange={(e) => setNewHost(e.target.value)}
            />
            <Input
              className="h-7 w-[190px] text-xs"
              placeholder="别名（逗号分隔，可选）"
              value={newAliases}
              onChange={(e) => setNewAliases(e.target.value)}
            />
            <Input
              className="h-7 w-[160px] text-xs"
              placeholder="备注（可选）"
              value={newComment}
              onChange={(e) => setNewComment(e.target.value)}
            />
            <Button variant="primary" size="sm" onClick={add}>
              添加
            </Button>
          </div>
        </CardBody>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>条目</CardTitle>
        </CardHeader>
        <CardBody>
          {allRows.length > 0 && (
            <div className="mb-2.5 flex flex-wrap items-center gap-2">
              <Segmented<HostFilter>
                size="sm"
                value={activeFilter}
                onChange={(v) => {
                  setFilter(v);
                  setPage(0);
                }}
                options={(
                  showComments
                    ? (["all", "enabled", "disabled", "comment"] as const)
                    : (["all", "enabled", "disabled"] as const)
                ).map((v) => ({
                  label:
                    v === "all"
                      ? "全部"
                      : v === "enabled"
                        ? "启用"
                        : v === "disabled"
                          ? "禁用"
                          : "注释",
                  value: v,
                }))}
              />
              <Input
                className="h-7 w-[220px] text-xs"
                placeholder="筛选：IP / 主机名 / 备注…"
                value={keyword}
                onChange={(e) => {
                  setKeyword(e.target.value);
                  setPage(0);
                }}
              />
              {(activeFilter !== "all" || keyword) && (
                <span className="sk-hint">
                  筛出 {rows.length} / {allRows.length} 条
                </span>
              )}
            </div>
          )}
          {rows.length === 0 ? (
            <Empty
              description={
                allRows.length === 0
                  ? info?.exists
                    ? "hosts 里没有主机映射（可能全是注释）"
                    : "hosts 文件不存在"
                  : "无匹配条目——换个筛选条件或清空关键字"
              }
            />
          ) : (
            <>
              <Table<HostRow>
                rowKey={(r) => r.idx}
                dataSource={pageRows}
                columns={columns}
              />
              {pageCount > 1 && (
                <div className="mt-2 flex items-center justify-end gap-2 text-xs text-sk-sub">
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
        </CardBody>
      </Card>
    </div>
  );
}
