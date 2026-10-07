/**
 * WiFi 工具页：周边 AP / 信道分析 / 二维码分享 / 已存密码 四个 tab
 * （对齐 Python `ui/wifi_page.py` 的完整功能面——此前 Rust 版只有「已存密码」）。
 *
 * 密码纪律（本页是凭据出入口，逐条锁死）：
 * - 已存密码的复制必须走敏感通道（判定见 wifipassword.ts，测试锁定）；
 * - 二维码的 WIFI: 编码串含明文密码——**只渲染成图，绝不显示串本身、
 *   不写日志**；页面上只展示 SSID 与认证方式；
 * - 扫描结果不含凭据，可随意展示与自动刷新。
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Alert,
  Button,
  Card,
  Descriptions,
  Empty,
  Input,
  Progress,
  Radio,
  Select,
  Space,
  Spin,
  Switch,
  Table,
  Tabs,
  Tag,
  Typography,
  message,
} from "antd";

import { svgDataUrl } from "./qr";
import { useFillRowHeight } from "./useFillRow";
import {
  aggregateChannels,
  channelAdviceText,
  channelLoadTag,
  copyResultMessage,
  maskPassword,
  passwordErrorHint,
  QR_AUTH_OPTIONS,
  qrAuthLabel,
  recommendChannels,
  SENSITIVE_COPY_COMMAND,
  scanFailedHint,
  scanSummaryText,
  signalTag,
  strengthHint,
  validateWifiQrInput,
  WIFI_SCAN_INTERVAL_MS, // Python 同名常量实值 8s（wifi_page.py:169），对齐
  type ChannelUsage,
  type ProfileCredential,
  type ProfilesResult,
  type ScanResult,
  type ScannedAp,
  type SensitiveCopyResult,
} from "./wifipassword";

/** WiFi 工具页容器：扫描结果与配置文件列表在父级持有，四个 tab 共享。 */
export function WifiPage() {
  const [aps, setAps] = useState<ScannedAp[]>([]);
  const [scanning, setScanning] = useState(false);
  const [scanErr, setScanErr] = useState("");
  const [scannedAt, setScannedAt] = useState<Date | null>(null);
  const [auto, setAuto] = useState(false);
  const [profiles, setProfiles] = useState<string[]>([]);
  const [profilesLoading, setProfilesLoading] = useState(false);
  // 在途互斥用 ref 而不是 state：interval 回调里读到的一定是最新值，
  // 不会因闭包捕获旧值而并发起两次 netsh
  const scanBusy = useRef(false);

  const scan = useCallback(async () => {
    if (scanBusy.current) return;
    scanBusy.current = true;
    setScanning(true);
    try {
      const r = await invoke<ScanResult>("sk_wifi_scan");
      setAps(r.aps);
      setScannedAt(new Date());
      setScanErr("");
    } catch (e) {
      // 失败时保留上一次的列表（信号是"这轮没扫到"，不是"清空重来"）
      setScanErr(scanFailedHint(String(e)));
    } finally {
      scanBusy.current = false;
      setScanning(false);
    }
  }, []);

  // Python 版在 showEvent 首显时才扫描：AntD Tabs 的 pane 首次激活才挂载，
  // 挂载即首显——用户不开 WiFi 页就不会白跑 netsh
  useEffect(() => {
    void scan();
  }, [scan]);

  // 自动刷新 tick：页面不可见（最小化/切走）时跳过本拍——对齐 Python hideEvent 停表
  useEffect(() => {
    if (!auto) return;
    const t = window.setInterval(() => {
      if (!document.hidden) void scan();
    }, WIFI_SCAN_INTERVAL_MS);
    return () => window.clearInterval(t);
  }, [auto, scan]);

  // 配置文件枚举（netsh 可达数百 ms）只做一次，「二维码分享」与「已存密码」共用
  const loadProfiles = useCallback(async () => {
    setProfilesLoading(true);
    try {
      const r = await invoke<ProfilesResult>("sk_wifi_profiles");
      setProfiles(r.profiles);
    } catch (e) {
      message.error(String(e));
    } finally {
      setProfilesLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadProfiles();
  }, [loadProfiles]);

  return (
    <Tabs
      items={[
        {
          key: "scan",
          label: "周边 AP",
          children: (
            <ScanTab
              aps={aps}
              scanning={scanning}
              scanErr={scanErr}
              scannedAt={scannedAt}
              auto={auto}
              onAutoChange={setAuto}
              onScan={() => void scan()}
            />
          ),
        },
        { key: "channels", label: "信道分析", children: <ChannelTab aps={aps} /> },
        {
          key: "share",
          label: "二维码分享",
          children: <ShareTab profiles={profiles} profilesLoading={profilesLoading} />,
        },
        {
          key: "passwords",
          label: "已存密码",
          children: (
            <PasswordsTab
              profiles={profiles}
              profilesLoading={profilesLoading}
              onRefreshProfiles={() => void loadProfiles()}
            />
          ),
        },
      ]}
    />
  );
}

// ---------------- Tab 1：周边 AP ----------------

function ScanTab(props: {
  aps: ScannedAp[];
  scanning: boolean;
  scanErr: string;
  scannedAt: Date | null;
  auto: boolean;
  onAutoChange: (v: boolean) => void;
  onScan: () => void;
}) {
  return (
    <Space direction="vertical" size={10} style={{ width: "100%" }}>
      <Space wrap>
        <Button size="small" type="primary" loading={props.scanning} onClick={props.onScan}>
          扫描
        </Button>
        <span className="sk-hint">每 10 秒自动刷新</span>
        <Switch size="small" checked={props.auto} onChange={props.onAutoChange} />
        {props.scannedAt && !props.scanErr && (
          <Tag>{scanSummaryText(props.aps.length, props.scannedAt)}</Tag>
        )}
      </Space>

      {props.scanErr && (
        <Alert type="warning" showIcon message="扫描失败" description={props.scanErr} />
      )}

      <Table<ScannedAp>
        size="small"
        rowKey={(r) => r.bssid || `${r.ssid}-${r.signal}`}
        pagination={{ pageSize: 20, showSizeChanger: false, hideOnSinglePage: true }}
        dataSource={props.aps}
        loading={props.scanning}
        locale={{
          emptyText: (
            <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="尚未扫描或未发现网络" />
          ),
        }}
        columns={[
          {
            title: "SSID",
            dataIndex: "ssid",
            ellipsis: true,
            render: (v: string, r: ScannedAp) => (
              <Space size={6}>
                {v === "(隐藏)" ? (
                  <Typography.Text type="secondary">{v}</Typography.Text>
                ) : (
                  <span>{v}</span>
                )}
                {r.radio && <span className="sk-hint">{r.radio}</span>}
              </Space>
            ),
          },
          {
            title: "BSSID",
            dataIndex: "bssid",
            width: 175,
            render: (v: string) => <Typography.Text code>{v}</Typography.Text>,
          },
          {
            title: "信号",
            dataIndex: "signal",
            width: 90,
            render: (v: number) => <Tag color={signalTag(v)}>{v}%</Tag>,
          },
          {
            title: "信道",
            dataIndex: "channel",
            width: 70,
            render: (v: number) => v || "—",
          },
          {
            title: "频段",
            dataIndex: "band",
            width: 80,
            render: (v: string) => (v === "?" ? <Tag>未知</Tag> : <Tag color="blue">{v}</Tag>),
          },
          {
            title: "认证",
            dataIndex: "auth",
            width: 150,
            ellipsis: true,
            render: (v: string) => v || "—",
          },
        ]}
      />

      <Typography.Paragraph type="secondary" className="sk-hint" style={{ marginBottom: 0 }}>
        提示：Win11 扫描周边 WiFi 需要系统「位置」服务开启（设置→隐私和安全性→位置）。
        扫描走 netsh，双频合一的网络按 BSSID 拆成多行（2.4G / 5G 各一条）。
      </Typography.Paragraph>
    </Space>
  );
}

// ---------------- Tab 2：信道分析 ----------------

function ChannelTab({ aps }: { aps: ScannedAp[] }) {
  const agg = useMemo(() => aggregateChannels(aps), [aps]);
  const advice = useMemo(() => recommendChannels(aps), [aps]);

  if (aps.length === 0) {
    return (
      <Empty
        image={Empty.PRESENTED_IMAGE_SIMPLE}
        description="暂无数据——先在「周边 AP」扫描一次"
      />
    );
  }

  const max24 = Math.max(1, ...agg.c24.map((c) => c.count));
  const max5 = Math.max(1, ...agg.c5.map((c) => c.count));

  const bandBlock = (title: string, rows: ChannelUsage[], max: number) => (
    <Card size="small" className="sk-card" title={title}>
      {rows.length === 0 ? (
        <Typography.Text type="secondary" className="sk-hint">
          未发现该频段的网络
        </Typography.Text>
      ) : (
        <Space direction="vertical" size={4} style={{ width: "100%" }}>
          {rows.map((r) => (
            <Space key={r.channel} size={8}>
              <Tag style={{ minWidth: 64, textAlign: "center" }}>信道 {r.channel}</Tag>
              <Progress
                percent={(r.count / max) * 100}
                showInfo={false}
                size="small"
                style={{ width: 320, maxWidth: "50vw", marginBottom: 0 }}
              />
              <Tag color={channelLoadTag(r.count)}>{r.count} 个 AP</Tag>
            </Space>
          ))}
        </Space>
      )}
    </Card>
  );

  return (
    <Space direction="vertical" size={10} style={{ width: "100%" }}>
      <Alert type="info" showIcon message={channelAdviceText(advice)} />
      {bandBlock("2.4GHz 信道占用（1~13，AP 数量）", agg.c24, max24)}
      {bandBlock("5GHz 信道占用（AP 数量）", agg.c5, max5)}
      <Typography.Paragraph type="secondary" className="sk-hint" style={{ marginBottom: 0 }}>
        条长 = 该信道 AP 数占本频段最挤信道的比例；颜色：绿 ≤2 个、橙 ≤4 个、红更多。
        推荐 2.4G 信道计入 ±4 内邻频重叠与信号强弱；5G 按 80MHz 块挑最空锚点并避开 DFS 雷达信道。
      </Typography.Paragraph>
    </Space>
  );
}

// ---------------- Tab 3：二维码分享 ----------------

function ShareTab(props: { profiles: string[]; profilesLoading: boolean }) {
  const [mode, setMode] = useState<"profile" | "manual">("profile");
  const [selected, setSelected] = useState<string | undefined>(undefined);
  const [ssid, setSsid] = useState("");
  const [password, setPassword] = useState("");
  const [auth, setAuth] = useState<string>("WPA");
  const [qrSvg, setQrSvg] = useState("");
  const [qrSsid, setQrSsid] = useState("");
  const [qrAuth, setQrAuth] = useState("");
  const [busy, setBusy] = useState(false);

  // 从已存配置生成：读真实密钥（需管理员）→ 后端拼标准 WIFI 串 → 本地渲染。
  // 全程用点击时捕获的 selected，响应回来只绑定 cred.name——不存在
  // 「A 的 SSID + B 的密码」错配（Python 版用过期检测防的就是这个）。
  const generateFromProfile = useCallback(async () => {
    if (!selected || busy) return;
    setBusy(true);
    try {
      const cred = await invoke<ProfileCredential>("sk_wifi_profile_password", { name: selected });
      const svg = await invoke<string>("sk_wifi_qr_svg", {
        ssid: cred.name,
        password: cred.password,
        auth: cred.auth,
      });
      setQrSvg(svg);
      setQrSsid(cred.name);
      setQrAuth(cred.auth || "开放式");
    } catch (e) {
      // 失败即收起旧码；文案不回带密码（后端也不写日志）
      setQrSvg("");
      setQrSsid("");
      setQrAuth("");
      message.error(passwordErrorHint(String(e)));
    } finally {
      setBusy(false);
    }
  }, [selected, busy]);

  const generateManual = useCallback(async () => {
    const invalid = validateWifiQrInput(ssid, password, auth);
    if (invalid) {
      message.warning(invalid);
      return;
    }
    if (busy) return;
    setBusy(true);
    try {
      const svg = await invoke<string>("sk_wifi_qr_svg", {
        ssid: ssid.trim(),
        password,
        auth,
      });
      setQrSvg(svg);
      setQrSsid(ssid.trim());
      setQrAuth(qrAuthLabel(auth));
    } catch (e) {
      setQrSvg("");
      setQrSsid("");
      setQrAuth("");
      message.error(String(e));
    } finally {
      setBusy(false);
    }
  }, [ssid, password, auth, busy]);

  // 保存 PNG：SVG 画上白底后转位图（静区必须白，扫码可靠性硬要求）
  const saveQrPng = useCallback(async () => {
    if (!qrSvg) return;
    const img = new Image();
    img.src = svgDataUrl(qrSvg);
    try {
      await new Promise<void>((res, rej) => {
        img.onload = () => res();
        img.onerror = () => rej(new Error("SVG 加载失败"));
      });
      const side = 560;
      const canvas = document.createElement("canvas");
      canvas.width = side;
      canvas.height = side;
      const ctx = canvas.getContext("2d");
      if (!ctx) throw new Error("画布不可用");
      ctx.fillStyle = "#fff";
      ctx.fillRect(0, 0, side, side);
      ctx.drawImage(img, 0, 0, side, side);
      const a = document.createElement("a");
      a.href = canvas.toDataURL("image/png");
      a.download = "wifi_qr.png";
      a.click();
      message.success("已保存 wifi_qr.png");
    } catch (e) {
      message.error(`二维码导出失败：${String(e)}`);
    }
  }, [qrSvg]);

  return (
    <Space direction="vertical" size={10} style={{ width: "100%" }}>
      <Space wrap>
        <Radio.Group
          optionType="button"
          buttonStyle="solid"
          size="small"
          value={mode}
          onChange={(e) => setMode(e.target.value)}
          options={[
            { value: "profile", label: "从已存配置生成" },
            { value: "manual", label: "手动输入" },
          ]}
        />
        {mode === "profile" ? (
          <>
            <Select
              style={{ minWidth: 240 }}
              placeholder="选择已保存的 WiFi"
              value={selected}
              onChange={setSelected}
              loading={props.profilesLoading}
              options={props.profiles.map((p) => ({ value: p, label: p }))}
            />
            <Button
              size="small"
              type="primary"
              disabled={!selected}
              loading={busy}
              onClick={() => void generateFromProfile()}
            >
              生成二维码
            </Button>
          </>
        ) : (
          <>
            <Input
              style={{ width: 180 }}
              placeholder="SSID"
              value={ssid}
              allowClear
              onChange={(e) => setSsid(e.target.value)}
            />
            <Input.Password
              style={{ width: 190 }}
              placeholder="密码（开放网络留空）"
              value={password}
              autoComplete="new-password"
              onChange={(e) => setPassword(e.target.value)}
            />
            <Select
              style={{ width: 175 }}
              value={auth}
              onChange={setAuth}
              options={QR_AUTH_OPTIONS.map((o) => ({ value: o.value, label: o.label }))}
            />
            <Button
              size="small"
              type="primary"
              loading={busy}
              onClick={() => void generateManual()}
            >
              生成二维码
            </Button>
          </>
        )}
        {props.profilesLoading && <Spin size="small" />}
      </Space>

      {mode === "profile" && !props.profilesLoading && props.profiles.length === 0 && (
        <Alert
          type="info"
          showIcon
          message="没有已保存的 WiFi 配置"
          description="本机没有保存过 WLAN 配置文件。可切换到「手动输入」直接填 SSID 与密码生成分享二维码。"
        />
      )}

      {qrSvg ? (
        <Card
          size="small"
          className="sk-card"
          title={`WiFi 二维码：${qrSsid}`}
          extra={
            <Button size="small" onClick={() => void saveQrPng()}>
              保存图片
            </Button>
          }
        >
          <Space direction="vertical" size={8} align="start">
            {/* 白底是扫码可靠性的硬要求（深色主题下静区也必须是白的） */}
            <div style={{ background: "#fff", padding: 8, width: 296 }}>
              <img src={svgDataUrl(qrSvg)} width={280} height={280} alt="WiFi 二维码" />
            </div>
            <Space size={6}>
              <Tag color="blue">SSID：{qrSsid}</Tag>
              {qrAuth && <Tag>认证：{qrAuth}</Tag>}
            </Space>
            <Typography.Text type="secondary" className="sk-hint">
              标准 WiFi 二维码格式，手机相机扫码即可加入网络。编码串包含明文密码，本页只渲染成图、
              不显示串本身——也别把二维码发给不该有网络权限的人。
            </Typography.Text>
          </Space>
        </Card>
      ) : (
        <Empty
          image={Empty.PRESENTED_IMAGE_SIMPLE}
          description={busy ? "生成中…" : "选择配置或手动输入后，生成后显示在此"}
        />
      )}
    </Space>
  );
}

// ---------------- Tab 4：已存密码 ----------------

/**
 * 已存密码：左栏已存 WiFi 列表（点击选中），右栏该网络的密钥详情——
 * 与密码库同款两栏形态（用户指定交互）。
 * 纪律不变：读取密钥需管理员（每次点击显式触发，**绝不自动读第一项**——
 * 免得进页面就弹 UAC）；复制走敏感路径。
 */
function PasswordsTab(props: {
  profiles: string[];
  profilesLoading: boolean;
  onRefreshProfiles: () => void;
}) {
  const [selected, setSelected] = useState("");
  const [cred, setCred] = useState<ProfileCredential | null>(null);
  const [err, setErr] = useState("");
  const [loading, setLoading] = useState(false);
  const [show, setShow] = useState(false);

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
  // 行高填满 Content 视口：32+ 条 WiFi 时只滚左列表，标签栏与右栏不动
  const row = useFillRowHeight(true);

  return (
    <div
      ref={row.ref}
      style={{
        display: "flex",
        gap: 12,
        alignItems: "stretch",
        height: row.h,
        overflow: "hidden",
      }}
    >
      {/* 左栏：已存 WiFi 列表（固定 ~280px，刷新嵌在卡标题）；列表在卡体内自滚 */}
      <Card
        size="small"
        className="sk-card"
        style={{ width: 280, flexShrink: 0, display: "flex", flexDirection: "column" }}
        styles={{ body: { padding: "4px 4px 8px", flex: 1, minHeight: 0, overflowY: "auto" } }}
        title={`已存 WiFi（${props.profiles.length}）`}
        extra={
          <Button size="small" loading={props.profilesLoading} onClick={props.onRefreshProfiles}>
            刷新
          </Button>
        }
      >
        <Table<string>
          size="small"
          rowKey={(r) => r}
          showHeader={false}
          loading={props.profilesLoading}
          dataSource={props.profiles}
          pagination={false}
          onRow={(r) => ({
            onClick: () => void read(r),
            style: { cursor: "pointer" },
          })}
          rowClassName={(r) => (r === selected ? "ant-table-row-selected" : "")}
          columns={[
            {
              key: "ssid",
              render: (_: unknown, name: string) => (
                <div
                  style={{
                    fontWeight: 500,
                    overflow: "hidden",
                    textOverflow: "ellipsis",
                    whiteSpace: "nowrap",
                  }}
                >
                  {name}
                </div>
              ),
            },
          ]}
          locale={{ emptyText: "本机没有已保存的 WiFi 配置（或无线网卡被禁用）" }}
        />
      </Card>

      {/* 右栏：所选网络的密钥详情（flex:1；未选中时给引导文案） */}
      <Card
        size="small"
        className="sk-card"
        style={{ flex: 1, minWidth: 0, display: "flex", flexDirection: "column" }}
        styles={{ body: { flex: 1, minHeight: 0, overflowY: "auto" } }}
        title={selected ? `${selected} 的密钥` : "WiFi 密钥"}
      >
        {!selected ? (
          <Empty
            image={Empty.PRESENTED_IMAGE_SIMPLE}
            description="点击左侧 WiFi 查看密钥——读取需要管理员权限，点一次读一个"
          />
        ) : loading ? (
          <div style={{ textAlign: "center", padding: "28px 0" }}>
            <Spin tip={`正在读取 ${selected} 的密钥…`} />
          </div>
        ) : err ? (
          <Alert type="warning" showIcon message="读取失败" description={err} />
        ) : cred ? (
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
        ) : null}
      </Card>
    </div>
  );
}
