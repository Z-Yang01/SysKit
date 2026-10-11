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
 *
 * v1.4 UI 迁移：脱离 AntD（Collapse → 自定义展开行，受控 openIds 跨组共用语义
 * 保留；Modal.confirm/message/Table/Switch/Progress → ui 件）。
 */
import { BackupRollbackCard } from "./BackupRollbackCard";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

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
import { Badge, Button, Card, CardBody, CardHeader, CardTitle, Empty, Input, Progress, Switch, Table, type Column } from "./ui";
import { Alert } from "./ui/alert";
import { confirm } from "./ui/confirm";
import { Tooltip as UiTooltip } from "./ui/tooltip";
import { useToast } from "./ui/toast";

/** antd 色名（statusTagColor/verdictColor/outcomeTag/riskToken 派生）→ Badge tone。 */
function colorTone(c: string): "ok" | "warn" | "err" | "neutral" {
  switch (c) {
    case "green":
      return "ok";
    case "orange":
      return "warn";
    case "red":
      return "err";
    default:
      return "neutral";
  }
}

/** Typography.Link 的替代（accent 文字按钮）。 */
function LinkButton({ children, onClick }: { children: React.ReactNode; onClick: () => void }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="cursor-pointer border-0 bg-transparent p-0 text-sk-accent hover:underline"
    >
      {children}
    </button>
  );
}

export function Diagnose({ quickRequest }: { quickRequest?: number }) {
  const toast = useToast();
  const [data, setData] = useState<RepairList | null>(null);
  const [loadError, setLoadError] = useState<string>("");
  const [busy, setBusy] = useState<string>("");
  const [outcomes, setOutcomes] = useState<Record<string, RepairOutcome>>({});
  const [paramValues, setParamValues] = useState<Record<string, Record<string, string>>>({});
  const [plan, setPlan] = useState<{ key: string; steps: ComboStep[] } | null>(null);
  const [report, setReport] = useState<DiagReport | null>(null);
  // 单项重查进行中的检查 id（"" = 空闲）；替换该行结果而不是整轮重跑
  const [recheckId, setRecheckId] = useState("");
  // 批量重查「全部失败/警告项」（v1.4 操作优化）：跑完修复动作后逐项确认效果
  const [batchRechecking, setBatchRechecking] = useState(false);
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
  /** 三个风险分组的展开集合（key = 动作 id，跨组唯一；各组展开行共用此集合）。 */
  const [openIds, setOpenIds] = useState<string[]>([]);
  /** 修复记录（手动/组合/后台监控自动修复共用，最新在前，上限 60 条）。 */
  const [records, setRecords] = useState<RepairRecord[]>([]);
  // 报告脱敏开关（v1.3）：默认关 = 导出/复制走分级脱敏；本机自用可开完整信息
  const [fullInfo, setFullInfo] = useState(false);
  // 只看未通过（v1.4 操作优化）：通过/提示行折叠，失败与警告顶上去
  const [failedOnly, setFailedOnly] = useState(false);
  // 动作完整输出的展开态（替代 AntD ellipsis expandable：输出框改为可展开的滚动区）
  const [expandedOutputs, setExpandedOutputs] = useState<Record<string, boolean>>({});

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
          if (r.verdict === "normal") toast.success(r.verdict_text);
          else toast.info(r.verdict_text);
        }
      } catch (e) {
        if (!silent) toast.error(`诊断失败：${String(e)}`);
      } finally {
        setDiagBusy("");
      }
    },
    [comboRun, toast],
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

  /**
   * 一键智能修复（傻瓜化，给不懂网络的用户）：点一个按钮完成
   * 「完整诊断 → 收集失败/警告项的可用修复 → 确认一次 → 逐个执行 → 自动复检」。
   *
   * 安全模型不降级：确认框一次性列出全部将执行的动作（含风险标签）；
   * 非管理员的改配置/强力类动作由后端跳过并在结果里注明；
   * 改配置动作执行前自动备份原值（可在「备份与回滚」还原）。
   */
  const smartRepair = useCallback(async () => {
    if (busy || comboRun || diagBusy) return;
    // 第一步：完整诊断（永远跑全量拿完整失败清单，快速档会漏项）
    setDiagBusy("full");
    let rep: DiagReport;
    try {
      rep = await invoke<DiagReport>("sk_diag_run", { quick: false });
      setReport(rep);
      saveReport(rep);
    } catch (e) {
      setDiagBusy("");
      toast.error(`诊断失败：${String(e)}`);
      return;
    }
    setDiagBusy("");

    // 第二步：只收「失败/警告」行的建议动作（通过/提示/跳过不执行），
    // 与已移植动作表求交集，Set 去重（flush_dns 可能被多项同时建议）
    const known = new Set((data?.actions ?? []).map((a) => a.id));
    const ids: string[] = [];
    for (const r of rep.results) {
      if (r.status !== "fail" && r.status !== "warn") continue;
      for (const id of r.fix_ids) {
        if (known.has(id) && !ids.includes(id)) ids.push(id);
      }
    }
    if (ids.length === 0) {
      toast.success("诊断未发现可自动修复的问题——网络看起来正常，或失败项需要人工处理（见详情列）");
      return;
    }
    const acts = ids
      .map((id) => (data?.actions ?? []).find((a) => a.id === id))
      .filter((a): a is RepairAction => !!a);

    // 第三步：一次确认列明全部动作（知情同意一次给足，而不是逐个弹窗打断）
    confirm({
      title: `一键智能修复：将执行以下 ${acts.length} 项`,
      width: 560,
      content: (
        <div>
          <div className="mb-2 whitespace-pre-wrap">
            {acts.map((a) => `· ${a.title}（${riskLabel(a.risk)}类）`).join("\n")}
            {!isAdmin ? "\n\n当前不是管理员身份：改配置类与强力类动作会被跳过。" : ""}
          </div>
          <div className="sk-hint text-xs">
            执行完成后自动复检展示修复效果；改配置类动作先备份原值，可在「备份与回滚」还原。
          </div>
        </div>
      ),
      okText: "开始修复",
      onOk: async () => {
        let ok = 0;
        let fail = 0;
        // 复用组合修复的进度形态（卡片按钮同步转圈 + 进度条）
        setComboRun({ key: "smart", title: "一键智能修复", total: acts.length, done: 0, current: "" });
        for (const a of acts) {
          setComboRun((r) => (r ? { ...r, current: a.title } : r));
          setBusy(a.id);
          try {
            const out = await invoke<RepairOutcome>("sk_repair_run", {
              id: a.id,
              params: paramValues[a.id] ?? {},
            });
            setOutcomes((m) => ({ ...m, [a.id]: out }));
            setRecords((list) =>
              pushRecord(list, {
                time: hhmmss(new Date()),
                mark: out.success ? "√" : "×",
                text: `${a.title} — ${clipText(out.message || (out.success ? "完成" : "失败"), 90)}`,
              }),
            );
            if (out.success) ok += 1;
            else fail += 1;
          } catch (e) {
            fail += 1;
            setRecords((list) =>
              pushRecord(list, {
                time: hhmmss(new Date()),
                mark: "×",
                text: `${a.title} — ${clipText(String(e), 90)}`,
              }),
            );
          } finally {
            setBusy("");
            setComboRun((r) => (r ? { ...r, done: r.done + 1 } : r));
          }
        }
        setComboRun(null);
        // 第四步：自动复检（快速档闭环）
        await refreshAfterCombo();
        if (fail === 0)
          toast.success(
            `一键修复完成：成功 ${ok} 项，已自动复检`,
          );
        else
          toast.info(
            `一键修复完成：成功 ${ok} 项，失败/跳过 ${fail} 项（详见修复记录），已自动复检`,
          );
      },
    });
  }, [report, data, busy, comboRun, diagBusy, paramValues, isAdmin, refreshAfterCombo, toast]);

  /** 重查全部失败/警告项（只读检查，安全）：跑完修复后逐项确认效果。
   *  逐项替换结果（与单项重查同一口径），全部完成后不自动刷新整轮——
   *  失败项在重查后转好的会即时变绿。 */
  const recheckAllFailed = useCallback(async () => {
    if (!report || batchRechecking || diagBusy || comboRun) return;
    const failed = report.results.filter((r) => r.status === "fail" || r.status === "warn");
    if (failed.length === 0) {
      toast.info("没有失败/警告项需要重查");
      return;
    }
    setBatchRechecking(true);
    let nowOk = 0;
    try {
      for (const r of failed) {
        setRecheckId(r.id);
        try {
          const fresh = await invoke<CheckResult>("sk_diag_one", { id: r.id });
          setReport((prev) =>
            prev
              ? {
                  ...prev,
                  results: prev.results.map((x) => (x.id === fresh.id ? fresh : x)),
                }
              : prev,
          );
          if (fresh.status === "pass") nowOk += 1;
        } catch (e) {
          // 单项失败不中断批量（与单项重查同语义），结果行保持原状
          console.warn(`重查 ${r.id} 失败`, e);
        }
      }
      if (nowOk > 0) toast.success(`重查完成：${failed.length} 项中 ${nowOk} 项已转为通过`);
      else toast.info(`重查完成：${failed.length} 项中 0 项已转为通过`);
    } finally {
      setRecheckId("");
      setBatchRechecking(false);
    }
  }, [report, batchRechecking, diagBusy, comboRun, toast]);

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
          if (lvl === "success") toast.success(`${action.title}：${out.message || "完成"}${rebootHint}`);
          else if (lvl === "warning") toast.info(`${action.title}：${out.message}${rebootHint}`);
          else toast.error(`${action.title}：${out.message || "失败"}${rebootHint}`);
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
          toast.error(`${action.title}：${String(e)}`);
        } finally {
          setBusy("");
        }
      };

      if (needsConfirm(action.risk)) {
        confirm({
          title: `${riskLabel(action.risk)}类动作：${action.title}`,
          content: <div className="whitespace-pre-wrap">{confirmText(action)}</div>,
          okText: "执行",
          onOk: doRun,
        });
      } else {
        await doRun();
      }
    },
    [paramValues, busy, comboRun, toast],
  );

  /** 组合修复：逐步执行并把进度刷到 comboRun，完成后落一条摘要。 */
  const executeCombo = useCallback(
    async (combo: Combo, steps: ComboStep[]) => {
      const runnable = steps.filter((s) => !s.skipped_reason);
      // 全被跳过（如非管理员点深度修复）就不算一次"执行"，直接说明原因
      if (runnable.length === 0) {
        toast.info(`${combo.title}：没有可执行的步骤（全部被跳过）`);
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
            toast.info(`${action.title} 未成功：${out.message}`);
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
          toast.error(`${action.title}：${String(e)}`);
        } finally {
          setBusy("");
          setComboRun((r) => (r ? { ...r, done: r.done + 1, current: "" } : r));
        }
      }
      setComboRun(null);
      setComboResult({ title: combo.title, ok, fail });
      // 有失败就不能报 success：之前无条件 message.success 会掩盖部分失败
      if (fail === 0) {
        toast.success(`${combo.title} 执行完毕：${ok} 步全部成功`);
      } else {
        toast.info(`${combo.title} 执行完毕：${ok} 成功 / ${fail} 失败（详见各动作面板）`);
      }
      // 组合改了网络配置，用快速静默复检刷新旧结论（Python 同款收尾）
      void refreshAfterCombo();
    },
    [data, paramValues, refreshAfterCombo, toast],
  );

  /** 组合修复：先取计划（含跳过项），确认后逐步执行。 */
  const runCombo = useCallback(
    async (combo: Combo) => {
      if (comboRun || busy) return; // 与单动作互斥，见 run 的单飞守卫
      let steps: ComboStep[] = [];
      try {
        steps = await invoke<ComboStep[]>("sk_repair_plan", { combo: combo.key, isAdmin });
      } catch (e) {
        toast.error(`无法获取组合计划：${String(e)}`);
        return;
      }
      setPlan({ key: combo.key, steps });
      const s = summarizePlan(steps);
      confirm({
        title: `${combo.title}（共 ${steps.length} 步）`,
        width: 620,
        content: (
          <div className="flex flex-col gap-1.5">
            <div>
              将执行 <b>{s.run}</b> 步
              {s.skipped > 0 ? `，跳过 ${s.skipped} 步` : ""}
              {s.needsReboot ? "；其中含需重启生效的动作" : ""}
            </div>
            {steps.map((st) => (
              <div key={st.id} className="sk-hint">
                {st.skipped_reason ? `${st.title} —— 跳过：${st.skipped_reason}` : st.title}
              </div>
            ))}
          </div>
        ),
        okText: "开始执行",
        // 故意不把 executeCombo 的 promise 返回给确认框：返回了确认框会一直
        // 转圈挡住页面，关掉它才能看到页面上的逐步进度条
        onOk: () => {
          void executeCombo(combo, steps);
        },
      });
    },
    [comboRun, busy, isAdmin, executeCombo, toast],
  );

  /**
   * 从体检结果/清单跳到对应修复动作：展开折叠项并滚动定位。
   *
   * 等 260ms 再滚是为了让折叠展开先完成——否则目标在滚动途中还在移动。
   */
  const focusAction = useCallback((id: string) => {
    setOpenIds((keys) => (keys.includes(id) ? keys : [...keys, id]));
    window.setTimeout(() => {
      document.getElementById(`repair-${id}`)?.scrollIntoView({ behavior: "smooth", block: "center" });
    }, 260);
  }, []);

  const toggleOpen = useCallback((id: string) => {
    setOpenIds((keys) => (keys.includes(id) ? keys.filter((k) => k !== id) : [...keys, id]));
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
              <LinkButton onClick={() => focusAction(fid)}>{fid}</LinkButton>
            ) : (
              <UiTooltip title="该动作尚未在 Rust 版接入，暂无执行入口">
                <span>{fid}</span>
              </UiTooltip>
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

  const checkColumns: Column<CheckResult>[] = [
    {
      title: "",
      dataIndex: "status",
      width: 46,
      render: (s: CheckStatus) => (
        <span style={{ color: `var(--sk-${statusToken(s)})`, fontWeight: 600 }}>{statusMark(s)}</span>
      ),
    },
    { title: "检查项", dataIndex: "name" },
    {
      title: "状态",
      dataIndex: "status",
      width: 90,
      render: (s: CheckStatus) => <Badge tone={colorTone(statusTagColor(s))}>{statusLabel(s)}</Badge>,
    },
    {
      title: "重查",
      key: "recheck",
      width: 60,
      render: (_: unknown, row) => {
        // 单项重查（sk_diag_one）：某项异常时不用整轮重跑
        const isBusy = recheckId !== "";
        return (
          <Button
            variant="ghost"
            size="sm"
            disabled={isBusy || diagBusy !== ""}
            loading={recheckId === row.id}
            onClick={() => {
              setRecheckId(row.id);
              invoke<CheckResult>("sk_diag_one", { id: row.id })
                .then((fresh) => {
                  setReport((prev) =>
                    prev
                      ? {
                          ...prev,
                          results: prev.results.map((x) => (x.id === fresh.id ? fresh : x)),
                        }
                      : prev,
                  );
                })
                .catch((e) => toast.error(`重查失败：${String(e)}`))
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
        <span className="flex flex-col gap-0.5">
          <span className="flex flex-wrap items-center gap-1">
            <span className="sk-hint">{v}</span>
            {/* 复制详情：贴给别人排查时需要自包含的一段信息 */}
            <Button
              variant="ghost"
              size="sm"
              onClick={() => {
                navigator.clipboard
                  .writeText(formatCheckForCopy(row, fullInfo))
                  .then(() => toast.success(`已复制「${row.name}」详情`))
                  .catch(() => toast.error("复制失败：剪贴板不可用"));
              }}
            >
              复制
            </Button>
          </span>
          {row.fix_ids.length > 0 && renderFixLinks(row.fix_ids)}
        </span>
      ),
    },
  ];

  const listColumns: Column<RepairAction>[] = [
    { title: "id", dataIndex: "id", width: 170 },
    { title: "名称", dataIndex: "title" },
    {
      title: "风险",
      dataIndex: "risk",
      width: 110,
      render: (r: RepairAction["risk"], row) => (
        <Badge tone={colorTone(riskToken(r))}>{row.risk_label || riskLabel(r)}</Badge>
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
        <LinkButton onClick={() => focusAction(row.id)}>展开定位</LinkButton>
      ),
    },
  ];

  return (
    <div className="flex flex-col gap-3.5">
      {!isAdmin && (
        <Alert tone="warning" title="当前不是管理员身份">
          改配置类与强力类动作会被跳过（组合修复里也会标注）；安全类动作可以正常执行。需要全部功能请以管理员身份运行。
        </Alert>
      )}

      <Card>
        <CardHeader>
          <CardTitle>组合修复</CardTitle>
          <span className="sk-hint">按 Python COMBOS 的定义逐步执行</span>
        </CardHeader>
        <CardBody>
          <div className="flex flex-col gap-2">
            <div className="flex flex-wrap items-center gap-2">
              {(data?.combos ?? []).map((c) => (
                <Button
                  key={c.key}
                  onClick={() => void runCombo(c)}
                  loading={comboRun?.key === c.key}
                  disabled={(comboRun !== null && comboRun.key !== c.key) || busy !== ""}
                >
                  {c.title}
                  <span className="sk-hint ml-1.5">{c.actions.length} 步</span>
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
            </div>
            {comboRun && (
              // 执行进度：之前是黑盒等待，用户不知道组合跑到第几步、还剩几步
              <div className="flex flex-col gap-1">
                <Progress
                  percent={
                    comboRun.total === 0 ? 100 : Math.round((comboRun.done / comboRun.total) * 100)
                  }
                />
                <span className="sk-hint">
                  正在执行「{comboRun.title}」：第 {Math.min(comboRun.done + 1, comboRun.total)}/
                  {comboRun.total} 步{comboRun.current ? ` —— ${comboRun.current}` : ""}
                </span>
              </div>
            )}
          </div>
        </CardBody>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>网络诊断</CardTitle>
        </CardHeader>
        <CardBody>
          <div className="flex flex-col gap-2.5">
            <div className="flex flex-wrap items-center gap-2">
              <span className="sk-hint select-none">只看未通过</span>
              <Switch checked={failedOnly} onChange={setFailedOnly} aria-label="只看未通过" />
              <span className="sk-hint select-none">含完整信息</span>
              <Switch checked={fullInfo} onChange={setFullInfo} aria-label="含完整信息" />
              <Button
                size="sm"
                disabled={!report || diagBusy !== ""}
                onClick={() => {
                  if (!report || diagBusy) return;
                  navigator.clipboard
                    .writeText(formatReportForCopy(report, new Date(), fullInfo))
                    .then(() =>
                      toast.success(fullInfo ? "诊断报告已复制（含完整信息）" : "诊断报告已复制（已脱敏）"),
                    )
                    .catch(() => toast.error("复制失败：剪贴板不可用"));
                }}
              >
                复制报告
              </Button>
              <Button
                size="sm"
                disabled={!report || diagBusy !== ""}
                onClick={() => {
                  if (!report || diagBusy) return;
                  void invoke<string>("sk_diag_export", {
                    content: formatReportForCopy(report, new Date(), fullInfo),
                  })
                    .then((target) => {
                      if (target) toast.success(`已导出：${target}`);
                    })
                    .catch((e) => toast.error(String(e)));
                }}
              >
                导出报告
              </Button>
              <Button
                variant="primary"
                size="sm"
                onClick={() => void smartRepair()}
                disabled={!!diagBusy || comboRun !== null}
                title="自动诊断并修复可自动处理的问题，全程无需理解网络术语"
              >
                🔧 一键智能修复
              </Button>
              <Button
                size="sm"
                onClick={() => void recheckAllFailed()}
                loading={batchRechecking}
                disabled={
                  !!diagBusy ||
                  comboRun !== null ||
                  recheckId !== "" ||
                  !report ||
                  !report.results.some((r) => r.status === "fail" || r.status === "warn")
                }
              >
                重查失败项
              </Button>
              <Button
                size="sm"
                onClick={() => void runDiag(true)}
                loading={diagBusy === "quick"}
                disabled={diagBusy === "full" || comboRun !== null}
              >
                快速检测
              </Button>
              <Button
                variant="primary"
                size="sm"
                onClick={() => void runDiag(false)}
                loading={diagBusy === "full"}
                disabled={diagBusy === "quick" || comboRun !== null}
              >
                完整检测
              </Button>
            </div>

            {report ? (
              <>
                <div className="flex flex-wrap items-center gap-2">
                  <Badge tone={colorTone(verdictColor(report.verdict))}>{report.verdict_text}</Badge>
                  {summarizeDiag(report).map((s) => (
                    <Badge key={s.status} tone={colorTone(statusTagColor(s.status))}>
                      {s.label} {s.count}
                    </Badge>
                  ))}
                  <span className="sk-hint">
                    {report.quick ? "快速检测（跳过慢速项）" : "完整检测"} · 共{" "}
                    {report.results.length} 项
                  </span>
                </div>

                {fixableIds.length > 0 && (
                  <Alert tone="info" title="诊断建议修复">
                    <span>
                      {fixableIds.map((fid, i) => (
                        <span key={fid}>
                          {i > 0 ? "、" : ""}
                          <LinkButton onClick={() => focusAction(fid)}>{fid}</LinkButton>
                        </span>
                      ))}
                      （点击展开并定位到对应动作）
                    </span>
                  </Alert>
                )}

                <Table<CheckResult>
                  rowKey={(r) => r.id}
                  dataSource={sortBySeverity(report.results).filter(
                    (r) => !failedOnly || r.status !== "pass",
                  )}
                  columns={checkColumns}
                />
              </>
            ) : diagBusy ? (
              // 首次检测没有 report：给动态文案让"在跑"看得见
              <div className="py-7 text-center">
                <div className="sk-hint">
                  正在检测…（含 ping / HTTP / DNS 查询与网卡采集，完整检测约需数秒）
                </div>
              </div>
            ) : (
              <Empty description="尚未检测。点「完整检测」运行全部 15 项网络检查（NlaSvc / 持久路由 / 静态 DNS / 代理 / 网关 / DNS 对比 / NCSI / 服务 / 路由 / IPv6 / WLAN …）；「快速检测」跳过 5 项慢速项。" />
            )}
          </div>
        </CardBody>
      </Card>

      {groups.map((g) => (
        <Card key={g.risk}>
          <CardHeader>
            <CardTitle>
              <span className="inline-flex items-center gap-2">
                <Badge tone={colorTone(riskToken(g.risk))}>{riskLabel(g.risk)}</Badge>
                <span>共 {g.items.length} 项</span>
              </span>
            </CardTitle>
          </CardHeader>
          <CardBody>
            <div className="flex flex-col divide-y divide-sk-border/60">
              {g.items.map((a) => {
                const out = outcomes[a.id];
                const skip = skipReason(a, isAdmin);
                const open = openIds.includes(a.id);
                return (
                  <div key={a.id} className="flex flex-col">
                    {/* 行头（AntD Collapse 面板头的替代）：整行可点开合 */}
                    <button
                      type="button"
                      onClick={() => toggleOpen(a.id)}
                      className="flex w-full cursor-pointer flex-wrap items-center gap-2 bg-transparent py-2 text-left"
                    >
                      <span className="w-3 text-sk-sub">{open ? "▼" : "▶"}</span>
                      {/* 锚点：体检「建议动作」/清单「定位」滚动到这里 */}
                      <span id={`repair-${a.id}`} className="font-medium text-sk-text">
                        {a.title}
                      </span>
                      {a.need_reboot && <Badge tone="err">需重启</Badge>}
                      {skip && <Badge>将被跳过</Badge>}
                      {out && <Badge tone={colorTone(outcomeTag(out).color)}>{outcomeTag(out).text}</Badge>}
                    </button>
                    {open && (
                      <div className="flex flex-col gap-2 pb-3">
                        <div className="whitespace-pre-wrap text-sm text-sk-text">{a.description}</div>
                        <div className="sk-hint">还原方式：{a.restore_hint}</div>
                        {skip && <Alert tone="info" title={skip} />}
                        {a.params.length > 0 && (
                          <div className="grid grid-cols-1 gap-2 md:grid-cols-2">
                            {a.params.map((p) => (
                              <div key={p.name} className="flex flex-col gap-1">
                                <div className="sk-hint">
                                  {p.name}：{p.desc}
                                  {p.required ? "（必填）" : "（可选）"}
                                </div>
                                <Input
                                  placeholder={p.name}
                                  value={paramValues[a.id]?.[p.name] ?? ""}
                                  onChange={(e) =>
                                    setParamValues((m) => ({
                                      ...m,
                                      [a.id]: { ...(m[a.id] ?? {}), [p.name]: e.target.value },
                                    }))
                                  }
                                />
                              </div>
                            ))}
                          </div>
                        )}
                        <div className="flex flex-wrap items-center gap-2">
                          <Button
                            variant="primary"
                            size="sm"
                            loading={busy === a.id}
                            disabled={(busy !== "" || comboRun !== null) && busy !== a.id}
                            onClick={() => void run(a)}
                          >
                            执行
                          </Button>
                          {out?.need_reboot && (
                            // 执行成功但需重启时给持久提醒，光靠一次性 toast 会被错过
                            <Badge tone="err">需重启后生效</Badge>
                          )}
                        </div>
                        {out?.output && (
                          // 完整输出可展开+可复制，替代原来静默截断 160 字符
                          <div className="flex flex-col gap-1">
                            <div
                              className="sk-hint whitespace-pre-wrap break-all"
                              style={
                                expandedOutputs[a.id]
                                  ? undefined
                                  : {
                                        display: "-webkit-box",
                                        WebkitBoxOrient: "vertical",
                                        WebkitLineClamp: 3,
                                        overflow: "hidden",
                                      }
                              }
                            >
                              {out.output}
                            </div>
                            <div className="flex items-center gap-2">
                              <Button
                                variant="ghost"
                                size="sm"
                                onClick={() =>
                                  setExpandedOutputs((m) => ({ ...m, [a.id]: !m[a.id] }))
                                }
                              >
                                {expandedOutputs[a.id] ? "收起" : "展开全部"}
                              </Button>
                              <Button
                                variant="ghost"
                                size="sm"
                                onClick={() => {
                                  navigator.clipboard
                                    .writeText(out.output)
                                    .then(() => toast.success("已复制完整输出"))
                                    .catch(() => toast.error("复制失败：剪贴板不可用"));
                                }}
                              >
                                复制完整输出
                              </Button>
                            </div>
                          </div>
                        )}
                        {out && !out.output && out.message && (
                          <div className="sk-hint">{out.message}</div>
                        )}
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          </CardBody>
        </Card>
      ))}

      {!data && (
        <Card>
          <CardBody>
            {loadError ? (
              // 加载失败 ≠ 加载中：给出原因和重试入口，而不是永远显示「正在读取」
              <div className="flex flex-col items-center gap-2">
                <Empty description={`修复动作库加载失败：${loadError}`} />
                <Button
                  size="sm"
                  onClick={() => {
                    setLoadError("");
                    void load();
                  }}
                >
                  重试
                </Button>
              </div>
            ) : (
              <Empty description="正在读取修复动作库…" />
            )}
          </CardBody>
        </Card>
      )}

      {data && (
        <Card>
          <CardHeader>
            <CardTitle>
              <span className="sk-hint font-normal">动作清单（表格视图，顺序与后端 ACTION_IDS 一致）</span>
            </CardTitle>
          </CardHeader>
          <CardBody>
            <Table<RepairAction> rowKey={(r) => r.id} dataSource={data.actions} columns={listColumns} />
          </CardBody>
        </Card>
      )}

      <BackupRollbackCard />

      <Card>
        <CardHeader>
          <CardTitle>修复记录（本次运行）</CardTitle>
          <span className="sk-hint">手动 / 组合 / 后台监控自动修复统一入账（最新在前，最多 60 条）</span>
        </CardHeader>
        <CardBody>
          {records.length === 0 ? (
            <Empty description="暂无记录。执行修复动作，或等待后台监控自动修复后这里会实时出现条目。" />
          ) : (
            // 后台自动修复原本只写日志：用户完全看不见「后台动过系统」，这里补上可见性
            <div className="max-h-[200px] overflow-y-auto">
              {records.map((r, i) => (
                <div key={`${r.time}-${i}`} className="py-0.5">
                  <span
                    style={{
                      color: `var(--sk-${r.mark === "√" ? "ok" : r.mark === "!" ? "warn" : "err"})`,
                      fontWeight: 600,
                    }}
                  >
                    {r.mark}
                  </span>
                  <span className="sk-hint ml-2">{r.time}</span>
                  <span className="ml-2">{r.text}</span>
                </div>
              ))}
            </div>
          )}
        </CardBody>
      </Card>
    </div>
  );
}
