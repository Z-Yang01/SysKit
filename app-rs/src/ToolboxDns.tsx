/**
 * 工具箱：DNS 查询 / 子网计算 / ping / 路由（对应 Python 侧 DnsSubnetTab + PingRouteTab）。
 */
import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Button,
  Card,
  Col,
  Descriptions,
  Empty,
  Input,
  InputNumber,
  Row,
  Select,
  Space,
  Table,
  Tabs,
  Tag,
  Typography,
  message,
} from "antd";

import {
  PREFIX_MAX,
  PREFIX_MIN,
  dnsLevel,
  dnsSummary,
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

function DnsPanel() {
  const [domain, setDomain] = useState("www.baidu.com");
  const [serverLabel, setServerLabel] = useState("系统默认");
  const [presets, setPresets] = useState<{ label: string; server: string }[]>([]);
  const [answer, setAnswer] = useState<DnsAnswer | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    // 预设列表随后端提供（避免前端硬编码与 Rust 常量分叉）
    void invoke<SubnetCalcResult>("sk_subnet_calc", { ip: "192.168.1.10", prefix: 24 })
      .then((r) => setPresets(r.presets ?? []))
      .catch(() => setPresets([{ label: "系统默认", server: "" }]));
  }, []);

  const query = useCallback(async () => {
    setBusy(true);
    try {
      const server = presets.find((p) => p.label === serverLabel)?.server ?? "";
      const r = await invoke<DnsAnswer>("sk_dns_query", { domain, server: server || null });
      setAnswer(r);
      const lvl = dnsLevel(r);
      message[lvl === "success" ? "success" : lvl === "warning" ? "warning" : "error"](
        r.message,
      );
    } catch (e) {
      message.error(String(e));
    } finally {
      setBusy(false);
    }
  }, [domain, serverLabel, presets]);

  return (
    <Space direction="vertical" size={10} style={{ width: "100%" }}>
      <Space wrap>
        <Input
          size="small"
          style={{ width: 260 }}
          placeholder="要查询的域名，如 www.baidu.com"
          value={domain}
          onChange={(e) => setDomain(e.target.value)}
          onPressEnter={() => void query()}
        />
        <Select
          size="small"
          style={{ width: 200 }}
          value={serverLabel}
          onChange={setServerLabel}
          options={presets.map((p) => ({ label: p.label, value: p.label }))}
        />
        <Button size="small" type="primary" loading={busy} onClick={() => void query()}>
          查询
        </Button>
        <span className="sk-hint">回车即可查询</span>
      </Space>

      {answer ? (
        <>
          <Space>
            <Tag color={answer.ok ? "green" : "red"}>{answer.ok ? "成功" : "失败"}</Tag>
            <span className="sk-hint">{dnsSummary(answer)}</span>
          </Space>
          {answer.ips.length > 0 && (
            <Table
              size="small"
              rowKey={(r: string) => r}
              pagination={false}
              dataSource={answer.ips}
              columns={[
                { title: "解析结果", dataIndex: "", render: (_: unknown, ip: string) => ip },
                { title: "类型", width: 90, render: (_: unknown, ip: string) => (ip.includes(":") ? "AAAA" : "A") },
              ]}
            />
          )}
        </>
      ) : (
        <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="输入域名后点查询" />
      )}
    </Space>
  );
}

function SubnetPanel() {
  const [ip, setIp] = useState("192.168.1.10");
  const [prefix, setPrefix] = useState(24);
  const [result, setResult] = useState<SubnetCalcResult | null>(null);

  const calc = useCallback(async () => {
    try {
      setResult(await invoke<SubnetCalcResult>("sk_subnet_calc", { ip, prefix }));
    } catch (e) {
      setResult(null);
      message.error(String(e));
    }
  }, [ip, prefix]);

  useEffect(() => {
    void calc();
  }, [calc]);

  return (
    <Space direction="vertical" size={10} style={{ width: "100%" }}>
      <Space wrap>
        <span className="sk-hint">IP:</span>
        <Input
          size="small"
          style={{ width: 180 }}
          value={ip}
          onChange={(e) => setIp(e.target.value)}
          onPressEnter={() => void calc()}
        />
        <span className="sk-hint">/</span>
        <InputNumber
          size="small"
          min={PREFIX_MIN}
          max={PREFIX_MAX}
          value={prefix}
          onChange={(v) => setPrefix(Number(v ?? 24))}
        />
        <Button size="small" type="primary" onClick={() => void calc()}>
          计算
        </Button>
      </Space>
      {result ? (
        <Descriptions
          size="small"
          bordered
          column={2}
          items={subnetRows(result).map(([label, value]) => ({
            key: label,
            label,
            children:
              label === "可用主机数" || label === "地址总数"
                ? formatCount(Number(value))
                : value,
          }))}
        />
      ) : (
        <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="输入 IP 与前缀长度后计算" />
      )}
    </Space>
  );
}

function PingPanel() {
  const [host, setHost] = useState("223.5.5.5");
  const [ping, setPing] = useState<PingResult | null>(null);
  const [trace, setTrace] = useState<TraceResult | null>(null);
  const [busy, setBusy] = useState<"" | "ping" | "trace">("");

  const doPing = useCallback(async () => {
    setBusy("ping");
    try {
      const r = await invoke<PingResult>("sk_ping", { host, count: 4 });
      setPing(r);
      message[r.ok ? "success" : "warning"](pingSummary(r));
    } catch (e) {
      message.error(String(e));
    } finally {
      setBusy("");
    }
  }, [host]);

  const doTrace = useCallback(async () => {
    setBusy("trace");
    try {
      const r = await invoke<TraceResult>("sk_tracert", { host, maxHops: 15 });
      setTrace(r);
      message.success(traceSummary(r));
    } catch (e) {
      message.error(String(e));
    } finally {
      setBusy("");
    }
  }, [host]);

  return (
    <Space direction="vertical" size={10} style={{ width: "100%" }}>
      <Space wrap>
        <Input
          size="small"
          style={{ width: 240 }}
          placeholder="主机名或 IP"
          value={host}
          onChange={(e) => setHost(e.target.value)}
          onPressEnter={() => void doPing()}
        />
        <Button size="small" type="primary" loading={busy === "ping"} onClick={() => void doPing()}>
          Ping
        </Button>
        <Button size="small" loading={busy === "trace"} onClick={() => void doTrace()}>
          路由跟踪
        </Button>
        {ping && (
          <Tag color={lossToken(ping.loss_percent) === "ok" ? "green" : lossToken(ping.loss_percent) === "err" ? "red" : "orange"}>
            {pingSummary(ping)}
          </Tag>
        )}
      </Space>

      {trace && trace.hops.length > 0 ? (
        <Table<TraceHop>
          size="small"
          rowKey="hop"
          pagination={false}
          dataSource={trace.hops}
          columns={[
            { title: "跃点", dataIndex: "hop", width: 70 },
            {
              title: "延迟",
              width: 220,
              render: (_: unknown, h) => formatHopLatencies(h.latencies),
            },
            {
              title: "主机",
              render: (_: unknown, h) =>
                hopTimedOut(h) ? (
                  <span className="sk-hint">{formatHopHost(h)}</span>
                ) : (
                  formatHopHost(h)
                ),
            },
          ]}
        />
      ) : (
        <Empty
          image={Empty.PRESENTED_IMAGE_SIMPLE}
          description={busy === "trace" ? "跟踪中…（最多 15 跳）" : "点 Ping 或路由跟踪"}
        />
      )}

      {ping?.raw && (
        <Typography.Paragraph
          style={{ whiteSpace: "pre-wrap", fontSize: 12, maxHeight: 160, overflow: "auto" }}
          type="secondary"
        >
          {ping.raw}
        </Typography.Paragraph>
      )}
    </Space>
  );
}

export function ToolboxDns() {
  return (
    <Card size="small" className="sk-card">
      <Row gutter={12}>
        <Col span={24}>
          <Tabs
            items={[
              { key: "dns", label: "DNS 查询", children: <DnsPanel /> },
              { key: "subnet", label: "子网计算", children: <SubnetPanel /> },
              { key: "ping", label: "ping / 路由", children: <PingPanel /> },
            ]}
          />
        </Col>
      </Row>
    </Card>
  );
}
