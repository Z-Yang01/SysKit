/**
 * 主界面壳：左侧导航 + 页面容器 + 主题切换。
 *
 * M0 的骨架页已替换为真实页面：概览/网络监控（M2）。剪贴板历史复用 M1 已有的
 * `sk_clip_*` 命令直读内存态。其余页面（诊断/修复/工具箱/系统助手/密码库/设置）
 * 属 M3-M7，先给明确的「里程碑待接入」占位而不是空白。
 *
 * v1.4 UI 迁移：脱离 AntD——ConfigProvider/AntD 主题映射随之移除（全部页面
 * 已用 ui 件，色板只经 CSS 变量消费）；Sider/Menu → 自建侧栏（brand 渐变与
 * 热键速查卡样式原样保留）。
 */
import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { Dashboard } from "./Overview";
import { ClipboardHistoryPanel } from "./ToolboxClipboard";
import { Diagnose } from "./Diagnose";
import { ToolboxHosts } from "./ToolboxHosts";
import { ToolboxPorts } from "./ToolboxPorts";
import { ToolboxDns } from "./ToolboxDns";
import { ToolboxSpeed } from "./ToolboxSpeed";
import { ToolboxAppNet } from "./ToolboxAppNet";
import { ToolboxQr } from "./ToolboxQr";
import { SystemPage } from "./SystemPage";
import { VaultPage } from "./VaultPage";
import { SettingsView, LogsView } from "./SettingsView";
import type { SettingsAll } from "./settingsPage";
import { applyHotkeyEcho, type HotkeyHints } from "./settingsPage";
import { applyTheme, palette, type Palette } from "./theme";
import { Badge, Empty } from "./ui";
import { Tabs } from "./ui/tabs";
import { useToast } from "./ui/toast";

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
      <div className="flex flex-col gap-2.5 p-4">
        <div className="text-[15px] font-semibold text-sk-text">{title}</div>
        <Badge tone="accent">{milestone}</Badge>
        <div className="text-sk-sub">{detail}</div>
        <Empty description="尚未接入" />
      </div>
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
  "logs",
];

const NAV_ITEMS = [
  { key: "overview", label: "🏠 概览 / 网络监控" },
  { key: "clipboard", label: "📋 剪贴板历史" },
  { key: "diagnose", label: "🔍 诊断 / 修复" },
  { key: "toolbox", label: "⚡ 工具箱" },
  { key: "system", label: "💼 系统助手" },
  { key: "vault", label: "🔒 密码库" },
  { key: "settings", label: "⚙ 设置" },
  { key: "logs", label: "📜 日志" },
];

export function App() {
  const toast = useToast();
  const [dark, setDark] = useState(false);
  // 深色偏好持久化：启动读 dark_mode 设置，切换写回（此前每次启动都回浅色）
  useEffect(() => {
    void invoke<Record<string, unknown>>("sk_settings_all")
      .then((all) => setDark(all["dark_mode"] === true))
      .catch(() => {});
  }, []);

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
  // Content 是唯一滚动容器（整窗 100vh 锁死）；切页回滚顶部——否则新页面
  // 继承上一页的滚动位置，用户会看到"新页面从中间开始"
  const contentRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    contentRef.current?.scrollTo({ top: 0 });
  }, [page]);
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

  // 应用内 Ctrl+K 呼出/收起命令面板（README 宣称的行为，Python 版有、Rust 版
  // 此前缺失）。输入框聚焦时放行（Ctrl+K 在编辑语境另有语义，不劫持）；
  // 实际开合走后端 toggle_palette（与全局热键同链路：定位光标屏、show+focus）。
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey && !e.altKey && !e.shiftKey && (e.key === "k" || e.key === "K")) {
        const t = e.target as HTMLElement | null;
        const tag = t?.tagName;
        if (tag === "INPUT" || tag === "TEXTAREA" || t?.isContentEditable) return;
        e.preventDefault();
        void invoke("sk_palette_toggle").catch(() => {});
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // 设置页保存 → 后端广播 sk://settings-changed → 这里同步运行时状态。
  // 深色开关此前只写文件不改这个 state，勾选后界面永不变暗（CDP 实锤回归）。
  useEffect(() => {
    const un = listen<{ key: string; value: unknown }>("sk://settings-changed", (ev) => {
      if (ev.payload.key === "dark_mode") setDark(ev.payload.value === true);
    });
    return () => {
      void un.then((f) => f());
    };
  }, []);

  useEffect(() => {
    void invoke<{ version: string }>("sk_version")
      .then((v) => setVersion(v.version))
      .catch(() => setVersion(""));
    // 后端气泡通知（托盘/粘贴降级提示）
    const un = listen<{ title: string; body: string }>("sk://notify", (ev) => {
      toast.info(`${ev.payload.title}：${ev.payload.body}`);
    });
    const un2 = listen<{ ok: boolean; message: string }>("sk://paste-result", (ev) => {
      if (ev.payload.message) {
        if (ev.payload.ok) toast.success(ev.payload.message);
        else toast.info(ev.payload.message);
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
  }, [toast]);

  const pal: Palette = palette(dark);

  // 子 tab 受控：palette 动作直达页内 tab（不再 DOM 点击 hack）
  // 子 tab 记忆（localStorage）：切页回来不丢——与概览速率窗口同一手法
  const [toolboxTab, setToolboxTab] = useState<string>(() => {
    const v = localStorage.getItem("sk.toolboxTab");
    return v || "hosts";
  });
  const [systemTab, setSystemTab] = useState<string>(() => {
    const v = localStorage.getItem("sk.systemTab");
    return v || "perf";
  });
  useEffect(() => {
    localStorage.setItem("sk.toolboxTab", toolboxTab);
  }, [toolboxTab]);
  useEffect(() => {
    localStorage.setItem("sk.systemTab", systemTab);
  }, [systemTab]);
  // palette quick:diag 的递增信号：Diagnose 挂载/更新时消费一次
  const [quickDiagRequest, setQuickDiagRequest] = useState(0);
  const pages: Record<string, React.ReactNode> = {
    overview: <Dashboard dark={dark} />,
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
          { key: "qrcode", label: "二维码识别", children: <ToolboxQr /> },
        ]}
      />
    ),
    system: <SystemPage initialTab={systemTab} onTabChange={setSystemTab} />,
    vault: <VaultPage />,
    settings: <SettingsView />,
    logs: <LogsView />,
  };

  return (
    /* 整窗锁死 100vh：滚动只发生在 Content 内部——Sider/Header 是固定框架，
        文档级滚动会把侧栏一起带走（用户实测：设置页一滚侧栏跟着跑）。 */
    <div className="flex h-screen overflow-hidden" style={{ background: "var(--sk-bg)" }}>
      {/* 侧栏：自建（AntD Sider/Menu 替代），固定 208px，自身可滚（项少实际不滚） */}
      <div className="relative h-screen w-[208px] shrink-0 overflow-y-auto border-r border-sk-glass">
        {/* 品牌区：渐变 S 标（accent 双色）+ 双行文案；徽标随主题取色 */}
        <div className="flex items-center gap-2.5 px-4 pb-3 pt-4">
          <div
            className="flex h-9 w-9 shrink-0 select-none items-center justify-center rounded-[10px] text-xl font-bold text-white"
            style={{
              background: `linear-gradient(135deg, ${pal.accent}, ${pal.accent_soft2})`,
              boxShadow: `0 4px 12px ${pal.shadow_color}`,
            }}
          >
            S
          </div>
          <div className="min-w-0 leading-tight">
            <div className="block text-[15px] font-semibold text-sk-text">SysKit</div>
            <div className="text-xs text-sk-sub">{version ? `v${version} ` : ""}系统工具箱</div>
          </div>
        </div>
        <nav className="flex flex-col gap-0.5 px-2">
          {NAV_ITEMS.map((it) => {
            const selected = page === it.key;
            return (
              <button
                key={it.key}
                onClick={() => setPage(it.key)}
                className={
                  "cursor-pointer rounded-[8px] border-0 px-2.5 py-2 text-left text-sm transition-colors duration-150 " +
                  (selected
                    ? "font-medium text-sk-accent"
                    : "text-sk-text hover:bg-sk-hover")
                }
                style={selected ? { background: "var(--sk-accent_soft)" } : { background: "transparent" }}
              >
                {it.label}
              </button>
            );
          })}
        </nav>
        {/* 热键速查卡：玻璃化（强玻璃类，静态内容允许 blur） */}
        <div
          className="sk-glass-strong"
          style={{
            position: "absolute",
            bottom: 12,
            left: 12,
            right: 12,
            padding: "8px 10px",
            borderRadius: 10,
            border: `1px solid ${pal.glass_border}`,
            fontSize: 11,
            lineHeight: 1.7,
            color: pal.brand_sub,
          }}
        >
          <div>快速粘贴：{hotkeys.quick}</div>
          <div>命令面板：{hotkeys.palette}</div>
          <div style={{ marginTop: 4, opacity: 0.7 }}>可在「设置」改键</div>
        </div>
      </div>

      {/* flex-1：占满侧栏外剩余宽度——缺了会收缩到内容宽，宽窗口下右侧留空底
          （换装后真机走查实锤）；min-w-0 允许内容收窄触发滚动而不是撑破 */}
      <div className="flex h-screen min-h-0 min-w-0 flex-1 flex-col overflow-hidden">
        {/* Header 目前无实际内容：只保留玻璃带分层，压到 8px 细线避免空带占高 */}
        <div className="h-2 shrink-0" />

        {/* flex 列：让 .sk-page（flex:1 0 auto）撑满剩余高度——滚动仍发生在本容器 */}
        <div
          ref={contentRef}
          className="flex min-h-0 flex-1 flex-col overflow-y-auto px-4 pb-3.5 pt-3.5"
        >
          {/* key=page 强制切页淡入（sk-page 动画），替代硬切 */}
          <div key={page} className="sk-page">
            {pages[page] ?? <Pending title={page} milestone="—" detail="未知页面" />}
          </div>
        </div>
      </div>
    </div>
  );
}
