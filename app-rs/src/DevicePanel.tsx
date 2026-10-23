/**
 * 系统助手 · 设备面板：已配对蓝牙设备的电量与快捷管理。
 *
 * 数据口径（后端 devices.rs 的能力边界）：
 * - 设备清单来自注册表（配对过就在），毫秒级；
 * - 电量靠 BLE GATT 读，**很多设备根本不上报**（经典耳机、老鼠标），
 *   读不到显示「—」而不是把失败当错误——这类设备在系统设置里同样看不到电量。
 *
 * 低电量提醒按「地址 + 档位」去重（20% 以下只提醒一次），否则每次刷新都会弹。
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import { Badge, Button, Card, CardBody, CardHeader, CardTitle, Empty, Table, type Column } from "./ui";
import { Alert } from "./ui/alert";
import { Tooltip as UiTooltip } from "./ui/tooltip";
import { useToast } from "./ui/toast";

export interface BtDevice {
  addr: string;
  name: string;
  kind: string;
}

/** 低电量阈值：20% 是"该充电了"的通用经验值。 */
export const LOW_BATTERY = 20;
/** 提醒去重键（localStorage）：同一设备同一档位只提醒一次。 */
const ALERTED_KEY = "sk.devices.alerted";

function loadAlerted(): Record<string, number> {
  try {
    return JSON.parse(localStorage.getItem(ALERTED_KEY) ?? "{}") as Record<string, number>;
  } catch {
    return {};
  }
}

/** 电量缓存：GATT 读取有成本（每设备最坏 3 秒），60 秒内的值直接复用。 */
const CACHE_KEY = "sk.devices.cache";
const CACHE_TTL_MS = 60_000;

type CacheEntry = { pct: number; at: number };

function loadCache(): Record<string, CacheEntry> {
  try {
    return JSON.parse(localStorage.getItem(CACHE_KEY) ?? "{}") as Record<string, CacheEntry>;
  } catch {
    return {};
  }
}

function saveCache(m: Record<string, CacheEntry>): void {
  try {
    localStorage.setItem(CACHE_KEY, JSON.stringify(m));
  } catch {
    /* 缓存是优化，失败不影响正确性 */
  }
}

/** 缓存是否还新鲜。 */
export function cacheFresh(e: CacheEntry | undefined, now: number): boolean {
  return !!e && Number.isFinite(e.pct) && now - e.at < CACHE_TTL_MS;
}

function saveAlerted(m: Record<string, number>): void {
  try {
    localStorage.setItem(ALERTED_KEY, JSON.stringify(m));
  } catch {
    /* 便利性，失败不影响功能 */
  }
}

/** 电量色调：<20 红、<40 黄、其余正常。 */
export function batteryTone(pct: number): "err" | "warn" | "ok" {
  if (pct < LOW_BATTERY) return "err";
  if (pct < 40) return "warn";
  return "ok";
}

/** 设备类型图标：只按后端给的粗分类给个一眼可辨的符号。 */
export function deviceIcon(kind: string): string {
  if (kind.includes("音频")) return "🎧";
  if (kind.includes("外设")) return "🖱";
  if (kind.includes("手机")) return "📱";
  if (kind.includes("电脑")) return "💻";
  if (kind.includes("穿戴")) return "⌚";
  return "📶";
}

export function DevicePanel() {
  const toast = useToast();
  const [devices, setDevices] = useState<BtDevice[] | null>(null);
  // 探测结果：connected = 已连接；battery = 上报的电量（null=设备不提供）
  const [probes, setProbes] = useState<Record<string, { connected: boolean; battery: number | null }>>({});
  const [loading, setLoading] = useState(false);
  const [probing, setProbing] = useState(false);
  const [err, setErr] = useState("");
  // 蓝牙无线电：区分"蓝牙没开"与"设备没连"——两者都表现为探测失败，
  // 但用户该做的事完全不同
  const [bt, setBt] = useState<{ available: boolean; on: boolean } | null>(null);
  const [cache, setCache] = useState<Record<string, CacheEntry>>(() => loadCache());
  const panelRef = useRef<HTMLDivElement>(null);

  /** 读电量：并行（后端），每个设备 4 秒超时；低电量顺便提醒一次。 */
  const probe = useCallback(
    async (list: BtDevice[]) => {
      if (list.length === 0) return;
      setProbing(true);
      try {
        const r = await invoke<{
          probe: Record<string, { connected: boolean; battery: number | null }>;
        }>("sk_devices_battery", {
          addrs: list.map((d) => d.addr),
        });
        setProbes(r.probe ?? {});
        // 写电量缓存：下一轮（含切页回来）先用缓存渲染，不必等 GATT
        const now = Date.now();
        const merged = { ...loadCache() };
        for (const [addr, p] of Object.entries(r.probe ?? {})) {
          if (p?.battery !== null && p?.battery !== undefined) {
            merged[addr] = { pct: p.battery, at: now };
          }
        }
        saveCache(merged);
        setCache(merged);
        // 低电量提醒：同一设备同一档位只弹一次（存 localStorage）
        const alerted = loadAlerted();
        const next = { ...alerted };
        for (const d of list) {
          const pct = r.probe?.[d.addr]?.battery;
          if (pct === null || pct === undefined || pct >= LOW_BATTERY) continue;
          if (next[d.addr] === pct) continue;
          next[d.addr] = pct;
          toast.info(`${d.name} 电量仅剩 ${pct}%，建议充电`);
        }
        if (JSON.stringify(next) !== JSON.stringify(alerted)) saveAlerted(next);
      } catch (e) {
        setErr(String(e));
      } finally {
        setProbing(false);
      }
    },
    [toast],
  );

  const load = useCallback(
    async (withBattery = true) => {
      setLoading(true);
      setErr("");
      try {
        // 蓝牙状态先取：关着的话探测必然全部失败，白等 3 秒还没信息量
        const st = await invoke<{ available: boolean; on: boolean }>("sk_bt_status").catch(
          () => null,
        );
        setBt(st);
        const r = await invoke<{ devices: BtDevice[] }>("sk_devices_list");
        setDevices(r.devices ?? []);
        const canProbe = st === null || !st.available || st.on;
        if (withBattery && canProbe) void probe(r.devices ?? []);
        else if (!canProbe) setProbes({});
        else setProbes({});
      } catch (e) {
        setErr(String(e));
        setDevices([]);
      } finally {
        setLoading(false);
      }
    },
    [probe],
  );

  useEffect(() => {
    void load();
  }, [load]);

  // 60 秒自动刷新清单（只刷清单，不反复读电量——GATT 每轮最坏 4 秒/设备）
  useEffect(() => {
    const t = window.setInterval(() => {
      if (document.hidden || panelRef.current?.offsetParent === null) return;
      void load(false);
    }, 60_000);
    return () => window.clearInterval(t);
  }, [load]);

  // 3 分钟自动复测"已连接"的设备：电量变化只发生在连着的时候，
  // 没必要每次把 9 台（大多数离线）全探一遍
  useEffect(() => {
    const t = window.setInterval(() => {
      if (document.hidden || panelRef.current?.offsetParent === null) return;
      if (bt && (!bt.available || !bt.on)) return;
      const live = (devices ?? []).filter((d) => probes[d.addr]?.connected);
      if (live.length > 0) void probe(live);
    }, 180_000);
    return () => window.clearInterval(t);
  }, [devices, probes, probe, bt]);

  const copy = (text: string, what: string) => {
    void navigator.clipboard
      .writeText(text)
      .then(() => toast.success(`${what}已复制`))
      .catch(() => toast.error("复制失败：剪贴板不可用"));
  };

  // 已连接的排前面：用户打开这页 90% 是为了看"现在连着哪个、还剩多少电"
  const rows = [...(devices ?? [])].sort((a, b) => {
    const ca = probes[a.addr]?.connected ? 0 : 1;
    const cb = probes[b.addr]?.connected ? 0 : 1;
    return ca - cb;
  });

  const columns: Column<BtDevice>[] = [
    {
      title: "设备",
      key: "name",
      render: (_: unknown, d) => (
        <span className="inline-flex items-center gap-1.5">
          <span aria-hidden>{deviceIcon(d.kind)}</span>
          <span className="truncate">{d.name}</span>
        </span>
      ),
    },
    { title: "类型", dataIndex: "kind", width: 130, render: (v: string) => <span className="sk-hint">{v}</span> },
    {
      title: "状态",
      key: "conn",
      width: 96,
      render: (_: unknown, d) => {
        const p = probes[d.addr];
        // 探测没结果 = 设备不在范围内/蓝牙关闭（不是错误）：说"离线"，
        // 与"配对了但没连"区分开，用户才知道该开设备还是开蓝牙
        if (!p) return <span className="sk-hint">{probing ? "探测中…" : "离线"}</span>;
        return p.connected ? <Badge tone="ok">已连接</Badge> : <span className="sk-hint">未连接</span>;
      },
    },
    {
      title: "电量",
      key: "battery",
      width: 150,
      render: (_: unknown, d) => {
        const p = probes[d.addr];
        // 探测值优先；本次没探到就用 60 秒内的缓存（弱化显示，注明是缓存）
        const cur = p?.battery ?? undefined;
        const cached = cacheFresh(cache[d.addr], Date.now()) ? cache[d.addr].pct : undefined;
        const pct = cur ?? cached;
        if (pct === undefined && p && p.battery === null) {
          return (
            <UiTooltip title="该设备未上报电量（部分耳机/鼠标不支持），系统设置里同样看不到">
              <span className="sk-hint">{probing ? "读取中…" : "未上报"}</span>
            </UiTooltip>
          );
        }
        if (pct === undefined) {
          return <span className="sk-hint">{probing ? "读取中…" : "—"}</span>;
        }
        const tone = batteryTone(pct);
        return (
          <span className="flex items-center gap-1.5">
            <span
              className="inline-block h-1.5 w-16 overflow-hidden rounded-full"
              style={{ background: "var(--sk-border)" }}
            >
              <span
                className="block h-full rounded-full"
                style={{
                  width: `${pct}%`,
                  background:
                    tone === "err" ? "var(--sk-err)" : tone === "warn" ? "var(--sk-warn)" : "var(--sk-ok)",
                }}
              />
            </span>
            <Badge tone={tone}>{pct}%</Badge>
            {cur === undefined && <span className="sk-hint text-[10px]">缓存</span>}
          </span>
        );
      },
    },
    {
      title: "地址",
      dataIndex: "addr",
      width: 150,
      render: (v: string) => (
        <button
          type="button"
          className="cursor-copy border-0 bg-transparent p-0 font-mono text-xs text-sk-sub hover:underline"
          onClick={() => copy(v, "地址")}
          title="点击复制地址"
        >
          {v}
        </button>
      ),
    },
  ];

  return (
    <div ref={panelRef} className="flex flex-col gap-2.5">
      <Card>
        <CardHeader>
          <CardTitle>蓝牙设备</CardTitle>
          <span className="flex items-center gap-2">
            <span className="sk-hint">
              {devices ? `${devices.length} 台已配对` : "读取中…"}
              {probing ? " · 读取电量中" : ""}
            </span>
            <Button size="sm" loading={loading} onClick={() => void load(true)}>
              刷新
            </Button>
            <Button
              size="sm"
              onClick={() =>
                void invoke("sk_open_bt_settings").catch((e) => toast.error(String(e)))
              }
            >
              蓝牙设置
            </Button>
          </span>
        </CardHeader>
        <CardBody>
          {bt && !bt.available && (
            <Alert tone="warning" className="mb-2.5" title="未检测到蓝牙适配器">
              这台电脑没有蓝牙（或被 BIOS / 系统完全禁用），设备列表来自历史配对记录。
            </Alert>
          )}
          {bt && bt.available && !bt.on && (
            <Alert tone="warning" className="mb-2.5" title="蓝牙已关闭">
              <span className="flex flex-wrap items-center gap-2">
                <span>蓝牙关闭时无法读取连接状态与电量，打开后点「刷新」。</span>
                <Button
                  size="sm"
                  onClick={() =>
                    void invoke("sk_open_bt_settings").catch((e) => toast.error(String(e)))
                  }
                >
                  打开蓝牙设置
                </Button>
              </span>
            </Alert>
          )}
          {err ? (
            <Empty description={`读取设备失败：${err}`} />
          ) : devices && devices.length === 0 ? (
            <Empty description="还没有配对的蓝牙设备。在系统「蓝牙和其他设备」里添加后回到这里刷新。" />
          ) : (
            <>
              <Table
                columns={columns}
                dataSource={rows}
                rowKey={(d) => d.addr}
                loading={loading && devices === null}
              />
              <div className="mt-2 sk-hint">
                电量来自设备自身广播的 GATT 电池服务：显示「未上报」说明该设备不提供电量，
                与是否连接无关（系统设置里也一样看不到）。
              </div>
            </>
          )}
        </CardBody>
      </Card>
    </div>
  );
}
