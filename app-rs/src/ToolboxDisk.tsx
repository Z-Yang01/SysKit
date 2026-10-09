/**
 * 系统助手：磁盘空间 + 垃圾清理（对应 Python 侧 `DiskTab` / `CleanupTab`）。
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Alert,
  Button,
  Card,
  Checkbox,
  Empty,
  Input,
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
  childIcon,
  cleanConfirmText,
  cleanResultSummary,
  collapsePath,
  countTreeNodes,
  diskSummary,
  drillErrorText,
  drillResetNeeded,
  driveLabel,
  extBars,
  firstLevelBars,
  flattenChildRows,
  humanSize,
  meaningfulKeys,
  needsSpaceWarning,
  rootEmptyText,
  selectedBytes,
  targetResultText,
  toggleExpanded,
  toggleKey,
  topFileRows,
  topFilesNote,
  treeGuard,
  truncatedNote,
  usageColor,
  validateClean,
  type ChildrenScan,
  type ChildEntry,
  type CleanRunResult,
  type CleanScan,
  type CleanTargetInfo,
  type DiskList,
  type DrillRequest,
  type DriveRow,
  type FlatChildRow,
  type OverviewBarRow,
  type ScanTreeResult,
} from "./disk";

/** 总览扩展名条形的 Top-N 行数（后端全量降序给出，前端只展示最大的几类）。 */
const OVERVIEW_EXT_ROWS = 15;

/** 总览条形/Top200 的纯 CSS 条形（不引图表库；宽度=percent，轨道用主题色）。 */
function OverviewBar({ percent }: { percent: number }) {
  return (
    <div
      style={{
        height: 6,
        borderRadius: 3,
        background: "var(--sk-border)",
        overflow: "hidden",
      }}
      aria-hidden
    >
      <div
        style={{
          width: `${percent}%`,
          height: "100%",
          borderRadius: 3,
          background: "#3fb950",
        }}
      />
    </div>
  );
}

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
                    {(r.roots ?? (r.root ? [r.root] : [])).join("；") || "（Shell COM 清空）"}
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

/**
 * 磁盘分析总览：输入根路径全量扫描（sk_disk_scan_tree），渲染根概要 +
 * 扩展名 Top-N 条形 + 最大文件 Top200 + 一级子目录条形概览。
 *
 * - 后端无进度事件：扫描期间按钮转圈 + 「取消扫描」走 sk_disk_scan_cancel
 *   （与下钻面板同一枚 SCAN_STOP）。
 * - 树**只渲染一级子目录**（结构上不递归，树再大 DOM 也不膨胀）；树超大时
 *   treeGuard 降级提示。更深层点击子目录行交给 DirDrillPanel 下钻（onDrill
 *   换根联动），本面板不重复建树渲染。
 * - 「定位」走既有 file_reveal 通道（explorer /select）。
 */
export function DiskOverviewPanel({ onDrill }: { onDrill: (path: string) => void }) {
  const [dirInput, setDirInput] = useState("");
  const [result, setResult] = useState<ScanTreeResult | null>(null);
  const [scanning, setScanning] = useState(false);
  const [scanError, setScanError] = useState("");

  const runScan = useCallback(async () => {
    const dir = dirInput.trim();
    if (!dir) {
      message.warning("请先输入要扫描的绝对路径");
      return;
    }
    setScanning(true);
    try {
      const r = await invoke<ScanTreeResult>("sk_disk_scan_tree", { root: dir });
      setResult(r);
      setScanError("");
    } catch (e) {
      // 失败与"还在扫"区分开；保留旧结果——刷新失败不该抹掉上一次的总览
      setScanError(String(e));
      message.error(`扫描失败：${String(e)}`);
    } finally {
      setScanning(false);
    }
  }, [dirInput]);

  const nodeCount = useMemo(() => (result ? countTreeNodes(result.root) : 0), [result]);
  const guard = useMemo(() => treeGuard(nodeCount), [nodeCount]);
  const bars = useMemo(
    () => (result ? firstLevelBars(result.root) : null),
    [result],
  );
  const exts = useMemo(
    () => (result ? extBars(result.ext_stats, OVERVIEW_EXT_ROWS) : null),
    [result],
  );
  const tops = useMemo(() => (result ? topFileRows(result.top_files) : []), [result]);
  const topNote = useMemo(() => (result ? topFilesNote(result.top_files) : null), [result]);

  const reveal = useCallback(async (path: string) => {
    try {
      message.success(
        await invoke<string>("sk_clip_action_run", { id: "file_reveal", arg: path }),
      );
    } catch (e) {
      message.error(`定位失败：${String(e)}`);
    }
  }, []);

  const renderBarRows = (rows: OverviewBarRow[], clickable: boolean) =>
    rows.map((r) => (
      <div key={r.path || r.label} style={{ display: "flex", alignItems: "center", gap: 8 }}>
        {clickable ? (
          <Button
            type="link"
            size="small"
            style={{
              padding: 0,
              width: 180,
              display: "inline-flex",
              justifyContent: "flex-start",
              overflow: "hidden",
              whiteSpace: "nowrap",
              textOverflow: "ellipsis",
            }}
            title={`${r.path}（点击在下方「目录逐级展开」中下钻）`}
            onClick={() => onDrill(r.path)}
          >
            {r.label}
          </Button>
        ) : (
          <span
            style={{ width: 180, overflow: "hidden", whiteSpace: "nowrap", textOverflow: "ellipsis" }}
            className="sk-hint"
            title={r.label}
          >
            {r.label}
          </span>
        )}
        <div style={{ flex: 1, minWidth: 120 }}>
          <OverviewBar percent={r.percent} />
        </div>
        <span style={{ width: 96, textAlign: "right" }} className="sk-hint">
          {humanSize(r.bytes)}
        </span>
      </div>
    ));

  return (
    <Space direction="vertical" size={10} style={{ width: "100%" }}>
      <Typography.Paragraph type="secondary" className="sk-hint" style={{ marginBottom: 0 }}>
        磁盘分析总览：输入绝对路径做全量扫描（无进度回调，大目录可能数十秒，可随时取消）。
        总览只渲染根的一级子目录与全局统计；点子目录名在下方「目录逐级展开」中逐级下钻。
      </Typography.Paragraph>

      <Space wrap>
        <Input
          size="small"
          style={{ width: 380 }}
          placeholder="绝对目录路径，如 C:\Users\x\Downloads"
          value={dirInput}
          onChange={(e) => setDirInput(e.target.value)}
          onPressEnter={() => void runScan()}
          disabled={scanning}
          aria-label="总览根路径"
        />
        <Button size="small" type="primary" loading={scanning} onClick={() => void runScan()}>
          开始扫描
        </Button>
        {scanning && (
          <Button size="small" onClick={() => void invoke("sk_disk_scan_cancel")}>
            取消扫描
          </Button>
        )}
      </Space>

      {scanError && <Alert type="error" showIcon message={`扫描失败：${scanError}`} />}

      {result && (
        <>
          <span className="sk-hint">
            {result.root.name} · 递归 {humanSize(result.root.total)}（自身文件{" "}
            {humanSize(result.root.size)}）· 文件 {result.file_count} · 目录{" "}
            {nodeCount - 1} · 跳过 {result.error_count} · {result.duration_ms} ms{" "}
            {/* 取消/整树失败都要如实标注，不得冒充完整或空目录 */}
            {result.cancelled && <Tag color="red">已取消（结果不完整）</Tag>}
            {!result.cancelled &&
              result.error_count > 0 &&
              result.root.children.length === 0 &&
              result.file_count === 0 && <Tag color="red">扫描不到内容（目录不存在或无权限）</Tag>}
          </span>

          {guard.degraded && <Alert type="warning" showIcon message="目录树较大，已降级显示" description={guard.note ?? undefined} />}

          <Card
            size="small"
            className="sk-card"
            title="扩展名统计"
            extra={
              <span className="sk-hint">
                {exts && exts.hidden > 0 ? `仅显示前 ${exts.rows.length} 类，其余 ${exts.hidden} 类` : `共 ${result.ext_stats.length} 类`}
              </span>
            }
          >
            {exts && exts.rows.length > 0 ? (
              <Space direction="vertical" size={6} style={{ width: "100%" }}>
                {renderBarRows(exts.rows, false)}
              </Space>
            ) : (
              <span className="sk-hint">（无文件）</span>
            )}
          </Card>

          <Card
            size="small"
            className="sk-card"
            title="最大文件 Top200"
            extra={<span className="sk-hint">{topNote ?? `共 ${tops.length} 个`}</span>}
          >
            {tops.length > 0 ? (
              <Table
                size="small"
                rowKey="path"
                pagination={{ pageSize: 20, size: "small", showSizeChanger: false }}
                dataSource={tops}
                columns={[
                  { title: "#", dataIndex: "rank", width: 52 },
                  { title: "大小", dataIndex: "human", width: 92 },
                  {
                    title: "路径",
                    dataIndex: "path",
                    ellipsis: { showTitle: false },
                    render: (p: string) => (
                      <span title={p} style={{ fontFamily: "monospace", fontSize: 12 }}>
                        {p}
                      </span>
                    ),
                  },
                  {
                    title: "",
                    width: 64,
                    render: (_: unknown, r: { path: string }) => (
                      <Button type="link" size="small" style={{ padding: 0 }} onClick={() => void reveal(r.path)}>
                        定位
                      </Button>
                    ),
                  },
                ]}
              />
            ) : (
              <span className="sk-hint">（无文件）</span>
            )}
          </Card>

          <Card
            size="small"
            className="sk-card"
            title="一级子目录概览"
            extra={
              <span className="sk-hint">
                {bars && bars.hidden > 0
                  ? `仅显示最大的前 ${bars.rows.length} 项，其余 ${bars.hidden} 项请在下钻面板查看`
                  : `共 ${result.root.children.length} 项 · 点击名称下钻`}
              </span>
            }
          >
            {bars && bars.rows.length > 0 ? (
              <Space direction="vertical" size={6} style={{ width: "100%" }}>
                {renderBarRows(bars.rows, true)}
              </Space>
            ) : (
              <span className="sk-hint">（无子目录）</span>
            )}
          </Card>
        </>
      )}
    </Space>
  );
}

/**
 * 目录逐级展开（下钻分析）：从绝对路径扫描直接子项，点目录行逐级下钻。
 * 展开状态仅内存态（不持久化）；扫描走 sk_disk_scan_children（一次只跑一个，
 * 与全量树共用取消令牌），取消沿用 sk_disk_scan_cancel。
 * 总览面板点子目录会经 external 换根本面板（drillResetNeeded 判定新请求）。
 */
export function DirDrillPanel({ external }: { external?: DrillRequest | null }) {
  const [dirInput, setDirInput] = useState("");
  const [root, setRoot] = useState<ChildrenScan | null>(null);
  const [scans, setScans] = useState<Record<string, ChildrenScan>>({});
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const [loadingPath, setLoadingPath] = useState<string | null>(null);
  /** 最近一次已应用的联动请求（token 相同的不重复换根）。 */
  const appliedRef = useRef<DrillRequest | null>(null);

  const runScan = useCallback(
    async (dir: string, isRoot: boolean) => {
      // 后端 SCAN_STOP 同一时刻只跑一个扫描：本面板也不再发第二个
      if (loadingPath !== null) return;
      setLoadingPath(dir);
      try {
        const s = await invoke<ChildrenScan>("sk_disk_scan_children", { dir });
        if (isRoot) {
          // 换根扫描：旧树的展开/缓存整体作废
          setRoot(s);
          setScans({ [s.path]: s });
          setExpanded({});
        } else {
          setScans((m) => ({ ...m, [s.path]: s }));
        }
      } catch (e) {
        if (isRoot) {
          message.error(`扫描失败：${String(e)}`);
        } else {
          // 展开失败：该行收回起态（不得停在「已展开但没内容」的假态），toast 说明
          setExpanded((x) => collapsePath(x, dir));
          message.error(drillErrorText(dir, e));
        }
      } finally {
        // 只清自己发起的那次 loading
        setLoadingPath((cur) => (cur === dir ? null : cur));
      }
    },
    [loadingPath],
  );

  // 总览联动换根：新请求（token 变化）才作废缓存重新扫描——复用轮次112 的
  // 换根作废逻辑（runScan 的 isRoot 分支 setRoot/setScans({})/setExpanded({})）。
  // 忙碌时不抢 SCAN_STOP，挂到 loadingPath 清空后的 effect 重跑再应用。
  useEffect(() => {
    if (!external) return;
    if (!drillResetNeeded(appliedRef.current, external)) return;
    if (loadingPath !== null) return;
    appliedRef.current = external;
    setDirInput(external.path);
    void runScan(external.path, true);
  }, [external, loadingPath, runScan]);

  const toggleRow = useCallback(
    (row: ChildEntry) => {
      if (!row.is_dir) return; // 文件行不可展开
      const wasExpanded = !!expanded[row.path];
      const needsFetch = !wasExpanded && !scans[row.path];
      // 没缓存、又赶上别的扫描在跑：不动展开态（否则会挂出一个永远转圈的占位行）
      if (needsFetch && loadingPath !== null) {
        message.warning("已有扫描进行中，请稍后再展开");
        return;
      }
      setExpanded((x) => toggleExpanded(x, row.path));
      // 首次展开且无缓存才发扫描；收起与缓存命中都不再打后端
      if (needsFetch) {
        void runScan(row.path, false);
      }
    },
    [expanded, scans, loadingPath, runScan],
  );

  const flat = useMemo(
    () => flattenChildRows(root, expanded, scans),
    [root, expanded, scans],
  );

  const renderNameCell = (_: unknown, r: FlatChildRow) => {
    if (r.kind === "loading") {
      return (
        <span style={{ paddingLeft: r.depth * 18 }} className="sk-hint">
          正在扫描 {r.path} …
        </span>
      );
    }
    const row = r.row!;
    const sub = scans[row.path];
    return (
      <div style={{ paddingLeft: r.depth * 18 }}>
        <Space size={4}>
          {row.is_dir ? (
            <Button
              type="link"
              size="small"
              style={{ padding: 0, minWidth: 16 }}
              onClick={() => toggleRow(row)}
              aria-label={expanded[row.path] ? `收起 ${row.name}` : `展开 ${row.name}`}
            >
              {expanded[row.path] ? "▼" : "▶"}
            </Button>
          ) : (
            <span style={{ display: "inline-block", width: 16 }} />
          )}
          <span>{childIcon(row)}</span>
          {row.is_dir ? (
            <Button type="link" size="small" style={{ padding: 0 }} onClick={() => toggleRow(row)}>
              {row.name}
            </Button>
          ) : (
            <span>{row.name}</span>
          )}
          {row.error_count > 0 && <Tag color="orange">跳过 {row.error_count}</Tag>}
          {/* 取消/竞态留下的部分结果要如实标注，不能冒充完整扫描 */}
          {row.is_dir && expanded[row.path] && sub?.cancelled && (
            <Tag color="red">已取消（结果不完整）</Tag>
          )}
          {/* 深层目录超 MAX_CHILD_ROWS 截断：与根概要行同一文案，不得静默丢行 */}
          {row.is_dir && expanded[row.path] && sub?.truncated && (
            <Tag color="orange">{truncatedNote(sub)}</Tag>
          )}
        </Space>
      </div>
    );
  };

  return (
    <Space direction="vertical" size={10} style={{ width: "100%" }}>
      <Typography.Paragraph type="secondary" className="sk-hint" style={{ marginBottom: 0 }}>
        目录逐级展开：输入绝对路径先扫一层直接子项，点目录名逐级下钻。
        「大小」按整棵子树递归统计（点开才扫，深层目录可能较慢）。
      </Typography.Paragraph>

      <Space wrap>
        <Input
          size="small"
          style={{ width: 380 }}
          placeholder="绝对目录路径，如 C:\Users\x\Downloads"
          value={dirInput}
          onChange={(e) => setDirInput(e.target.value)}
          onPressEnter={() => void runScan(dirInput.trim(), true)}
          disabled={loadingPath !== null}
          aria-label="目录路径"
        />
        <Button
          size="small"
          type="primary"
          loading={loadingPath !== null}
          onClick={() => void runScan(dirInput.trim(), true)}
        >
          开始扫描
        </Button>
        {loadingPath !== null && (
          <Button size="small" onClick={() => void invoke("sk_disk_scan_cancel")}>
            取消扫描
          </Button>
        )}
      </Space>

      {root && (
        <span className="sk-hint">
          {root.name} · 递归 {humanSize(root.total)}（自身文件 {humanSize(root.size)}）· 子项{" "}
          {root.children.length}
          {root.truncated ? `（${truncatedNote(root)}）` : ""} · {root.duration_ms} ms{" "}
          {/* 根扫描取消残留同样要如实标注——空表格不得冒充「空目录」 */}
          {root.cancelled && <Tag color="red">已取消（结果不完整）</Tag>}
        </span>
      )}

      <Table<FlatChildRow>
        size="small"
        rowKey={(r) => (r.kind === "row" ? r.row!.path : `loading:${r.path}`)}
        pagination={false}
        dataSource={flat}
        locale={{ emptyText: rootEmptyText(root) }}
        columns={[
          { title: "名称", render: renderNameCell },
          {
            title: "大小（递归）",
            dataIndex: "total",
            width: 130,
            render: (_: unknown, r) => (r.kind === "row" ? humanSize(r.row!.total) : ""),
          },
          {
            title: "文件数",
            dataIndex: "file_count",
            width: 90,
            render: (_: unknown, r) =>
              r.kind === "row" && r.row!.is_dir ? r.row!.file_count : "",
          },
        ]}
      />
    </Space>
  );
}
