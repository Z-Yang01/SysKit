/**
 * 按应用流量卡（网络监控 tab）：今天 / 本周 / 本月 × 应用过滤 × Top 15。
 *
 * 数据 ETW 采集（需管理员）：available=false 时给诚实的降级说明——
 * 一张空表会让人误以为「真的没流量」。
 */
import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Alert,
  Col,
  Input,
  Progress,
  Row,
  Segmented,
  Space,
  Table,
  Typography,
} from "antd";

import {
  APP_RANGES,
  sharePct,
  topRows,
  type AppTrafficRange,
  type AppTrafficSummary,
  type AppUsageRow,
} from "./apptraffic";
import { humanBytes } from "./dashboard";

const COLUMNS = ["应用", "下载", "上传", "合计", "占比"] as const;

export function AppTrafficCard() {
  const [range, setRange] = useState<AppTrafficRange>("day");
  const [q, setQ] = useState("");
  const [data, setData] = useState<AppTrafficSummary | null>(null);
  const [loading, setLoading] = useState(false);

  const load = useCallback(async (r: AppTrafficRange, query: string) => {
    setLoading(true);
    try {
      setData(
        await invoke<AppTrafficSummary>("sk_apptraffic_summary", {
          range: r,
          q: query || null,
          top: 15,
        }),
      );
    } catch {
      setData({ available: false, reason: "读取失败", rows: [], sent: 0, recv: 0 });
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load(range, q);
  }, [load, range]);

  // 搜索防抖 400ms（应用名可能几十个，每键一发会抖）
  useEffect(() => {
    const t = window.setTimeout(() => void load(range, q), 400);
    return () => window.clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [q]);

  const rows = useMemo(() => topRows(data?.rows ?? []), [data]);

  const columns = [
    {
      title: COLUMNS[0],
      dataIndex: "app",
      ellipsis: true,
      render: (v: string) => <Typography.Text code>{v}</Typography.Text>,
    },
    {
      title: COLUMNS[1],
      dataIndex: "recv",
      width: 96,
      align: "right" as const,
      render: (v: number) => humanBytes(v),
    },
    {
      title: COLUMNS[2],
      dataIndex: "sent",
      width: 96,
      align: "right" as const,
      render: (v: number) => humanBytes(v),
    },
    {
      title: COLUMNS[3],
      key: "total",
      width: 110,
      align: "right" as const,
      render: (_: unknown, r: AppUsageRow) => {
        const total = r.sent + r.recv;
        return (
          <Typography.Text strong={r === rows[0]}>
            {humanBytes(total)}
          </Typography.Text>
        );
      },
    },
    {
      title: COLUMNS[4],
      key: "share",
      width: 130,
      render: (_: unknown, r: AppUsageRow) => {
        const pct = sharePct(r, data?.sent ?? 0, data?.recv ?? 0);
        return (
          <Space size={6} style={{ width: "100%" }}>
            <Progress
              percent={pct}
              showInfo={false}
              size="small"
              style={{ width: 64, marginBottom: 0 }}
            />
            <span className="sk-hint">{pct.toFixed(0)}%</span>
          </Space>
        );
      },
    },
  ];

  return (
    <div className="sk-card">
      <Row justify="space-between" align="middle" style={{ marginBottom: 8 }}>
        <Space>
          <Typography.Text strong>应用流量</Typography.Text>
          <span className="sk-hint">
            按进程实时统计（ETW），复制/下载全计入；仅本机留存
          </span>
        </Space>
        <Space>
          <Input
            size="small"
            allowClear
            placeholder="搜应用名…"
            style={{ width: 150 }}
            value={q}
            onChange={(e) => setQ(e.target.value)}
          />
          <Segmented
            size="small"
            value={range}
            onChange={(v) => setRange(v as AppTrafficRange)}
            options={APP_RANGES.map((r) => ({ value: r.value, label: r.label }))}
          />
        </Space>
      </Row>

      {data && !data.available ? (
        <Alert
          type="info"
          showIcon
          message="应用流量暂不可用"
          description={data.reason || "需要管理员身份运行"}
        />
      ) : (
        <>
          <Table<AppUsageRow>
            size="small"
            rowKey="app"
            loading={loading}
            pagination={false}
            dataSource={rows}
            locale={{ emptyText: "暂无数据——刚开启时需要等流量经过；或换个时间范围" }}
            columns={columns}
          />
          <Typography.Text type="secondary" className="sk-hint">
            本范围合计：下行 {humanBytes(data?.recv ?? 0)} · 上行{" "}
            {humanBytes(data?.sent ?? 0)}（Top 15 之外的应用不计入此合计）
          </Typography.Text>
        </>
      )}
      {!data && !loading && <Col />}
    </div>
  );
}
