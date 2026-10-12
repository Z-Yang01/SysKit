/**
 * 系统助手：电池健康（对应 Python 侧 `BatteryTab`）。
 *
 * 实时状态每 10 秒刷新（`GetSystemPowerStatus` 微秒级）；健康度**只在用户点刷新时**
 * 查一次——那一步要起 PowerShell 查 WMI，最坏 30 秒，轮询会把机器拖住。
 * 与 Python 的懒加载同策略。
 *
 * v1.4 UI 迁移：脱离 AntD（Descriptions → 键值行，Badge/Alert/Empty → ui 件）。
 */
import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import {
  capacityText,
  chargingLabel,
  healthText,
  lowBatteryWarning,
  percentText,
  percentToken,
  type BatteryHealth,
  type PowerStatus,
} from "./battery";
import { Badge, Button, Card, CardBody, CardHeader, CardTitle, Empty } from "./ui";
import { Alert } from "./ui/alert";
import { useToast } from "./ui/toast";

function toneOf(token: string): "ok" | "warn" | "err" | "neutral" {
  switch (token) {
    case "ok":
      return "ok";
    case "warn":
      return "warn";
    case "err":
      return "err";
    default:
      return "neutral";
  }
}

/** 键值行（AntD Descriptions 的替代：label 左 / value 右，三列布局简化为三行组）。 */
function KV({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex items-baseline gap-3 py-1">
      <span className="w-28 shrink-0 text-sk-sub">{label}</span>
      <span className="min-w-0 text-sk-text">{children}</span>
    </div>
  );
}

export function ToolboxBattery() {
  const toast = useToast();
  const [power, setPower] = useState<PowerStatus | null>(null);
  const [powerFailed, setPowerFailed] = useState(false);
  const [health, setHealth] = useState<BatteryHealth | null>(null);
  const [querying, setQuerying] = useState(false);

  // 实时状态：轻量，可轮询
  useEffect(() => {
    const load = () =>
      void invoke<PowerStatus>("sk_power_status")
        .then((p) => {
          setPower(p);
          setPowerFailed(false);
        })
        .catch(() => {
          // 轮询接口偶发失败不打断已有展示；但首次就失败时不能把"读取失败"
          // 伪装成"正在读取…"，所以要单独记失败标志（成功后清除）
          setPowerFailed(true);
        });
    load();
    const t = window.setInterval(load, 10000);
    return () => window.clearInterval(t);
  }, []);

  const refreshHealth = useCallback(async () => {
    setQuerying(true);
    try {
      setHealth(await invoke<BatteryHealth>("sk_battery_health"));
    } catch (e) {
      toast.error(`查询电池健康度失败：${String(e)}`);
    } finally {
      setQuerying(false);
    }
  }, [toast]);

  const warning = lowBatteryWarning(power);

  return (
    <div className="flex flex-col gap-2.5">
      {warning && <Alert tone="warning" title={warning} />}

      <Card>
        <CardHeader>
          <CardTitle>实时电源状态</CardTitle>
          <span className="sk-hint">每 10 秒刷新 · GetSystemPowerStatus</span>
        </CardHeader>
        <CardBody>
          {power?.has_battery ? (
            <div className="flex flex-col divide-y divide-sk-border/60">
              <KV label="电量">
                <Badge tone={toneOf(percentToken(power))}>{percentText(power)}</Badge>
              </KV>
              <KV label="供电">{chargingLabel(power)}</KV>
              <KV label="预计可用">{power.remaining_human || "—"}</KV>
            </div>
          ) : (
            <Empty
              description={
                power
                  ? power.summary
                  : powerFailed
                    ? "读取电源状态失败（每 10 秒自动重试）"
                    : "正在读取电源状态…"
              }
            />
          )}
        </CardBody>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>电池健康度</CardTitle>
          <Button
            size="sm"
            loading={querying}
            onClick={() => void refreshHealth()}
          >
            {health ? "重新查询" : "查询"}
          </Button>
        </CardHeader>
        <CardBody>
          {health ? (
            <>
              <div className="flex flex-col divide-y divide-sk-border/60">
                <KV label="设计容量">{capacityText(health.design_mwh)}</KV>
                <KV label="满充容量">{capacityText(health.full_mwh)}</KV>
                <KV label="健康度（满充/设计）">
                  <Badge tone={toneOf(health.token)}>{healthText(health)}</Badge>
                </KV>
              </div>
              <div className="sk-hint mb-0 mt-2">{health.note}</div>
            </>
          ) : (
            <div className="flex flex-col gap-1 text-sk-sub">
              <span>点「查询」读取电池健康度。</span>
              <span className="sk-hint">
                这一步要查 WMI（BatteryStaticData / BatteryFullChargedCapacity），最坏约 30 秒，
                所以不做自动轮询——只在你点的时候查一次。
              </span>
            </div>
          )}
        </CardBody>
      </Card>
    </div>
  );
}
