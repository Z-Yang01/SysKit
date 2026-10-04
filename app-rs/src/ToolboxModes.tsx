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
  // 会话内推断的当前模式（后端不持久化，切页后丢失——标签上如实注明）
  const [active, setActive] = useState<ModeActive>({ office: false, game: false });

  useEffect(() => {
    void invoke<ModesInfo>("sk_modes_info")
      .then(setInfo)
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
        // 只有全部步骤成功才更新"进行中"推断；部分失败时状态未知，不改
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
            <Tooltip title="按本次会话内最近一次成功执行推断；后端不持久化模式状态，重启应用后此标记消失。">
              <Tag color="green">进行中 · 本次会话</Tag>
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
            <Tooltip title="按本次会话内最近一次成功执行推断；后端不持久化模式状态，重启应用后此标记消失。">
              <Tag color="green">进行中 · 本次会话</Tag>
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
                    {failed.map((s) => (
                      <span key={s.name} className="sk-hint">
                        {s.name}：{s.detail}
                      </span>
                    ))}
                  </Space>
                )
              }
            />
            <Table<ModeStep>
              size="small"
              rowKey="name"
              pagination={false}
              dataSource={steps}
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
