/**
 * 概览页 + 网络监控页（对应 Python 侧 ui/dashboard_page.py）。
 *
 * 数据来源：`sk://tick` 事件（每秒一拍）+ `sk_overview_init` / `sk_stats_series` /
 * `sk_daily_report` / `sk_events` 命令。图表用 ECharts（与 Python 侧 pyqtgraph 的
 * 曲线/柱状职责对应），主题取 `--sk-*` CSS 变量。
 */
import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Card, Col, Empty, Radio, Row, Segmented, Space, Statistic, Table, Tabs, Tag, Typography } from "antd";
import ReactECharts from "echarts-for-react";

import {
  CHART_WINDOW,
  GRANULARITIES,
  REPORT_COLUMNS,
  eventLine,
  formatDuration,
  humanBytes,
  humanRate,
  pushTick,
  seriesPoints,
  stateLabel,
  summarizeReport,
  trafficCell,
  type DayReport,
  type EventItem,
  type Granularity,
  type Series,
  type Tick,
} from "./dashboard";

interface OverviewInit {
  tick: Tick | null;
  today_sent: number;
  today_recv: number;
  events: EventItem[];
  known_ssids: string[];
}

/** 图表统一风格：透明底 + 取主题变量，切主题时随 CSS 变量自动变。 */
function baseChartOption() {
  const css = getComputedStyle(document.documentElement);
  const v = (name: string, fallback: string) => css.getPropertyValue(name).trim() || fallback;
  return {
    textStyle: { color: v("--sk-text", "#2d3748"), fontSize: 12 },
    grid: { left: 8, right: 8, top: 28, bottom: 4, containLabel: true },
    tooltip: { trigger: "axis" as const },
    legend: { textStyle: { color: v("--sk-sub", "#7b8794") }, top: 0 },
    xAxis: {
      type: "category" as const,
      axisLine: { lineStyle: { color: v("--sk-grid", "#d0d7e0") } },
      axisLabel: { color: v("--sk-sub", "#7b8794") },
    },
    yAxis: {
      type: "value" as const,
      splitLine: { lineStyle: { color: v("--sk-grid", "#d0d7e0") } },
      axisLabel: { color: v("--sk-sub", "#7b8794") },
    },
  };
}

const ACCENT = "var(--sk-accent)";
const OK = "var(--sk-ok)";
const WARN = "var(--sk-warn)";

export function Dashboard() {
  const [tick, setTick] = useState<Tick | null>(null);
  const [history, setHistory] = useState<Tick[]>([]);
  const [events, setEvents] = useState<EventItem[]>([]);
  const [ssids, setSsids] = useState<string[]>([]);
  const [granularity, setGranularity] = useState<Granularity>("minute");
  const [ssid, setSsid] = useState<string>("");
  const [series, setSeries] = useState<Series | null>(null);
  const [reportDays, setReportDays] = useState<number>(7);
  const [report, setReport] = useState<DayReport[]>([]);
  const [ready, setReady] = useState(false);

  // ---------------- 初始加载 ----------------
  const init = useCallback(async () => {
    try {
      const data = await invoke<OverviewInit>("sk_overview_init");
      if (data.tick) {
        setTick(data.tick);
        setHistory((h) => pushTick(h, data.tick as Tick));
      }
      setEvents(data.events ?? []);
      setSsids(data.known_ssids ?? []);
    } catch {
      // 后端未就绪（例如纯浏览器预览）时保持空态，不弹错
    }
    setReady(true);
  }, []);

  useEffect(() => {
    void init();
  }, [init]);

  // ---------------- 每秒 tick ----------------
  useEffect(() => {
    const un = listen<Tick>("sk://tick", (ev) => {
      setTick(ev.payload);
      setHistory((h) => pushTick(h, ev.payload));
    });
    return () => {
      void un.then((f) => f());
    };
  }, []);

  // ---------------- 多粒度序列 ----------------
  const loadSeries = useCallback(async () => {
    try {
      const s = await invoke<Series>("sk_stats_series", {
        granularity,
        ssid: ssid || null,
      });
      setSeries(s);
    } catch {
      setSeries(null);
    }
  }, [granularity, ssid]);

  useEffect(() => {
    void loadSeries();
  }, [loadSeries]);

  // 事件/网络名每分钟刷新一次（事件由网络监控写入，不必每秒读盘）
  useEffect(() => {
    const t = window.setInterval(() => {
      void invoke<EventItem[]>("sk_events", { limit: 30 }).then(setEvents).catch(() => {});
      void invoke<string[]>("sk_known_ssids").then(setSsids).catch(() => {});
    }, 60_000);
    return () => window.clearInterval(t);
  }, []);

  // ---------------- 日报 ----------------
  const loadReport = useCallback(async () => {
    try {
      const r = await invoke<{ days: number; rows: DayReport[] }>("sk_daily_report", {
        days: reportDays,
      });
      setReport(r.rows ?? []);
    } catch {
      setReport([]);
    }
  }, [reportDays]);

  useEffect(() => {
    void loadReport();
  }, [loadReport]);

  // ---------------- 图表数据 ----------------
  const rateOption = useMemo(() => {
    const labels = history.map((_, i) => `${i}`);
    return {
      ...baseChartOption(),
      xAxis: { ...baseChartOption().xAxis, data: labels, show: false },
      series: [
        {
          name: "下行",
          type: "line",
          smooth: true,
          showSymbol: false,
          data: history.map((t) => t.down),
          lineStyle: { color: ACCENT },
          areaStyle: { opacity: 0.12 },
        },
        {
          name: "上行",
          type: "line",
          smooth: true,
          showSymbol: false,
          data: history.map((t) => t.up),
          lineStyle: { color: OK },
        },
      ],
    };
  }, [history]);

  const usageOption = useMemo(() => {
    const labels = history.map((_, i) => `${i}`);
    return {
      ...baseChartOption(),
      xAxis: { ...baseChartOption().xAxis, data: labels, show: false },
      yAxis: { ...baseChartOption().yAxis, max: 100, min: 0 },
      series: [
        {
          name: "CPU %",
          type: "line",
          smooth: true,
          showSymbol: false,
          data: history.map((t) => Number(t.cpu.toFixed(1))),
          lineStyle: { color: WARN },
        },
        {
          name: "内存 %",
          type: "line",
          smooth: true,
          showSymbol: false,
          data: history.map((t) => Number(t.mem.toFixed(1))),
          lineStyle: { color: ACCENT },
        },
      ],
    };
  }, [history]);

  const trafficOption = useMemo(() => {
    const pts = seriesPoints(series);
    return {
      ...baseChartOption(),
      xAxis: { ...baseChartOption().xAxis, data: pts.labels },
      series: [
        {
          name: "下行",
          type: "bar",
          stack: "t",
          data: pts.recv,
          itemStyle: { color: ACCENT },
        },
        { name: "上行", type: "bar", stack: "t", data: pts.sent, itemStyle: { color: OK } },
      ],
    };
  }, [series]);

  const summary = useMemo(() => summarizeReport(report), [report]);
  const currentState = tick ? "ok" : "offline";
  const st = stateLabel(currentState);

  return (
    <Tabs
      defaultActiveKey="overview"
      items={[
        {
          key: "overview",
          label: "概览",
          children: (
            <Space direction="vertical" size={14} style={{ width: "100%" }}>
              <Row gutter={14}>
                <Col span={6}>
                  <Card size="small" className="sk-card">
                    <Statistic
                      title="实时下行"
                      value={tick ? humanRate(tick.down) : "—"}
                      valueStyle={{ fontSize: 20 }}
                    />
                  </Card>
                </Col>
                <Col span={6}>
                  <Card size="small" className="sk-card">
                    <Statistic
                      title="实时上行"
                      value={tick ? humanRate(tick.up) : "—"}
                      valueStyle={{ fontSize: 20 }}
                    />
                  </Card>
                </Col>
                <Col span={6}>
                  <Card size="small" className="sk-card">
                    <Statistic
                      title="CPU"
                      value={tick ? tick.cpu.toFixed(1) : "—"}
                      suffix="%"
                      valueStyle={{ fontSize: 20 }}
                    />
                  </Card>
                </Col>
                <Col span={6}>
                  <Card size="small" className="sk-card">
                    <Statistic
                      title="内存"
                      value={tick ? tick.mem.toFixed(1) : "—"}
                      suffix="%"
                      valueStyle={{ fontSize: 20 }}
                    />
                  </Card>
                </Col>
              </Row>

              <Row gutter={14}>
                <Col span={8}>
                  <Card size="small" className="sk-card">
                    <Statistic
                      title="今日下行"
                      value={humanBytes(tick?.today_recv ?? 0)}
                      valueStyle={{ fontSize: 18 }}
                    />
                  </Card>
                </Col>
                <Col span={8}>
                  <Card size="small" className="sk-card">
                    <Statistic
                      title="今日上行"
                      value={humanBytes(tick?.today_sent ?? 0)}
                      valueStyle={{ fontSize: 18 }}
                    />
                  </Card>
                </Col>
                <Col span={8}>
                  <Card size="small" className="sk-card">
                    <Statistic
                      title="磁盘 读 / 写"
                      value={
                        tick ? `${humanBytes(tick.disk_r)} / ${humanBytes(tick.disk_w)}` : "—"
                      }
                      valueStyle={{ fontSize: 18 }}
                    />
                  </Card>
                </Col>
              </Row>

              <div className="sk-card">
                <Row justify="space-between" align="middle" style={{ marginBottom: 6 }}>
                  <Typography.Text strong>网络速率</Typography.Text>
                  <Tag color={st.token === "ok" ? "green" : st.token === "err" ? "red" : "orange"}>
                    {st.text}
                  </Tag>
                </Row>
                <ReactECharts option={rateOption} style={{ height: 200 }} lazyUpdate />
              </div>

              <div className="sk-card">
                <Typography.Text strong>CPU / 内存</Typography.Text>
                <ReactECharts option={usageOption} style={{ height: 200 }} lazyUpdate />
              </div>
            </Space>
          ),
        },
        {
          key: "netmon",
          label: "网络监控",
          children: (
            <Space direction="vertical" size={14} style={{ width: "100%" }}>
              <div className="sk-card">
                <Row justify="space-between" align="middle" style={{ marginBottom: 10 }}>
                  <Space>
                    <Segmented
                      value={granularity}
                      onChange={(v) => setGranularity(v as Granularity)}
                      options={GRANULARITIES.map((g) => ({ value: g.value, label: g.label }))}
                    />
                    <Radio.Group
                      size="small"
                      value={ssid}
                      onChange={(e) => setSsid(e.target.value as string)}
                      options={[
                        { label: "全部网络", value: "" },
                        ...ssids.map((s) => ({ label: s, value: s })),
                      ]}
                      optionType="button"
                    />
                  </Space>
                  <Space size={16}>
                    <Typography.Text type="secondary">
                      区间下行 {humanBytes(series?.recv ?? 0)}
                    </Typography.Text>
                    <Typography.Text type="secondary">
                      区间上行 {humanBytes(series?.sent ?? 0)}
                    </Typography.Text>
                  </Space>
                </Row>
                <ReactECharts option={trafficOption} style={{ height: 240 }} lazyUpdate />
              </div>

              <Row gutter={14}>
                <Col span={12}>
                  <div className="sk-card" style={{ minHeight: 260 }}>
                    <Row justify="space-between" align="middle" style={{ marginBottom: 8 }}>
                      <Typography.Text strong>近 7 / 30 天日报</Typography.Text>
                      <Segmented
                        size="small"
                        value={reportDays}
                        onChange={(v) => setReportDays(Number(v))}
                        options={[
                          { label: "7 天", value: 7 },
                          { label: "30 天", value: 30 },
                        ]}
                      />
                    </Row>
                    <Space size={20} style={{ marginBottom: 8 }}>
                      <Typography.Text type="secondary">
                        断网 {summary.outages} 次
                      </Typography.Text>
                      <Typography.Text type="secondary">
                        累计 {formatDuration(summary.outageSec)}
                      </Typography.Text>
                      <Typography.Text type="secondary">
                        有记录 {summary.daysWithEvents} 天
                      </Typography.Text>
                    </Space>
                    <Table<DayReport>
                      size="small"
                      rowKey="date"
                      pagination={false}
                      scroll={{ y: 200 }}
                      dataSource={report}
                      columns={[
                        { title: REPORT_COLUMNS[0], dataIndex: "date", width: 100 },
                        { title: REPORT_COLUMNS[1], dataIndex: "outages", width: 80 },
                        {
                          title: REPORT_COLUMNS[2],
                          dataIndex: "outage_sec",
                          width: 120,
                          render: (v: number) => formatDuration(v),
                        },
                        {
                          title: REPORT_COLUMNS[3],
                          dataIndex: "recv",
                          render: (v: number, r) => trafficCell(r.has_traffic, v),
                        },
                        {
                          title: REPORT_COLUMNS[4],
                          dataIndex: "sent",
                          render: (v: number, r) => trafficCell(r.has_traffic, v),
                        },
                      ]}
                    />
                  </div>
                </Col>
                <Col span={12}>
                  <div className="sk-card" style={{ minHeight: 260 }}>
                    <Typography.Text strong>事件时间线</Typography.Text>
                    <div style={{ marginTop: 8, maxHeight: 260, overflowY: "auto" }}>
                      {events.length === 0 ? (
                        <Empty
                          image={Empty.PRESENTED_IMAGE_SIMPLE}
                          description="暂无事件（网络监控在 M3 接入后写入状态切换）"
                        />
                      ) : (
                        events.map((ev, i) => (
                          <div key={`${ev.ts}-${i}`} className="sk-hint" style={{ padding: "2px 0" }}>
                            {eventLine(ev)}
                          </div>
                        ))
                      )}
                    </div>
                  </div>
                </Col>
              </Row>
            </Space>
          ),
        },
      ]}
      tabBarExtraContent={
        <Typography.Text type="secondary" className="sk-hint">
          {ready ? `曲线窗口 ${CHART_WINDOW} 拍` : "正在加载…"}
        </Typography.Text>
      }
    />
  );
}
