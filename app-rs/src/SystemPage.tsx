/**
 * 系统助手：7 个子 tab 的容器（对应 Python 侧 `ui/system_page.py` 的多个 Tab）。
 *
 * 性能监控（PerfPanel）与 WiFi 密码实现在本文件；启动项/磁盘/清理/电池/场景
 * 五个面板拆分在各自文件。剪贴板 tab 已整体移除，不要恢复。
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  Alert,
  Button,
  Card,
  Col,
  Descriptions,
  Empty,
  Row,
  Select,
  Space,
  Spin,
  Statistic,
  Table,
  Tabs,
  Tag,
  Typography,
  message,
} from "antd";

import { ToolboxStartup } from "./ToolboxStartup";
import { CleanupPanel, DiskPanel } from "./ToolboxDisk";
import { BatteryPanel } from "./ToolboxBattery";
import { ModesPanel } from "./ToolboxModes";

import {
  SERIES_LEN,
  formatRate,
  humanBytes,
  isStale,
  procsSummary,
  procRows,
  pushSample,
  signalToken,
  usageToken,
  wifiSummary,
  type PerfTick,
  type TopProcs,
  type WifiInfo,
} from "./system";
import {
  copyResultMessage,
  maskPassword,
  passwordErrorHint,
  SENSITIVE_COPY_COMMAND,
  strengthHint,
  type ProfileCredential,
  type ProfilesResult,
  type SensitiveCopyResult,
} from "./wifipassword";

function tokenColor(token: string): string {
  switch (token) {
    case "ok":
      return "#3fb950";
    case "warn":
      return "#d29922";
    case "err":
      return "#f85149";
    default:
      return "#8b949e";
  }
}

/** 极简 sparkline（不引图表库：本页只需要一条折线）。 */
function Spark({
  values,
  color,
  height = 46,
}: {
  values: number[];
  color: string;
  height?: number;
}) {
  if (values.length < 2) {
    return <div style={{ height }} className="sk-hint" />;
  }
  const max = Math.max(...values, 1);
  const min = Math.min(...values, 0);
  const span = max - min || 1;
  const pts = values
    .map((v, i) => {
      const x = (i / (values.length - 1)) * 100;
      const y = height - ((v - min) / span) * (height - 4) - 2;
      return `${x.toFixed(2)},${y.toFixed(2)}`;
    })
    .join(" ");
  return (
    <svg width="100%" height={height} viewBox={`0 0 100 ${height}`} preserveAspectRatio="none">
      <polyline points={pts} fill="none" stroke={color} strokeWidth="1.2" vectorEffect="non-scaling-stroke" />
    </svg>
  );
}

export function SystemPage() {
  return (
    <Card size="small" className="sk-card">
      <Tabs
        items={[
          { key: "perf", label: "性能监控", children: <PerfPanel /> },
          { key: "startup", label: "启动项管理", children: <ToolboxStartup /> },
          { key: "disk", label: "磁盘空间", children: <DiskPanel /> },
          { key: "clean", label: "垃圾清理", children: <CleanupPanel /> },
          { key: "battery", label: "电池健康", children: <BatteryPanel /> },
          { key: "modes", label: "场景模式", children: <ModesPanel /> },
          { key: "wifipwd", label: "WiFi 密码", children: <WifiPasswordPanel /> },
        ]}
      />
    </Card>
  );
}

function PerfPanel() {
  const [cpu, setCpu] = useState<number[]>([]);
  const [mem, setMem] = useState<number[]>([]);
  const [diskR, setDiskR] = useState<number[]>([]);
  const [diskW, setDiskW] = useState<number[]>([]);
  const [wifi, setWifi] = useState<WifiInfo | null>(null);
  const [wifiFailed, setWifiFailed] = useState(false);
  const [procs, setProcs] = useState<TopProcs | null>(null);
  const [procsFailed, setProcsFailed] = useState(false);
  const [last, setLast] = useState<PerfTick | null>(null);
  const lastTickAt = useRef<number | null>(null);

  // 订阅遥测（每拍 1 秒）
  useEffect(() => {
    let un: (() => void) | undefined;
    void listen<PerfTick>("sk://tick", (e) => {
      const t = e.payload;
      // 断流守卫：页面隐藏期间采样被跳过，切回时清空旧序列，
      // 免得把 10 分钟前的点和现在的点连成一条"平稳曲线"
      const now = Date.now();
      if (isStale(lastTickAt.current, now)) {
        setCpu([]);
        setMem([]);
        setDiskR([]);
        setDiskW([]);
      }
      lastTickAt.current = now;
      setCpu((s) => pushSample(s, t.cpu));
      setMem((s) => pushSample(s, t.mem));
      setDiskR((s) => pushSample(s, t.disk_r));
      setDiskW((s) => pushSample(s, t.disk_w));
      setLast(t);
    }).then((f) => {
      un = f;
    });
    return () => un?.();
  }, []);

  // WiFi 与 Top 进程各自轮询：都是"昂贵"读数，不该挤进 1 秒的 tick 里
  useEffect(() => {
    const load = () =>
      void invoke<WifiInfo>("sk_wifi_info")
        .then((w) => {
          setWifi(w);
          setWifiFailed(false);
        })
        .catch(() => {
          // 失败与"还没读到"必须区分：否则读不到时会永远显示"正在读取…"骗人
          setWifi(null);
          setWifiFailed(true);
        });
    load();
    const t = window.setInterval(load, 5000);
    return () => window.clearInterval(t);
  }, []);

  useEffect(() => {
    const load = () =>
      void invoke<TopProcs>("sk_top_procs", { limit: 15 })
        .then((p) => {
          setProcs(p);
          setProcsFailed(false);
        })
        .catch(() => {
          setProcs(null);
          setProcsFailed(true);
        });
    load();
    // 后端有 2 秒缓存，这里 3 秒轮询即可（tasklist 一次约 0.5 秒）
    const t = window.setInterval(load, 3000);
    return () => window.clearInterval(t);
  }, []);

  const latestCpu = cpu.length ? cpu[cpu.length - 1] : 0;
  const latestMem = mem.length ? mem[mem.length - 1] : 0;
  const diskPeak = useMemo(
    () => Math.max(...diskR, ...diskW, 1),
    [diskR, diskW],
  );

  return (
    <Space direction="vertical" size={12} style={{ width: "100%" }}>
      <Row gutter={12}>
        <Col span={6}>
          <Card size="small" className="sk-card">
            <Statistic
              title="CPU"
              value={latestCpu}
              precision={1}
              suffix="%"
              valueStyle={{ color: tokenColor(usageToken(latestCpu)) }}
            />
            <Spark values={cpu} color={tokenColor(usageToken(latestCpu))} />
          </Card>
        </Col>
        <Col span={6}>
          <Card size="small" className="sk-card">
            <Statistic
              title={`内存${last ? `（已用 ${last.mem_used_gb.toFixed(1)} GB）` : ""}`}
              value={latestMem}
              precision={1}
              suffix="%"
              valueStyle={{ color: tokenColor(usageToken(latestMem)) }}
            />
            <Spark values={mem} color={tokenColor(usageToken(latestMem))} />
          </Card>
        </Col>
        <Col span={12}>
          <Card size="small" className="sk-card" title="磁盘读写">
            <Space direction="vertical" size={2} style={{ width: "100%" }}>
              <Space>
                <Tag color="green">读 {humanBytes(last?.disk_r ?? 0)}</Tag>
                <Tag color="orange">写 {humanBytes(last?.disk_w ?? 0)}</Tag>
                <span className="sk-hint">峰值 {humanBytes(diskPeak)}</span>
              </Space>
              <Spark values={diskR} color="#3fb950" height={28} />
              <Spark values={diskW} color="#d29922" height={28} />
            </Space>
          </Card>
        </Col>
      </Row>

      <Card
        size="small"
        className="sk-card"
        title="WiFi 状态"
        extra={<span className="sk-hint">每 5 秒刷新 · 数据来自 wlanapi.dll</span>}
      >
        {wifi?.connected ? (
          <Descriptions
            size="small"
            column={3}
            items={[
              {
                key: "ssid",
                label: "网络",
                children: wifi.ssid || <span className="sk-hint">隐藏网络</span>,
              },
              {
                key: "signal",
                label: "信号",
                children: (
                  <Tag color={signalToken(wifi.signal) === "ok" ? "green" : signalToken(wifi.signal) === "warn" ? "orange" : "red"}>
                    {wifi.signal ?? "—"}%{wifi.dbm !== null ? `（约 ${wifi.dbm} dBm）` : ""} · {wifi.label}
                  </Tag>
                ),
              },
              { key: "bssid", label: "BSSID", children: wifi.bssid || "—" },
              { key: "rx", label: "接收链路", children: formatRate(wifi.rx_kbps) },
              { key: "tx", label: "发送链路", children: formatRate(wifi.tx_kbps) },
            ]}
          />
        ) : (
          <Empty
            image={Empty.PRESENTED_IMAGE_SIMPLE}
            description={
              wifi
                ? "未连接 WiFi（或本机无无线网卡）"
                : wifiFailed
                  ? "读取 WiFi 状态失败（每 5 秒自动重试）"
                  : "正在读取 WiFi 状态…"
            }
          />
        )}
      </Card>

      <Card
        size="small"
        className="sk-card"
        title="内存占用最高的进程"
        extra={<span className="sk-hint">{procsSummary(procs)}</span>}
      >
        {procs && procs.rows.length > 0 ? (
          <Table
            size="small"
            rowKey="pid"
            pagination={false}
            dataSource={procRows(procs)}
            columns={[
              {
                title: "进程",
                dataIndex: "name",
                render: (v: string, r: { pid: number }) => (
                  <Space size={6}>
                    <span>{v}</span>
                    <span className="sk-hint">PID {r.pid}</span>
                  </Space>
                ),
              },
              {
                title: "内存",
                dataIndex: "bytes",
                width: 130,
                render: (v: number) => humanBytes(v).replace("/s", ""),
              },
              {
                title: "占进程合计",
                dataIndex: "share",
                width: 150,
                render: (v: number) => (
                  <>
                    <Tag color={v >= 10 ? "orange" : "default"}>{v.toFixed(1)}%</Tag>
                  </>
                ),
              },
            ]}
          />
        ) : (
          <Empty
            image={Empty.PRESENTED_IMAGE_SIMPLE}
            description={procsFailed ? "读取进程列表失败（每 3 秒自动重试）" : "正在采集进程列表…"}
          />
        )}
        <Typography.Paragraph type="secondary" className="sk-hint" style={{ marginTop: 8, marginBottom: 0 }}>
          数据源为 <code>tasklist /fo csv</code>（约 0.5 秒/次，后端 2 秒缓存）。
          占比分母是**进程内存合计**而不是物理内存——tasklist 报的是工作集，合计会大于物理内存。
        </Typography.Paragraph>
      </Card>

      <Card size="small" className="sk-card" title="采样窗口">
        <Typography.Paragraph type="secondary" className="sk-hint" style={{ marginBottom: 0 }}>
          曲线保留最近 {SERIES_LEN} 个采样点（1 秒/点，约 3 分钟）。页面隐藏期间采样会暂停，
          切回时若断流超过 10 分钟自动清空旧序列——避免把两段不连续的数据画成一条连续曲线。
          {last && (
            <>
              {" "}
              当前：<Tag>{wifiSummary(wifi)}</Tag>
            </>
          )}
        </Typography.Paragraph>
      </Card>
    </Space>
  );
}

/** WiFi 密码：枚举已保存配置文件，读取明文密钥（需管理员），复制走敏感路径。 */
function WifiPasswordPanel() {
  const [profiles, setProfiles] = useState<string[]>([]);
  const [selected, setSelected] = useState("");
  const [cred, setCred] = useState<ProfileCredential | null>(null);
  const [err, setErr] = useState("");
  const [loading, setLoading] = useState(false);
  const [show, setShow] = useState(false);

  const loadProfiles = useCallback(async () => {
    try {
      const r = await invoke<ProfilesResult>("sk_wifi_profiles");
      setProfiles(r.profiles);
    } catch (e) {
      message.error(String(e));
    }
  }, []);

  useEffect(() => {
    void loadProfiles();
  }, [loadProfiles]);

  const read = useCallback(async (name: string) => {
    setSelected(name);
    setLoading(true);
    setErr("");
    setCred(null);
    setShow(false);
    try {
      setCred(await invoke<ProfileCredential>("sk_wifi_profile_password", { name }));
    } catch (e) {
      setErr(passwordErrorHint(String(e)));
    } finally {
      setLoading(false);
    }
  }, []);

  const copy = useCallback(async () => {
    if (!cred?.password) return;
    try {
      // 纪律：密码必须走敏感路径（不进历史 / 不进 Win+V 与云剪贴板 / 到期清空）
      const r = await invoke<SensitiveCopyResult>(SENSITIVE_COPY_COMMAND, {
        text: cred.password,
      });
      message.success(copyResultMessage(r));
    } catch (e) {
      message.error(String(e));
    }
  }, [cred]);

  const strength = strengthHint(cred?.password ?? "");

  return (
    <Space direction="vertical" size={10} style={{ width: "100%" }}>
      <Space wrap>
        <Select
          style={{ minWidth: 260 }}
          placeholder="选择已保存的 WiFi"
          value={selected || undefined}
          onChange={(v) => void read(v)}
          options={profiles.map((p) => ({ value: p, label: p }))}
        />
        <Button size="small" onClick={() => void loadProfiles()}>
          刷新列表
        </Button>
        {loading && <Spin size="small" />}
      </Space>

      {profiles.length === 0 && !loading && (
        <Alert
          type="info"
          showIcon
          message="没有找到已保存的 WiFi 配置"
          description="本机没有保存过 WLAN 配置文件，或无线网卡被禁用。这里只读取本机已保存的记录，不会扫描周边网络。"
        />
      )}

      {err && <Alert type="warning" showIcon message="读取失败" description={err} />}

      {cred && (
        <Card size="small" title={`${cred.name} 的密钥`}>
          <Space direction="vertical" size={8} style={{ width: "100%" }}>
            <Descriptions size="small" column={1}>
              <Descriptions.Item label="认证方式">{cred.auth || "—"}</Descriptions.Item>
              <Descriptions.Item label="密钥">
                {cred.has_password ? (
                  <Space size={8}>
                    <Typography.Text code>{maskPassword(cred.password, show)}</Typography.Text>
                    <Button size="small" type="link" onClick={() => setShow((s) => !s)}>
                      {show ? "隐藏" : "显示"}
                    </Button>
                  </Space>
                ) : (
                  <Typography.Text type="secondary">（无密钥，可能是开放网络）</Typography.Text>
                )}
              </Descriptions.Item>
              {cred.has_password && (
                <Descriptions.Item label="强度">
                  <Space size={6}>
                    <Tag color={strength.score >= 3 ? "green" : strength.score >= 2 ? "orange" : "red"}>
                      {strength.label}
                    </Tag>
                    <span className="sk-hint">{strength.advice}</span>
                  </Space>
                </Descriptions.Item>
              )}
            </Descriptions>
            {cred.has_password && (
              <Space>
                <Button type="primary" size="small" onClick={() => void copy()}>
                  复制密钥
                </Button>
                <Typography.Text type="secondary" className="sk-hint">
                  复制会走敏感通道：不进剪贴板历史、不进 Win+V 与云剪贴板，
                  {copyResultMessage({
                    ttl: 0,
                    note: "并按设置里的「敏感内容存活」到期清空",
                  })}
                </Typography.Text>
              </Space>
            )}
          </Space>
        </Card>
      )}
    </Space>
  );
}


