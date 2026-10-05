/**
 * 主界面壳：左侧导航 + 页面容器 + 主题切换。
 *
 * M0 的骨架页已替换为真实页面：概览/网络监控（M2）。剪贴板历史复用 M1 已有的
 * `sk_clip_*` 命令直读内存态。其余页面（诊断/修复/工具箱/系统助手/密码库/设置）
 * 属 M3-M7，先给明确的「里程碑待接入」占位而不是空白。
 */
import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  ConfigProvider,
  Empty,
  Layout,
  Menu,
  Space,
  Switch,
  Tabs,
  theme as antdThemeApi,
  Tag,
  Typography,
  message,
} from "antd";
import { Dashboard } from "./Overview";
import { ClipboardHistoryPanel } from "./ToolboxClipboard";
import { Diagnose } from "./Diagnose";
import { ToolboxHosts } from "./ToolboxHosts";
import { ToolboxPorts } from "./ToolboxPorts";
import { ToolboxDns } from "./ToolboxDns";
import { ToolboxSpeed } from "./ToolboxSpeed";
import { ToolboxAppNet } from "./ToolboxAppNet";
import { SystemPage } from "./SystemPage";
import { VaultPage } from "./VaultPage";
import { SettingsView } from "./SettingsView";
import type { SettingsAll } from "./settingsPage";
import { applyHotkeyEcho, type HotkeyHints } from "./settingsPage";
import { applyTheme, palette, type Palette } from "./theme";


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

/** 页名 ↔ Python stack 索引（last_page 往返契约，顺序勿动）。 */
const ORDER_KEYS = [
  "overview",
  "clipboard",
  "diagnose",
  "toolbox",
  "system",
  "vault",
  "settings",
];

export function App() {
  const [dark, setDark] = useState(false);
  // 深色偏好持久化：启动读 dark_mode 设置，切换写回（此前每次启动都回浅色）
  useEffect(() => {
    void invoke<Record<string, unknown>>("sk_settings_all")
      .then((all) => setDark(all["dark_mode"] === true))
      .catch(() => {});
  }, []);
  const toggleDark = (v: boolean) => {
    setDark(v);
    void invoke("sk_settings_set", { key: "dark_mode", value: v }).catch(() => {});
  };

  useEffect(() => {
    void invoke<SettingsAll>("sk_settings_all")
      .then((all) => {
        // sk_settings_all 返回 {count, items:[{key,current,...}]}，不是平铺 map
        const cur = (k: string) =>
          String(all.items?.find((i) => i.key === k)?.current ?? "");
        const q = cur("clip_quick_hotkey");
        const p = cur("palette_hotkey");
        setHotkeys({
          quick: q || "（未设）",
          palette: p || "（未设）",
        });
        const dark = all.items?.find((i) => i.key === "dark_mode")?.current;
        if (dark === true) setDark(true);
        const idx = Number(all.items?.find((i) => i.key === "last_page")?.current ?? 0);
        if (Number.isInteger(idx) && idx >= 0 && idx < ORDER_KEYS.length) {
          setPageState(ORDER_KEYS[idx]);
        }
      })
      .catch(() => {});
  }, []);
  // 启动恢复上次页面（对齐 Python main_window 的 last_page 往返：索引起步、
  // 切页写回）。未知索引回落概览。
  const [page, setPageState] = useState("overview");
  const setPage = (key: string) => {
    setPageState(key);
    const idx = ORDER_KEYS.indexOf(key);
    if (idx >= 0) void invoke("sk_settings_set", { key: "last_page", value: idx }).catch(() => {});
  };
  // 侧栏底部热键速查：读真实配置（用户可改键，不能写死）
  const [hotkeys, setHotkeys] = useState<HotkeyHints>({
    quick: "",
    palette: "",
  });

  // 热键速查三方一致（轮次36）：注册链 × 设置页 × 此处。被占回退
  // （sk://hotkey-reverted，载荷即后端已持久化的实际生效值）与设置页改键
  // （sk:hotkeys-changed，SettingsView 保存热键后派发）都要即时刷新——
  // 此前只在启动读一次，改键/回退后速查会一直显示并未注册的组合。
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    const refresh = () => {
      void invoke<SettingsAll>("sk_settings_all")
        .then((all) => {
          const cur = (k: string) =>
            String(all.items?.find((i) => i.key === k)?.current ?? "");
          setHotkeys({
            quick: cur("clip_quick_hotkey") || "（未设）",
            palette: cur("palette_hotkey") || "（未设）",
          });
        })
        .catch(() => {});
    };
    void listen<{ key: string; value: string }>("sk://hotkey-reverted", (e) => {
      // 载荷是权威值（后端先持久化再发事件），直接回显，不必再查一轮
      setHotkeys((h) => applyHotkeyEcho(h, e.payload.key, e.payload.value));
    }).then((f) => {
      unlisten = f;
    });
    window.addEventListener("sk:hotkeys-changed", refresh);
    return () => {
      unlisten?.();
      window.removeEventListener("sk:hotkeys-changed", refresh);
    };
  }, []);
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
    // 命令面板动作分发：nav/tab 跳页（tab 同时受控切子 tab）、quick:diag 触发
    // 快速检测（Diagnose 挂载时读 quickDiagRequest 一次性信号）。
    const un3 = listen<{ page: string; tab?: string; quick?: boolean }>(
      "sk://palette-nav",
      (ev) => {
        const { page: p, tab, quick } = ev.payload;
        if (ORDER_KEYS.includes(p)) setPage(p);
        if (p === "toolbox" && tab) setToolboxTab(tab);
        if (p === "system" && tab) setSystemTab(tab);
        if (quick) setQuickDiagRequest((n) => n + 1);
      },
    );
    return () => {
      void un.then((f) => f());
      void un2.then((f) => f());
      void un3.then((f) => f());
    };
  }, []);

  const pal: Palette = palette(dark);

  const antdTheme = {
    // 深浅算法必须跟着切：只叠深色 token 不够——AntD 组件（Alert/表格/弹窗）
    // 仍按亮色算法推导派生色，出现浅蓝底配暗字的不可读组合（真机实锤）
    algorithm: dark ? antdThemeApi.darkAlgorithm : antdThemeApi.defaultAlgorithm,
    token: {
      colorPrimary: pal.accent,
      colorBgBase: pal.bg,
      colorBgContainer: pal.card,
      colorText: pal.text,
      colorBorder: pal.border,
      borderRadius: 8,
    },
  };

  // 子 tab 受控：palette 动作直达页内 tab（不再 DOM 点击 hack）
  const [toolboxTab, setToolboxTab] = useState("hosts");
  const [systemTab, setSystemTab] = useState("perf");
  // palette quick:diag 的递增信号：Diagnose 挂载/更新时消费一次
  const [quickDiagRequest, setQuickDiagRequest] = useState(0);
  const pages: Record<string, React.ReactNode> = {
    overview: <Dashboard />,
    clipboard: <ClipboardHistoryPanel />,
    diagnose: <Diagnose quickRequest={quickDiagRequest} />,
    toolbox: (
      <Tabs
        activeKey={toolboxTab}
        onChange={setToolboxTab}
        items={[
          { key: "hosts", label: "Hosts 编辑", children: <ToolboxHosts /> },
          { key: "ports", label: "端口 / 局域网", children: <ToolboxPorts /> },
          { key: "dns", label: "DNS / 子网 / ping", children: <ToolboxDns /> },
          { key: "speed", label: "测速", children: <ToolboxSpeed /> },
          { key: "appnet", label: "应用联网", children: <ToolboxAppNet /> },
        ]}
      />
    ),
    system: <SystemPage initialTab={systemTab} onTabChange={setSystemTab} />,
    vault: <VaultPage />,
    settings: <SettingsView />,
  };

  return (
    <ConfigProvider theme={antdTheme}>
      <Layout style={{ minHeight: "100vh", background: pal.bg }}>
        <Layout.Sider theme={dark ? "dark" : "light"} width={200} style={{ position: "relative", background: pal.card, borderRight: `1px solid ${pal.border}` }}>
          {/* 品牌区（对齐 Python 版 brandCard）：渐变 S 标 + 两行文案，不引图片资源 */}
          <div style={{ display: "flex", alignItems: "center", gap: 9, padding: "12px 14px" }}>
            <div
              style={{
                width: 34,
                height: 34,
                flexShrink: 0,
                borderRadius: 8,
                background: "linear-gradient(135deg, #1668dc, #4096ff)",
                color: "#fff",
                fontSize: 20,
                fontWeight: 700,
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                userSelect: "none",
              }}
            >
              S
            </div>
            <div style={{ lineHeight: 1.35, minWidth: 0 }}>
              <Typography.Text strong style={{ display: "block" }}>
                SysKit
              </Typography.Text>
              <Typography.Text type="secondary" style={{ fontSize: 12 }}>
                {version ? `v${version} ` : ""}系统工具箱
              </Typography.Text>
            </div>
          </div>
          <Menu
            mode="inline"
            selectedKeys={[page]}
            onClick={(e) => setPage(e.key)}
            style={{ borderInlineEnd: "none" }}
            items={[
              {
                type: "group",
                label: "网络",
                children: [
                  { key: "overview", label: "🏠 概览 / 网络监控" },
                  { key: "clipboard", label: "📋 剪贴板历史" },
                  { key: "diagnose", label: "🔍 诊断 / 修复" },
                ],
              },
              {
                type: "group",
                label: "工具",
                children: [
                  { key: "toolbox", label: "⚡ 工具箱" },
                  { key: "system", label: "💼 系统助手" },
                  { key: "vault", label: "🔒 密码库" },
                ],
              },
              {
                type: "group",
                label: "其他",
                children: [{ key: "settings", label: "⚙ 设置 / 日志" }],
              },
            ]}
          />
          <div
            style={{
              position: "absolute",
              bottom: 12,
              left: 16,
              right: 16,
              fontSize: 11,
              lineHeight: 1.7,
              color: pal.border === "#303030" ? "#8a8a8a" : "#999",
            }}
          >
            <div>快速粘贴：{hotkeys.quick}</div>
            <div>命令面板：{hotkeys.palette}</div>
            <div style={{ marginTop: 4, opacity: 0.7 }}>可在「设置 / 日志」改键</div>
          </div>
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
              <Switch checked={dark} onChange={toggleDark} />
            </Space>
          </Layout.Header>

          <Layout.Content style={{ padding: 14, overflow: "auto" }}>
            {/* key=page 强制切页淡入（sk-page 动画），替代硬切 */}
            <div key={page} className="sk-page">
              {pages[page] ?? <Pending title={page} milestone="—" detail="未知页面" />}
            </div>
          </Layout.Content>
        </Layout>
      </Layout>
    </ConfigProvider>
  );
}

