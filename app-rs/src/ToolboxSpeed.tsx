/**
 * 工具箱：DNS 测速 + 下载测速（对应 Python 侧 DnsSpeedTab / SpeedTestTab）。
 *
 * 下载测速走 WinInet（系统 TLS + 系统根证书 + 跟随系统代理），与 Python 版
 * urllib 同构：逐源独立开窗下载、峰值/平均速率、下载即断不落盘。
 *
 * v1.4 UI 迁移：脱离 AntD（Table/Tag/InputNumber/Tooltip/message → ui 件）。
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import {
  downloadSpeedAvailable,
  downloadSpeedNotice,
  formatBps,
  formatBytes,
  formatLatency,
  formatRate,
  latencyToken,
  peakToken,
  rateToken,
  recommendedProbe,
  speedStats,
  urlHost,
  type DnsProbe,
  type DnsSpeedResult,
  type DownloadTestResult,
  type DownloadUrlResult,
  type SpeedtestStatus,
} from "./speed";
import { Badge, Button, Card, Empty, Input, Table, type Column } from "./ui";
import { InputNumber } from "./ui/input-number";
import { Tooltip as UiTooltip } from "./ui/tooltip";
import { Alert } from "./ui/alert";
import { useToast } from "./ui/toast";

/** token（ok/warn/err/其他）→ Badge tone（latencyToken/rateToken/peakToken 共用）。 */
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

function DnsSpeedPanel() {
  const toast = useToast();
  const [domain, setDomain] = useState("www.baidu.com");
  const [times, setTimes] = useState(3);
  const [result, setResult] = useState<DnsSpeedResult | null>(null);
  const [busy, setBusy] = useState(false);
  // 已用时（秒）。6 台服务器并发、每台串行 times 次，最坏也要十几秒，
  // 只转圈的按钮分不清是在测还是卡死
  const [elapsed, setElapsed] = useState(0);

  useEffect(() => {
    if (!busy) return;
    setElapsed(0);
    const t = window.setInterval(() => setElapsed((s) => s + 1), 1000);
    return () => window.clearInterval(t);
  }, [busy]);

  const run = useCallback(async () => {
    // 输入框回车没有 loading 态挡重复触发；并发两轮会互相覆盖 result，
    // 且先完成的 finally 提前关 busy，用时反馈就会中途消失
    if (busy) return;
    setBusy(true);
    try {
      const r = await invoke<DnsSpeedResult>("sk_dns_speed", { domain, times });
      setResult(r);
      const s = speedStats(r);
      if (s.usable > 0) toast.success(`${s.usable} 台可用 / ${s.failed} 台无响应`);
      else toast.info(`${s.usable} 台可用 / ${s.failed} 台无响应`);
    } catch (e) {
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  }, [domain, times, busy, toast]);

  const rec = recommendedProbe(result);
  const stats = speedStats(result);

  const columns: Column<DnsProbe>[] = [
    {
      title: "DNS 服务器",
      dataIndex: "server",
      width: 180,
      render: (v: string, r) => (
        <span className="inline-flex items-center gap-1.5">
          <span className="font-mono">{v}</span>
          <span className="sk-hint">{r.name}</span>
          {r.recommended && <Badge tone="ok">推荐</Badge>}
        </span>
      ),
    },
    {
      title: "平均延迟",
      dataIndex: "avg_ms",
      width: 120,
      render: (v: number | null) => (
        <Badge tone={tokenTone(latencyToken(v))}>{formatLatency(v)}</Badge>
      ),
    },
    {
      title: "成功率",
      width: 110,
      render: (_: unknown, r) => (
        <Badge tone={tokenTone(rateToken(r.ok_count / Math.max(1, r.total)))}>{formatRate(r)}</Badge>
      ),
    },
    {
      title: "说明",
      dataIndex: "ok_count",
      render: (_: number, r) =>
        r.ok_count === 0 ? (
          <span className="sk-hint">全部超时——可能被网络屏蔽或该 DNS 不可用</span>
        ) : r.ok_count < r.total ? (
          <span className="sk-hint">丢包：{r.total - r.ok_count}/{r.total} 次无应答</span>
        ) : (
          <span className="sk-hint">稳定</span>
        ),
    },
  ];

  return (
    <div className="flex flex-col gap-2.5">
      <div className="flex flex-wrap items-center gap-2">
        <span className="sk-hint">测试域名:</span>
        <Input
          className="h-7 w-[220px] text-xs"
          value={domain}
          onChange={(e) => setDomain(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void run();
          }}
        />
        <span className="sk-hint">次数:</span>
        <InputNumber
          min={1}
          max={5}
          value={times}
          onChange={(v: number | null) => setTimes(Number(v ?? 3))}
        />
        <Button variant="primary" size="sm" loading={busy} onClick={() => void run()}>
          开始测速
        </Button>
        {busy && <span className="sk-hint">测速中… 已用时 {elapsed} 秒</span>}
        {result && (
          <>
            <Badge tone="ok">可用 {stats.usable}</Badge>
            {stats.failed > 0 && <Badge tone="err">无响应 {stats.failed}</Badge>}
          </>
        )}
      </div>

      <Alert
        tone="info"
        title="每台服务器串行探测（避免自我并发干扰），服务器之间并发；单次超时按失败计入、不重发——测速要的是真实一次往返的表现。"
      />

      {result ? (
        <>
          {rec && (
            <Alert tone="success" title={result.recommendation}>
              可到「诊断 / 修复」页用 dns_preset 动作把系统 DNS 换成它。
            </Alert>
          )}
          <Table<DnsProbe>
            rowKey={(r) => r.server}
            dataSource={result.results}
            columns={columns}
          />
        </>
      ) : (
        <Empty
          description={busy ? "测速中…（最多 6 台 × 5 次）" : "点「开始测速」对 6 台公共 DNS 做对比"}
        />
      )}
    </div>
  );
}

function DownloadSpeedPanel() {
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<DownloadTestResult | null>(null);
  const [err, setErr] = useState("");
  const [status, setStatus] = useState<SpeedtestStatus | null>(null);
  // 已用时（秒）。整轮 = 源数 × 8 秒，是用户自己能预估的真实进度上限
  const [elapsed, setElapsed] = useState(0);

  useEffect(() => {
    if (!running) return;
    setElapsed(0);
    const t = window.setInterval(() => setElapsed((s) => s + 1), 1000);
    return () => window.clearInterval(t);
  }, [running]);

  useEffect(() => {
    void invoke<SpeedtestStatus>("sk_speedtest_status")
      .then(setStatus)
      .catch(() => setStatus(null));
  }, []);

  // 逐源调用（真进度）：每源完成即入表——原来一次 invoke 3 源，用户盯
  // 空表格 24 秒不知道在干嘛。失败的单源照常入表（err 字段展示）。
  // 中止：本地标志停发后续源 + 后端 CANCEL 让当前源尽快收尾。
  // ref 立即生效（state 异步，连点「停止」会漏）；新一轮 start 时复位。
  const abortedRef = useRef(false);
  const stop = useCallback(async () => {
    abortedRef.current = true;
    try {
      await invoke("sk_download_cancel");
    } catch {
      // 后端标志仅加速当前源收尾；本地循环已停，失败不影响正确性
    }
  }, []);

  const start = useCallback(async () => {
    if (running) return;
    abortedRef.current = false;
    try {
      await invoke("sk_download_reset");
    } catch {
      // 复位失败仍可测（首轮场景标志本为 false）
    }
    setRunning(true);
    setErr("");
    setResult(null);
    const urls = status?.urls.length ? status.urls : [];
    if (urls.length === 0) {
      setErr("测速源列表未就绪，请稍后重试");
      setRunning(false);
      return;
    }
    const acc: DownloadUrlResult[] = [];
    try {
      for (const url of urls) {
        if (abortedRef.current) break; // 停止后不再发下一源
        try {
          acc.push(
            await invoke<DownloadUrlResult>("sk_download_test_one", {
              url,
              seconds: 8,
            }),
          );
        } catch (e) {
          if (abortedRef.current) break;
          acc.push({ url, ok: false, err: String(e), bytes: 0, avg_bps: 0, peak_bps: 0 });
        }
        const okCount = acc.filter((r) => r.ok).length;
        const best = acc.reduce((m, r) => Math.max(m, r.peak_bps), 0);
        setResult({ results: [...acc], ok_count: okCount, best_peak_bps: best });
      }
    } catch (e) {
      setErr(String(e));
    } finally {
      setRunning(false);
    }
  }, [status, running]);

  const available = downloadSpeedAvailable(status);
  const best = result ? formatBps(result.best_peak_bps) : "—";
  // 源数以后端能力为准（status 未到时按 3 兜底，仅用于进度预估文案）
  const estimate = (status?.urls.length || 3) * 8;

  const columns: Column<DownloadUrlResult>[] = [
    {
      title: "测速源",
      dataIndex: "url",
      render: (v: string, r) => (
        <span className="inline-flex items-center gap-1">
          {/* 主机名足够辨认来源，完整 URL 放悬浮提示；失败原因跟着行内显示 */}
          <UiTooltip title={v}>
            <span className="font-mono">{urlHost(v)}</span>
          </UiTooltip>
          {!r.ok && r.err && <span className="text-sk-err">{r.err}</span>}
        </span>
      ),
    },
    {
      title: "下载量",
      dataIndex: "bytes",
      width: 110,
      // 下载量是累计字节数，之前误用速率格式标成「xx/s」
      render: (v: number) => formatBytes(v),
    },
    {
      title: "平均",
      dataIndex: "avg_bps",
      width: 110,
      render: (v: number) => formatBps(v),
    },
    {
      title: "峰值",
      dataIndex: "peak_bps",
      width: 110,
      render: (v: number) => <Badge tone={tokenTone(peakToken(v))}>{formatBps(v)}</Badge>,
    },
  ];

  return (
    <div className="flex flex-col gap-2.5">
      {!available ? (
        <Alert tone="warning" title="下载测速尚未接入">
          {downloadSpeedNotice(status)}
        </Alert>
      ) : (
        <>
          <div className="flex flex-wrap items-center gap-2">
            {running ? (
              <Button size="sm" onClick={() => void stop()}>
                停止（已用时 {elapsed} / 约 {estimate} 秒）
              </Button>
            ) : (
              <Button variant="primary" size="sm" onClick={() => void start()}>
                开始下载测速
              </Button>
            )}
            <span className="sk-hint">
              {status?.urls.length ?? 3} 个源 × 8 秒，下载即断不落盘（约消耗几十 MB 流量）
            </span>
            <Badge tone={tokenTone(peakToken(result?.best_peak_bps ?? 0))}>峰值 {best}</Badge>
            {err && <span className="text-sk-err">{err}</span>}
          </div>
          <Table<DownloadUrlResult>
            rowKey={(r) => r.url}
            loading={running}
            columns={columns}
            dataSource={result?.results ?? []}
            empty={
              <Empty description="点「开始下载测速」——依次对 3 个测速源各下载 8 秒（约消耗几十 MB 流量）" />
            }
          />
        </>
      )}
    </div>
  );
}

export function ToolboxSpeed() {
  return (
    <Card>
      <div className="flex flex-col gap-3 px-4 pb-4 pt-3">
        <DnsSpeedPanel />
        <DownloadSpeedPanel />
      </div>
    </Card>
  );
}
