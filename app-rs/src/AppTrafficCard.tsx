/**
 * 按应用流量卡（网络监控 tab）：今天 / 本周 / 本月 × 应用过滤 × Top 15。
 *
 * 数据 ETW 采集（需管理员）：available=false 时给诚实的降级说明——
 * 一张空表会让人误以为「真的没流量」。
 *
 * v1.4 UI 迁移：脱离 AntD（Table/Progress/Segmented/Row/Col → ui 件 + flex）。
 */
import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import {
  APP_RANGES,
  sharePct,
  topRows,
  type AppTrafficRange,
  type AppTrafficSummary,
  type AppUsageRow,
} from "./apptraffic";
import { humanBytes } from "./dashboard";
import { Empty, Input, Progress, Segmented, Table, type Column } from "./ui";
import { Alert } from "./ui/alert";

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

  const columns: Column<AppUsageRow>[] = [
    {
      title: COLUMNS[0],
      dataIndex: "app",
      render: (v: string) => (
        <code className="rounded bg-sk-hover px-1.5 py-0.5 font-mono text-xs">{v}</code>
      ),
    },
    {
      title: COLUMNS[1],
      dataIndex: "recv",
      width: 96,
      align: "right",
      render: (v: number) => humanBytes(v),
    },
    {
      title: COLUMNS[2],
      dataIndex: "sent",
      width: 96,
      align: "right",
      render: (v: number) => humanBytes(v),
    },
    {
      title: COLUMNS[3],
      key: "total",
      width: 110,
      align: "right",
      render: (_: unknown, r: AppUsageRow) => {
        const total = r.sent + r.recv;
        return (
          <span className={r === rows[0] ? "font-semibold" : undefined}>{humanBytes(total)}</span>
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
          <span className="flex w-full items-center gap-1.5">
            <Progress percent={pct} showInfo={false} className="w-16 flex-none" />
            <span className="sk-hint">{pct.toFixed(0)}%</span>
          </span>
        );
      },
    },
  ];

  return (
    <div className="sk-card">
      <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
        <div className="flex items-center gap-2">
          <span className="text-[15px] font-semibold text-sk-text">应用流量</span>
          <span className="sk-hint">按进程实时统计（ETW），复制/下载全计入；仅本机留存</span>
        </div>
        <div className="flex items-center gap-2">
          <Input
            className="h-7 w-[150px] text-xs"
            placeholder="搜应用名…"
            value={q}
            onChange={(e) => setQ(e.target.value)}
          />
          <Segmented<AppTrafficRange>
            size="sm"
            value={range}
            onChange={setRange}
            options={APP_RANGES.map((r) => ({ value: r.value, label: r.label }))}
          />
        </div>
      </div>

      {data && !data.available ? (
        <Alert tone="info" title="应用流量暂不可用">
          {data.reason || "需要管理员身份运行"}
        </Alert>
      ) : (
        <>
          <Table<AppUsageRow>
            rowKey={(r) => r.app}
            loading={loading}
            dataSource={rows}
            empty={<Empty description="暂无数据——刚开启时需要等流量经过；或换个时间范围" />}
            columns={columns}
          />
          <div className="sk-hint mt-1">
            本范围合计：下行 {humanBytes(data?.recv ?? 0)} · 上行{" "}
            {humanBytes(data?.sent ?? 0)}（Top 15 之外的应用不计入此合计）
          </div>
        </>
      )}
    </div>
  );
}
