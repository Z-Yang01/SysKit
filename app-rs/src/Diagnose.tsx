/**
 * 诊断 / 修复页。
 *
 * 三块内容全部走后端真实命令：
 * - 网络体检：`sk_diag_run` 跑 15 项检查，问题行的「建议动作」可直接跳到下方对应修复动作；
 * - 组合修复：`sk_repair_plan` 出计划 → 确认 → 逐步 `sk_repair_run`（执行中有进度条），
 *   完成后静默快速复检刷新旧结论（Python 同款收尾）；
 * - 动作库：`sk_repair_list`，按风险三级折叠展示，改配置/强力类先确认再执行。
 *
 * 会话内可见性（对齐 Python 诊断/修复页）：
 * - 体检报告经 sessionStorage 保存，切页回来结论还在；「复制报告」出全文存档；
 * - 手动 / 组合 / 后台监控自动修复统一记入「修复记录」——后台监控的事件
 *   （`sk://monitor-event`）实时出现在列表里，不再是只有日志的黑盒。
 *
 * 体验纪律：非管理员提前标注跳过；执行中防重复触发；加载失败给重试入口
 * 而不是永远停在「正在读取」。
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  Alert,
  Button,
  Card,
  Col,
  Collapse,
  Empty,
  Input,
  Modal,
  Progress,
  Row,
  Space,
  Spin,
  Table,
  Tag,
  Tooltip,
  Typography,
  message,
} from "antd";

import {
  actionableFixes,
  clipText,
  confirmText,
  formatCheckForCopy,
  formatMonitorEvent,
  formatReportForCopy,
  groupByRisk,
  hhmmss,
  loadReport,
  needsConfirm,
  outcomeLevel,
  outcomeTag,
  pushRecord,
  riskLabel,
  riskToken,
  saveReport,
  skipReason,
  sortBySeverity,
  statusMark,
  statusTagColor,
  statusToken,
  summarizeDiag,
  summarizePlan,
  verdictColor,
  willBeSkipped,
  type CheckResult,
  type CheckStatus,
  type Combo,
  type ComboStep,
  type DiagReport,
  type MonitorEvent,
  type RepairAction,
  type RepairList,
  type RepairOutcome,
  type RepairRecord,
} from "./repair";

import { statusLabel } from "./repair";

export function Diagnose({ quickRequest }: { quickRequest?: number }) {
  const [data, setData] = useState<RepairList | null>(null);
  const [loadError, setLoadError] = useState<string>("");
  const [busy, setBusy] = useState<string>("");
  const [outcomes, setOutcomes] = useState<Record<string, RepairOutcome>>({});
  const [paramValues, setParamValues] = useState<Record<string, Record<string, string>>>({});
  const [plan, setPlan] = useState<{ key: string; steps: ComboStep[] } | null>(null);
  const [report, setReport] = useState<DiagReport | null>(null);
  // 单项重查进行中的检查 id（"" = 空闲）；替换该行结果而不是整轮重跑
  const [recheckId, setRecheckId] = useState("");
  const [diagBusy, setDiagBusy] = useState<"" | "quick" | "full">("");
  /** 组合修复执行中的进度（null = 空闲）；total 只数真正会跑的步。 */
  const [comboRun, setComboRun] = useState<{
    key: string;
    title: string;
    total: number;
    done: number;
    current: string;
  } | null>(null);
  /** 最近一次组合执行的结果摘要（成功/失败各几步）。 */
  const [comboResult, setComboResult] = useState<{ title: string; ok: number; fail: number } | null>(
    null,
  );
  /** 三个风险分组的 Collapse 共用的展开集合（key = 动作 id，跨组唯一）。 */
  const [openIds, setOpenIds] = useState<string[]>([]);
  /** 修复记录（手动/组合/后台监控自动修复共用，最新在前，上限 60 条）。 */
  const [records, setRecords] = useState<RepairRecord[]>([]);

  const load = useCallback(async () => {
    try {
      setData(await invoke<RepairList>("sk_repair_list"));
      setLoadError("");
    } catch (e) {
      // 必须区分「还在加载」和「加载失败」：失败若也停在 data=null，
      // 界面会永远显示「正在读取修复动作库…」骗人
      setData(null);
      setLoadError(String(e));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  // 「上次结果」会话恢复 + 后台监控事件订阅。
  // Python 侧诊断结果与修复记录在窗口存活期间常驻（切页不丢）；这里组件切页
  // 会卸载，用 sessionStorage 恢复报告、用事件流续记自动修复（对齐
  // main_window._on_monitor_action/_on_monitor_suggest 的可见反馈）。
  useEffect(() => {
    setReport(loadReport());
    const un = listen<MonitorEvent>("sk://monitor-event", (e) => {
      const { mark, text } = formatMonitorEvent(e.payload);
      setRecords((list) => pushRecord(list, { time: hhmmss(new Date()), mark, text }));
    });
    return () => {
      void un.then((f) => f());
    };
  }, []);

  const isAdmin = data?.is_admin ?? false;

  /** 运行诊断（quick=true 跳过慢速外网探测；silent=true 只刷新不弹提示）。 */
  const runDiag = useCallback(
    async (quick: boolean, silent = false) => {
      if (comboRun) return; // 组合修复会改网络配置，中途探测结果不可信
      setDiagBusy(quick ? "quick" : "full");
      try {
        const r = await invoke<DiagReport>("sk_diag_run", { quick });
        setReport(r);
        saveReport(r); // 会话内保存：切页回来结论还在（对齐 Python 常驻结果）
        if (!silent) {
          const lvl = r.verdict === "normal" ? "success" : "warning";
          message[lvl](r.verdict_text);
        }
      } catch (e) {
        if (!silent) message.error(`诊断失败：${String(e)}`);
      } finally {
        setDiagBusy("");
      }
    },
    [comboRun],
  );

  // palette「快速诊断」动作信号：递增即触发一次快速检测（消费一次）
  const ranRef = useRef(quickRequest ?? 0);
  useEffect(() => {
    if (quickRequest === undefined || quickRequest === ranRef.current) return;
    ranRef.current = quickRequest;
    if (!diagBusy && !comboRun) void runDiag(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [quickRequest]);

  /**
   * 组合修复后的静默快速复检（对齐 Python 组合完成后的
   * `run_diagnosis(quick=True, silent=True)`）：组合刚改过网络配置，屏幕上
   * 留着旧结论会误导下一步判断；silent 是 Python 同名语义——刷新但不打扰。
   * 独立于 runDiag：不读 comboRun 闭包值（执行中的旧闭包会误判成「正忙」直接返回）。
   */
  const refreshAfterCombo = useCallback(async () => {
    setDiagBusy("quick");
    try {
      const r = await invoke<DiagReport>("sk_diag_run", { quick: true });
      setReport(r);
      saveReport(r);
    } catch {
      // 静默复检失败不打扰：旧报告仍在，用户可手动再点检测
    } finally {
      setDiagBusy("");
    }
  }, []);

  /** 执行一个动作（改配置/强力类先确认）。 */
  const run = useCallback(
    async (action: RepairAction) => {
      // 单飞守卫：组合修复执行中或已有动作在跑时不再并发触发——
      // 两支修复命令同时改系统配置，结果互相踩
      if (busy || comboRun) return;
      const doRun = async () => {
        setBusy(action.id);
        try {
          const out = await invoke<RepairOutcome>("sk_repair_run", {
            id: action.id,
            params: paramValues[action.id] ?? {},
          });
          setOutcomes((m) => ({ ...m, [action.id]: out }));
          // 修复记录（对齐 Python _add_repair_record：手动执行同样入账）
          setRecords((list) =>
            pushRecord(list, {
              time: hhmmss(new Date()),
              mark: out.success ? "√" : "×",
              text: `${action.title} — ${clipText(out.message || (out.success ? "完成" : "失败"), 90)}`,
            }),
          );
          const lvl = outcomeLevel(out);
          const rebootHint = out.success && out.need_reboot ? "（需重启后生效）" : "";
          message[lvl === "success" ? "success" : lvl === "warning" ? "warning" : "error"](
            `${action.title}：${out.message || (out.success ? "完成" : "失败")}${rebootHint}`,
          );
        } catch (e) {
          const out: RepairOutcome = {
            success: false,
            message: String(e),
            need_reboot: false,
            output: "",
          };
          setOutcomes((m) => ({ ...m, [action.id]: out }));
          setRecords((list) =>
            pushRecord(list, {
              time: hhmmss(new Date()),
              mark: "×",
              text: `${action.title} — ${clipText(String(e), 90)}`,
            }),
          );
          message.error(`${action.title}：${String(e)}`);
        } finally {
          setBusy("");
        }
      };

      if (needsConfirm(action.risk)) {
        Modal.confirm({
          title: `${riskLabel(action.risk)}类动作：${action.title}`,
          content: (
            <Typography.Paragraph style={{ whiteSpace: "pre-wrap" }}>
              {confirmText(action)}
            </Typography.Paragraph>
          ),
          okText: "执行",
          cancelText: "取消",
          onOk: doRun,
        });
      } else {
        await doRun();
      }
    },
    [paramValues, busy, comboRun],
  );

  /** 组合修复：逐步执行并把进度刷到 comboRun，完成后落一条摘要。 */
  const executeCombo = useCallback(
    async (combo: Combo, steps: ComboStep[]) => {
      const runnable = steps.filter((s) => !s.skipped_reason);
      // 全被跳过（如非管理员点深度修复）就不算一次"执行"，直接说明原因
      if (runnable.length === 0) {
        message.info(`${combo.title}：没有可执行的步骤（全部被跳过）`);
        return;
      }
      let ok = 0;
      let fail = 0;
      setComboRun({ key: combo.key, title: combo.title, total: runnable.length, done: 0, current: "" });
      for (const st of runnable) {
        const action = data?.actions.find((a) => a.id === st.id);
        if (!action) continue;
        setComboRun((r) => (r ? { ...r, current: st.title } : r));
        setBusy(action.id); // 卡片里对应动作的按钮同步转圈，能看出正在跑哪一步
        try {
          const out = await invoke<RepairOutcome>("sk_repair_run", {
            id: action.id,
            params: paramValues[action.id] ?? {},
          });
          setOutcomes((m) => ({ ...m, [action.id]: out }));
          // 每步入账（对齐 Python：组合内的每项结果都进修复记录）
          setRecords((list) =>
            pushRecord(list, {
              time: hhmmss(new Date()),
              mark: out.success ? "√" : "×",
              text: `${action.title} — ${clipText(out.message || (out.success ? "完成" : "失败"), 90)}`,
            }),
          );
          if (out.success) {
            ok += 1;
          } else {
            fail += 1;
            message.warning(`${action.title} 未成功：${out.message}`);
          }
        } catch (e) {
          fail += 1;
          setRecords((list) =>
            pushRecord(list, {
              time: hhmmss(new Date()),
              mark: "×",
              text: `${action.title} — ${clipText(String(e), 90)}`,
            }),
          );
          message.error(`${action.title}：${String(e)}`);
        } finally {
          setBusy("");
          setComboRun((r) => (r ? { ...r, done: r.done + 1, current: "" } : r));
        }
      }
      setComboRun(null);
      setComboResult({ title: combo.title, ok, fail });
      // 有失败就不能报 success：之前无条件 message.success 会掩盖部分失败
      if (fail === 0) {
        message.success(`${combo.title} 执行完毕：${ok} 步全部成功`);
      } else {
        message.warning(`${combo.title} 执行完毕：${ok} 成功 / ${fail} 失败（详见各动作面板）`);
      }
      // 组合改了网络配置，用快速静默复检刷新旧结论（Python 同款收尾）
      void refreshAfterCombo();
    },
    [data, paramValues, refreshAfterCombo],
  );

  /** 组合修复：先取计划（含跳过项），确认后逐步执行。 */
  const runCombo = useCallback(
    async (combo: Combo) => {
      if (comboRun || busy) return; // 与单动作互斥，见 run 的单飞守卫
      let steps: ComboStep[] = [];
      try {
        steps = await invoke<ComboStep[]>("sk_repair_plan", { combo: combo.key, isAdmin });
      } catch (e) {
        message.error(`无法获取组合计划：${String(e)}`);
        return;
      }
      setPlan({ key: combo.key, steps });
      const s = summarizePlan(steps);
      Modal.confirm({
        title: `${combo.title}（共 ${steps.length} 步）`,
        width: 620,
        content: (
          <Space direction="vertical" size={6} style={{ width: "100%" }}>
            <Typography.Text>
              将执行 <b>{s.run}</b> 步
              {s.skipped > 0 ? `，跳过 ${s.skipped} 步` : ""}
              {s.needsReboot ? "；其中含需重启生效的动作" : ""}
            </Typography.Text>
            {steps.map((st) => (
              <div key={st.id} className="sk-hint">
                {st.skipped_reason ? `${st.title} —— 跳过：${st.skipped_reason}` : st.title}
              </div>
            ))}
          </Space>
        ),
        okText: "开始执行",
        cancelText: "取消",
        // 故意不把 executeCombo 的 promise 返回给 Modal：返回了确认框会一直
        // 转圈挡住页面，关掉它才能看到页面上的逐步进度条
        onOk: () => {
          void executeCombo(combo, steps);
        },
      });
    },
    [comboRun, busy, isAdmin, executeCombo],
  );

  /**
   * 从体检结果/清单跳到对应修复动作：展开折叠项并滚动定位。
   *
   * 等 260ms 再滚是为了让折叠展开动画先走完——否则目标在滚动途中还在移动。
   */
  const focusAction = useCallback((id: string) => {
    setOpenIds((keys) => (keys.includes(id) ? keys : [...keys, id]));
    window.setTimeout(() => {
      document.getElementById(`repair-${id}`)?.scrollIntoView({ behavior: "smooth", block: "center" });
    }, 260);
  }, []);

  /** 体检建议里真正有入口可跳的动作（fix_ids 可能指向尚未移植的动作）。 */
  const fixableIds = useMemo(
    () => (report ? actionableFixes(report, (data?.actions ?? []).map((a) => a.id)) : []),
    [report, data],
  );

  /** 体检行的建议动作渲染：有入口的做成链接，没接入的标注原因。 */
  const renderFixLinks = useCallback(
    (fixIds: string[]) => (
      <span className="sk-hint">
        建议动作：
        {fixIds.map((fid, i) => (
          <span key={fid}>
            {i > 0 ? "、" : ""}
            {(data?.actions ?? []).some((a) => a.id === fid) ? (
              <Typography.Link onClick={() => focusAction(fid)}>{fid}</Typography.Link>
            ) : (
              <Tooltip title="该动作尚未在 Rust 版接入，暂无执行入口">{fid}</Tooltip>
            )}
          </span>
        ))}
      </span>
    ),
    [data, focusAction],
  );

  /** 最近一次出过的组合计划摘要（只算一次，别在 JSX 里重复调）。 */
  const planSummary = plan ? summarizePlan(plan.steps) : null;

  const groups = useMemo(() => groupByRisk(data?.actions ?? []), [data]);

  return (
    <Space direction="vertical" size={14} style={{ width: "100%" }}>
      {!isAdmin && (
        <Alert
          type="warning"
          showIcon
          message="当前不是管理员身份"
          description="改配置类与强力类动作会被跳过（组合修复里也会标注）；安全类动作可以正常执行。需要全部功能请以管理员身份运行。"
        />
      )}

      <Card
        size="small"
        className="sk-card"
        title="组合修复"
        extra={<span className="sk-hint">按 Python COMBOS 的定义逐步执行</span>}
      >
        <Space direction="vertical" size={8} style={{ width: "100%" }}>
          <Space wrap>
            {(data?.combos ?? []).map((c) => (
              <Button
                key={c.key}
                onClick={() => void runCombo(c)}
                loading={comboRun?.key === c.key}
                disabled={(comboRun !== null && comboRun.key !== c.key) || busy !== ""}
              >
                {c.title}
                <span className="sk-hint" style={{ marginLeft: 6 }}>
                  {c.actions.length} 步
                </span>
              </Button>
            ))}
            {!comboRun && plan && planSummary && (
              <span className="sk-hint">
                最近计划：{plan.steps.length} 步（{planSummary.run} 执行 / {planSummary.skipped} 跳过）
              </span>
            )}
            {!comboRun && comboResult && (
              <span className="sk-hint">
                上次组合「{comboResult.title}」：{comboResult.ok} 成功
                {comboResult.fail > 0 ? ` / ${comboResult.fail} 失败` : "（全部成功）"}
              </span>
            )}
          </Space>
          {comboRun && (
            // 执行进度：之前是黑盒等待，用户不知道组合跑到第几步、还剩几步
            <div>
              <Progress
                size="small"
                status="active"
                percent={comboRun.total === 0 ? 100 : Math.round((comboRun.done / comboRun.total) * 100)}
              />
              <Typography.Text type="secondary" className="sk-hint">
                正在执行「{comboRun.title}」：第 {Math.min(comboRun.done + 1, comboRun.total)}/
                {comboRun.total} 步{comboRun.current ? ` —— ${comboRun.current}` : ""}
              </Typography.Text>
            </div>
          )}
        </Space>
      </Card>

      <Card
        size="small"
        className="sk-card"
        title="网络诊断"
        extra={
          <Space>
            <Button
              size="small"
              disabled={!report || diagBusy !== ""}
              onClick={() => {
                if (!report || diagBusy) return;
                navigator.clipboard
                  .writeText(formatReportForCopy(report))
                  .then(() => message.success("诊断报告已复制到剪贴板"))
                  .catch(() => message.error("复制失败：剪贴板不可用"));
              }}
            >
              复制报告
            </Button>
            <Button
              size="small"
              disabled={!report || diagBusy !== ""}
              onClick={() => {
                if (!report || diagBusy) return;
                void invoke<string>("sk_diag_export", {
                  content: formatReportForCopy(report),
                })
                  .then((target) => {
                    if (target) message.success(`已导出：${target}`);
                  })
                  .catch((e) => message.error(String(e)));
              }}
            >
              导出报告
            </Button>
            <Button
              size="small"
              onClick={() => void runDiag(true)}
              loading={diagBusy === "quick"}
              disabled={diagBusy === "full" || comboRun !== null}
            >
              快速检测
            </Button>
            <Button
              type="primary"
              size="small"
              onClick={() => void runDiag(false)}
              loading={diagBusy === "full"}
              disabled={diagBusy === "quick" || comboRun !== null}
            >
              完整检测
            </Button>
          </Space>
        }
      >
        <Space direction="vertical" size={10} style={{ width: "100%" }}>
          {report ? (
            <>
              <Space wrap>
                <Tag color={verdictColor(report.verdict)}>{report.verdict_text}</Tag>
                {summarizeDiag(report).map((s) => (
                  <Tag key={s.status} color={statusTagColor(s.status)}>
                    {s.label} {s.count}
                  </Tag>
                ))}
                <span className="sk-hint">
                  {report.quick ? "快速检测（跳过慢速项）" : "完整检测"} · 共{" "}
                  {report.results.length} 项
                </span>
              </Space>

              {fixableIds.length > 0 && (
                <Alert
                  type="info"
                  showIcon
                  message={
                    <span>
                      诊断建议修复：
                      {fixableIds.map((fid, i) => (
                        <span key={fid}>
                          {i > 0 ? "、" : ""}
                          <Typography.Link onClick={() => focusAction(fid)}>
                            {fid}
                          </Typography.Link>
                        </span>
                      ))}
                      （点击展开并定位到对应动作）
                    </span>
                  }
                />
              )}

              <Table<CheckResult>
                size="small"
                rowKey="id"
                pagination={false}
                dataSource={sortBySeverity(report.results)}
                columns={[
                  {
                    title: "",
                    dataIndex: "status",
                    width: 46,
                    render: (s: CheckStatus) => (
                      <span style={{ color: `var(--sk-${statusToken(s)})`, fontWeight: 600 }}>
                        {statusMark(s)}
                      </span>
                    ),
                  },
                  { title: "检查项", dataIndex: "name" },
                  {
                    title: "状态",
                    dataIndex: "status",
                    width: 90,
                    render: (s: CheckStatus) => (
                      <Tag color={statusTagColor(s)}>{statusLabel(s)}</Tag>
                    ),
                  },
                  {
                    title: "重查",
                    key: "recheck",
                    width: 60,
                    render: (_: unknown, row) => {
                      // 单项重查（sk_diag_one）：某项异常时不用整轮重跑
                      const busy = recheckId !== "";
                      return (
                        <Button
                          size="small"
                          type="text"
                          className="sk-hint"
                          style={{ height: "auto", padding: "0 4px" }}
                          disabled={busy || diagBusy !== ""}
                          loading={recheckId === row.id}
                          onClick={() => {
                            setRecheckId(row.id);
                            invoke<CheckResult>("sk_diag_one", { id: row.id })
                              .then((fresh) => {
                                setReport((prev) =>
                                  prev
                                    ? {
                                        ...prev,
                                        results: prev.results.map((x) =>
                                          x.id === fresh.id ? fresh : x,
                                        ),
                                      }
                                    : prev,
                                );
                              })
                              .catch((e) => message.error(`重查失败：${String(e)}`))
                              .finally(() => setRecheckId(""));
                          }}
                        >
                          重查
                        </Button>
                      );
                    },
                  },
                  {
                    title: "详情",
                    dataIndex: "detail",
                    render: (v: string, row) => (
                      <Space direction="vertical" size={2}>
                        <Space size={4} wrap>
                          <span className="sk-hint">{v}</span>
                          {/* 复制详情：贴给别人排查时需要自包含的一段信息 */}
                          <Button
                            size="small"
                            type="text"
                            className="sk-hint"
                            style={{ height: "auto", padding: "0 4px" }}
                            onClick={() => {
                              navigator.clipboard
                                .writeText(formatCheckForCopy(row))
                                .then(() => message.success(`已复制「${row.name}」详情`))
                                .catch(() => message.error("复制失败：剪贴板不可用"));
                            }}
                          >
                            复制
                          </Button>
                        </Space>
                        {row.fix_ids.length > 0 && renderFixLinks(row.fix_ids)}
                      </Space>
                    ),
                  },
                ]}
              />
            </>
          ) : diagBusy ? (
            // 首次检测没有 report：用 Spin 而不是一条静态文案，让"在跑"看得见
            <div style={{ textAlign: "center", padding: "28px 0" }}>
              <Spin />
              <div className="sk-hint" style={{ marginTop: 10 }}>
                正在检测…（含 ping / HTTP / DNS 查询与网卡采集，完整检测约需数秒）
              </div>
            </div>
          ) : (
            <Empty
              image={Empty.PRESENTED_IMAGE_SIMPLE}
              description="尚未检测。点「完整检测」运行全部 15 项网络检查（NlaSvc / 持久路由 / 静态 DNS / 代理 / 网关 / DNS 对比 / NCSI / 服务 / 路由 / IPv6 / WLAN …）；「快速检测」跳过 5 项慢速项。"
            />
          )}
        </Space>
      </Card>

      {groups.map((g) => (
        <Card
          key={g.risk}
          size="small"
          className="sk-card"
          title={
            <Space>
              <Tag color={riskToken(g.risk) === "ok" ? "green" : riskToken(g.risk) === "warn" ? "orange" : "red"}>
                {riskLabel(g.risk)}
              </Tag>
              <span>共 {g.items.length} 项</span>
            </Space>
          }
        >
          <Collapse
            accordion={false}
            activeKey={openIds}
            onChange={(k) => {
              // 三个分组的 Collapse 共用 openIds；onChange 回传的数组可能只含
              // 本组的 key，必须并回全集，否则收起 A 组会把 B 组展开的项一起收掉
              const arr = (Array.isArray(k) ? k : [k]).map(String);
              const groupKeys = new Set(g.items.map((a) => a.id));
              setOpenIds((prev) => [
                ...prev.filter((x) => !groupKeys.has(x) && !arr.includes(x)),
                ...arr,
              ]);
            }}
            items={g.items.map((a) => {
              const out = outcomes[a.id];
              const skip = skipReason(a, isAdmin);
              return {
                key: a.id,
                label: (
                  <Space>
                    {/* 锚点：体检「建议动作」/清单「定位」滚动到这里 */}
                    <span id={`repair-${a.id}`}>{a.title}</span>
                    {a.need_reboot && <Tag color="red">需重启</Tag>}
                    {skip && <Tag color="default">将被跳过</Tag>}
                    {out && <Tag color={outcomeTag(out).color}>{outcomeTag(out).text}</Tag>}
                  </Space>
                ),
                children: (
                  <Space direction="vertical" size={8} style={{ width: "100%" }}>
                    <Typography.Paragraph style={{ margin: 0, whiteSpace: "pre-wrap" }}>
                      {a.description}
                    </Typography.Paragraph>
                    <Typography.Text type="secondary" className="sk-hint">
                      还原方式：{a.restore_hint}
                    </Typography.Text>
                    {skip && <Alert type="info" showIcon message={skip} />}
                    {a.params.length > 0 && (
                      <Row gutter={8}>
                        {a.params.map((p) => (
                          <Col span={12} key={p.name}>
                            <div className="sk-hint">
                              {p.name}：{p.desc}
                              {p.required ? "（必填）" : "（可选）"}
                            </div>
                            <Input
                              size="small"
                              placeholder={p.name}
                              value={paramValues[a.id]?.[p.name] ?? ""}
                              onChange={(e) =>
                                setParamValues((m) => ({
                                  ...m,
                                  [a.id]: { ...(m[a.id] ?? {}), [p.name]: e.target.value },
                                }))
                              }
                            />
                          </Col>
                        ))}
                      </Row>
                    )}
                    <Space wrap>
                      <Button
                        type="primary"
                        size="small"
                        loading={busy === a.id}
                        disabled={(busy !== "" || comboRun !== null) && busy !== a.id}
                        onClick={() => void run(a)}
                      >
                        执行
                      </Button>
                      {out?.need_reboot && (
                        // 执行成功但需重启时给持久提醒，光靠一次性 message 会被错过
                        <Tag color="red">需重启后生效</Tag>
                      )}
                    </Space>
                    {out?.output && (
                      // 完整输出可展开+可复制，替代原来静默截断 160 字符
                      <Typography.Paragraph
                        type="secondary"
                        className="sk-hint"
                        style={{ marginBottom: 0, whiteSpace: "pre-wrap" }}
                        ellipsis={{ rows: 3, expandable: true, symbol: "展开全部" }}
                        copyable={{ text: out.output, tooltips: ["复制完整输出", "已复制"] }}
                      >
                        {out.output}
                      </Typography.Paragraph>
                    )}
                    {out && !out.output && out.message && (
                      <Typography.Text type="secondary" className="sk-hint">
                        {out.message}
                      </Typography.Text>
                    )}
                  </Space>
                ),
              };
            })}
          />
        </Card>
      ))}

      {!data && (
        <Card size="small" className="sk-card">
          {loadError ? (
            // 加载失败 ≠ 加载中：给出原因和重试入口，而不是永远显示「正在读取」
            <Empty
              image={Empty.PRESENTED_IMAGE_SIMPLE}
              description={
                <Space direction="vertical" size={8}>
                  <span>修复动作库加载失败：{loadError}</span>
                  <Button
                    size="small"
                    onClick={() => {
                      setLoadError("");
                      void load();
                    }}
                  >
                    重试
                  </Button>
                </Space>
              }
            />
          ) : (
            <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="正在读取修复动作库…" />
          )}
        </Card>
      )}

      {data && (
        <Table<RepairAction>
          size="small"
          rowKey="id"
          pagination={false}
          dataSource={data.actions}
          title={() => <span className="sk-hint">动作清单（表格视图，顺序与后端 ACTION_IDS 一致）</span>}
          columns={[
            { title: "id", dataIndex: "id", width: 170 },
            { title: "名称", dataIndex: "title" },
            {
              title: "风险",
              dataIndex: "risk",
              width: 110,
              render: (r: RepairAction["risk"], row) => (
                <Tag color={riskToken(r) === "ok" ? "green" : riskToken(r) === "warn" ? "orange" : "red"}>
                  {row.risk_label || riskLabel(r)}
                </Tag>
              ),
            },
            {
              title: "需重启",
              dataIndex: "need_reboot",
              width: 80,
              render: (v: boolean) => (v ? "是" : "否"),
            },
            {
              title: "非管理员",
              width: 100,
              render: (_: unknown, row) => (willBeSkipped(row, isAdmin) ? "跳过" : "可执行"),
            },
            {
              title: "操作",
              width: 90,
              render: (_: unknown, row) => (
                // 平铺清单看不到分组结构，给个直达入口省得在上三组折叠里翻找
                <Typography.Link onClick={() => focusAction(row.id)}>展开定位</Typography.Link>
              ),
            },
          ]}
        />
      )}

      <Card
        size="small"
        className="sk-card"
        title="修复记录（本次运行）"
        extra={
          <span className="sk-hint">
            手动 / 组合 / 后台监控自动修复统一入账（最新在前，最多 60 条）
          </span>
        }
      >
        {records.length === 0 ? (
          <Empty
            image={Empty.PRESENTED_IMAGE_SIMPLE}
            description="暂无记录。执行修复动作，或等待后台监控自动修复后这里会实时出现条目。"
          />
        ) : (
          // 后台自动修复原本只写日志：用户完全看不见「后台动过系统」，这里补上可见性
          <div style={{ maxHeight: 200, overflowY: "auto" }}>
            {records.map((r, i) => (
              <div key={`${r.time}-${i}`} style={{ padding: "2px 0" }}>
                <span
                  style={{
                    color: `var(--sk-${r.mark === "√" ? "ok" : r.mark === "!" ? "warn" : "err"})`,
                    fontWeight: 600,
                  }}
                >
                  {r.mark}
                </span>
                <span className="sk-hint" style={{ marginLeft: 8 }}>
                  {r.time}
                </span>
                <span style={{ marginLeft: 8 }}>{r.text}</span>
              </div>
            ))}
          </div>
        )}
      </Card>
    </Space>
  );
}
