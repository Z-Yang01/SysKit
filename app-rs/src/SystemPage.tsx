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
import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { ToolboxStartup } from "./ToolboxStartup";
import { CleanupPanel, DiskOverviewPanel, DiskPanel, DirDrillPanel } from "./ToolboxDisk";
import { BatteryPanel } from "./ToolboxBattery";
import { ModesPanel } from "./ToolboxModes";
import { WifiPage } from "./WifiPasswordPanel";

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
  wifiSummary,
  type PerfTick,
  type TopProcs,
  type WifiInfo,
} from "./system";
import { Badge, Card, Empty, Table, type Column } from "./ui";
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

/** 极简 sparkline（不引图表库）：逐采样迷你柱，与全应用「不用折线图」统一。 */
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
  // viewBox 宽 100、preserveAspectRatio=none：柱宽随容器伸缩，1px 间隙留缝
  const slot = 100 / values.length;
  return (
    <svg width="100%" height={height} viewBox={`0 0 100 ${height}`} preserveAspectRatio="none">
      {/* fill 走 style：SVG 展示属性不吃 var()，style 才跟随主题变量 */}
      {values.map((v, i) => {
        const h = Math.max(((v - min) / span) * (height - 2), v > 0 ? 1 : 0);
        return (
          <rect
            key={i}
            x={(i * slot).toFixed(2)}
            y={(height - h).toFixed(2)}
            width={(slot - 0.8).toFixed(2)}
            height={h.toFixed(2)}
            style={{ fill: color }}
          />
        );
      })}
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
          { key: "battery", label: "电池健康", children: <BatteryPanel /> },
          { key: "modes", label: "场景模式", children: <ModesPanel /> },
          { key: "wifipwd", label: "WiFi 密码", children: <WifiPage /> },
        ]}
      />
    </Card>
  );
}

function PerfPanel() {
  const [cpu, setCpu] = useState<number[]>([]);
  const [mem, setMem] = useState<number[]>([]);
  const [diskR, setDiskR] = useState<number[]>([]);
  const [diskW, setDiskW] = useState<number[]>([]);
  const [wifi, setWifi] = useState<WifiInfo | null>(null);
  const [wifiFailed, setWifiFailed] = useState(false);
  const [procs, setProcs] = useState<TopProcs | null>(null);
  const [procsFailed, setProcsFailed] = useState(false);
  const [last, setLast] = useState<PerfTick | null>(null);
  const lastTickAt = useRef<number | null>(null);

  // 订阅遥测（每拍 1 秒）
  useEffect(() => {
    let un: (() => void) | undefined;
    void listen<PerfTick>("sk://tick", (e) => {
      // 窗口最小化/隐藏时跳过本拍（4 组 setState + 重渲染是纯浪费）；
      // isStale 守卫会在切回时清旧序列，两条守卫互补
      if (document.hidden) {
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
      }
      lastTickAt.current = now;
      setCpu((s) => pushSample(s, t.cpu));
      setMem((s) => pushSample(s, t.mem));
      setDiskR((s) => pushSample(s, t.disk_r));
      setDiskW((s) => pushSample(s, t.disk_w));
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
      if (!document.hidden) load();
    }, 5000);
    return () => window.clearInterval(t);
  }, []);

  useEffect(() => {
    const load = () =>
      void invoke<TopProcs>("sk_top_procs", { limit: 15 })
        .then((p) => {
          setProcs(p);
          setProcsFailed(false);
        })
        .catch(() => {
          setProcs(null);
          setProcsFailed(true);
        });
    load();
    // 后端有 2 秒缓存，这里 3 秒轮询即可（tasklist 一次约 0.5 秒）
    const t = window.setInterval(() => {
      if (!document.hidden) load();
    }, 3000);
    return () => window.clearInterval(t);
  }, []);

  const latestCpu = cpu.length ? cpu[cpu.length - 1] : 0;
  const latestMem = mem.length ? mem[mem.length - 1] : 0;
  const diskPeak = useMemo(
    () => Math.max(...diskR, ...diskW, 1),
    [diskR, diskW],
  );

  const procColumns: Column<{ name: string; pid: number; bytes: number; share: number }>[] = [
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
      render: (v: number) => humanBytes(v).replace("/s", ""),
    },
    {
      title: "占进程合计",
      dataIndex: "share",
      width: 150,
      render: (v: number) => <Badge tone={v >= 10 ? "warn" : "neutral"}>{v.toFixed(1)}%</Badge>,
    },
  ];

  return (
    <div className="flex flex-col gap-3">
      <div className="grid grid-cols-1 gap-3 md:grid-cols-4">
        <Card>
          <Stat title="CPU" value={`${latestCpu.toFixed(1)}%`} color={tokenColor(usageToken(latestCpu))}>
            <Spark values={cpu} color={tokenColor(usageToken(latestCpu))} />
          </Stat>
        </Card>
        <Card>
          <Stat
            title={`内存${last ? `（已用 ${last.mem_used_gb.toFixed(1)} GB）` : ""}`}
            value={`${latestMem.toFixed(1)}%`}
            color={tokenColor(usageToken(latestMem))}
          >
            <Spark values={mem} color={tokenColor(usageToken(latestMem))} />
          </Stat>
        </Card>
        <Card className="md:col-span-2">
          <div className="px-4 pb-3 pt-3">
            <div className="text-xs text-sk-sub">磁盘读写</div>
            <div className="mt-1.5 flex flex-col gap-1.5">
              <div className="flex items-center gap-2">
                <Badge tone="ok">读 {humanBytes(last?.disk_r ?? 0)}</Badge>
                <Badge tone="warn">写 {humanBytes(last?.disk_w ?? 0)}</Badge>
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
          <span className="sk-hint">每 5 秒刷新 · 数据来自 wlanapi.dll</span>
        </div>
        <div className="px-4 pb-4 pt-1">
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
            <Empty
              description={
                wifi
                  ? "未连接 WiFi（或本机无无线网卡）"
                  : wifiFailed
                    ? "读取 WiFi 状态失败（每 5 秒自动重试）"
                    : "正在读取 WiFi 状态…"
              }
            />
          )}
        </div>
      </Card>

      <Card>
        <div className="flex items-center justify-between gap-2 px-4 pb-1 pt-3">
          <span className="text-[15px] font-semibold text-sk-text">内存占用最高的进程</span>
          <span className="sk-hint">{procsSummary(procs)}</span>
        </div>
        <div className="px-4 pb-4 pt-1">
          {procs && procs.rows.length > 0 ? (
            <Table
              rowKey={(r) => r.pid}
              dataSource={procRows(procs)}
              columns={procColumns}
            />
          ) : (
            <Empty
              description={procsFailed ? "读取进程列表失败（每 3 秒自动重试）" : "正在采集进程列表…"}
            />
          )}
          <div className="sk-hint mt-2">
            数据源为 <code>tasklist /fo csv</code>（约 0.5 秒/次，后端 2 秒缓存）。
            占比分母是**进程内存合计**而不是物理内存——tasklist 报的是工作集，合计会大于物理内存。
          </div>
        </div>
      </Card>

      <Card>
        <div className="px-4 pb-4 pt-3">
          <div className="sk-hint">
            曲线保留最近 {SERIES_LEN} 个采样点（1 秒/点，约 3 分钟）。页面隐藏期间采样会暂停，
            切回时若断流超过 10 分钟自动清空旧序列——避免把两段不连续的数据画成一条连续曲线。
            {last && <> 当前：<Badge>{wifiSummary(wifi)}</Badge></>}
          </div>
        </div>
      </Card>
    </div>
  );
}
