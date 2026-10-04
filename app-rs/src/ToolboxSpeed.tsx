/**
 * 工具箱：DNS 测速 + 下载测速（对应 Python 侧 DnsSpeedTab / SpeedTestTab）。
 *
 * 下载测速走 WinInet（系统 TLS + 系统根证书 + 跟随系统代理），与 Python 版
 * urllib 同构：逐源独立开窗下载、峰值/平均速率、下载即断不落盘。
 */
import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Alert,
  Button,
  Card,
  Empty,
  Input,
  InputNumber,
  Space,
  Table,
  Tag,
  Tooltip,
  message,
} from "antd";

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

function tokenColor(token: string): string {
  switch (token) {
    case "ok":
      return "green";
    case "warn":
      return "orange";
    case "err":
      return "red";
    default:
      return "default";
  }
}

function DnsSpeedPanel() {
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
      message[s.usable > 0 ? "success" : "warning"](
        `${s.usable} 台可用 / ${s.failed} 台无响应`,
      );
    } catch (e) {
      message.error(String(e));
    } finally {
      setBusy(false);
    }
  }, [domain, times, busy]);

  const rec = recommendedProbe(result);
  const stats = speedStats(result);

  return (
    <Space direction="vertical" size={10} style={{ width: "100%" }}>
      <Space wrap>
        <span className="sk-hint">测试域名:</span>
        <Input
          size="small"
          style={{ width: 220 }}
          value={domain}
          onChange={(e) => setDomain(e.target.value)}
          onPressEnter={() => void run()}
        />
        <span className="sk-hint">次数:</span>
        <InputNumber size="small" min={1} max={5} value={times} onChange={(v) => setTimes(Number(v ?? 3))} />
        <Button size="small" type="primary" loading={busy} onClick={() => void run()}>
          开始测速
        </Button>
        {busy && <span className="sk-hint">测速中… 已用时 {elapsed} 秒</span>}
        {result && (
          <>
            <Tag color="green">可用 {stats.usable}</Tag>
            {stats.failed > 0 && <Tag color="red">无响应 {stats.failed}</Tag>}
          </>
        )}
      </Space>

      <Alert
        type="info"
        showIcon
        message="每台服务器串行探测（避免自我并发干扰），服务器之间并发；单次超时按失败计入、不重发——测速要的是真实一次往返的表现。"
      />

      {result ? (
        <>
          {rec && (
            <Alert
              type="success"
              showIcon
              message={result.recommendation}
              description="可到「诊断 / 修复」页用 dns_preset 动作把系统 DNS 换成它。"
            />
          )}
          <Table<DnsProbe>
            size="small"
            rowKey="server"
            pagination={false}
            dataSource={result.results}
            columns={[
              {
                title: "DNS 服务器",
                dataIndex: "server",
                width: 180,
                render: (v: string, r) => (
                  <Space size={6}>
                    <span style={{ fontFamily: "Consolas, monospace" }}>{v}</span>
                    <span className="sk-hint">{r.name}</span>
                    {r.recommended && <Tag color="green">推荐</Tag>}
                  </Space>
                ),
              },
              {
                title: "平均延迟",
                dataIndex: "avg_ms",
                width: 120,
                render: (v: number | null) => (
                  <Tag color={tokenColor(latencyToken(v))}>{formatLatency(v)}</Tag>
                ),
              },
              {
                title: "成功率",
                width: 110,
                render: (_: unknown, r) => (
                  <Tag color={tokenColor(rateToken(r.ok_count / Math.max(1, r.total)))}>
                    {formatRate(r)}
                  </Tag>
                ),
              },
              {
                title: "说明",
                render: (_: unknown, r) =>
                  r.ok_count === 0 ? (
                    <span className="sk-hint">全部超时——可能被网络屏蔽或该 DNS 不可用</span>
                  ) : r.ok_count < r.total ? (
                    <span className="sk-hint">丢包：{r.total - r.ok_count}/{r.total} 次无应答</span>
                  ) : (
                    <span className="sk-hint">稳定</span>
                  ),
              },
            ]}
          />
        </>
      ) : (
        <Empty
          image={Empty.PRESENTED_IMAGE_SIMPLE}
          description={busy ? "测速中…（最多 6 台 × 5 次）" : "点「开始测速」对 6 台公共 DNS 做对比"}
        />
      )}
    </Space>
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
  const start = useCallback(async () => {
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
        try {
          acc.push(
            await invoke<DownloadUrlResult>("sk_download_test_one", {
              url,
              seconds: 8,
            }),
          );
        } catch (e) {
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
  }, [status]);

  const available = downloadSpeedAvailable(status);
  const best = result ? formatBps(result.best_peak_bps) : "—";
  // 源数以后端能力为准（status 未到时按 3 兜底，仅用于进度预估文案）
  const estimate = (status?.urls.length || 3) * 8;

  return (
    <Space direction="vertical" size={10} style={{ width: "100%" }}>
      {!available ? (
        <>
          <Alert
            type="warning"
            showIcon
            message="下载测速尚未接入"
            description={downloadSpeedNotice(status)}
          />
        </>
      ) : (
        <>
          <Space wrap>
            <Button type="primary" size="small" loading={running} onClick={() => void start()}>
              {running ? `测速中… ${elapsed} / 约 ${estimate} 秒` : "开始下载测速"}
            </Button>
            <span className="sk-hint">
              {status?.urls.length ?? 3} 个源 × 8 秒，下载即断不落盘（约消耗几十 MB 流量）
            </span>
            <Tag color={peakToken(result?.best_peak_bps ?? 0)}>
              峰值 {best}
            </Tag>
            {err && <span style={{ color: "var(--sk-err)" }}>{err}</span>}
          </Space>
          <Table<DownloadUrlResult>
            size="small"
            rowKey="url"
            loading={running}
            columns={[
              {
                title: "测速源",
                dataIndex: "url",
                ellipsis: true,
                render: (v: string, r) => (
                  <Space size={4}>
                    {/* 主机名足够辨认来源，完整 URL 放悬浮提示；失败原因跟着行内显示 */}
                    <Tooltip title={v}>
                      <span style={{ fontFamily: "Consolas, monospace" }}>{urlHost(v)}</span>
                    </Tooltip>
                    {!r.ok && r.err && <span style={{ color: "var(--sk-err)" }}>{r.err}</span>}
                  </Space>
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
                render: (v: number) => <Tag color={peakToken(v)}>{formatBps(v)}</Tag>,
              },
            ]}
            dataSource={result?.results ?? []}
            pagination={false}
            locale={{ emptyText: "点「开始下载测速」——依次对 3 个测速源各下载 8 秒（约消耗几十 MB 流量）" }}
          />
        </>
      )}
    </Space>
  );
}

export function ToolboxSpeed() {
  return (
    <Card size="small" className="sk-card">
      <Space direction="vertical" size={12} style={{ width: "100%" }}>
        <DnsSpeedPanel />
        <DownloadSpeedPanel />
      </Space>
    </Card>
  );
}
