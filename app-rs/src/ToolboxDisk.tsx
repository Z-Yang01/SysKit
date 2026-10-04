/**
 * 系统助手：磁盘空间 + 垃圾清理（对应 Python 侧 `DiskTab` / `CleanupTab`）。
 */
import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Alert,
  Button,
  Checkbox,
  Empty,
  List,
  Modal,
  Progress,
  Space,
  Table,
  Tag,
  Typography,
  message,
} from "antd";

import {
  allTargetKeys,
  cleanConfirmText,
  cleanResultSummary,
  diskSummary,
  driveLabel,
  humanSize,
  meaningfulKeys,
  needsSpaceWarning,
  selectedBytes,
  targetResultText,
  toggleKey,
  usageColor,
  validateClean,
  type CleanRunResult,
  type CleanScan,
  type CleanTargetInfo,
  type DiskList,
  type DriveRow,
} from "./disk";

export function DiskPanel() {
  const [list, setList] = useState<DiskList | null>(null);
  const [busy, setBusy] = useState(false);
  const [loadError, setLoadError] = useState("");

  const load = useCallback(async () => {
    setBusy(true);
    try {
      setList(await invoke<DiskList>("sk_disk_list"));
      setLoadError("");
    } catch (e) {
      // 失败要和"还在读"区分开（空态才能给出重试而不是永远"正在读取…"）；
      // 已有数据时保留旧表——刷新失败不该把上一次的结果也抹掉
      setLoadError(String(e));
      message.error(`读取磁盘信息失败：${String(e)}`);
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const rows = list?.rows ?? [];
  const warnings = useMemo(() => rows.filter(needsSpaceWarning), [rows]);

  return (
    <Space direction="vertical" size={10} style={{ width: "100%" }}>
      {warnings.map((d) => (
        <Alert
          key={d.letter}
          type="warning"
          showIcon
          message={`${d.letter} 空间紧张：已用 ${d.used_percent.toFixed(0)}%，剩余 ${humanSize(d.free_total)}`}
        />
      ))}

      <Space>
        <Button size="small" type="primary" loading={busy} onClick={() => void load()}>
          刷新
        </Button>
        {/* 首次加载完成前不显示摘要："没有本地固定磁盘"在加载期会误导 */}
        {list && <span className="sk-hint">{diskSummary(rows)}</span>}
      </Space>

      {rows.length === 0 ? (
        <Empty
          image={Empty.PRESENTED_IMAGE_SIMPLE}
          description={
            list
              ? "没有枚举到任何本地卷"
              : loadError
                ? `读取磁盘信息失败：${loadError}`
                : "正在读取磁盘信息…"
          }
        >
          {loadError && (
            <Button size="small" type="primary" onClick={() => void load()}>
              重试
            </Button>
          )}
        </Empty>
      ) : (
        <Table<DriveRow>
          size="small"
          rowKey="letter"
          pagination={false}
          dataSource={rows}
          columns={[
            {
              title: "卷",
              dataIndex: "letter",
              width: 200,
              render: (_v: string, r) => (
                <Space size={6}>
                  <span>{driveLabel(r)}</span>
                  {r.removable && <Tag>可移动</Tag>}
                  <span className="sk-hint">{r.fs}</span>
                </Space>
              ),
            },
            {
              title: "使用率",
              dataIndex: "used_percent",
              width: 260,
              render: (v: number, r) => (
                <Space size={8} style={{ width: "100%" }}>
                  <Progress
                    percent={Number(v.toFixed(0))}
                    size="small"
                    status={r.token === "err" ? "exception" : "normal"}
                    strokeColor={r.token === "err" ? "#f85149" : r.token === "warn" ? "#d29922" : "#3fb950"}
                    style={{ width: 140, marginBottom: 0 }}
                  />
                  <Tag color={usageColor(r.token)}>{v.toFixed(0)}%</Tag>
                </Space>
              ),
            },
            {
              title: "已用 / 总量",
              width: 210,
              render: (_: unknown, r) => `${humanSize(r.used)} / ${humanSize(r.total)}`,
            },
            {
              title: "可用",
              dataIndex: "free_total",
              width: 120,
              render: (v: number) => humanSize(v),
            },
          ]}
        />
      )}

      <Typography.Paragraph type="secondary" className="sk-hint" style={{ marginBottom: 0 }}>
        「可用」按**整卷空闲**计算（不是当前用户配额内的可用量）——前者才是"这个盘还剩多少"。
        可移动盘不参与空间告警。
      </Typography.Paragraph>
    </Space>
  );
}

export function CleanupPanel() {
  const [scan, setScan] = useState<CleanScan | null>(null);
  const [selected, setSelected] = useState<string[]>([]);
  const [scanning, setScanning] = useState(false);
  const [cleaning, setCleaning] = useState(false);
  const [scanError, setScanError] = useState("");
  const [result, setResult] = useState<CleanRunResult | null>(null);

  const doScan = useCallback(async () => {
    setScanning(true);
    try {
      const s = await invoke<CleanScan>("sk_clean_scan");
      setScan(s);
      setScanError("");
    } catch (e) {
      // 扫描失败要和"还在扫"区分开（空态才能给出重试）；已有结果时保留旧表
      setScanError(String(e));
      message.error(`扫描失败：${String(e)}`);
    } finally {
      setScanning(false);
    }
  }, []);

  useEffect(() => {
    void doScan();
  }, [doScan]);

  const rows: CleanTargetInfo[] = scan?.rows ?? [];

  const run = useCallback(() => {
    const v = validateClean(selected);
    if (!v.ok) {
      message.warning(v.reason);
      return;
    }
    Modal.confirm({
      title: "清理选中项",
      width: 560,
      content: (
        <Typography.Paragraph style={{ whiteSpace: "pre-wrap" }}>
          {cleanConfirmText(rows, selected)}
        </Typography.Paragraph>
      ),
      okText: "开始清理",
      okButtonProps: { danger: true },
      cancelText: "取消",
      onOk: async () => {
        setCleaning(true);
        try {
          const r = await invoke<CleanRunResult>("sk_clean_run", { keys: selected });
          setResult(r);
          const skipped = (r.outcome?.failed ?? 0) > 0;
          message[skipped ? "warning" : "success"](cleanResultSummary(r));
          await doScan();
        } catch (e) {
          message.error(String(e));
        } finally {
          setCleaning(false);
        }
      },
    });
  }, [selected, rows, doScan]);

  return (
    <Space direction="vertical" size={10} style={{ width: "100%" }}>
      <Alert
        type="info"
        showIcon
        message="白名单式清理"
        description="只清理下列固定位置；只删除文件、不删除目录；正在使用的文件会自动跳过并如实汇报。"
      />

      <Space wrap>
        <Button size="small" loading={scanning} disabled={cleaning} onClick={() => void doScan()}>
          扫描大小
        </Button>
        <Button
          size="small"
          type="primary"
          danger
          loading={cleaning}
          disabled={scanning}
          onClick={run}
        >
          清理选中项
        </Button>
        {/* 勾选辅助：清理是白名单固定项，全选安全；"只选有内容的"省得清 0 字节目标 */}
        <Button
          size="small"
          disabled={rows.length === 0}
          onClick={() => setSelected(allTargetKeys(rows))}
        >
          全选
        </Button>
        <Button
          size="small"
          disabled={rows.length === 0}
          onClick={() => setSelected(meaningfulKeys(rows))}
        >
          只选有内容的
        </Button>
        <Button size="small" disabled={selected.length === 0} onClick={() => setSelected([])}>
          清空选择
        </Button>
        <span className="sk-hint">
          {scan ? `合计可清理 ${scan.total_human}` : scanError ? "扫描失败" : "正在扫描…"}
        </span>
        {selected.length > 0 && (
          <Tag color="orange">已选 {selected.length} 项 · 约 {humanSize(selectedBytes(rows, selected))}</Tag>
        )}
      </Space>

      {rows.length === 0 ? (
        <Empty
          image={Empty.PRESENTED_IMAGE_SIMPLE}
          description={
            scan
              ? "没有可清理的目标"
              : scanError
                ? `扫描失败：${scanError}`
                : "正在扫描清理目标…"
          }
        >
          {scanError && (
            <Button size="small" type="primary" onClick={() => void doScan()}>
              重试
            </Button>
          )}
        </Empty>
      ) : (
        <Checkbox.Group
          style={{ width: "100%" }}
          value={selected}
          onChange={(v) => setSelected(v as string[])}
        >
          <List
            size="small"
            dataSource={rows}
            renderItem={(r) => (
              <List.Item
                style={{ cursor: "pointer" }}
                onClick={() => setSelected((s) => toggleKey(s, r.key))}
              >
                <Space size={10} style={{ width: "100%" }}>
                  {/* checkbox 自己的点击会冒泡到行 onClick 造成二次翻转，阻断它 */}
                  <span onClick={(e) => e.stopPropagation()}>
                    <Checkbox value={r.key} />
                  </span>
                  <span style={{ minWidth: 140 }}>{r.title}</span>
                  <Tag color={(r.bytes ?? 0) > 0 ? "orange" : "default"}>
                    {(r.bytes ?? 0) > 0 ? (r.human ?? humanSize(r.bytes ?? 0)) : "无"}
                  </Tag>
                  <span className="sk-hint" style={{ fontSize: 12 }}>
                    {r.root ?? "（Shell COM 清空）"}
                  </span>
                </Space>
              </List.Item>
            )}
          />
        </Checkbox.Group>
      )}

      {result && (
        <Alert
          type={(result.outcome?.failed ?? 0) > 0 ? "warning" : "success"}
          showIcon
          message="清理结果"
          description={
            <Space direction="vertical" size={2}>
              <span>{cleanResultSummary(result)}</span>
              {result.per_target.map((t) => (
                <span key={t.key} className="sk-hint">
                  {t.title}：{targetResultText(t)}
                </span>
              ))}
            </Space>
          }
        />
      )}
    </Space>
  );
}
