/**
 * 工具箱：DNS 测速 + 下载测速状态（对应 Python 侧 DnsSpeedTab / SpeedTestTab）。
 *
 * **下载测速在 Rust 版尚未接入**：Python 版的三个测速源全是 https，而 Rust 侧
 * 目前没有 TLS 能力（标准库不提供，需引入 rustls/native-tls 并做依赖决策）。
 * 本页如实显示未接入说明与原因，不做"假装测速"——一个假的速率数字会让用户
 * 据此误判网络好坏。
 */
import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Alert,
  Button,
  Card,
  Descriptions,
  Empty,
  Input,
  InputNumber,
  Space,
  Table,
  Tag,
  Typography,
  message,
} from "antd";

import {
  downloadSpeedAvailable,
  downloadSpeedNotice,
  formatLatency,
  formatRate,
  latencyToken,
  rateToken,
  recommendedProbe,
  speedStats,
  type DnsProbe,
  type DnsSpeedResult,
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

  const run = useCallback(async () => {
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
  }, [domain, times]);

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
  const [status, setStatus] = useState<SpeedtestStatus | null>(null);

  useEffect(() => {
    void invoke<SpeedtestStatus>("sk_speedtest_status")
      .then(setStatus)
      .catch(() => setStatus(null));
  }, []);

  const available = downloadSpeedAvailable(status);

  return (
    <Space direction="vertical" size={10} style={{ width: "100%" }}>
      {available ? (
        <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="下载测速已接入" />
      ) : (
        <>
          <Alert
            type="warning"
            showIcon
            message="下载测速尚未接入（Rust 版缺 HTTPS 能力）"
            description={downloadSpeedNotice(status)}
          />
          <Card size="small" title="Python 版使用的测速源（供对照）">
            <Typography.Paragraph type="secondary" className="sk-hint">
              {status?.python_urls?.length
                ? "以下地址均为 https，需要 TLS 支持："
                : "正在读取…"}
            </Typography.Paragraph>
            {status?.python_urls?.map((u) => (
              <div key={u} className="sk-hint" style={{ fontFamily: "Consolas, monospace" }}>
                {u}
              </div>
            ))}
            <Descriptions
              size="small"
              column={1}
              style={{ marginTop: 10 }}
              items={[
                { key: "a", label: "为什么不做假结果", children: "速率数字会被用来判断网络好坏；假的数字比没有更糟。" },
                { key: "b", label: "接入条件", children: "引入 rustls 或 native-tls（并确认与 WebView2/系统根证书的配合）。" },
                { key: "c", label: "替代手段", children: "当前可用「网络监控」页看实时速率与多粒度流量统计。" },
              ]}
            />
          </Card>
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
