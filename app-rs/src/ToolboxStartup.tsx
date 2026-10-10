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
  startupStats,
  toggleConfirmText,
  toggleErrorText,
  type StartupItem,
  type StartupList,
  type StartupStatusFilter,
} from "./startup";
import { Badge, Button, Empty, Input, Segmented, Table, type Column } from "./ui";
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
  const [page, setPage] = useState(0);
  // 记录"正在切换哪一行"而不是全局布尔：全局 busy 会让每一行的按钮同时转圈，
  // 用户分不清刚才点的是哪个
  const [busyKey, setBusyKey] = useState("");
  const [loadError, setLoadError] = useState("");

  const load = useCallback(async () => {
    try {
      setList(await invoke<StartupList>("sk_startup_list"));
      // 成功后清掉旧错误，重试入口随 Empty 一起消失
      setLoadError("");
    } catch (e) {
      // 必须区分「还在加载」和「加载失败」：失败若也停在 list=null，
      // 界面会永远显示"正在枚举启动项…"骗人（toast 提示 3 秒后就没了）。
      // 已有数据时保留旧表——刷新失败不该把上一次的枚举结果也抹掉。
      setLoadError(String(e));
      toast.error(`枚举启动项失败：${String(e)}`);
    }
  }, [toast]);

  useEffect(() => {
    void load();
  }, [load]);

  const rows = useMemo(
    () => filterStartup(list?.rows ?? [], keyword, status),
    [list, keyword, status],
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

  const lockedCount = useMemo(
    () => (isAdmin ? 0 : (list?.rows ?? []).filter((r) => !canToggle(r, isAdmin).ok).length),
    [list, isAdmin],
  );

  const columns: Column<StartupItem>[] = [
    {
      title: "名称",
      dataIndex: "name",
      width: 220,
      render: (v: string, r) => (
        <span className="flex flex-col">
          {/* 禁用态压暗用 opacity 而不是写死灰色：写死的 #8b949e 在浅色
              主题下对比度不足（约 3:1），opacity 对深浅主题都成立 */}
          <span style={{ opacity: r.enabled ? 1 : 0.55 }}>{v}</span>
          <span className="sk-hint">{commandProgram(r.command) || "—"}</span>
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
        <UiTooltip title={v}>
          <span className="sk-hint font-mono text-xs">{v || "—"}</span>
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
            disabled={busyKey !== ""}
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
          <Button variant="primary" size="sm" onClick={() => void load()}>
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

      <div className="sk-hint">
        禁用不会删除程序，只是让它下次登录时不自启；禁用记录存在
        <code>HKCU\Software\NetTool\DisabledRun</code>（与 Python 版共用），随时可恢复。
        启动文件夹项禁用后文件名会加 <code>.disabled</code> 后缀。
      </div>
    </div>
  );
}
