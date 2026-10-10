/**
 * WiFi 工具页：周边 AP / 信道分析 / 二维码分享 / 已存密码 四个 tab
 * （对齐 Python `ui/wifi_page.py` 的完整功能面——此前 Rust 版只有「已存密码」）。
 *
 * 密码纪律（本页是凭据出入口，逐条锁死）：
 * - 已存密码的复制必须走敏感通道（判定见 wifipassword.ts，测试锁定）；
 * - 二维码的 WIFI: 编码串含明文密码——**只渲染成图，绝不显示串本身、
 *   不写日志**；页面上只展示 SSID 与认证方式；
 * - 扫描结果不含凭据，可随意展示与自动刷新。
 *
 * v1.4 UI 迁移：脱离 AntD（Table/Select/Radio.Group/Descriptions/Progress → ui 件；
 * 已存密码左列表由 Table 改为纯列表行）。
 */
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

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
import { Badge, Button, Card, CardBody, CardHeader, CardTitle, Empty, Input, Progress, Segmented, Switch, Table, type Column } from "./ui";
import { Select } from "./ui/select";
import { Alert } from "./ui/alert";
import { Tabs } from "./ui/tabs";
import { useToast } from "./ui/toast";

/** "green"/"orange"/"red" 分档 → Badge tone（signalTag/channelLoadTag 共用）。 */
function colorTone(c: string): "ok" | "warn" | "err" {
  return c === "green" ? "ok" : c === "orange" ? "warn" : "err";
}

/** WiFi 工具页容器：扫描结果与配置文件列表在父级持有，四个 tab 共享。 */
export function WifiPage() {
  const toast = useToast();
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

  // ui Tabs keepMounted：本组件挂载（用户切到 WiFi 密码页）即首扫——
  // 用户不开 WiFi 页就不会白跑 netsh（对齐 Python showEvent 首显扫描）
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
      toast.error(String(e));
    } finally {
      setProfilesLoading(false);
    }
  }, [toast]);

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

const SCAN_PAGE_SIZE = 20;

function ScanTab(props: {
  aps: ScannedAp[];
  scanning: boolean;
  scanErr: string;
  scannedAt: Date | null;
  auto: boolean;
  onAutoChange: (v: boolean) => void;
  onScan: () => void;
}) {
  const [page, setPage] = useState(0);
  const pageCount = Math.max(1, Math.ceil(props.aps.length / SCAN_PAGE_SIZE));
  const cur = Math.min(page, pageCount - 1);
  const pageRows = props.aps.slice(cur * SCAN_PAGE_SIZE, cur * SCAN_PAGE_SIZE + SCAN_PAGE_SIZE);

  const columns: Column<ScannedAp>[] = [
    {
      title: "SSID",
      dataIndex: "ssid",
      render: (v: string, r: ScannedAp) => (
        <span className="inline-flex items-center gap-1.5">
          {v === "(隐藏)" ? <span className="text-sk-sub">{v}</span> : <span>{v}</span>}
          {r.radio && <span className="sk-hint">{r.radio}</span>}
        </span>
      ),
    },
    {
      title: "BSSID",
      dataIndex: "bssid",
      width: 175,
      render: (v: string) => <code className="rounded bg-sk-hover px-1.5 py-0.5 font-mono text-xs">{v}</code>,
    },
    {
      title: "信号",
      dataIndex: "signal",
      width: 90,
      render: (v: number) => <Badge tone={colorTone(signalTag(v))}>{v}%</Badge>,
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
      render: (v: string) => (v === "?" ? <Badge>未知</Badge> : <Badge tone="accent">{v}</Badge>),
    },
    {
      title: "认证",
      dataIndex: "auth",
      width: 150,
      render: (v: string) => v || "—",
    },
  ];

  return (
    <div className="flex flex-col gap-2.5">
      <div className="flex flex-wrap items-center gap-2">
        <Button variant="primary" size="sm" loading={props.scanning} onClick={props.onScan}>
          扫描
        </Button>
        <span className="sk-hint">每 10 秒自动刷新</span>
        <Switch checked={props.auto} onChange={props.onAutoChange} aria-label="自动刷新" />
        {props.scannedAt && !props.scanErr && <Badge>{scanSummaryText(props.aps.length, props.scannedAt)}</Badge>}
      </div>

      {props.scanErr && (
        <Alert tone="warning" title="扫描失败">
          {props.scanErr}
        </Alert>
      )}

      <Table<ScannedAp>
        rowKey={(r) => r.bssid || `${r.ssid}-${r.signal}`}
        dataSource={pageRows}
        loading={props.scanning}
        empty={<Empty description="尚未扫描或未发现网络" />}
        columns={columns}
      />
      {pageCount > 1 && (
        <div className="flex items-center justify-end gap-2 text-xs text-sk-sub">
          <span>
            共 {props.aps.length} 个 · 第 {cur + 1}/{pageCount} 页
          </span>
          <Button size="sm" disabled={cur === 0} onClick={() => setPage(cur - 1)}>
            上一页
          </Button>
          <Button size="sm" disabled={cur >= pageCount - 1} onClick={() => setPage(cur + 1)}>
            下一页
          </Button>
        </div>
      )}

      <div className="sk-hint">
        提示：Win11 扫描周边 WiFi 需要系统「位置」服务开启（设置→隐私和安全性→位置）。
        扫描走 netsh，双频合一的网络按 BSSID 拆成多行（2.4G / 5G 各一条）。
      </div>
    </div>
  );
}

// ---------------- Tab 2：信道分析 ----------------

function ChannelTab({ aps }: { aps: ScannedAp[] }) {
  const agg = useMemo(() => aggregateChannels(aps), [aps]);
  const advice = useMemo(() => recommendChannels(aps), [aps]);

  if (aps.length === 0) {
    return <Empty description="暂无数据——先在「周边 AP」扫描一次" />;
  }

  const max24 = Math.max(1, ...agg.c24.map((c) => c.count));
  const max5 = Math.max(1, ...agg.c5.map((c) => c.count));

  const bandBlock = (title: string, rows: ChannelUsage[], max: number) => (
    <Card>
      <CardHeader>
        <CardTitle>{title}</CardTitle>
      </CardHeader>
      <CardBody>
        {rows.length === 0 ? (
          <div className="sk-hint">未发现该频段的网络</div>
        ) : (
          <div className="flex flex-col gap-1">
            {rows.map((r) => (
              <div key={r.channel} className="flex items-center gap-2">
                <Badge className="min-w-16 justify-center">信道 {r.channel}</Badge>
                <Progress
                  percent={(r.count / max) * 100}
                  showInfo={false}
                  className="w-[320px] max-w-[50vw]"
                  tone={colorTone(channelLoadTag(r.count))}
                />
                <Badge tone={colorTone(channelLoadTag(r.count))}>{r.count} 个 AP</Badge>
              </div>
            ))}
          </div>
        )}
      </CardBody>
    </Card>
  );

  return (
    <div className="flex flex-col gap-2.5">
      <Alert tone="info" title={channelAdviceText(advice)} />
      {bandBlock("2.4GHz 信道占用（1~13，AP 数量）", agg.c24, max24)}
      {bandBlock("5GHz 信道占用（AP 数量）", agg.c5, max5)}
      <div className="sk-hint">
        条长 = 该信道 AP 数占本频段最挤信道的比例；颜色：绿 ≤2 个、橙 ≤4 个、红更多。
        推荐 2.4G 信道计入 ±4 内邻频重叠与信号强弱；5G 按 80MHz 块挑最空锚点并避开 DFS 雷达信道。
      </div>
    </div>
  );
}

// ---------------- Tab 3：二维码分享 ----------------

function ShareTab(props: { profiles: string[]; profilesLoading: boolean }) {
  const toast = useToast();
  const [mode, setMode] = useState<"profile" | "manual">("profile");
  const [selected, setSelected] = useState("");
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
      toast.error(passwordErrorHint(String(e)));
    } finally {
      setBusy(false);
    }
  }, [selected, busy, toast]);

  const generateManual = useCallback(async () => {
    const invalid = validateWifiQrInput(ssid, password, auth);
    if (invalid) {
      toast.info(invalid);
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
      toast.error(String(e));
    } finally {
      setBusy(false);
    }
  }, [ssid, password, auth, busy, toast]);

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
      toast.success("已保存 wifi_qr.png");
    } catch (e) {
      toast.error(`二维码导出失败：${String(e)}`);
    }
  }, [qrSvg, toast]);

  return (
    <div className="flex flex-col gap-2.5">
      <div className="flex flex-wrap items-center gap-2">
        <Segmented<"profile" | "manual">
          size="sm"
          value={mode}
          onChange={setMode}
          options={[
            { value: "profile", label: "从已存配置生成" },
            { value: "manual", label: "手动输入" },
          ]}
        />
        {mode === "profile" ? (
          <>
            <Select
              className="min-w-60"
              placeholder="选择已保存的 WiFi"
              value={selected}
              onChange={setSelected}
              disabled={props.profilesLoading}
              options={props.profiles.map((p) => ({ value: p, label: p }))}
            />
            <Button
              variant="primary"
              size="sm"
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
              className="w-44"
              placeholder="SSID"
              value={ssid}
              onChange={(e) => setSsid(e.target.value)}
            />
            <Input
              className="w-48"
              type="password"
              placeholder="密码（开放网络留空）"
              value={password}
              autoComplete="new-password"
              onChange={(e) => setPassword(e.target.value)}
            />
            <Select
              className="w-44"
              value={auth}
              onChange={setAuth}
              options={QR_AUTH_OPTIONS.map((o) => ({ value: o.value, label: o.label }))}
            />
            <Button variant="primary" size="sm" loading={busy} onClick={() => void generateManual()}>
              生成二维码
            </Button>
          </>
        )}
        {props.profilesLoading && <span className="sk-hint">配置读取中…</span>}
      </div>

      {mode === "profile" && !props.profilesLoading && props.profiles.length === 0 && (
        <Alert tone="info" title="没有已保存的 WiFi 配置">
          本机没有保存过 WLAN 配置文件。可切换到「手动输入」直接填 SSID 与密码生成分享二维码。
        </Alert>
      )}

      {qrSvg ? (
        <Card>
          <CardHeader>
            <CardTitle>WiFi 二维码：{qrSsid}</CardTitle>
            <Button size="sm" onClick={() => void saveQrPng()}>
              保存图片
            </Button>
          </CardHeader>
          <CardBody>
            <div className="flex flex-col items-start gap-2">
              {/* 白底是扫码可靠性的硬要求（深色主题下静区也必须是白的） */}
              <div className="w-[296px] bg-white p-2">
                <img src={svgDataUrl(qrSvg)} width={280} height={280} alt="WiFi 二维码" />
              </div>
              <div className="flex items-center gap-1.5">
                <Badge tone="accent">SSID：{qrSsid}</Badge>
                {qrAuth && <Badge>认证：{qrAuth}</Badge>}
              </div>
              <div className="sk-hint">
                标准 WiFi 二维码格式，手机相机扫码即可加入网络。编码串包含明文密码，本页只渲染成图、
                不显示串本身——也别把二维码发给不该有网络权限的人。
              </div>
            </div>
          </CardBody>
        </Card>
      ) : (
        <Empty description={busy ? "生成中…" : "选择配置或手动输入后，生成后显示在此"} />
      )}
    </div>
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
  const toast = useToast();
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
      toast.success(copyResultMessage(r));
    } catch (e) {
      toast.error(String(e));
    }
  }, [cred, toast]);

  const strength = strengthHint(cred?.password ?? "");
  // 行高填满 Content 视口：32+ 条 WiFi 时只滚左列表，标签栏与右栏不动
  const row = useFillRowHeight(true);

  return (
    <div ref={row.ref} className="flex items-stretch gap-3 overflow-hidden" style={{ height: row.h }}>
      {/* 左栏：已存 WiFi 列表（固定 ~280px，刷新嵌在卡标题）；列表在卡体内自滚 */}
      <Card className="flex w-[280px] flex-none flex-col">
        <CardHeader>
          <CardTitle>已存 WiFi（{props.profiles.length}）</CardTitle>
          <Button size="sm" loading={props.profilesLoading} onClick={props.onRefreshProfiles}>
            刷新
          </Button>
        </CardHeader>
        <div className="min-h-0 flex-1 overflow-y-auto px-1 pb-2">
          {props.profilesLoading ? (
            <div className="py-4 text-center text-sm text-sk-sub">读取中…</div>
          ) : props.profiles.length === 0 ? (
            <div className="px-2 py-4 text-center text-[13px] text-sk-sub">
              本机没有已保存的 WiFi 配置（或无线网卡被禁用）
            </div>
          ) : (
            props.profiles.map((name) => (
              <button
                key={name}
                type="button"
                onClick={() => void read(name)}
                className={
                  "block w-full cursor-pointer truncate rounded-md px-2 py-1.5 text-left text-sm font-medium transition-colors " +
                  (name === selected
                    ? "bg-sk-accent/12 text-sk-accent"
                    : "text-sk-text hover:bg-sk-hover")
                }
              >
                {name}
              </button>
            ))
          )}
        </div>
      </Card>

      {/* 右栏：所选网络的密钥详情（flex:1；未选中时给引导文案） */}
      <Card className="flex min-w-0 flex-1 flex-col">
        <CardHeader>
          <CardTitle>{selected ? `${selected} 的密钥` : "WiFi 密钥"}</CardTitle>
        </CardHeader>
        <CardBody className="min-h-0 flex-1 overflow-y-auto">
          {!selected ? (
            <Empty description="点击左侧 WiFi 查看密钥——读取需要管理员权限，点一次读一个" />
          ) : loading ? (
            <div className="py-7 text-center text-sm text-sk-sub">正在读取 {selected} 的密钥…</div>
          ) : err ? (
            <Alert tone="warning" title="读取失败">
              {err}
            </Alert>
          ) : cred ? (
            <div className="flex flex-col gap-2">
              <div className="flex flex-col divide-y divide-sk-border/60">
                <div className="flex items-baseline gap-3 py-1">
                  <span className="w-20 shrink-0 text-sk-sub">认证方式</span>
                  <span className="min-w-0 text-sk-text">{cred.auth || "—"}</span>
                </div>
                <div className="flex items-baseline gap-3 py-1">
                  <span className="w-20 shrink-0 text-sk-sub">密钥</span>
                  <span className="min-w-0 text-sk-text">
                    {cred.has_password ? (
                      <span className="inline-flex items-center gap-2">
                        <code className="rounded bg-sk-hover px-1.5 py-0.5 font-mono text-xs">
                          {maskPassword(cred.password, show)}
                        </code>
                        <Button variant="ghost" size="sm" onClick={() => setShow((s) => !s)}>
                          {show ? "隐藏" : "显示"}
                        </Button>
                      </span>
                    ) : (
                      <span className="text-sk-sub">（无密钥，可能是开放网络）</span>
                    )}
                  </span>
                </div>
                {cred.has_password && (
                  <div className="flex items-baseline gap-3 py-1">
                    <span className="w-20 shrink-0 text-sk-sub">强度</span>
                    <span className="inline-flex min-w-0 items-center gap-1.5">
                      <Badge tone={strength.score >= 3 ? "ok" : strength.score >= 2 ? "warn" : "err"}>
                        {strength.label}
                      </Badge>
                      <span className="sk-hint">{strength.advice}</span>
                    </span>
                  </div>
                )}
              </div>
              {cred.has_password && (
                <div className="flex flex-wrap items-center gap-2">
                  <Button variant="primary" size="sm" onClick={() => void copy()}>
                    复制密钥
                  </Button>
                  <span className="sk-hint">
                    复制会走敏感通道：不进剪贴板历史、不进 Win+V 与云剪贴板，
                    {copyResultMessage({
                      ttl: 0,
                      note: "并按设置里的「敏感内容存活」到期清空",
                    })}
                  </span>
                </div>
              )}
            </div>
          ) : null}
        </CardBody>
      </Card>
    </div>
  );
}
