/**
 * 系统助手：7 个子 tab 的容器（对应 Python 侧 `ui/system_page.py` 的多个 Tab）。
 *
 * 性能监控（PerfPanel）实现在本文件；启动项/磁盘/清理/电池/场景五个面板拆分
 * 在各自文件；WiFi 工具（周边 AP/信道分析/二维码分享/已存密码）在
 * WifiPasswordPanel.tsx。剪贴板 tab 已整体移除，不要恢复。
 *
 * v1.4 UI 迁移：脱离 AntD（Row/Col/Statistic/Descriptions/Table/Tabs → ui 件 + grid）；
 * tokenColor 的写死 hex 改为 --sk-* CSS 变量（Spark 经 style 属性使用，跟随换肤）。
 */
import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { ToolboxStartup } from "./ToolboxStartup";
import { CleanupPanel, DiskOverviewPanel, DiskPanel, DirDrillPanel } from "./ToolboxDisk";
import { ToolboxBattery } from "./ToolboxBattery";
import { ToolboxModes } from "./ToolboxModes";
import { WifiPasswordPanel } from "./WifiPasswordPanel";
import { DevicePanel } from "./DevicePanel";

import type { DrillRequest } from "./disk";

import {
  SERIES_LEN,
  formatRate,
  humanBytes,
  isStale,
  procsSummary,
  procRows,
  pushSample,
  signalToken,
  usageToken,
  type PerfTick,
  type TopProcs,
  type WifiInfo,
} from "./system";
import { ema, humanBytes as humanBytesPlain } from "./dashboard";
import { Badge, Button, Card, Empty, Table, type Column } from "./ui";
import { confirm } from "./ui/confirm";
import { useToast } from "./ui/toast";
import { killConfirmTextFor } from "./ports";
import { Tabs } from "./ui/tabs";

/** token → 状态色（CSS 变量；Spark 折线经 style.stroke 使用，可解析 var()）。 */
function tokenColor(token: string): string {
  switch (token) {
    case "ok":
      return "var(--sk-ok)";
    case "warn":
      return "var(--sk-warn)";
    case "err":
      return "var(--sk-err)";
    default:
      return "var(--sk-sub)";
  }
}

/** token → Badge tone（signal/usage 等 ok/warn/err token 共用）。 */
function tokenTone(token: string): "ok" | "warn" | "err" | "neutral" {
  switch (token) {
    case "ok":
      return "ok";
    case "warn":
      return "warn";
    case "err":
      return "err";
    default:
      return "neutral";
  }
}

/**
 * 磁盘分析 tab：总览（全量扫描）与下钻（逐级展开）两面板联动——
 * 总览点一级子目录 → 发一条 DrillRequest（token 每次都不同）→
 * DirDrillPanel 换根重扫并作废旧缓存（轮次112 换根作废逻辑复用）。
 */
function DiskAnalysisTab() {
  const [drill, setDrill] = useState<DrillRequest | null>(null);
  return (
    <div className="flex flex-col gap-6">
      <DiskOverviewPanel onDrill={(path) => setDrill({ path, token: Date.now() })} />
      <DirDrillPanel external={drill} />
    </div>
  );
}

/** 键值行（AntD Descriptions 的替代，同 ToolboxBattery）。 */
function KV({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex items-baseline gap-3 py-1">
      <span className="w-20 shrink-0 text-sk-sub">{label}</span>
      <span className="min-w-0 text-sk-text">{children}</span>
    </div>
  );
}

/** AntD Statistic 的替代：小标题 + 大号数值（带单位后缀）+ 附加内容。 */
function Stat({
  title,
  value,
  color,
  children,
}: {
  title: React.ReactNode;
  value: string;
  color: string;
  children?: React.ReactNode;
}) {
  return (
    <div className="px-4 pb-3 pt-3">
      <div className="text-xs text-sk-sub">{title}</div>
      <div className="mt-0.5 text-2xl font-semibold leading-none" style={{ color }}>
        {value}
      </div>
      {children}
    </div>
  );
}

/** 极简 sparkline（不引图表库）：Catmull-Rom 转三次贝塞尔的平滑曲线，
    与全应用「连续平滑曲线」统一。 */
function Spark({
  values,
  color,
  height = 46,
}: {
  values: number[];
  color: string;
  height?: number;
}) {
  if (values.length < 2) {
    return <div style={{ height }} className="sk-hint" />;
  }
  const max = Math.max(...values, 1);
  const min = Math.min(...values, 0);
  const span = max - min || 1;
  // viewBox 宽 100、preserveAspectRatio=none：曲线随容器伸缩
  const pts = values.map((v, i) => {
    const x = (i / (values.length - 1)) * 100;
    const y = height - ((v - min) / span) * (height - 4) - 2;
    return [x, y] as const;
  });
  // Catmull-Rom（张力 1/6）转三次贝塞尔：过点且处处光滑，无有棱角折线段
  let d = `M ${pts[0][0].toFixed(2)},${pts[0][1].toFixed(2)}`;
  for (let i = 0; i < pts.length - 1; i++) {
    const p0 = pts[Math.max(0, i - 1)];
    const p1 = pts[i];
    const p2 = pts[i + 1];
    const p3 = pts[Math.min(pts.length - 1, i + 2)];
    const c1x = p1[0] + (p2[0] - p0[0]) / 6;
    const c1y = p1[1] + (p2[1] - p0[1]) / 6;
    const c2x = p2[0] - (p3[0] - p1[0]) / 6;
    const c2y = p2[1] - (p3[1] - p1[1]) / 6;
    d += ` C ${c1x.toFixed(2)},${c1y.toFixed(2)} ${c2x.toFixed(2)},${c2y.toFixed(2)} ${p2[0].toFixed(2)},${p2[1].toFixed(2)}`;
  }
  return (
    <svg width="100%" height={height} viewBox={`0 0 100 ${height}`} preserveAspectRatio="none">
      {/* stroke 走 style：SVG 展示属性不吃 var()，style 才跟随主题变量 */}
      <path
        d={d}
        fill="none"
        style={{ stroke: color }}
        strokeWidth="1.2"
        vectorEffect="non-scaling-stroke"
      />
    </svg>
  );
}

export function SystemPage({
  initialTab,
  onTabChange,
}: {
  initialTab?: string;
  onTabChange?: (key: string) => void;
}) {
  return (
    <Card>
      <Tabs
        className="px-4 pb-4 pt-1"
        {...(initialTab !== undefined
          ? { activeKey: initialTab, onChange: (k: string) => onTabChange?.(k) }
          : {})}
        items={[
          { key: "perf", label: "性能监控", children: <PerfPanel /> },
          { key: "startup", label: "启动项管理", children: <ToolboxStartup /> },
          {
            key: "disk",
            label: "磁盘空间",
            children: (
              <div className="flex flex-col gap-6">
                <DiskPanel />
                <DiskAnalysisTab />
              </div>
            ),
          },
          { key: "clean", label: "垃圾清理", children: <CleanupPanel /> },
          { key: "battery", label: "电池健康", children: <ToolboxBattery /> },
          { key: "modes", label: "场景模式", children: <ToolboxModes /> },
          { key: "wifipwd", label: "WiFi 密码", children: <WifiPasswordPanel /> },
          { key: "devices", label: "设备电量", children: <DevicePanel /> },
        ]}
      />
    </Card>
  );
}

/** 内存占用最高的进程卡（memo）：每 3 秒才变的数据独立成子树，
    面板每秒的 tick 重渲染不再对 15 行表格做无谓 vdom diff。 */
const TopProcsCard = memo(function TopProcsCard({
  procs,
  failed,
  onRefresh,
}: {
  procs: TopProcs | null;
  failed: boolean;
  /** 结束进程后让父级立即重拉榜单；父级用 useCallback 保持引用稳定，memo 不失效 */
  onRefresh: () => void;
}) {
  const toast = useToast();
  // 正在结束哪一行：与启动项页同款"行级 busy"语义，避免全表按钮一起转圈
  const [killingPid, setKillingPid] = useState<number | null>(null);

  const kill = (name: string, pid: number) => {
    confirm({
      title: "结束进程",
      width: 520,
      content: (
        <div className="whitespace-pre-wrap">
          {killConfirmTextFor(name, pid) +
            "\n后端会拒绝结束系统进程（PID 0/4）与 SysKit 自身。"}
        </div>
      ),
      okText: "结束进程",
      danger: true,
      onOk: async () => {
        setKillingPid(pid);
        try {
          toast.success(await invoke<string>("sk_kill_process", { pid }));
          // 立即重拉：榜单里还躺着刚被结束的进程会误导（PID 也可能被复用）
          onRefresh();
        } catch (e) {
          toast.error(String(e));
        } finally {
          setKillingPid(null);
        }
      },
    });
  };

  const columns: Column<{ name: string; pid: number; bytes: number; share: number }>[] = [
    {
      title: "进程",
      dataIndex: "name",
      render: (v: string, r) => (
        <span className="inline-flex items-center gap-1.5">
          <span>{v}</span>
          <span className="sk-hint">PID {r.pid}</span>
        </span>
      ),
    },
    {
      title: "内存",
      dataIndex: "bytes",
      width: 130,
      render: (v: number) => humanBytesPlain(v),
    },
    {
      title: "占进程合计",
      dataIndex: "share",
      width: 130,
      render: (v: number) => <Badge tone={v >= 10 ? "warn" : "neutral"}>{v.toFixed(1)}%</Badge>,
    },
    {
      title: "",
      key: "kill",
      width: 76,
      render: (_: unknown, r) => (
        <Button
          variant="ghost"
          size="sm"
          loading={killingPid === r.pid}
          disabled={killingPid !== null && killingPid !== r.pid}
          onClick={() => kill(r.name, r.pid)}
        >
          结束
        </Button>
      ),
    },
  ];
  return (
    <Card>
      <div className="flex items-center justify-between gap-2 px-4 pb-1 pt-3">
        <span className="text-[15px] font-semibold text-sk-text">内存占用最高的进程</span>
        <span className="sk-hint">{procsSummary(procs)}</span>
      </div>
      <div className="px-4 pb-4 pt-1">
        {procs && procs.rows.length > 0 ? (
          <Table rowKey={(r) => r.pid} dataSource={procRows(procs)} columns={columns} />
        ) : (
          <Empty
            description={failed ? "读取进程列表失败（每 3 秒自动重试）" : "正在采集进程列表…"}
          />
        )}

      </div>
    </Card>
  );
});

function PerfPanel() {
  const [cpu, setCpu] = useState<number[]>([]);
  const [mem, setMem] = useState<number[]>([]);
  const [diskR, setDiskR] = useState<number[]>([]);
  const [diskW, setDiskW] = useState<number[]>([]);
  // 磁盘读写也是「上一秒的增量」，直接展示会每秒乱跳；用 EMA 平滑后再画/显示
  const [diskNow, setDiskNow] = useState({ r: 0, w: 0 });
  const diskSmooth = useRef<{ r: number; w: number } | null>(null);
  const [wifi, setWifi] = useState<WifiInfo | null>(null);
  const [wifiFailed, setWifiFailed] = useState(false);
  const [procs, setProcs] = useState<TopProcs | null>(null);
  const [procsFailed, setProcsFailed] = useState(false);
  const [last, setLast] = useState<PerfTick | null>(null);
  const lastTickAt = useRef<number | null>(null);
  // keepMounted 面板的可见性守卫：切到系统助手其他 tab 时本面板 display:none
  // （ui Tabs 用 hidden 属性切换），offsetParent 为 null——每 3 秒的 tasklist
  // （约 0.5 秒 CPU）和每 5 秒的 WiFi 查询都是白烧，必须停
  const panelRef = useRef<HTMLDivElement>(null);
  const panelHidden = () => panelRef.current?.offsetParent === null;

  // 订阅遥测（每拍 1 秒）
  useEffect(() => {
    let un: (() => void) | undefined;
    void listen<PerfTick>("sk://tick", (e) => {
      // 窗口最小化/面板不可见时跳过本拍（4 组 setState + 重渲染是纯浪费）；
      // isStale 守卫会在切回时清旧序列，两条守卫互补
      if (document.hidden || panelHidden()) {
        lastTickAt.current = null;
        return;
      }
      const t = e.payload;
      // 断流守卫：页面隐藏期间采样被跳过，切回时清空旧序列，
      // 免得把 10 分钟前的点和现在的点连成一条"平稳曲线"
      const now = Date.now();
      if (isStale(lastTickAt.current, now)) {
        setCpu([]);
        setMem([]);
        setDiskR([]);
        setDiskW([]);
        diskSmooth.current = null; // 断流后重新起算，别把旧值当基准
      }
      lastTickAt.current = now;
      setCpu((s) => pushSample(s, t.cpu));
      setMem((s) => pushSample(s, t.mem));
      const dr = ema(diskSmooth.current?.r, t.disk_r);
      const dw = ema(diskSmooth.current?.w, t.disk_w);
      diskSmooth.current = { r: dr, w: dw };
      setDiskR((s) => pushSample(s, dr));
      setDiskW((s) => pushSample(s, dw));
      setDiskNow({ r: dr, w: dw });
      setLast(t);
    }).then((f) => {
      un = f;
    });
    return () => un?.();
  }, []);

  // WiFi 与 Top 进程各自轮询：都是"昂贵"读数，不该挤进 1 秒的 tick 里
  useEffect(() => {
    const load = () =>
      void invoke<WifiInfo>("sk_wifi_info")
        .then((w) => {
          setWifi(w);
          setWifiFailed(false);
        })
        .catch(() => {
          // 失败与"还没读到"必须区分：否则读不到时会永远显示"正在读取…"骗人
          setWifi(null);
          setWifiFailed(true);
        });
    load();
    const t = window.setInterval(() => {
      if (!document.hidden && !panelHidden()) load();
    }, 5000);
    return () => window.clearInterval(t);
  }, []);

  // 抽成 useCallback：结束进程后卡片要立即重拉榜单，同时保持引用稳定
  // （否则每秒重渲染都会换新函数，TopProcsCard 的 memo 就白做了）
  const loadProcs = useCallback(() => {
    return invoke<TopProcs>("sk_top_procs", { limit: 15 })
      .then((p) => {
        setProcs(p);
        setProcsFailed(false);
      })
      .catch(() => {
        setProcs(null);
        setProcsFailed(true);
      });
  }, []);

  useEffect(() => {
    void loadProcs();
    const load = loadProcs;
    // 后端 2 秒 TTL + 原生快照 16ms（原来是 tasklist 子进程 1.4 秒，只能 3 秒一刷）。
    // 现在跟着 TTL 走 2 秒，榜单更跟手，代价可以忽略。
    const t = window.setInterval(() => {
      if (!document.hidden && !panelHidden()) load();
    }, 2000);
    return () => window.clearInterval(t);
  }, [loadProcs]);

  const latestCpu = cpu.length ? cpu[cpu.length - 1] : 0;
  const latestMem = mem.length ? mem[mem.length - 1] : 0;
  // 曲线窗口：SERIES_LEN 个点 × 1 秒一拍。写出来用户才知道"这条线是多长时间"
  const windowMin = Math.round(SERIES_LEN / 60);
  const diskPeak = useMemo(
    () => Math.max(...diskR, ...diskW, 1),
    [diskR, diskW],
  );

  return (
    <div ref={panelRef} className="flex flex-col gap-3">
      <div className="grid grid-cols-1 gap-3 md:grid-cols-4">
        <Card>
          <Stat title={`CPU · 近 ${windowMin} 分钟`} value={`${latestCpu.toFixed(1)}%`} color={tokenColor(usageToken(latestCpu))}>
            <Spark values={cpu} color={tokenColor(usageToken(latestCpu))} />
          </Stat>
        </Card>
        <Card>
          <Stat
            title={`内存${last ? `（已用 ${last.mem_used_gb.toFixed(1)} GB）` : ""} · 近 ${windowMin} 分钟`}
            value={`${latestMem.toFixed(1)}%`}
            color={tokenColor(usageToken(latestMem))}
          >
            <Spark values={mem} color={tokenColor(usageToken(latestMem))} />
          </Stat>
        </Card>
        <Card className="md:col-span-2">
          <div className="px-4 pb-3 pt-3">
            <div className="text-xs text-sk-sub">磁盘读写 · 近 {windowMin} 分钟</div>
            <div className="mt-1.5 flex flex-col gap-1.5">
              <div className="flex items-center gap-2">
                <Badge tone="ok">读 {humanBytes(diskNow.r)}</Badge>
                <Badge tone="warn">写 {humanBytes(diskNow.w)}</Badge>
                <span className="sk-hint">峰值 {humanBytes(diskPeak)}</span>
              </div>
              <Spark values={diskR} color="var(--sk-ok)" height={28} />
              <Spark values={diskW} color="var(--sk-warn)" height={28} />
            </div>
          </div>
        </Card>
      </div>

      <Card>
        <div className="flex items-center justify-between gap-2 px-4 pb-1 pt-3">
          <span className="text-[15px] font-semibold text-sk-text">WiFi 状态</span>
        </div>
        <div className={wifi?.connected ? "px-4 pb-4 pt-1" : "px-4 pb-2 pt-1"}>
          {wifi?.connected ? (
            <div className="grid grid-cols-1 gap-x-6 md:grid-cols-3">
              <KV label="网络">{wifi.ssid || <span className="sk-hint">隐藏网络</span>}</KV>
              <KV label="信号">
                <Badge tone={tokenTone(signalToken(wifi.signal))}>
                  {wifi.signal ?? "—"}%{wifi.dbm !== null ? `（约 ${wifi.dbm} dBm）` : ""} · {wifi.label}
                </Badge>
              </KV>
              <KV label="BSSID">{wifi.bssid || "—"}</KV>
              <KV label="接收链路">{formatRate(wifi.rx_kbps)}</KV>
              <KV label="发送链路">{formatRate(wifi.tx_kbps)}</KV>
            </div>
          ) : (
            <div className="sk-hint py-1 text-center text-[13px]">
              {wifi
                ? "未连接 WiFi（或本机无无线网卡）"
                : wifiFailed
                  ? "读取 WiFi 状态失败（每 5 秒自动重试）"
                  : "正在读取 WiFi 状态…"}
            </div>
          )}
        </div>
      </Card>

      <TopProcsCard procs={procs} failed={procsFailed} onRefresh={loadProcs} />

    </div>
  );
}
