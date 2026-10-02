/**
 * 诊断 / 修复页。
 *
 * 本页当前交付的是**修复动作库**（20 项分级 + 组合 + 管理员门禁），
 * 数据来自后端 `sk_repair_list` / `sk_repair_plan` / `sk_repair_run`。
 *
 * 网络诊断 15 项检查（netcheck）尚未移植到 Rust，故「诊断结果」区显示明确的
 * 待接入说明而不是空表——按 M3 计划那一批在下一步落地。
 */
import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Alert,
  Button,
  Card,
  Col,
  Collapse,
  Empty,
  Input,
  Modal,
  Row,
  Space,
  Table,
  Tag,
  Typography,
  message,
} from "antd";

import {
  actionableFixes,
  confirmText,
  groupByRisk,
  needsConfirm,
  outcomeLevel,
  riskLabel,
  riskToken,
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
  type RepairAction,
  type RepairList,
  type RepairOutcome,
} from "./repair";

import { statusLabel } from "./repair";

export function Diagnose() {
  const [data, setData] = useState<RepairList | null>(null);
  const [busy, setBusy] = useState<string>("");
  const [outcomes, setOutcomes] = useState<Record<string, RepairOutcome>>({});
  const [paramValues, setParamValues] = useState<Record<string, Record<string, string>>>({});
  const [plan, setPlan] = useState<{ key: string; steps: ComboStep[] } | null>(null);
  const [report, setReport] = useState<DiagReport | null>(null);
  const [diagBusy, setDiagBusy] = useState<"" | "quick" | "full">("");

  const load = useCallback(async () => {
    try {
      setData(await invoke<RepairList>("sk_repair_list"));
    } catch {
      setData(null);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const isAdmin = data?.is_admin ?? false;

  /** 运行诊断（quick=true 跳过慢速外网探测）。 */
  const runDiag = useCallback(async (quick: boolean) => {
    setDiagBusy(quick ? "quick" : "full");
    try {
      const r = await invoke<DiagReport>("sk_diag_run", { quick });
      setReport(r);
      const lvl = r.verdict === "normal" ? "success" : "warning";
      message[lvl](r.verdict_text);
    } catch (e) {
      message.error(`诊断失败：${String(e)}`);
    } finally {
      setDiagBusy("");
    }
  }, []);

  /** 执行一个动作（改配置/强力类先确认）。 */
  const run = useCallback(
    async (action: RepairAction) => {
      const doRun = async () => {
        setBusy(action.id);
        try {
          const out = await invoke<RepairOutcome>("sk_repair_run", {
            id: action.id,
            params: paramValues[action.id] ?? {},
          });
          setOutcomes((m) => ({ ...m, [action.id]: out }));
          const lvl = outcomeLevel(out);
          message[lvl === "success" ? "success" : lvl === "warning" ? "warning" : "error"](
            `${action.title}：${out.message || (out.success ? "完成" : "失败")}`,
          );
        } catch (e) {
          const out: RepairOutcome = {
            success: false,
            message: String(e),
            need_reboot: false,
            output: "",
          };
          setOutcomes((m) => ({ ...m, [action.id]: out }));
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
    [paramValues],
  );

  /** 组合修复：先取计划（含跳过项），确认后逐步执行。 */
  const runCombo = useCallback(
    async (combo: Combo) => {
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
        onOk: async () => {
          for (const st of steps) {
            if (st.skipped_reason) continue;
            const action = data?.actions.find((a) => a.id === st.id);
            if (!action) continue;
            // 组合内逐步执行：安全类直接跑，改配置类已在组合确认里一并确认
            setBusy(action.id);
            try {
              const out = await invoke<RepairOutcome>("sk_repair_run", {
                id: action.id,
                params: paramValues[action.id] ?? {},
              });
              setOutcomes((m) => ({ ...m, [action.id]: out }));
              if (!out.success) {
                message.warning(`${action.title} 未成功：${out.message}`);
              }
            } catch (e) {
              message.error(`${action.title}：${String(e)}`);
            } finally {
              setBusy("");
            }
          }
          message.success(`${combo.title} 执行完毕`);
        },
      });
    },
    [data, isAdmin, paramValues],
  );

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
        <Space wrap>
          {(data?.combos ?? []).map((c) => (
            <Button key={c.key} onClick={() => void runCombo(c)} loading={busy !== ""}>
              {c.title}
              <span className="sk-hint" style={{ marginLeft: 6 }}>
                {c.actions.length} 步
              </span>
            </Button>
          ))}
          {plan && (
            <span className="sk-hint">
              最近计划：{plan.steps.length} 步（{summarizePlan(plan.steps).run} 执行 /{" "}
              {summarizePlan(plan.steps).skipped} 跳过）
            </span>
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
              onClick={() => void runDiag(true)}
              loading={diagBusy === "quick"}
            >
              快速检测
            </Button>
            <Button
              type="primary"
              size="small"
              onClick={() => void runDiag(false)}
              loading={diagBusy === "full"}
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

              {actionableFixes(report).length > 0 && (
                <Alert
                  type="info"
                  showIcon
                  message={`诊断建议修复：${actionableFixes(report).join("、")}（可在下方对应动作里执行）`}
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
                    title: "详情",
                    dataIndex: "detail",
                    render: (v: string, row) => (
                      <Space direction="vertical" size={2}>
                        <span className="sk-hint">{v}</span>
                        {row.fix_ids.length > 0 && (
                          <span className="sk-hint">建议动作：{row.fix_ids.join("、")}</span>
                        )}
                      </Space>
                    ),
                  },
                ]}
              />
            </>
          ) : (
            <Empty
              image={Empty.PRESENTED_IMAGE_SIMPLE}
              description={
                diagBusy
                  ? "正在检测…（含 ping / HTTP / DNS 查询与网卡采集，完整检测约需数秒）"
                  : "尚未检测。点「完整检测」运行全部 15 项网络检查（NlaSvc / 持久路由 / 静态 DNS / 代理 / 网关 / DNS 对比 / NCSI / 服务 / 路由 / IPv6 / WLAN …）；「快速检测」跳过 5 项慢速项。"
              }
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
            items={g.items.map((a) => {
              const out = outcomes[a.id];
              const skip = skipReason(a, isAdmin);
              return {
                key: a.id,
                label: (
                  <Space>
                    <span>{a.title}</span>
                    {a.need_reboot && <Tag color="red">需重启</Tag>}
                    {skip && <Tag color="default">将被跳过</Tag>}
                    {out && (
                      <Tag color={out.success ? "green" : out.message.includes("尚未") ? "orange" : "red"}>
                        {out.success ? "已成功" : out.message.includes("尚未") ? "未接入" : "失败"}
                      </Tag>
                    )}
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
                    <Space>
                      <Button
                        type="primary"
                        size="small"
                        loading={busy === a.id}
                        onClick={() => void run(a)}
                      >
                        执行
                      </Button>
                      {out?.output && (
                        <Typography.Text type="secondary" className="sk-hint">
                          输出：{out.output.slice(0, 160)}
                        </Typography.Text>
                      )}
                    </Space>
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
          <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="正在读取修复动作库…" />
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
          ]}
        />
      )}
    </Space>
  );
}
