/**
 * 工具箱：DNS 查询 / 子网计算 / ping / 路由（对应 Python 侧 DnsSubnetTab + PingRouteTab）。
 *
 * v1.4 UI 迁移：脱离 AntD（Descriptions/Table/Tabs/Select/copyable → ui 件 +
 * CopyText 小件）；Row/Col 空壳栅格移除。
 */
import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import {
  PREFIX_MAX,
  PREFIX_MIN,
  dnsLevel,
  formatCount,
  formatHopHost,
  formatHopLatencies,
  hopTimedOut,
  lossToken,
  pingSummary,
  subnetRows,
  traceSummary,
  type DnsAnswer,
  type PingResult,
  type SubnetCalcResult,
  type TraceHop,
  type TraceResult,
} from "./dnstools";
import { Badge, Button, Card, Empty, Input, Table, type Column } from "./ui";
import { Tabs } from "./ui/tabs";
import { InputNumber } from "./ui/input-number";
import { Select } from "./ui/select";
import { Tooltip as UiTooltip } from "./ui/tooltip";
import { useToast } from "./ui/toast";

/** AntD Typography.copyable 的替代：值 + 点击复制（短暂反馈）。 */
function CopyText({ text, children }: { text: string; children: React.ReactNode }) {
  const [copied, setCopied] = useState(false);
  return (
    <UiTooltip title={copied ? "已复制" : "复制"}>
      <button
        type="button"
        className="inline-flex cursor-pointer items-center gap-1 rounded bg-transparent p-0 font-mono text-sk-text hover:text-sk-accent"
        onClick={() => {
          void invoke("sk_clip_copy_plain", { text }).then(() => {
            setCopied(true);
            window.setTimeout(() => setCopied(false), 1200);
          });
        }}
      >
        {children}
        <span aria-hidden className="text-xs text-sk-sub">
          {copied ? "✓" : "⧉"}
        </span>
      </button>
    </UiTooltip>
  );
}

/** 键值行（AntD Descriptions 的替代）。 */
function KV({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex items-baseline gap-3 py-1">
      <span className="w-24 shrink-0 text-sk-sub">{label}</span>
      <span className="min-w-0 text-sk-text">{children}</span>
    </div>
  );
}

function DnsPanel() {
  const toast = useToast();
  const [domain, setDomain] = useState("www.baidu.com");
  const [serverLabel, setServerLabel] = useState("系统默认");
  const [presets, setPresets] = useState<{ label: string; server: string }[]>([]);
  const [answer, setAnswer] = useState<DnsAnswer | null>(null);
  const [busy, setBusy] = useState(false);
  // 会话内的查询历史（去重、最新在前，最多 8 条）。换 DNS 服务器对比解析结果时
  // 要反复查同几个域名，历史条一点就能重查，不用重新敲
  const [history, setHistory] = useState<string[]>([]);

  useEffect(() => {
    // 预设列表随后端提供（避免前端硬编码与 Rust 常量分叉）
    void invoke<SubnetCalcResult>("sk_subnet_calc", { ip: "192.168.1.10", prefix: 24 })
      .then((r) => setPresets(r.presets ?? []))
      .catch(() => setPresets([{ label: "系统默认", server: "" }]));
  }, []);

  const query = useCallback(
    async (d?: string) => {
      // 历史条会带域名进来，参数优先于输入框当前值
      const target = (d ?? domain).trim();
      if (!target) {
        toast.info("请输入要查询的域名");
        return;
      }
      // 历史标签没有 loading 态挡点击；并发查询会互相覆盖 answer，
      // 且先完成的 finally 会提前关掉 busy（查询按钮的用时/转圈随之消失）
      if (busy) return;
      setBusy(true);
      try {
        const server = presets.find((p) => p.label === serverLabel)?.server ?? "";
        const r = await invoke<DnsAnswer>("sk_dns_query", { domain: target, server: server || null });
        setAnswer(r);
        setHistory((prev) => [target, ...prev.filter((h) => h !== target)].slice(0, 8));
        const lvl = dnsLevel(r);
        if (lvl === "success") toast.success(r.message);
        else if (lvl === "warning") toast.info(r.message);
        else toast.error(r.message);
      } catch (e) {
        toast.error(String(e));
      } finally {
        setBusy(false);
      }
    },
    [domain, serverLabel, presets, busy, toast],
  );

  const ipColumns: Column<string>[] = [
    {
      title: "解析结果",
      render: (_: unknown, ip: string) => (
        // IP 常被拿去填到别处（诊断工单/其他工具），一键复制省得选中拖拽
        <CopyText text={ip}>{ip}</CopyText>
      ),
    },
    {
      title: "类型",
      width: 90,
      render: (_: unknown, ip: string) => (ip.includes(":") ? "AAAA" : "A"),
    },
  ];

  return (
    <div className="flex flex-col gap-2.5">
      <div className="flex flex-wrap items-center gap-2">
        <Input
          className="h-7 w-[260px] text-xs"
          placeholder="要查询的域名，如 www.baidu.com"
          value={domain}
          onChange={(e) => setDomain(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void query();
          }}
        />
        <Select
          className="h-7 text-xs"
          value={serverLabel}
          onChange={setServerLabel}
          options={presets.map((p) => ({ label: p.label, value: p.label }))}
        />
        <Button variant="primary" size="sm" loading={busy} onClick={() => void query()}>
          查询
        </Button>
        <span className="sk-hint">回车即可查询</span>
      </div>

      {history.length > 0 && (
        <div className="flex flex-wrap items-center gap-1">
          <span className="sk-hint">最近:</span>
          {history.map((h) => (
            <button
              key={h}
              type="button"
              onClick={() => {
                setDomain(h);
                void query(h);
              }}
              className="cursor-pointer rounded-md border border-sk-border px-1.5 py-0.5 text-xs leading-none text-sk-sub transition-colors hover:border-sk-accent/40 hover:text-sk-text"
            >
              {h}
            </button>
          ))}
        </div>
      )}

      {answer ? (
        <>
          <div className="flex flex-wrap items-center gap-2">
            <Badge tone={answer.ok ? "ok" : "err"}>{answer.ok ? "成功" : "失败"}</Badge>
            {answer.ok && <Badge tone="accent">服务器 {answer.server || "系统默认"}</Badge>}
            {answer.ok && <Badge>{answer.ms} ms</Badge>}
            <span className="sk-hint">{answer.message}</span>
          </div>
          {answer.ips.length > 0 && (
            <Table dataSource={answer.ips} rowKey={(r) => r} columns={ipColumns} />
          )}
        </>
      ) : (
        <Empty description="输入域名后点查询" />
      )}
    </div>
  );
}

function SubnetPanel() {
  const toast = useToast();
  const [ip, setIp] = useState("192.168.1.10");
  const [prefix, setPrefix] = useState(24);
  const [result, setResult] = useState<SubnetCalcResult | null>(null);

  const calc = useCallback(async () => {
    try {
      setResult(await invoke<SubnetCalcResult>("sk_subnet_calc", { ip, prefix }));
    } catch (e) {
      setResult(null);
      toast.error(String(e));
    }
  }, [ip, prefix, toast]);

  useEffect(() => {
    void calc();
  }, [calc]);

  return (
    <div className="flex flex-col gap-2.5">
      <div className="flex flex-wrap items-center gap-2">
        <span className="sk-hint">IP:</span>
        <Input
          className="h-7 w-[180px] text-xs"
          value={ip}
          onChange={(e) => setIp(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void calc();
          }}
        />
        <span className="sk-hint">/</span>
        <InputNumber
          min={PREFIX_MIN}
          max={PREFIX_MAX}
          value={prefix}
          onChange={(v) => setPrefix(Number(v ?? 24))}
        />
        <Button variant="primary" size="sm" onClick={() => void calc()}>
          计算
        </Button>
      </div>
      {result ? (
        <div className="grid grid-cols-1 gap-x-6 md:grid-cols-2">
          {subnetRows(result).map(([label, value]) => {
            const isCount = label === "可用主机数" || label === "地址总数";
            return (
              <KV key={label} label={label}>
                {isCount ? (
                  formatCount(Number(value))
                ) : (
                  // 网段信息常被复制到别的工具/工单里，一键复制比手动选中可靠
                  <CopyText text={value}>{value}</CopyText>
                )}
              </KV>
            );
          })}
        </div>
      ) : (
        <Empty description="输入 IP 与前缀长度后计算" />
      )}
    </div>
  );
}

function PingPanel() {
  const toast = useToast();
  const [host, setHost] = useState("223.5.5.5");
  const [ping, setPing] = useState<PingResult | null>(null);
  const [trace, setTrace] = useState<TraceResult | null>(null);
  const [busy, setBusy] = useState<"" | "ping" | "trace">("");
  // 执行已用时（秒）。tracert 15 跳要 15~30 秒以上，只转圈的按钮让用户分不清
  // 是在跑还是卡死；秒数是真实信息（后端一次 invoke 没有进度事件，百分比只能编）
  const [elapsed, setElapsed] = useState(0);

  useEffect(() => {
    if (!busy) return;
    setElapsed(0);
    const t = window.setInterval(() => setElapsed((s) => s + 1), 1000);
    return () => window.clearInterval(t);
  }, [busy]);

  const doPing = useCallback(async () => {
    if (!host.trim()) {
      toast.info("请输入主机名或 IP");
      return;
    }
    // busy 是 ping/trace 共享的单值：Ping 跑着又点「路由跟踪」（或输入框回车），
    // 先完成者的 finally 会清掉 busy——仍在跑的那个就失去了 spinner 和用时反馈
    if (busy) return;
    setBusy("ping");
    try {
      const r = await invoke<PingResult>("sk_ping", { host, count: 4 });
      setPing(r);
      if (r.ok) toast.success(pingSummary(r));
      else toast.info(pingSummary(r));
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy("");
    }
  }, [host, busy, toast]);

  const doTrace = useCallback(async () => {
    if (!host.trim()) {
      toast.info("请输入主机名或 IP");
      return;
    }
    if (busy) return;
    setBusy("trace");
    try {
      const r = await invoke<TraceResult>("sk_tracert", { host, maxHops: 15 });
      setTrace(r);
      toast.success(traceSummary(r));
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy("");
    }
  }, [host, busy, toast]);

  const lossTone: "ok" | "warn" | "err" = !ping
    ? "ok"
    : lossToken(ping.loss_percent) === "ok"
      ? "ok"
      : lossToken(ping.loss_percent) === "err"
        ? "err"
        : "warn";

  const hopColumns: Column<TraceHop>[] = [
    { title: "跃点", dataIndex: "hop", width: 70 },
    {
      title: "延迟",
      width: 220,
      dataIndex: "latencies",
      render: (_: unknown, h) => formatHopLatencies(h.latencies),
    },
    {
      title: "主机",
      dataIndex: "host",
      render: (_: unknown, h) =>
        hopTimedOut(h) ? <span className="sk-hint">{formatHopHost(h)}</span> : formatHopHost(h),
    },
  ];

  return (
    <div className="flex flex-col gap-2.5">
      <div className="flex flex-wrap items-center gap-2">
        <Input
          className="h-7 w-[240px] text-xs"
          placeholder="主机名或 IP"
          value={host}
          onChange={(e) => setHost(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void doPing();
          }}
        />
        <Button
          variant="primary"
          size="sm"
          loading={busy === "ping"}
          disabled={busy === "trace"}
          onClick={() => void doPing()}
        >
          Ping
        </Button>
        <Button
          size="sm"
          loading={busy === "trace"}
          disabled={busy === "ping"}
          onClick={() => void doTrace()}
        >
          路由跟踪
        </Button>
        {busy && (
          <span className="sk-hint">
            {busy === "trace" ? "跟踪中… 已用时" : "Ping 中… 已用时"} {elapsed} 秒
          </span>
        )}
        {ping && !busy && <Badge tone={lossTone}>{pingSummary(ping)}</Badge>}
      </div>

      {trace && trace.hops.length > 0 ? (
        <Table<TraceHop>
          rowKey={(r) => r.hop}
          loading={busy === "trace"}
          dataSource={trace.hops}
          columns={hopColumns}
        />
      ) : (
        <Empty
          description={
            busy === "trace"
              ? "跟踪中…（最多 15 跳）"
              : busy === "ping"
                ? "Ping 中…"
                : "点 Ping 或路由跟踪"
          }
        />
      )}

      {ping?.raw && (
        <div className="sk-hint max-h-40 overflow-auto whitespace-pre-wrap text-xs">
          {ping.raw}
        </div>
      )}

      {/* tracert 的原始输出后端一直有返回但从未展示；逐跳表格丢掉了 TTL/中间路由细节，
          疑难排查时要对照原样输出，所以补上（样式与 ping 原始输出一致） */}
      {trace?.raw && (
        <div className="sk-hint max-h-50 overflow-auto whitespace-pre-wrap text-xs">
          {trace.raw}
        </div>
      )}
    </div>
  );
}

export function ToolboxDns() {
  return (
    <Card>
      <Tabs
        className="px-4 pb-4 pt-1"
        items={[
          { key: "dns", label: "DNS 查询", children: <DnsPanel /> },
          { key: "subnet", label: "子网计算", children: <SubnetPanel /> },
          { key: "ping", label: "ping / 路由", children: <PingPanel /> },
        ]}
      />
    </Card>
  );
}
