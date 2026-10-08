/**
 * 概览页 + 网络监控页（对应 Python 侧 ui/dashboard_page.py）。
 *
 * 数据来源：`sk://tick` 事件（每秒一拍）+ `sk_overview_init` / `sk_stats_series` /
 * `sk_daily_report` / `sk_events` 命令。图表用 ECharts（与 Python 侧 pyqtgraph 的
 * 曲线/柱状职责对应），主题色经 chartTheme 直取 palette(dark)（切主题当帧
 * 换色，不依赖 CSS 变量写入时序，见 src/chartTheme.ts 头注）。
 */
import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Card, Col, Empty, Radio, Row, Segmented, Space, Table, Tabs, Tag, Typography } from "antd";
import ReactECharts from "echarts-for-react";

import { baseChartOption, chartAccent, chartOk, chartWarn } from "./chartTheme";
import {
  GRANULARITIES,
  RATE_WINDOWS,
  REPORT_COLUMNS,
  axisTimeLabels,
  eventLine,
  formatAxisBps,
  formatDuration,
  humanBps,
  humanBytes,
  peakPoint,
  pushTick,
  rateViewFromBuckets,
  rateViewFromTicks,
  seriesPoints,
  stateLabel,
  summarizeReport,
  trafficCell,
  type DayReport,
  type EventItem,
  type Granularity,
  type RateWindow,
  type Series,
  type SeriesRow,
  type Tick,
} from "./dashboard";
import { AppTrafficCard } from "./AppTrafficCard";

interface OverviewInit {
  tick: Tick | null;
  today_sent: number;
  today_recv: number;
  events: EventItem[];
  known_ssids: string[];
}

/**
 * 图表取色统一走 chartTheme（palette(dark) 直取）。历史教训（轮次115）：
 * 此前经 getComputedStyle 读 --sk-*，而 applyTheme 在 App 的 effect 里
 * （渲染提交后）才写变量——切主题瞬间 option memo 重算读到的还是旧主题色，
 * 速率图在桶窗口下会一直停在旧色直到换窗口。现 option 是 (data, dark) 的
 * 纯函数，切主题立即换色。
 */

/** 速率窗口 → 桶取数参数：1h 用分钟桶 60 个，1d 用小时桶 24 个。 */
const RATE_BUCKET_CFG: Record<Exclude<RateWindow, "1m">, { gran: Granularity; bucketSec: number; count: number }> = {
  "1h": { gran: "minute", bucketSec: 60, count: 60 },
  "1d": { gran: "hour", bucketSec: 3600, count: 24 },
};

export function Dashboard({ dark }: { dark: boolean }) {
  const [tick, setTick] = useState<Tick | null>(null);
  const [history, setHistory] = useState<Tick[]>([]);
  const [events, setEvents] = useState<EventItem[]>([]);
  const [ssids, setSsids] = useState<string[]>([]);
  // 粒度/速率窗口选择持久化到 localStorage：页面切换会卸载重建 Dashboard，
  // state 回默认值——用户调好的「1 天」看一眼别页回来就没了（CDP 往返实测丢失）。
  const [granularity, setGranularity] = useState<Granularity>(() => {
    const v = localStorage.getItem("sk.granularity");
    return GRANULARITIES.some((g) => g.value === v) ? (v as Granularity) : "minute";
  });
  useEffect(() => {
    localStorage.setItem("sk.granularity", granularity);
  }, [granularity]);
  const [ssid, setSsid] = useState<string>("");
  const [series, setSeries] = useState<Series | null>(null);
  const [reportDays, setReportDays] = useState<number>(7);
  const [report, setReport] = useState<DayReport[]>([]);
  // 速率曲线窗口：1m=逐拍实时；1h/1d=后端分钟/小时桶的均值速率
  const [rateWindow, setRateWindow] = useState<RateWindow>(() => {
    const v = localStorage.getItem("sk.rateWindow");
    return v === "1h" || v === "1d" ? (v as RateWindow) : "1m";
  });
  useEffect(() => {
    localStorage.setItem("sk.rateWindow", rateWindow);
  }, [rateWindow]);
  const [rateBuckets, setRateBuckets] = useState<SeriesRow[]>([]);

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
  }, []);

  useEffect(() => {
    void init();
  }, [init]);

  // ---------------- 每秒 tick ----------------
  useEffect(() => {
    const un = listen<Tick>("sk://tick", (ev) => {
      const t = ev.payload;
      t.ts = Date.now();
      setTick(t);
      setHistory((h) => pushTick(h, t));
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
      if (document.hidden) return; // 隐藏时读盘轮询无意义
      void invoke<EventItem[]>("sk_events", { limit: 30 }).then(setEvents).catch(() => {});
      void invoke<string[]>("sk_known_ssids").then(setSsids).catch(() => {});
    }, 60_000);
    return () => window.clearInterval(t);
  }, []);

  // ---------------- 速率窗口（1m 逐拍 / 1h 分钟桶 / 1d 小时桶） ----------------
  const loadRateBuckets = useCallback(async () => {
    if (rateWindow === "1m") return; // 逐拍窗口不读盘
    try {
      const cfg = RATE_BUCKET_CFG[rateWindow];
      const s = await invoke<Series>("sk_stats_series", { granularity: cfg.gran, ssid: null });
      setRateBuckets(s.rows ?? []);
    } catch {
      setRateBuckets([]); // 读盘失败画全 0 轴（桶窗口语义：无数据 ≠ 假曲线）
    }
  }, [rateWindow]);

  useEffect(() => {
    void loadRateBuckets();
  }, [loadRateBuckets]);

  // 桶窗口每分钟拉一次（进行中的最新桶要长大）；隐藏时跳过
  useEffect(() => {
    if (rateWindow === "1m") return;
    const t = window.setInterval(() => {
      if (!document.hidden) void loadRateBuckets();
    }, 60_000);
    return () => window.clearInterval(t);
  }, [rateWindow, loadRateBuckets]);

  // 速率图数据：统一到 RateView（1m 每秒重算，桶窗口只在桶更新时变）
  const rateView = useMemo(() => {
    if (rateWindow === "1m") return rateViewFromTicks(history);
    const cfg = RATE_BUCKET_CFG[rateWindow];
    return rateViewFromBuckets(rateBuckets, cfg.bucketSec, cfg.count);
  }, [rateWindow, history, rateBuckets]);

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
    if (!rateView) return {};
    const base = baseChartOption(dark);
    const { labels, tips, down, up } = rateView;
    const peak = peakPoint(down);
    return {
      ...base,
      xAxis: {
        ...base.xAxis,
        // 1m=相对秒（-59s…0s）；1h/1d=桶标签（HH:MM / HH时）
        data: labels,
        boundaryGap: false, // 曲线贴边，首尾即窗口两端
        axisLabel: { ...base.xAxis.axisLabel, hideOverlap: true },
      },
      yAxis: {
        ...base.yAxis,
        // 裸字节值（15,000,000）换成人读刻度（0 / 3MB / 6MB）
        axisLabel: { ...base.yAxis.axisLabel, formatter: formatAxisBps },
      },
      tooltip: {
        ...base.tooltip,
        formatter: (params: unknown) => {
          const arr = (Array.isArray(params) ? params : [params]) as Array<
            Record<string, unknown>
          >;
          const first = arr[0] ?? {};
          const idx = Number(first.dataIndex ?? 0);
          // 1m：绝对时刻 HH:MM:SS（缺 ts 退回轴标签）；桶窗口：日期+桶标签
          const title = tips[idx] || String(first.axisValueLabel ?? first.name ?? "");
          const rows = arr
            .map(
              (p) =>
                `${String(p.marker ?? "")}${String(p.seriesName ?? "")} ${humanBps(
                  Number(p.value) || 0,
                )}`,
            )
            .join("<br/>");
          return `${title}<br/>${rows}`;
        },
      },
      series: [
        {
          name: "下行",
          type: "line",
          smooth: true,
          showSymbol: false, // 端点不落点，读值一律走 axis 触发的 tooltip
          data: down,
          lineStyle: { color: chartAccent(dark), width: 2 },
          areaStyle: { color: chartAccent(dark), opacity: 0.12 }, // 只给下行轻填充
          markPoint: {
            symbol: "circle",
            symbolSize: 7,
            itemStyle: { color: chartAccent(dark) },
            label: {
              show: true,
              position: "top",
              fontSize: 11,
              color: chartAccent(dark),
              formatter: () => `峰值 ${humanBps(peak?.value ?? 0)}`,
            },
            // 全 0 窗口给空 data：合并模式下显式清空，防止残留上一窗口的旧峰
            data: peak && peak.value > 0 ? [{ coord: [peak.index, peak.value] }] : [],
          },
        },
        {
          name: "上行",
          type: "line",
          smooth: true,
          showSymbol: false,
          data: up,
          // 虚线 + 不填充：与实线轻填充的下行双通道区分，避免被下行面积盖住
          lineStyle: { color: chartOk(dark), width: 1.5, type: "dashed" },
        },
      ],
    };
  }, [rateView, dark]);

  const usageOption = useMemo(() => {
    const base = baseChartOption(dark);
    return {
      ...base,
      xAxis: {
        ...base.xAxis,
        data: axisTimeLabels(history), // 与速率图同一套相对时刻
        boundaryGap: false,
        axisLabel: { ...base.xAxis.axisLabel, hideOverlap: true },
      },
      yAxis: { ...base.yAxis, max: 100, min: 0 },
      tooltip: { ...base.tooltip, valueFormatter: (val: unknown) => `${Number(val).toFixed(1)}%` },
      series: [
        {
          name: "CPU",
          type: "line",
          smooth: true,
          showSymbol: false,
          data: history.map((t) => Number(t.cpu.toFixed(1))),
          lineStyle: { color: chartWarn(dark), width: 1.5 },
        },
        {
          name: "内存",
          type: "line",
          smooth: true,
          showSymbol: false,
          data: history.map((t) => Number(t.mem.toFixed(1))),
          lineStyle: { color: chartAccent(dark), width: 1.5 },
        },
      ],
    };
  }, [history, dark]);

  const trafficOption = useMemo(() => {
    const base = baseChartOption(dark);
    const pts = seriesPoints(series);
    return {
      ...base,
      // 总量轴与速率轴共用紧凑字节格式（函数只管字节量级、不带 /s）
      yAxis: {
        ...base.yAxis,
        axisLabel: { ...base.yAxis.axisLabel, formatter: formatAxisBps },
      },
      tooltip: {
        ...base.tooltip,
        valueFormatter: (val: unknown) => humanBytes(Number(val) || 0),
      },
      xAxis: { ...base.xAxis, data: pts.labels },
      series: [
        {
          name: "下行",
          type: "bar",
          stack: "t",
          data: pts.recv,
          itemStyle: { color: chartAccent(dark) },
        },
        { name: "上行", type: "bar", stack: "t", data: pts.sent, itemStyle: { color: chartOk(dark) } },
      ],
    };
  }, [series, dark]);

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
              {/* 速率面板（参考 NetSpeedMonitor 风格）：左侧紧凑数值列（↑下行/↓上行/⚙今日累计）
                  + 右侧连续实时曲线——一屏同览，数值与曲线同源同拍 */}
              <Card size="small" className="sk-card" styles={{ body: { padding: "10px 14px" } }}>
                <div style={{ display: "flex", gap: 18, alignItems: "stretch" }}>
                  <div
                    style={{
                      display: "flex",
                      flexDirection: "column",
                      justifyContent: "center",
                      gap: 6,
                      flexShrink: 0,
                      minWidth: 150,
                      borderRight: `1px solid var(--sk-border)`,
                      paddingRight: 18,
                    }}
                  >
                    <div style={{ display: "flex", alignItems: "baseline", gap: 6 }}>
                      <span style={{ color: chartOk(dark) }}>↑</span>
                      <span style={{ fontSize: 18, fontWeight: 600 }}>
                        {tick ? humanBps(tick.up) : "—"}
                      </span>
                    </div>
                    <div style={{ display: "flex", alignItems: "baseline", gap: 6 }}>
                      <span style={{ color: chartAccent(dark) }}>↓</span>
                      <span style={{ fontSize: 18, fontWeight: 600 }}>
                        {tick ? humanBps(tick.down) : "—"}
                      </span>
                    </div>
                    <div style={{ display: "flex", alignItems: "baseline", gap: 6 }}>
                      <span style={{ color: "var(--sk-accent)", fontSize: 12 }}>CPU</span>
                      <span style={{ fontWeight: 600 }}>
                        {tick ? tick.cpu.toFixed(1) : "—"}
                        <span className="sk-hint">%</span>
                      </span>
                    </div>
                    <div style={{ display: "flex", alignItems: "baseline", gap: 6 }}>
                      <span style={{ color: "var(--sk-accent)", fontSize: 12 }}>内存</span>
                      <span style={{ fontWeight: 600 }}>
                        {tick ? tick.mem.toFixed(1) : "—"}
                        <span className="sk-hint">%</span>
                      </span>
                    </div>
                    <div style={{ display: "flex", alignItems: "baseline", gap: 6 }}>
                      <span style={{ color: "var(--sk-accent)", fontSize: 12 }}>磁盘</span>
                      <span style={{ fontWeight: 600 }}>
                        {tick ? humanBps(tick.disk_r + tick.disk_w) : "—"}
                      </span>
                    </div>
                    <div className="sk-hint" style={{ display: "flex", alignItems: "baseline", gap: 6 }}>
                      <span>⚙</span>
                      <span>
                        今日 {humanBytes(tick?.today_recv ?? 0)} / {humanBytes(tick?.today_sent ?? 0)}
                      </span>
                    </div>
                  </div>
                  <div style={{ flex: 1, minWidth: 0 }}>
                    <ReactECharts option={rateOption} style={{ height: 150 }} lazyUpdate />
                  </div>
                </div>
              </Card>

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

              <AppTrafficCard />

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
        <Space size={8}>
          <Typography.Text type="secondary" className="sk-hint">
            速率曲线窗口
          </Typography.Text>
          <Segmented
            size="small"
            value={rateWindow}
            onChange={(v) => setRateWindow(v as RateWindow)}
            options={RATE_WINDOWS.map((w) => ({ value: w.value, label: w.label }))}
          />
        </Space>
      }
    />
  );
}
