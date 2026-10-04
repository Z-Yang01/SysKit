/**
 * 系统助手：启动项管理（对应 Python 侧 `ui/system_page.py::StartupTab`）。
 *
 * 禁用不改注册表的"启用标志"（Windows 没有这个字段），而是把值搬到
 * `HKCU\Software\NetTool\DisabledRun`；启动文件夹项改名加 `.disabled`。
 * 这个键名是**沿用 Python 版**的（旧产品名 NetTool），两版共享同一份标记。
 */
import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Alert,
  Button,
  Empty,
  Input,
  Modal,
  Segmented,
  Space,
  Table,
  Tag,
  Tooltip,
  Typography,
  message,
} from "antd";

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

export function ToolboxStartup() {
  const [list, setList] = useState<StartupList | null>(null);
  const [keyword, setKeyword] = useState("");
  const [status, setStatus] = useState<StartupStatusFilter>("all");
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
      // 界面会永远显示"正在枚举启动项…"骗人（message 提示 3 秒后就没了）。
      // 已有数据时保留旧表——刷新失败不该把上一次的枚举结果也抹掉。
      setLoadError(String(e));
      message.error(`枚举启动项失败：${String(e)}`);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const rows = useMemo(
    () => filterStartup(list?.rows ?? [], keyword, status),
    [list, keyword, status],
  );
  const stats = useMemo(() => startupStats(list?.rows ?? []), [list]);
  const isAdmin = list?.is_admin ?? false;

  const toggle = useCallback(
    (item: StartupItem) => {
      const check = canToggle(item, isAdmin);
      if (!check.ok) {
        message.warning(check.reason);
        return;
      }
      const enable = !item.enabled;
      Modal.confirm({
        title: enable ? "启用启动项" : "禁用启动项",
        width: 520,
        content: (
          <Typography.Paragraph style={{ whiteSpace: "pre-wrap" }}>
            {toggleConfirmText(item, enable)}
          </Typography.Paragraph>
        ),
        okText: enable ? "启用" : "禁用",
        okButtonProps: { danger: !enable },
        cancelText: "取消",
        onOk: async () => {
          const key = `${item.location}|${item.name}`;
          setBusyKey(key);
          try {
            message.success(
              await invoke<string>("sk_startup_set", {
                location: item.location,
                name: item.name,
                enable,
              }),
            );
            await load();
          } catch (e) {
            message.error(toggleErrorText(enable, e));
          } finally {
            setBusyKey("");
          }
        },
      });
    },
    [isAdmin, load],
  );

  const lockedCount = useMemo(
    () => (isAdmin ? 0 : (list?.rows ?? []).filter((r) => !canToggle(r, isAdmin).ok).length),
    [list, isAdmin],
  );

  return (
    <Space direction="vertical" size={10} style={{ width: "100%" }}>
      {lockedCount > 0 && (
        <Alert
          type="info"
          showIcon
          message={`有 ${lockedCount} 项位于 HKLM（所有用户），需要管理员权限才能启用/禁用；其余项可直接操作。`}
        />
      )}

      <Space wrap>
        <Button size="small" type="primary" onClick={() => void load()}>
          刷新
        </Button>
        <Input
          size="small"
          style={{ width: 260 }}
          allowClear
          placeholder="筛选：名称 / 命令 / 位置…"
          value={keyword}
          onChange={(e) => setKeyword(e.target.value)}
        />
        {/* 状态档是高频意图：禁完一串想回头检查，或想找出"还有什么在自启" */}
        <Segmented
          size="small"
          value={status}
          onChange={(v) => setStatus(v as StartupStatusFilter)}
          options={[
            { value: "all", label: "全部" },
            { value: "enabled", label: "已启用" },
            { value: "disabled", label: "已禁用" },
          ]}
        />
        <Tag color="green">启用 {stats.enabled}</Tag>
        <Tag>禁用 {stats.disabled}</Tag>
        <span className="sk-hint">共 {stats.total} 项</span>
      </Space>

      {rows.length === 0 ? (
        <Empty
          image={Empty.PRESENTED_IMAGE_SIMPLE}
          description={
            list
              ? "没有匹配的启动项"
              : loadError
                ? `枚举启动项失败：${loadError}`
                : "正在枚举启动项…"
          }
        >
          {loadError && (
            <Button size="small" type="primary" onClick={() => void load()}>
              重试
            </Button>
          )}
        </Empty>
      ) : (
        <Table<StartupItem>
          size="small"
          rowKey={(r) => `${r.location}|${r.name}`}
          pagination={{ pageSize: 15, showSizeChanger: false }}
          dataSource={rows}
          columns={[
            {
              title: "名称",
              dataIndex: "name",
              width: 220,
              render: (v: string, r) => (
                <Space direction="vertical" size={0}>
                  {/* 禁用态压暗用 opacity 而不是写死灰色：写死的 #8b949e 在浅色
                      主题下对比度不足（约 3:1），opacity 对深浅主题都成立 */}
                  <span style={{ opacity: r.enabled ? 1 : 0.55 }}>{v}</span>
                  <span className="sk-hint">{commandProgram(r.command) || "—"}</span>
                </Space>
              ),
            },
            {
              title: "位置",
              dataIndex: "location",
              width: 170,
              render: (v: string) => (
                <Tooltip title={locationHint(v)}>
                  <Tag color={locationColor(v)}>{v}</Tag>
                </Tooltip>
              ),
            },
            {
              title: "命令",
              dataIndex: "command",
              ellipsis: true,
              render: (v: string) => (
                <span className="sk-hint" style={{ fontFamily: "Consolas, monospace", fontSize: 12 }}>
                  {v || "—"}
                </span>
              ),
            },
            {
              title: "状态",
              dataIndex: "enabled",
              width: 90,
              render: (v: boolean) => (v ? <Tag color="green">已启用</Tag> : <Tag>已禁用</Tag>),
            },
            {
              title: "操作",
              width: 100,
              render: (_: unknown, r) => {
                const check = canToggle(r, isAdmin);
                return check.ok ? (
                  <Button
                    size="small"
                    type="link"
                    loading={busyKey === `${r.location}|${r.name}`}
                    disabled={busyKey !== ""}
                    onClick={() => toggle(r)}
                  >
                    {r.enabled ? "禁用" : "启用"}
                  </Button>
                ) : (
                  <Tooltip title={check.reason}>
                    <span className="sk-hint">需管理员</span>
                  </Tooltip>
                );
              },
            },
          ]}
        />
      )}

      <Typography.Paragraph type="secondary" className="sk-hint" style={{ marginBottom: 0 }}>
        禁用不会删除程序，只是让它下次登录时不自启；禁用记录存在
        <code>HKCU\Software\NetTool\DisabledRun</code>（与 Python 版共用），随时可恢复。
        启动文件夹项禁用后文件名会加 <code>.disabled</code> 后缀。
      </Typography.Paragraph>
    </Space>
  );
}
