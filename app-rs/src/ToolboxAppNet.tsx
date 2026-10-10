/**
 * 工具箱：应用联网（对应 Python 侧 AppNetTab）。
 *
 * 用 Windows 防火墙出站规则按程序阻断联网。规则名由 `SysKit-AppNet-<sha1>` 生成，
 * **与 Python 版逐字节一致**——两版共享 `appnet.json` 台账与同一批系统规则，
 * 迁移期间不会产生重复规则。
 *
 * 三态展示：已禁止 / 已放行 / 已失效（动作被外部改成"允许"）。最后一种必须显眼——
 * 否则用户以为程序被拦住了，实际它正畅通无阻。
 *
 * v1.4 UI 迁移：脱离 AntD（Card/Table 分页→页内切片；Tag/Modal.confirm/message → ui 件）。
 */
import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import {
  adminNotice,
  appNetStats,
  attentionNotice,
  baseName,
  blockConfirmText,
  dirName,
  isReallyBlocking,
  needsAttention,
  stateColor,
  stateLabel,
  type AppNetList,
  type AppNetRow,
  type AppNetValidate,
} from "./appnet";
import { Badge, Button, Card, CardBody, CardHeader, CardTitle, Empty, Input, Table, type Column } from "./ui";
import { Alert } from "./ui/alert";
import { confirm } from "./ui/confirm";
import { Tooltip as UiTooltip } from "./ui/tooltip";
import { useToast } from "./ui/toast";

const PAGE_SIZE = 15;

/** stateColor 的 antd 色名 → Badge tone。 */
function stateTone(state: AppNetRow["state"]): "ok" | "neutral" | "err" | "warn" {
  switch (stateColor(state)) {
    case "green":
      return "ok";
    case "red":
      return "err";
    case "orange":
      return "warn";
    default:
      return "neutral";
  }
}

export function ToolboxAppNet() {
  const toast = useToast();
  const [list, setList] = useState<AppNetList | null>(null);
  const [path, setPath] = useState("");
  const [busy, setBusy] = useState(false);
  const [validation, setValidation] = useState<AppNetValidate | null>(null);
  const [page, setPage] = useState(0);

  const load = useCallback(async () => {
    try {
      setList(await invoke<AppNetList>("sk_appnet_list"));
    } catch (e) {
      toast.error(`读取失败：${String(e)}`);
    }
  }, [toast]);

  useEffect(() => {
    void load();
  }, [load]);

  // 路径变化即时校验（含规则名与错误原因）
  useEffect(() => {
    if (!path.trim()) {
      setValidation(null);
      return;
    }
    let alive = true;
    void invoke<AppNetValidate>("sk_appnet_validate", { program: path })
      .then((v) => {
        if (alive) setValidation(v);
      })
      .catch(() => {
        if (alive) setValidation(null);
      });
    return () => {
      alive = false;
    };
  }, [path]);

  const rows = list?.rows ?? [];
  const stats = useMemo(() => appNetStats(rows), [rows]);
  const notice = useMemo(() => attentionNotice(rows), [rows]);
  const adminMsg = adminNotice(list, "修改防火墙规则需要管理员权限。请以管理员身份运行 SysKit 后重试。");

  const pageCount = Math.max(1, Math.ceil(rows.length / PAGE_SIZE));
  const cur = Math.min(page, pageCount - 1);
  const pageRows = rows.slice(cur * PAGE_SIZE, cur * PAGE_SIZE + PAGE_SIZE);

  const pick = useCallback(async () => {
    try {
      // 走 Win32 原生对话框（后端 GetOpenFileNameW），不为一次选择引入插件依赖
      const picked = await invoke<string>("sk_appnet_pick_exe");
      if (picked) setPath(picked);
    } catch (e) {
      toast.error(`选择文件失败：${String(e)}`);
    }
  }, [toast]);

  const apply = useCallback(
    async (program: string, blocked: boolean) => {
      // 非管理员时防火墙写入必然失败——前置拦截给出可读原因，而不是让用户
      // 对着后端的原始报错猜（顶部 Alert 只是静态提示，点击时的反馈更直接）
      if (list && !list.is_admin) {
        toast.info(adminMsg);
        return;
      }
      setBusy(true);
      try {
        const r = await invoke<{ message: string }>("sk_appnet_add", { program, blocked });
        toast.success(r.message);
        setPath("");
        await load();
      } catch (e) {
        toast.error(String(e));
      } finally {
        setBusy(false);
      }
    },
    [load, list, adminMsg, toast],
  );

  const add = useCallback(() => {
    if (!validation || validation.error) {
      toast.info(validation?.error || "请先选择或输入程序路径");
      return;
    }
    confirm({
      title: "禁止程序联网",
      width: 560,
      content: <div className="whitespace-pre-wrap">{blockConfirmText(validation.program)}</div>,
      okText: "禁止联网",
      danger: true,
      onOk: () => apply(validation.program, true),
    });
  }, [validation, apply]);

  const remove = useCallback(
    (row: AppNetRow) => {
      // 同 apply：删除规则也要写防火墙，非管理员时前置拦截
      if (list && !list.is_admin) {
        toast.info(adminMsg);
        return;
      }
      confirm({
        title: "移除联网控制",
        content: `将删除 ${baseName(row.program)} 的防火墙规则并从台账移除。`,
        okText: "移除",
        danger: true,
        onOk: async () => {
          try {
            toast.success(await invoke<string>("sk_appnet_remove", { program: row.program }));
            await load();
          } catch (e) {
            toast.error(String(e));
          }
        },
      });
    },
    [load, list, adminMsg, toast],
  );

  const columns: Column<AppNetRow>[] = [
    {
      title: "程序",
      dataIndex: "program",
      render: (v: string) => (
        <span className="flex flex-col">
          <span>{baseName(v)}</span>
          <span className="sk-hint">{dirName(v)}</span>
        </span>
      ),
    },
    {
      title: "状态",
      dataIndex: "state",
      width: 210,
      render: (v: AppNetRow["state"], r) => (
        <UiTooltip title={needsAttention(r) ? "台账与实际不一致，请处理" : r.rule}>
          <Badge tone={stateTone(v)}>{stateLabel(v)}</Badge>
        </UiTooltip>
      ),
    },
    { title: "加入时间", dataIndex: "added", width: 160 },
    {
      title: "操作",
      width: 180,
      render: (_: unknown, r) => (
        <span className="flex gap-1">
          {isReallyBlocking(r) ? (
            <Button variant="ghost" size="sm" onClick={() => void apply(r.program, false)}>
              放行
            </Button>
          ) : (
            <Button variant="ghost" size="sm" loading={busy} onClick={() => void apply(r.program, true)}>
              重新禁止
            </Button>
          )}
          <Button variant="danger" size="sm" onClick={() => remove(r)}>
            移除
          </Button>
        </span>
      ),
    },
  ];

  return (
    <div className="flex flex-col gap-3">
      {adminMsg && <Alert tone="warning" title={adminMsg} />}
      {notice && (
        <Alert tone="danger" title="规则状态异常">
          {notice}
        </Alert>
      )}

      <Card>
        <CardHeader>
          <CardTitle>添加受控程序</CardTitle>
          <div className="flex items-center gap-2">
            <Badge tone="ok">禁止 {stats.blocking}</Badge>
            <Badge>放行 {stats.released}</Badge>
            {stats.attention > 0 && <Badge tone="err">异常 {stats.attention}</Badge>}
            <Button size="sm" onClick={() => void load()}>
              刷新
            </Button>
          </div>
        </CardHeader>
        <CardBody>
          <div className="flex flex-wrap items-center gap-2">
            <Input
              className="w-[380px]"
              placeholder="程序完整路径，如 C:\Program Files\App\app.exe"
              value={path}
              onChange={(e) => setPath(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") add();
              }}
            />
            <Button size="sm" onClick={() => void pick()}>
              浏览…
            </Button>
            <Button variant="danger" size="sm" loading={busy} onClick={add}>
              禁止联网
            </Button>
            {validation && (
              <span className="sk-hint">
                {validation.error ? (
                  <span className="text-sk-err">{validation.error}</span>
                ) : (
                  <>
                    规则名 <code>{validation.rule}</code>
                  </>
                )}
              </span>
            )}
          </div>
        </CardBody>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>受控程序</CardTitle>
        </CardHeader>
        <CardBody>
          {rows.length === 0 ? (
            <Empty description="还没有受控程序。上面选一个 exe 就能禁止它联网。" />
          ) : (
            <>
              <Table<AppNetRow>
                rowKey={(r) => r.rule}
                dataSource={pageRows}
                columns={columns}
              />
              {pageCount > 1 && (
                <div className="mt-2 flex items-center justify-end gap-2 text-xs text-sk-sub">
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
        </CardBody>
      </Card>
    </div>
  );
}
