/**
 * 网络质量趋势卡（网络监控 tab）：网关/公网 延迟曲线 + 丢包率 + DNS 耗时。
 *
 * 数据来自后台 sk-netquality 探测线程（10 秒一轮 ICMP + 系统 DNS 计时），
 * `sk_quality_series` 出近 60 分钟 / 近 48 小时序列。
 * 图表主题走 chartTheme(dark)——与 Overview 其他图同一套，切主题即时跟随。
 */
import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import ReactECharts from "echarts-for-react";
import { Segmented, Space, Typography } from "antd";
import { Badge as UiBadge } from "./ui";

import {
  QUALITY_RANGES,
  avgDnsMs,
  avgLatency,
  avgLossPct,
  type QualityRange,
  type QualitySeries,
} from "./netquality";
import { baseChartOption, chartAccent, chartOk } from "./chartTheme";

export function QualityCard({ dark }: { dark: boolean }) {
  const [range, setRange] = useState<QualityRange>("minute");
  const [data, setData] = useState<QualitySeries | null>(null);

  const load = useCallback(async (r: QualityRange) => {
    try {
      setData(await invoke<QualitySeries>("sk_quality_series", { granularity: r }));
    } catch {
      setData({ granularity: r, rows: [] });
    }
  }, []);

  useEffect(() => {
    void load(range);
    // 桶粒度随 range 变化：60 分钟档每分钟一个新桶，60 秒刷一次即可跟随
    const t = window.setInterval(() => void load(range), range === "minute" ? 60_000 : 300_000);
    return () => window.clearInterval(t);
  }, [load, range]);

  const rows = data?.rows ?? [];
  const gwLoss = avgLossPct(rows, "gw");
  const pubLoss = avgLossPct(rows, "pub");
  const gwLat = avgLatency(rows, "gw");
  const pubLat = avgLatency(rows, "pub");
  const dnsMs = avgDnsMs(rows);

  const option = useMemo(() => {
    const base = baseChartOption(dark);
    return {
      ...base,
      xAxis: {
        ...base.xAxis,
        data: rows.map((r) => r.key),
        boundaryGap: false,
        axisLabel: { ...base.xAxis.axisLabel, hideOverlap: true },
      },
      yAxis: {
        ...base.yAxis,
        axisLabel: { ...base.yAxis.axisLabel, formatter: "{value} ms" },
      },
      tooltip: { ...base.tooltip, valueFormatter: (v: unknown) => `${Number(v).toFixed(1)} ms` },
      series: [
        {
          name: "网关延迟",
          type: "line",
          smooth: true,
          showSymbol: false,
          connectNulls: false, // 全丢包桶不连（断网区间如实断线）
          data: rows.map((r) => r.gw_avg_ms),
          lineStyle: { color: chartAccent(dark), width: 2 },
        },
        {
          name: "公网延迟",
          type: "line",
          smooth: true,
          showSymbol: false,
          connectNulls: false,
          data: rows.map((r) => r.pub_avg_ms),
          lineStyle: { color: chartOk(dark), width: 1.5, type: "dashed" },
        },
      ],
    };
  }, [rows, dark]);

  return (
    <div className="sk-card">
      <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
        <Space wrap>
          <Typography.Text strong>网络质量</Typography.Text>
          {gwLoss >= 5 && <UiBadge tone="err">网关丢包 {gwLoss.toFixed(1)}%</UiBadge>}
          {gwLoss > 0 && gwLoss < 5 && <UiBadge tone="warn">网关丢包 {gwLoss.toFixed(1)}%</UiBadge>}
          {gwLoss === 0 && rows.length > 0 && <UiBadge tone="ok">无丢包</UiBadge>}
        </Space>
        <Segmented
          size="small"
          value={range}
          onChange={(v) => setRange(v as QualityRange)}
          options={QUALITY_RANGES.map((r) => ({ value: r.value, label: r.label }))}
        />
      </div>

      <ReactECharts option={option} style={{ height: 180 }} lazyUpdate />

      <div className="mt-1 flex flex-wrap items-center gap-2 text-xs text-sk-sub">
        <span>
          平均延迟：网关 {gwLat !== null ? `${gwLat.toFixed(1)} ms` : "—"} · 公网{" "}
          {pubLat !== null ? `${pubLat.toFixed(1)} ms` : "—"}
        </span>
        <span>·</span>
        <span>
          丢包：网关 {gwLoss.toFixed(1)}% · 公网 {pubLoss.toFixed(1)}%
        </span>
        <span>·</span>
        <span>DNS {dnsMs !== null ? `${dnsMs.toFixed(0)} ms` : "—"}</span>
      </div>
    </div>
  );
}
