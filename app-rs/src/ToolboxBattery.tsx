/**
 * 系统助手：电池健康（对应 Python 侧 `BatteryTab`）。
 *
 * 实时状态每 10 秒刷新（`GetSystemPowerStatus` 微秒级）；健康度**只在用户点刷新时**
 * 查一次——那一步要起 PowerShell 查 WMI，最坏 30 秒，轮询会把机器拖住。
 * 与 Python 的懒加载同策略。
 */
import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Alert, Button, Card, Descriptions, Empty, Space, Tag, Typography, message } from "antd";

import {
  capacityText,
  chargingLabel,
  healthColor,
  healthText,
  lowBatteryWarning,
  percentText,
  percentToken,
  type BatteryHealth,
  type PowerStatus,
} from "./battery";

function tokenTagColor(token: string): string {
  switch (token) {
    case "ok":
      return "green";
    case "warn":
      return "orange";
    case "err":
      return "red";
    default:
      return "default";
  }
}

export function BatteryPanel() {
  const [power, setPower] = useState<PowerStatus | null>(null);
  const [health, setHealth] = useState<BatteryHealth | null>(null);
  const [querying, setQuerying] = useState(false);

  // 实时状态：轻量，可轮询
  useEffect(() => {
    const load = () =>
      void invoke<PowerStatus>("sk_power_status")
        .then(setPower)
        .catch(() => setPower(null));
    load();
    const t = window.setInterval(load, 10000);
    return () => window.clearInterval(t);
  }, []);

  const refreshHealth = useCallback(async () => {
    setQuerying(true);
    try {
      setHealth(await invoke<BatteryHealth>("sk_battery_health"));
    } catch (e) {
      message.error(`查询电池健康度失败：${String(e)}`);
    } finally {
      setQuerying(false);
    }
  }, []);

  const warning = lowBatteryWarning(power);

  return (
    <Space direction="vertical" size={10} style={{ width: "100%" }}>
      {warning && <Alert type="warning" showIcon message={warning} />}

      <Card
        size="small"
        title="实时电源状态"
        extra={<span className="sk-hint">每 10 秒刷新 · GetSystemPowerStatus</span>}
      >
        {power?.has_battery ? (
          <Descriptions
            size="small"
            column={3}
            items={[
              {
                key: "pct",
                label: "电量",
                children: (
                  <Tag color={tokenTagColor(percentToken(power))}>{percentText(power)}</Tag>
                ),
              },
              { key: "src", label: "供电", children: chargingLabel(power) },
              {
                key: "left",
                label: "预计可用",
                children: power.remaining_human || "—",
              },
            ]}
          />
        ) : (
          <Empty
            image={Empty.PRESENTED_IMAGE_SIMPLE}
            description={power ? power.summary : "正在读取电源状态…"}
          />
        )}
      </Card>

      <Card
        size="small"
        title="电池健康度"
        extra={
          <Button size="small" loading={querying} onClick={() => void refreshHealth()}>
            {health ? "重新查询" : "查询"}
          </Button>
        }
      >
        {health ? (
          <>
            <Descriptions
              size="small"
              column={3}
              items={[
                { key: "d", label: "设计容量", children: capacityText(health.design_mwh) },
                { key: "f", label: "满充容量", children: capacityText(health.full_mwh) },
                {
                  key: "h",
                  label: "健康度（满充/设计）",
                  children: (
                    <Tag color={healthColor(health.token)}>{healthText(health)}</Tag>
                  ),
                },
              ]}
            />
            <Typography.Paragraph type="secondary" className="sk-hint" style={{ marginTop: 8, marginBottom: 0 }}>
              {health.note}
            </Typography.Paragraph>
          </>
        ) : (
          <Space direction="vertical" size={4}>
            <Typography.Text type="secondary">
              点「查询」读取电池健康度。
            </Typography.Text>
            <Typography.Text type="secondary" className="sk-hint">
              这一步要查 WMI（BatteryStaticData / BatteryFullChargedCapacity），最坏约 30 秒，
              所以不做自动轮询——只在你点的时候查一次。
            </Typography.Text>
          </Space>
        )}
      </Card>
    </Space>
  );
}
