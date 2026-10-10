/**
 * 工具箱：端口/进程 与 局域网设备（对应 Python 侧 PortTab / LanTab）。
 *
 * 两者放在一个页面里用 Tab 切换，与 Python 工具箱的分页结构对应。
 * 数据来自后端解析好的 `netstat -ano` / `tasklist` / `arp -a`。
 */
import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Button,
  Card,
  Empty,
  Input,
  Modal,
  Space,
  Switch,
  Table,
  Tabs,
  Tag,
  Typography,
  message,
} from "antd";

import {
  canKill,
  filterPorts,
  formatLatency,
  formatMac,
  killConfirmText,
  lanSummary,
  sortDevices,
  stateTagColor,
  summarizePorts,
  type LanDevice,
  type LanResult,
  type PortRow,
  isValidMac,
  sendWol,
} from "./ports";
import {
  Button as UiButton,
  Input as UiInput,
  Empty as UiEmpty,
  Table as UiTable,
} from "./ui";
import { Alert as UiAlert } from "./ui/alert";
import { useToast } from "./ui/toast";

function PortPanel() {
  const [rows, setRows] = useState<PortRow[]>([]);
  const [keyword, setKeyword] = useState("");
  const [listenOnly, setListenOnly] = useState(false);
  const [auto, setAuto] = useState(false);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    setBusy(true);
    try {
      const r = await invoke<{ rows: PortRow[]; total: number }>("sk_ports_list");
      setRows(r.rows);
    } catch (e) {
      message.error(`读取端口列表失败：${String(e)}`);
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  // 每 5 秒自动刷新（与 Python 的 chk_auto 同语义）
  useEffect(() => {
    if (!auto) return;
    const t = window.setInterval(() => void load(), 5000);
    return () => window.clearInterval(t);
  }, [auto, load]);

  const shown = useMemo(() => filterPorts(rows, keyword, listenOnly), [rows, keyword, listenOnly]);
  const stats = useMemo(() => summarizePorts(rows), [rows]);

  const kill = useCallback(
    (row: PortRow) => {
      const check = canKill(row.pid);
      if (!check.ok) {
        message.warning(check.reason);
        return;
      }
      Modal.confirm({
        title: "结束进程",
        content: (
          <Typography.Paragraph style={{ whiteSpace: "pre-wrap" }}>
            {killConfirmText(row)}
          </Typography.Paragraph>
        ),
        okText: "结束进程",
        okButtonProps: { danger: true },
        cancelText: "取消",
        onOk: async () => {
          try {
            message.success(await invoke<string>("sk_kill_process", { pid: row.pid }));
            await load();
          } catch (e) {
            message.error(String(e));
          }
        },
      });
    },
    [load],
  );

  return (
    <Space direction="vertical" size={10} style={{ width: "100%" }}>
      <Space wrap>
        <Button size="small" type="primary" loading={busy} onClick={() => void load()}>
          刷新
        </Button>
        <span className="sk-hint">每 5 秒自动刷新</span>
        <Switch size="small" checked={auto} onChange={setAuto} />
        <span className="sk-hint">只看监听端口</span>
        <Switch size="small" checked={listenOnly} onChange={setListenOnly} />
        <Input
          size="small"
          style={{ width: 260 }}
          allowClear
          placeholder="筛选：端口 / PID / 进程名 / IP / 状态…"
          value={keyword}
          onChange={(e) => setKeyword(e.target.value)}
        />
        <Tag>共 {stats.total} 条</Tag>
        <Tag color="green">监听 {stats.listening}</Tag>
        <Tag color="blue">进程 {stats.processes}</Tag>
        {keyword && <Tag color="orange">筛出 {shown.length} 条</Tag>}
      </Space>

      <Table<PortRow>
        size="small"
        rowKey={(r) => `${r.proto}-${r.local}-${r.remote}-${r.pid}`}
        pagination={{ pageSize: 20, showSizeChanger: false }}
        dataSource={shown}
        loading={busy}
        locale={{ emptyText: <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="无匹配连接" /> }}
        columns={[
          { title: "协议", dataIndex: "proto", width: 70 },
          { title: "本地地址", dataIndex: "local", width: 210 },
          { title: "远程地址", dataIndex: "remote", width: 210 },
          {
            title: "状态",
            dataIndex: "state",
            width: 120,
            render: (v: string, r) => (
              <Tag color={stateTagColor(v, r.listening)}>{v || "监听"}</Tag>
            ),
          },
          { title: "PID", dataIndex: "pid", width: 80 },
          { title: "进程", dataIndex: "process", ellipsis: true },
          {
            title: "操作",
            width: 100,
            render: (_: unknown, r) => {
              const c = canKill(r.pid);
              return c.ok ? (
                <Button size="small" type="link" danger onClick={() => kill(r)}>
                  结束进程
                </Button>
              ) : (
                <span className="sk-hint" title={c.reason}>
                  受保护
                </span>
              );
            },
          },
        ]}
      />
    </Space>
  );
}

function LanPanel() {
  const toast = useToast();
  const [result, setResult] = useState<LanResult | null>(null);
  const [scanning, setScanning] = useState(false);
  // Wake-on-LAN：手动输 MAC 唤醒 + 行级快捷唤醒（扫描结果里的设备）
  const [wakeMac, setWakeMac] = useState("");
  const [waking, setWaking] = useState(""); // 正在唤醒的 MAC（防重复点击）
  // 扫描已用时（秒）。后端一次 invoke 拿全量结果、没有进度事件，百分比只能是编的；
  // 真实秒数同样能回答「卡住了吗」——预期 8~15 秒，超过就该怀疑网络而不是界面
  const [elapsed, setElapsed] = useState(0);

  useEffect(() => {
    if (!scanning) return;
    setElapsed(0);
    const t = window.setInterval(() => setElapsed((s) => s + 1), 1000);
    return () => window.clearInterval(t);
  }, [scanning]);

  const scan = useCallback(async () => {
    setScanning(true);
    try {
      const r = await invoke<LanResult>("sk_lan_scan", { prefix: null });
      setResult(r);
      toast.success(lanSummary(r));
    } catch (e) {
      toast.error(`扫描失败：${String(e)}`);
    } finally {
      setScanning(false);
    }
  }, []);

  const wake = useCallback(
    async (mac: string) => {
      if (!isValidMac(mac)) {
        toast.error("MAC 地址格式不正确（示例 AA-BB-CC-DD-EE-FF）");
        return;
      }
      setWaking(mac);
      try {
        // 定向广播：用扫描结果的网段前缀拼（192.168.1.255）
        toast.success(await sendWol(mac, result ? `${result.prefix}.255` : undefined));
      } catch (e) {
        toast.error(`唤醒失败：${String(e)}`);
      } finally {
        setWaking("");
      }
    },
    [result, toast],
  );

  const devices = useMemo(() => (result ? sortDevices(result.devices) : []), [result]);

  return (
    <div className="flex flex-col gap-2.5">
      <UiAlert tone="info" title="扫描本机所在 /24 网段（并发 ping 唤醒邻居后读 ARP 表），约需 8~15 秒" />
      <div className="flex flex-wrap items-center gap-2">
        <UiButton variant="primary" loading={scanning} onClick={() => void scan()}>
          扫描局域网
        </UiButton>
        <UiInput
          className="w-[210px] font-mono"
          placeholder="MAC 手动唤醒，如 AA-BB-CC-DD-EE-FF"
          value={wakeMac}
          onChange={(e) => setWakeMac(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && wakeMac.trim()) void wake(wakeMac.trim());
          }}
        />
        <UiButton loading={waking === wakeMac.trim()} disabled={!wakeMac.trim()} onClick={() => void wake(wakeMac.trim())}>
          唤醒
        </UiButton>
        <span className="sk-hint">
          {scanning ? `扫描中… 已用时 ${elapsed} 秒（并发 ping 254 个地址 + 读 ARP，约 8~15 秒）` : lanSummary(result)}
        </span>
      </div>

      {devices.length === 0 ? (
        <UiEmpty description={result ? "未发现设备（可能网段推断有误或邻居表为空）" : "点「扫描局域网」开始"} />
      ) : (
        <UiTable<LanDevice>
          rowKey={(r) => r.ip}
          loading={scanning}
          dataSource={devices}
          columns={[
            { title: "IP 地址", dataIndex: "ip", width: 160 },
            {
              title: "MAC 地址",
              dataIndex: "mac",
              width: 200,
              render: (v: string) => <span className="font-mono">{formatMac(v)}</span>,
            },
            {
              title: "延迟",
              dataIndex: "latency_ms",
              width: 100,
              render: (v: number | null) => formatLatency(v),
            },
            { title: "类型", dataIndex: "kind", width: 100 },
            {
              title: "操作",
              key: "wol",
              width: 90,
              render: (_: unknown, r: LanDevice) => (
                <UiButton size="sm" loading={waking === r.mac} onClick={() => void wake(r.mac)}>
                  唤醒
                </UiButton>
              ),
            },
          ]}
          empty={
            <UiEmpty
              description={result ? "未发现设备（可能网段推断有误或邻居表为空）" : "点「扫描局域网」开始"}
            />
          }
        />
      )}
    </div>
  );
}

export function ToolboxPorts() {
  return (
    <Card size="small" className="sk-card">
      <Tabs
        items={[
          { key: "ports", label: "端口 / 进程", children: <PortPanel /> },
          { key: "lan", label: "局域网设备", children: <LanPanel /> },
        ]}
      />
    </Card>
  );
}
