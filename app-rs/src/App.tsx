/**
 * 主界面壳：左侧导航 + 页面容器 + 主题切换。
 *
 * M0 的骨架页已替换为真实页面：概览/网络监控（M2）。剪贴板历史复用 M1 已有的
 * `sk_clip_*` 命令直读内存态。其余页面（诊断/修复/工具箱/系统助手/密码库/设置）
 * 属 M3-M7，先给明确的「里程碑待接入」占位而不是空白。
 */
import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import {
  Button,
  ConfigProvider,
  Empty,
  Layout,
  Menu,
  Space,
  Switch,
  Table,
  Tabs,
  Tag,
  Typography,
  message,
} from "antd";
import { Dashboard } from "./Overview";
import { Diagnose } from "./Diagnose";
import { ToolboxHosts } from "./ToolboxHosts";
import { ToolboxPorts } from "./ToolboxPorts";
import { applyTheme, palette, type Palette } from "./theme";

interface ClipRow {
  idx: number;
  ts: number;
  text: string;
  kind: string;
  pinned: boolean;
  data: string;
  preview: string;
}

/** 里程碑占位页：说清「这块归属哪个 M」，不留白。 */
function Pending({
  title,
  milestone,
  detail,
}: {
  title: string;
  milestone: string;
  detail: string;
}) {
  return (
    <div className="sk-card">
      <Space direction="vertical" size={10}>
        <Typography.Title level={5} style={{ margin: 0 }}>
          {title}
        </Typography.Title>
        <Tag color="blue">{milestone}</Tag>
        <Typography.Text type="secondary">{detail}</Typography.Text>
        <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="尚未接入" />
      </Space>
    </div>
  );
}

/** 剪贴板历史页（复用 M1 的 sk_clip_* 命令；记录侧监听属 M5）。 */
function ClipboardPage() {
  const [rows, setRows] = useState<ClipRow[]>([]);
  const [summary, setSummary] = useState<Record<string, number>>({});

  const reload = useCallback(async () => {
    try {
      setRows(await invoke<ClipRow[]>("sk_clip_list", {}));
      setSummary(await invoke<Record<string, number>>("sk_clip_summary"));
    } catch {
      setRows([]);
    }
  }, []);

  useEffect(() => {
    void reload();
  }, [reload]);

  return (
    <div className="sk-card">
      <Space direction="vertical" size={10} style={{ width: "100%" }}>
        <Space>
          <Typography.Text strong>剪贴板历史</Typography.Text>
          <Typography.Text type="secondary">
            共 {summary.total ?? 0} 条 · 置顶 {summary.pinned ?? 0} · 图片 {summary.images ?? 0} ·
            上限 {summary.max_items ?? 0}
          </Typography.Text>
          <Button size="small" onClick={() => void reload()}>
            刷新
          </Button>
          <Button
            size="small"
            danger
            onClick={async () => {
              await invoke("sk_clip_clear");
              await reload();
              message.success("已清空");
            }}
          >
            清空
          </Button>
        </Space>
        <Table<ClipRow>
          size="small"
          rowKey="idx"
          pagination={{ pageSize: 12 }}
          dataSource={rows}
          columns={[
            { title: "类型", dataIndex: "kind", width: 80 },
            {
              title: "内容",
              dataIndex: "preview",
              render: (v: string, r) => (r.pinned ? `📌 ${v}` : v),
            },
            {
              title: "操作",
              width: 240,
              render: (_: unknown, r) => (
                <Space size={4}>
                  <Button
                    size="small"
                    type="link"
                    onClick={async () => {
                      const ok = await invoke<boolean>("sk_clip_write_back", { index: r.idx });
                      message[ok ? "success" : "warning"](ok ? "已放回剪贴板" : "内容不可用");
                    }}
                  >
                    放回剪贴板
                  </Button>
                  <Button
                    size="small"
                    type="link"
                    onClick={async () => {
                      await invoke("sk_clip_toggle_pin", { index: r.idx });
                      await reload();
                    }}
                  >
                    {r.pinned ? "取消置顶" : "置顶"}
                  </Button>
                  <Button
                    size="small"
                    type="link"
                    danger
                    onClick={async () => {
                      await invoke("sk_clip_remove", { index: r.idx });
                      await reload();
                    }}
                  >
                    删除
                  </Button>
                </Space>
              ),
            },
          ]}
        />
      </Space>
    </div>
  );
}

export function App() {
  const [dark, setDark] = useState(false);
  const [page, setPage] = useState("overview");
  const [pong, setPong] = useState("—");
  const [version, setVersion] = useState<string>("");

  useEffect(() => {
    applyTheme(dark);
  }, [dark]);

  useEffect(() => {
    void invoke<{ version: string }>("sk_version")
      .then((v) => setVersion(v.version))
      .catch(() => setVersion(""));
    // 后端气泡通知（托盘/粘贴降级提示）
    const un = listen<{ title: string; body: string }>("sk://notify", (ev) => {
      message.info(`${ev.payload.title}：${ev.payload.body}`);
    });
    const un2 = listen<{ ok: boolean; message: string }>("sk://paste-result", (ev) => {
      if (ev.payload.message) {
        (ev.payload.ok ? message.success : message.warning)(ev.payload.message);
      }
    });
    return () => {
      void un.then((f) => f());
      void un2.then((f) => f());
    };
  }, []);

  const pal: Palette = palette(dark);

  const antdTheme = {
    token: {
      colorPrimary: pal.accent,
      colorBgBase: pal.bg,
      colorBgContainer: pal.card,
      colorText: pal.text,
      colorBorder: pal.border,
      borderRadius: 8,
    },
  };

  const pages: Record<string, React.ReactNode> = {
    overview: <Dashboard />,
    clipboard: <ClipboardPage />,
    diagnose: <Diagnose />,
    toolbox: (
      <Tabs
        items={[
          { key: "hosts", label: "Hosts 编辑", children: <ToolboxHosts /> },
          { key: "ports", label: "端口 / 局域网", children: <ToolboxPorts /> },
          {
            key: "soon",
            label: "待接入",
            children: (
              <Pending
                title="工具箱其余项"
                milestone="M4"
                detail="测速 / DNS 子网计算 / ping 路由 / DNS 测速 / 应用联网 —— 逐轮接入。"
              />
            ),
          },
        ]}
      />
    ),
    system: (
      <Pending
        title="系统助手"
        milestone="M5"
        detail="性能 / 启动项 / 电池 / 清理 / 磁盘分析 / 剪贴板记录侧 / 场景模式。"
      />
    ),
    vault: (
      <Pending
        title="密码库"
        milestone="M6"
        detail="rusqlite + aes-gcm + DPAPI 密钥包裹；vault.key 格式兼容是风险项。"
      />
    ),
    settings: (
      <Pending
        title="设置 / 日志"
        milestone="M7"
        detail="设置页读写、日志查看、命令面板 provider 扩展。"
      />
    ),
  };

  return (
    <ConfigProvider theme={antdTheme}>
      <Layout style={{ minHeight: "100vh", background: pal.bg }}>
        <Layout.Sider theme="light" width={168} style={{ borderRight: `1px solid ${pal.border}` }}>
          <div style={{ padding: "12px 16px" }}>
            <Typography.Text strong>SysKit</Typography.Text>
            <div className="sk-hint">{version ? `v${version}` : ""}</div>
          </div>
          <Menu
            mode="inline"
            selectedKeys={[page]}
            onClick={(e) => setPage(e.key)}
            style={{ borderInlineEnd: "none" }}
            items={[
              { key: "overview", label: "概览 / 网络监控" },
              { key: "clipboard", label: "剪贴板历史" },
              { key: "diagnose", label: "诊断 / 修复" },
              { key: "toolbox", label: "工具箱" },
              { key: "system", label: "系统助手" },
              { key: "vault", label: "密码库" },
              { key: "settings", label: "设置 / 日志" },
            ]}
          />
        </Layout.Sider>

        <Layout style={{ background: pal.bg }}>
          <Layout.Header
            style={{
              background: pal.card,
              borderBottom: `1px solid ${pal.border}`,
              padding: "0 16px",
              height: 48,
              lineHeight: "48px",
              display: "flex",
              alignItems: "center",
              justifyContent: "space-between",
            }}
          >
            <Space>
              <span className="sk-hint">深色主题</span>
              <Switch checked={dark} onChange={setDark} />
              <Button
                size="small"
                onClick={async () => {
                  try {
                    setPong(await invoke<string>("ping"));
                  } catch (e) {
                    setPong(`失败: ${String(e)}`);
                  }
                }}
              >
                调用 Rust ping
              </Button>
              <span className="sk-hint">后端: {pong}</span>
            </Space>
            <Space>
              <Button size="small" onClick={() => void getCurrentWindow().hide()}>
                最小化到托盘
              </Button>
            </Space>
          </Layout.Header>

          <Layout.Content style={{ padding: 14, overflow: "auto" }}>
            {pages[page] ?? <Pending title={page} milestone="—" detail="未知页面" />}
          </Layout.Content>
        </Layout>
      </Layout>
    </ConfigProvider>
  );
}
