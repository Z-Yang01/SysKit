/**
 * 系统助手：场景模式（对应 Python 侧 `ModesTab`）。
 *
 * 每步结果都列出来——"暂停更新需要管理员"这类步骤失败时用户必须看到，
 * 不能只给一句"已进入游戏模式"。
 */
import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Alert,
  Button,
  Card,
  Checkbox,
  Empty,
  Modal,
  Space,
  Table,
  Tag,
  Tooltip,
  Typography,
  message,
} from "antd";

import {
  adminNote,
  applyModeRun,
  enterConfirmText,
  failedSteps,
  isEntering,
  modeLabel,
  outcomeLevel,
  outcomeSummary,
  updatePolicyNote,
  type ModeActive,
  type ModeKey,
  type ModeRunResult,
  type ModeStep,
  type ModesInfo,
} from "./modes";

export function ModesPanel() {
  const [info, setInfo] = useState<ModesInfo | null>(null);
  const [openApps, setOpenApps] = useState(true);
  const [clearBg, setClearBg] = useState(true);
  const [result, setResult] = useState<ModeRunResult | null>(null);
  const [busy, setBusy] = useState<ModeKey | "">("");
  // 会话内推断的当前模式（挂载时以后端双键持久化值为准，跨会话可见——标签上如实注明）
  const [active, setActive] = useState<ModeActive>({ office: false, game: false });

  useEffect(() => {
    void invoke<ModesInfo>("sk_modes_info")
      .then((info) => {
        setInfo(info);
        // 后端持久化的当前模式优先（跨会话真值）；缺字段回落全 false
        if (info.active_mode) setActive(info.active_mode);
      })
      .catch(() => setInfo(null));
  }, []);

  const run = useCallback(
    async (key: ModeKey) => {
      setBusy(key);
      try {
        const r = await invoke<ModeRunResult>("sk_modes_run", {
          mode: key,
          openApps,
          clearBg,
        });
        setResult(r);
        // 与后端 modes_dual_after_run 同语义：只在全部步骤成功时动标记
        // （进入互斥清另一族、退出只清自己族）；部分失败时系统真实状态未知，
        // 保持原标记不动。旧实现此处硬编码赋值且任意退出清双标记，
        // 会丢仍在生效那族的「进行中」。
        setActive((prev) => applyModeRun(prev, key, r.outcome.fail_count === 0));
        const lvl = outcomeLevel(r.outcome);
        message[lvl === "success" ? "success" : "warning"](r.summary);
      } catch (e) {
        message.error(String(e));
      } finally {
        setBusy("");
      }
    },
    [openApps, clearBg],
  );

  const confirmAndRun = useCallback(
    (key: ModeKey) => {
      if (!isEntering(key)) {
        // 退出是恢复动作，直接执行（出错也会列出步骤）
        void run(key);
        return;
      }
      Modal.confirm({
        title: modeLabel(key),
        width: 560,
        content: (
          <Typography.Paragraph style={{ whiteSpace: "pre-wrap" }}>
            {enterConfirmText(key, { openApps, clearBg })}
          </Typography.Paragraph>
        ),
        okText: modeLabel(key),
        cancelText: "取消",
        onOk: () => run(key),
      });
    },
    [run, openApps, clearBg],
  );

  const steps: ModeStep[] = result?.outcome.steps ?? [];
  const failed = failedSteps(result?.outcome ?? null);
  const admin = adminNote(info);

  return (
    <Space direction="vertical" size={10} style={{ width: "100%" }}>
      {admin && <Alert type="info" showIcon message="部分步骤需要管理员" description={admin} />}

      <Card
        size="small"
        title="办公模式"
        extra={
          active.office && (
            <Tooltip title="以后端持久化记录为准（全部步骤成功才标记）；退出模式即清除。">
              <Tag color="green">进行中</Tag>
            </Tooltip>
          )
        }
      >
        <Space direction="vertical" size={8} style={{ width: "100%" }}>
          <Typography.Text type="secondary">
            勿扰（关通知）+ 静音 + 打开常用软件 + 备份当前配置。退出会恢复通知并取消静音
            （恢复到进模式前的音量）。
          </Typography.Text>
          <Checkbox checked={openApps} onChange={(e) => setOpenApps(e.target.checked)}>
            同时打开常用软件（{info?.office_default_apps?.join(" / ") ?? "记事本 / 计算器 / 资源管理器 / 画图"}）
          </Checkbox>
          <Space>
            <Button
              type="primary"
              size="small"
              loading={busy === "office"}
              // 执行期间四个按钮全部锁住：办公/游戏互相改通知与电源，交错执行结果不可预期
              disabled={busy !== "" && busy !== "office"}
              onClick={() => confirmAndRun("office")}
            >
              进入办公模式
            </Button>
            <Button
              size="small"
              loading={busy === "office_exit"}
              disabled={busy !== "" && busy !== "office_exit"}
              onClick={() => confirmAndRun("office_exit")}
            >
              退出办公模式
            </Button>
          </Space>
        </Space>
      </Card>

      <Card
        size="small"
        title="游戏模式"
        extra={
          active.game && (
            <Tooltip title="以后端持久化记录为准（全部步骤成功才标记）；退出模式即清除。">
              <Tag color="green">进行中</Tag>
            </Tooltip>
          )
        }
      >
        <Space direction="vertical" size={8} style={{ width: "100%" }}>
          <Typography.Text type="secondary">
            关通知 + 高性能电源 + 暂停 Windows 更新 + 清理白名单后台。退出会恢复通知、
            平衡电源与更新策略。
          </Typography.Text>
          <Alert type="warning" showIcon message={updatePolicyNote(info)} />
          <Checkbox checked={clearBg} onChange={(e) => setClearBg(e.target.checked)}>
            同时清理白名单后台（仅限 {info?.whitelist?.length ?? 16} 个白名单应用，不碰系统进程）
          </Checkbox>
          <Space>
            <Button
              type="primary"
              size="small"
              loading={busy === "game"}
              disabled={busy !== "" && busy !== "game"}
              onClick={() => confirmAndRun("game")}
            >
              进入游戏模式
            </Button>
            <Button
              size="small"
              loading={busy === "game_exit"}
              disabled={busy !== "" && busy !== "game_exit"}
              onClick={() => confirmAndRun("game_exit")}
            >
              退出游戏模式
            </Button>
          </Space>
        </Space>
      </Card>

      <Card
        size="small"
        title="执行结果"
        extra={
          result && (
            <Tag color={outcomeLevel(result.outcome) === "success" ? "green" : "orange"}>
              {result.outcome.ok_count}/{result.outcome.steps.length}
            </Tag>
          )
        }
      >
        {steps.length === 0 ? (
          <Empty
            image={Empty.PRESENTED_IMAGE_SIMPLE}
            description="尚未执行模式。点击上方按钮查看逐步结果。"
          />
        ) : (
          <Space direction="vertical" size={8} style={{ width: "100%" }}>
            <Alert
              type={outcomeLevel(result?.outcome ?? null)}
              showIcon
              message={outcomeSummary(result?.outcome ?? null)}
              description={
                failed.length > 0 && (
                  <Space direction="vertical" size={2}>
            {failed.map((s, i) => (
              // key 用序号：互斥进入时还原步与进入步各有「通知/勿扰」，
              // name 会撞 key（轮次38 互审实锤）
              <span key={i} className="sk-hint">
                {s.name}：{s.detail}
              </span>
            ))}
                  </Space>
                )
              }
            />
            <Table<ModeStep>
              size="small"
              pagination={false}
              dataSource={steps}
              // rowKey 用行号：互斥进入时还原步与进入步各有「通知/勿扰」
              // （写同一注册表值，先还原后进入），name 撞 key 会让 React
              // 出现重复 key（轮次38 互审实锤）。
              rowKey={(_step, index) => index ?? 0}
              columns={[
                { title: "步骤", dataIndex: "name", width: 200 },
                {
                  title: "结果",
                  dataIndex: "ok",
                  width: 90,
                  render: (v: boolean) => (v ? <Tag color="green">成功</Tag> : <Tag color="red">失败</Tag>),
                },
                { title: "说明", dataIndex: "detail", ellipsis: true },
              ]}
            />
          </Space>
        )}
      </Card>
    </Space>
  );
}
