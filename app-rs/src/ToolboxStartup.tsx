/**
 * 系统助手：启动项管理（对应 Python 侧 `ui/system_page.py::StartupTab`）。
 *
 * 禁用不改注册表的"启用标志"（Windows 没有这个字段），而是把值搬到
 * `HKCU\Software\NetTool\DisabledRun`；启动文件夹项改名加 `.disabled`。
 * 这个键名是**沿用 Python 版**的（旧产品名 NetTool），两版共享同一份标记。
 *
 * v1.4 UI 迁移：脱离 AntD（Table 分页改为页内切片 + 轻量翻页条；
 * Modal.confirm/message/Tag/Segmented → ui 件）。
 */
import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import {
  canToggle,
  commandProgram,
  filterStartup,
  locationColor,
  locationHint,
  sortStartup,
  startupStats,
  toggleConfirmText,
  toggleErrorText,
  STARTUP_SORTS,
  type StartupItem,
  type StartupList,
  type StartupSort,
  type StartupStatusFilter,
} from "./startup";
import { Badge, Button, Checkbox, Empty, Input, Segmented, Table, type Column } from "./ui";
import { Select } from "./ui/select";
import { Alert } from "./ui/alert";
import { confirm } from "./ui/confirm";
import { Tooltip as UiTooltip } from "./ui/tooltip";
import { useToast } from "./ui/toast";

const PAGE_SIZE = 15;

/** locationColor 的 antd 色名 → Badge tone（HKLM 更"重"，用不同色提示影响范围）。 */
function locationTone(location: string): "err" | "accent" | "ok" | "neutral" {
  switch (locationColor(location)) {
    case "red":
      return "err";
    case "blue":
      return "accent";
    case "green":
      return "ok";
    default:
      return "neutral";
  }
}

export function ToolboxStartup() {
  const toast = useToast();
  const [list, setList] = useState<StartupList | null>(null);
  const [keyword, setKeyword] = useState("");
  const [status, setStatus] = useState<StartupStatusFilter>("all");
  const [sort, setSort] = useState<StartupSort>("default");
  const [page, setPage] = useState(0);
  // 记录"正在切换哪一行"而不是全局布尔：全局 busy 会让每一行的按钮同时转圈，
  // 用户分不清刚才点的是哪个
  const [busyKey, setBusyKey] = useState("");
  const [loadError, setLoadError] = useState("");
  // 枚举本身要读四个注册表位置 + 两个启动文件夹，几百毫秒起；没有加载态时
  // 点 🔄 完全没反馈，用户会以为按钮坏了（重复点还会并发枚举）
  const [loading, setLoading] = useState(false);
  // 多选（key = location|name）：批量禁用是"清理启动项"最常见的用法，
  // 逐条点确认太慢；只允许勾选可操作项，HKLM 项在没有管理员时不给勾
  const [picked, setPicked] = useState<string[]>([]);
  const [batchBusy, setBatchBusy] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setList(await invoke<StartupList>("sk_startup_list"));
      // 成功后清掉旧错误，重试入口随 Empty 一起消失
      setLoadError("");
      setPicked([]); // 重新枚举后旧选择可能已不存在
    } catch (e) {
      // 必须区分「还在加载」和「加载失败」：失败若也停在 list=null，
      // 界面会永远显示"正在枚举启动项…"骗人（toast 提示 3 秒后就没了）。
      // 已有数据时保留旧表——刷新失败不该把上一次的枚举结果也抹掉。
      setLoadError(String(e));
      toast.error(`枚举启动项失败：${String(e)}`);
    } finally {
      setLoading(false);
    }
  }, [toast]);

  useEffect(() => {
    void load();
  }, [load]);

  const rows = useMemo(
    () => sortStartup(filterStartup(list?.rows ?? [], keyword, status), sort),
    [list, keyword, status, sort],
  );
  const stats = useMemo(() => startupStats(list?.rows ?? []), [list]);
  const isAdmin = list?.is_admin ?? false;

  const pageCount = Math.max(1, Math.ceil(rows.length / PAGE_SIZE));
  const cur = Math.min(page, pageCount - 1);
  const pageRows = rows.slice(cur * PAGE_SIZE, cur * PAGE_SIZE + PAGE_SIZE);

  const toggle = useCallback(
    (item: StartupItem) => {
      const check = canToggle(item, isAdmin);
      if (!check.ok) {
        toast.info(check.reason);
        return;
      }
      const enable = !item.enabled;
      confirm({
        title: enable ? "启用启动项" : "禁用启动项",
        width: 520,
        content: <div className="whitespace-pre-wrap">{toggleConfirmText(item, enable)}</div>,
        okText: enable ? "启用" : "禁用",
        danger: !enable,
        onOk: async () => {
          const key = `${item.location}|${item.name}`;
          setBusyKey(key);
          try {
            toast.success(
              await invoke<string>("sk_startup_set", {
                location: item.location,
                name: item.name,
                enable,
              }),
            );
            await load();
          } catch (e) {
            toast.error(toggleErrorText(enable, e));
          } finally {
            setBusyKey("");
          }
        },
      });
    },
    [isAdmin, load, toast],
  );


  const keyOf = (r: StartupItem) => `${r.location}|${r.name}`;

  /** 批量启用/禁用：逐条串行执行（注册表写入不能并发），失败项汇总报告。 */
  const batch = useCallback(
    (enable: boolean) => {
      // 从**全量**列表取目标，不是当前筛选结果：用户可能先勾了几项再改筛选，
      // 用 rows 会让"已选 5 项"实际只执行其中 2 项，且没有任何提示
      const all = list?.rows ?? [];
      const targets = all.filter(
        (r) => picked.includes(keyOf(r)) && canToggle(r, isAdmin).ok && r.enabled !== enable,
      );
      if (targets.length === 0) {
        toast.info(
          picked.length === 0
            ? "先勾选要操作的启动项"
            : `选中的 ${picked.length} 项都已经${enable ? "启用" : "禁用"}，无需操作`,
        );
        return;
      }
      confirm({
        title: `${enable ? "启用" : "禁用"} ${targets.length} 个启动项`,
        width: 560,
        content: (
          <div className="flex flex-col gap-1">
            <span>
              将对以下 {targets.length} 项执行{enable ? "启用" : "禁用"}：
            </span>
            <span className="sk-hint max-h-40 overflow-y-auto">
              {targets.map((t) => `${t.name}（${t.location}）`).join("、")}
            </span>
            <span className="sk-hint">
              写回注册表/启动文件夹前会自动备份，可逐条恢复。
            </span>
          </div>
        ),
        okText: enable ? "全部启用" : "全部禁用",
        danger: !enable,
        onOk: async () => {
          setBatchBusy(true);
          let ok = 0;
          const failed: string[] = [];
          try {
            for (const t of targets) {
              try {
                await invoke<string>("sk_startup_set", {
                  location: t.location,
                  name: t.name,
                  enable,
                });
                ok += 1;
              } catch (e) {
                failed.push(`${t.name}：${String(e)}`);
              }
            }
          } finally {
            setBatchBusy(false);
          }
          await load();
          if (failed.length === 0) {
            toast.success(`已${enable ? "启用" : "禁用"} ${ok} 项`);
          } else {
            // 逐条列出失败项：批量操作最怕"部分成功但不知道哪几条"
            toast.error(
              `${ok} 项成功，${failed.length} 项失败：${failed.slice(0, 3).join("；")}` +
                (failed.length > 3 ? " …" : ""),
            );
          }
        },
      });
    },
    [isAdmin, list, load, picked, toast],
  );

  const operable = useMemo(() => rows.filter((r) => canToggle(r, isAdmin).ok), [rows, isAdmin]);

  const lockedCount = useMemo(
    () => (isAdmin ? 0 : (list?.rows ?? []).filter((r) => !canToggle(r, isAdmin).ok).length),
    [list, isAdmin],
  );

  const columns: Column<StartupItem>[] = [
    {
      title: "",
      key: "pick",
      width: 36,
      render: (_: unknown, r) => {
        // 不可操作的项（无管理员时的 HKLM）不给勾：勾了也只会批量失败
        if (!canToggle(r, isAdmin).ok) return null;
        const k = keyOf(r);
        return (
          <span onClick={(e) => e.stopPropagation()}>
            <Checkbox
              checked={picked.includes(k)}
              onChange={() =>
                setPicked((p) => (p.includes(k) ? p.filter((x) => x !== k) : [...p, k]))
              }
              aria-label={`选择 ${r.name}`}
            />
          </span>
        );
      },
    },
    {
      title: "名称",
      dataIndex: "name",
      width: 220,
      render: (v: string, r) => (
        <span className="flex flex-col">
          {/* 禁用态压暗用 opacity 而不是写死灰色：写死的 #8b949e 在浅色
              主题下对比度不足（约 3:1），opacity 对深浅主题都成立 */}
          <span className="truncate" style={{ opacity: r.enabled ? 1 : 0.55 }}>{v}</span>
          <span className="sk-hint truncate" title={commandProgram(r.command) || undefined}>
            {commandProgram(r.command) || "—"}
          </span>
        </span>
      ),
    },
    {
      title: "位置",
      dataIndex: "location",
      width: 170,
      render: (v: string) => (
        <UiTooltip title={locationHint(v)}>
          <Badge tone={locationTone(v)}>{v}</Badge>
        </UiTooltip>
      ),
    },
    {
      title: "命令",
      dataIndex: "command",
      render: (v: string) => (
        // ui Table 无 AntD ellipsis 列语义：不钳宽会长命令把表格撑出横向滚动，
        // 操作列被挤出视口（真机走查实锤）；完整内容走 Tooltip。
        // Tooltip 包裹层是 inline-flex，在 table-fixed 单元格里 shrink-to-fit
        // 会超出列宽（实测 271 > 235px），必须 max-w-full 钳住、内层 w-full 截断
        <UiTooltip title={v} className="max-w-full">
          <span className="block w-full truncate sk-hint font-mono text-xs">{v || "—"}</span>
        </UiTooltip>
      ),
    },
    {
      title: "状态",
      dataIndex: "enabled",
      width: 90,
      render: (v: boolean) => (v ? <Badge tone="ok">已启用</Badge> : <Badge>已禁用</Badge>),
    },
    {
      title: "操作",
      width: 100,
      render: (_: unknown, r) => {
        const check = canToggle(r, isAdmin);
        return check.ok ? (
          <Button
            variant="ghost"
            size="sm"
            loading={busyKey === `${r.location}|${r.name}`}
            disabled={busyKey !== "" || batchBusy}
            onClick={() => toggle(r)}
          >
            {r.enabled ? "禁用" : "启用"}
          </Button>
        ) : (
          <UiTooltip title={check.reason}>
            <span className="sk-hint">需管理员</span>
          </UiTooltip>
        );
      },
    },
  ];

  return (
    <div className="flex flex-col gap-2.5">
      {lockedCount > 0 && (
        <Alert
          tone="info"
          title={`有 ${lockedCount} 项位于 HKLM（所有用户），需要管理员权限才能启用/禁用；其余项可直接操作。`}
        />
      )}

      <div className="flex flex-wrap items-center gap-2">
        <UiTooltip title="重新枚举启动项">
          <Button variant="primary" size="sm" loading={loading} onClick={() => void load()}>
            🔄
          </Button>
        </UiTooltip>
        <Input
          className="h-7 w-[260px] text-xs"
          placeholder="筛选：名称 / 命令 / 位置…"
          value={keyword}
          onChange={(e) => {
            setKeyword(e.target.value);
            setPage(0);
          }}
        />
        {/* 状态档是高频意图：禁完一串想回头检查，或想找出"还有什么在自启"。
            计数并入标签、去重复 Tag（用户反馈：三处计数挤在一起看不清）。 */}
        <Segmented<StartupStatusFilter>
          size="sm"
          value={status}
          onChange={(v) => {
            setStatus(v);
            setPage(0);
          }}
          options={[
            { value: "all", label: `全部(${stats.total})` },
            { value: "enabled", label: `已启用(${stats.enabled})` },
            { value: "disabled", label: `已禁用(${stats.disabled})` },
          ]}
        />
        <Select
          className="h-7 text-xs"
          aria-label="排序方式"
          value={sort}
          onChange={(v) => {
            setSort(v as StartupSort);
            setPage(0);
          }}
          options={STARTUP_SORTS.map((s) => ({ value: s.value, label: s.label }))}
        />
        {/* 批量：勾选是"一次性清理启动项"的主路径，逐条点确认太慢 */}
        {operable.length > 0 && (
          <>
            <Button
              size="sm"
              disabled={batchBusy || loading || picked.length === operable.length}
              onClick={() => setPicked(operable.map(keyOf))}
            >
              全选（{operable.length}）
            </Button>
            <Button size="sm" loading={batchBusy} disabled={picked.length === 0 || loading} onClick={() => batch(true)}>
              批量启用
            </Button>
            <Button
              size="sm"
              variant="danger"
              disabled={batchBusy || picked.length === 0 || loading}
              onClick={() => batch(false)}
            >
              批量禁用
            </Button>
            {picked.length > 0 && (
              <span className="flex items-center gap-1">
                <span className="sk-hint">已选 {picked.length} 项</span>
                <Button size="sm" variant="ghost" onClick={() => setPicked([])}>
                  取消
                </Button>
              </span>
            )}
          </>
        )}
      </div>

      {rows.length === 0 ? (
        <div className="flex flex-col items-center gap-2">
          <Empty
            description={
              list
                ? "没有匹配的启动项"
                : loadError
                  ? `枚举启动项失败：${loadError}`
                  : "正在枚举启动项…"
            }
          />
          {loadError && (
            <Button variant="primary" size="sm" onClick={() => void load()}>
              重试
            </Button>
          )}
        </div>
      ) : (
        <>
          <Table<StartupItem>
            rowKey={(r) => `${r.location}|${r.name}`}
            dataSource={pageRows}
            columns={columns}
            fixed
          />
          {pageCount > 1 && (
            <div className="flex items-center justify-end gap-2 text-xs text-sk-sub">
              <span>
                共 {rows.length} 项 · 第 {cur + 1}/{pageCount} 页
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

      <div className="sk-hint">禁用只是不自启，可随时恢复；写回前自动备份。</div>
    </div>
  );
}
