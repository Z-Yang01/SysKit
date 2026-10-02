/**
 * 系统助手：性能监控（对应 Python 侧 `ui/system_page.py::PerfTab`）。
 *
 * 已交付：CPU / 内存 / 磁盘曲线 + WiFi 状态（SSID、信号、dBm、链路速率）。
 * 其余分页（启动项/电池/清理/磁盘/剪贴板/场景）逐轮补。
 */
import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Card, Col, Descriptions, Empty, Row, Space, Statistic, Tag, Typography } from "antd";

import {
  SERIES_LEN,
  formatRate,
  humanBytes,
  isStale,
  pushSample,
  signalToken,
  usageToken,
  wifiSummary,
  type PerfTick,
  type WifiInfo,
} from "./system";

function tokenColor(token: string): string {
  switch (token) {
    case "ok":
      return "#3fb950";
    case "warn":
      return "#d29922";
    case "err":
      return "#f85149";
    default:
      return "#8b949e";
  }
}

/** 极简 sparkline（不引图表库：本页只需要一条折线）。 */
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
  const pts = values
    .map((v, i) => {
      const x = (i / (values.length - 1)) * 100;
      const y = height - ((v - min) / span) * (height - 4) - 2;
      return `${x.toFixed(2)},${y.toFixed(2)}`;
    })
    .join(" ");
  return (
    <svg width="100%" height={height} viewBox={`0 0 100 ${height}`} preserveAspectRatio="none">
      <polyline points={pts} fill="none" stroke={color} strokeWidth="1.2" vectorEffect="non-scaling-stroke" />
    </svg>
  );
}

export function SystemPage() {
  const [cpu, setCpu] = useState<number[]>([]);
  const [mem, setMem] = useState<number[]>([]);
  const [diskR, setDiskR] = useState<number[]>([]);
  const [diskW, setDiskW] = useState<number[]>([]);
  const [wifi, setWifi] = useState<WifiInfo | null>(null);
  const [last, setLast] = useState<PerfTick | null>(null);
  const lastTickAt = useRef<number | null>(null);

  // 订阅遥测（每拍 1 秒）
  useEffect(() => {
    let un: (() => void) | undefined;
    void listen<PerfTick>("sk://tick", (e) => {
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

  // WiFi 单独读一次（微秒级 API；不必挤进 tick 事件里）
  useEffect(() => {
    const load = () =>
      void invoke<WifiInfo>("sk_wifi_info")
        .then(setWifi)
        .catch(() => setWifi(null));
    load();
    const t = window.setInterval(load, 5000);
    return () => window.clearInterval(t);
  }, []);

  const latestCpu = cpu.length ? cpu[cpu.length - 1] : 0;
  const latestMem = mem.length ? mem[mem.length - 1] : 0;
  const diskPeak = useMemo(
    () => Math.max(...diskR, ...diskW, 1),
    [diskR, diskW],
  );

  return (
    <Space direction="vertical" size={12} style={{ width: "100%" }}>
      <Row gutter={12}>
        <Col span={6}>
          <Card size="small" className="sk-card">
            <Statistic
              title="CPU"
              value={latestCpu}
              precision={1}
              suffix="%"
              valueStyle={{ color: tokenColor(usageToken(latestCpu)) }}
            />
            <Spark values={cpu} color={tokenColor(usageToken(latestCpu))} />
          </Card>
        </Col>
        <Col span={6}>
          <Card size="small" className="sk-card">
            <Statistic
              title={`内存${last ? `（已用 ${last.mem_used_gb.toFixed(1)} GB）` : ""}`}
              value={latestMem}
              precision={1}
              suffix="%"
              valueStyle={{ color: tokenColor(usageToken(latestMem)) }}
            />
            <Spark values={mem} color={tokenColor(usageToken(latestMem))} />
          </Card>
        </Col>
        <Col span={12}>
          <Card size="small" className="sk-card" title="磁盘读写">
            <Space direction="vertical" size={2} style={{ width: "100%" }}>
              <Space>
                <Tag color="green">读 {humanBytes(last?.disk_r ?? 0)}</Tag>
                <Tag color="orange">写 {humanBytes(last?.disk_w ?? 0)}</Tag>
                <span className="sk-hint">峰值 {humanBytes(diskPeak)}</span>
              </Space>
              <Spark values={diskR} color="#3fb950" height={28} />
              <Spark values={diskW} color="#d29922" height={28} />
            </Space>
          </Card>
        </Col>
      </Row>

      <Card
        size="small"
        className="sk-card"
        title="WiFi 状态"
        extra={<span className="sk-hint">每 5 秒刷新 · 数据来自 wlanapi.dll</span>}
      >
        {wifi?.connected ? (
          <Descriptions
            size="small"
            column={3}
            items={[
              {
                key: "ssid",
                label: "网络",
                children: wifi.ssid || <span className="sk-hint">隐藏网络</span>,
              },
              {
                key: "signal",
                label: "信号",
                children: (
                  <Tag color={signalToken(wifi.signal) === "ok" ? "green" : signalToken(wifi.signal) === "warn" ? "orange" : "red"}>
                    {wifi.signal ?? "—"}%{wifi.dbm !== null ? `（约 ${wifi.dbm} dBm）` : ""} · {wifi.label}
                  </Tag>
                ),
              },
              { key: "bssid", label: "BSSID", children: wifi.bssid || "—" },
              { key: "rx", label: "接收链路", children: formatRate(wifi.rx_kbps) },
              { key: "tx", label: "发送链路", children: formatRate(wifi.tx_kbps) },
            ]}
          />
        ) : (
          <Empty
            image={Empty.PRESENTED_IMAGE_SIMPLE}
            description={wifi ? "未连接 WiFi（或本机无无线网卡）" : "正在读取 WiFi 状态…"}
          />
        )}
      </Card>

      <Card size="small" className="sk-card" title="采样窗口">
        <Typography.Paragraph type="secondary" className="sk-hint" style={{ marginBottom: 0 }}>
          曲线保留最近 {SERIES_LEN} 个采样点（1 秒/点，约 3 分钟）。页面隐藏期间采样会暂停，
          切回时若断流超过 10 分钟自动清空旧序列——避免把两段不连续的数据画成一条连续曲线。
          {last && (
            <>
              {" "}
              当前：<Tag>{wifiSummary(wifi)}</Tag>
            </>
          )}
        </Typography.Paragraph>
      </Card>
    </Space>
  );
}
