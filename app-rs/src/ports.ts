import { invoke } from "@tauri-apps/api/core";

/**
 * 端口/进程页与局域网页纯逻辑（无 React/DOM，vitest 覆盖）。
 *
 * 数据来自后端 `sk_ports_list` / `sk_lan_scan`（后端已用 `netstat -ano`、
 * `tasklist`、`arp -a` 解析完毕）。这里只做**展示与筛选**规则。
 */

/** 后端 `sk_ports_list` 的一行。 */
export interface PortRow {
  proto: string;
  local: string;
  remote: string;
  state: string;
  pid: number;
  process: string;
  listening: boolean;
  port: number | null;
}

/** 后端 `sk_lan_scan` 的设备行。 */
export interface LanDevice {
  ip: string;
  mac: string;
  kind: string;
  latency_ms: number | null;
}

export interface LanResult {
  prefix: string;
  scanned: number;
  devices: LanDevice[];
  count: number;
}

/** 状态 → 语义色 token（LISTENING 用 ok，ESTABLISHED 用 accent，其余 sub）。 */
export function stateToken(state: string, listening: boolean): string {
  if (listening) return "ok";
  const s = state.toUpperCase();
  if (s === "ESTABLISHED") return "accent";
  if (s === "TIME_WAIT" || s === "CLOSE_WAIT") return "warn";
  return "sub";
}

/** 状态 → AntD Tag 颜色。 */
export function stateTagColor(state: string, listening: boolean): string {
  if (listening) return "green";
  const s = state.toUpperCase();
  if (s === "ESTABLISHED") return "blue";
  if (s === "TIME_WAIT" || s === "CLOSE_WAIT") return "orange";
  return "default";
}

/**
 * 端口表筛选：关键字（端口/PID/进程名/地址/状态）+ 只看监听。
 *
 * 与后端 `PortRow::matches` 同一判据——两边都实现是有意的：前端筛选是**即时**的
 * （不等后端往返），后端那份用于命令式查询。
 */
export function filterPorts(rows: PortRow[], keyword: string, listenOnly: boolean): PortRow[] {
  const kw = keyword.trim().toLowerCase();
  return rows.filter((r) => {
    if (listenOnly && !r.listening) return false;
    if (!kw) return true;
    const hay = [
      r.port ?? "",
      r.pid,
      r.local,
      r.remote,
      r.state,
      r.process,
      r.proto,
    ]
      .join(" ")
      .toLowerCase();
    return hay.includes(kw);
  });
}

/** 汇总：总数 / 监听数 / 涉及进程数。 */
export function summarizePorts(rows: PortRow[]): {
  total: number;
  listening: number;
  processes: number;
} {
  const pids = new Set<number>();
  let listening = 0;
  for (const r of rows) {
    pids.add(r.pid);
    if (r.listening) listening += 1;
  }
  return { total: rows.length, listening, processes: pids.size };
}

/** 能否结束该进程（与后端 `sk_kill_process` 的拒绝规则一致）。 */
export function canKill(pid: number, selfPid?: number): { ok: boolean; reason: string } {
  if (pid === 0) return { ok: false, reason: "PID 0 是系统空闲进程" };
  if (pid === 4) return { ok: false, reason: "PID 4 是系统进程" };
  if (selfPid !== undefined && pid === selfPid) {
    return { ok: false, reason: "不能结束 SysKit 自身" };
  }
  return { ok: true, reason: "" };
}

/** 结束进程前的确认文案（说明后果，避免误点）。性能监控页复用同一口径。 */
export function killConfirmTextFor(name: string, pid: number): string {
  const who = name || `PID ${pid}`;
  return (
    `确定结束 ${who}（PID ${pid}）吗？\n\n` +
    `该进程未保存的数据会丢失；若它是系统服务，可能导致网络或系统功能异常。`
  );
}

/** 端口表行版：转发到 name+pid 版（两处文案必须一致，抽出来才好测）。 */
export function killConfirmText(row: Pick<PortRow, "process" | "pid">): string {
  return killConfirmTextFor(row.process, row.pid);
}

/** MAC 显示：`aa-bb-cc-dd-ee-ff` → 大写（与多数路由器界面一致）。 */
export function formatMac(mac: string): string {
  return mac.toUpperCase();
}

/** 延迟显示：无回应显示「—」而不是 0。 */
export function formatLatency(ms: number | null): string {
  if (ms === null || ms === undefined) return "—";
  return `${ms} ms`;
}

/** 设备行排序：按 IP 最后一段数值升序（与后端一致）。 */
export function sortDevices(devices: LanDevice[]): LanDevice[] {
  const last = (ip: string) => Number(ip.split(".").pop() ?? 0) || 0;
  return [...devices].sort((a, b) => last(a.ip) - last(b.ip));
}

/** 扫描结果摘要文案。 */
export function lanSummary(result: LanResult | null): string {
  if (!result) return "尚未扫描";
  return `网段 ${result.prefix}.0/24 · 扫描 ${result.scanned} 个地址 · 发现 ${result.count} 台设备`;
}

// ---------------- Wake-on-LAN（v1.4） ----------------

/** 前端侧 MAC 形态预检（与后端 wol::parse_mac 同口径：12 位十六进制）。 */
export function isValidMac(s: string): boolean {
  return /^[0-9A-Fa-f]{2}([-:][0-9A-Fa-f]{2}){5}$|^[0-9A-Fa-f]{12}$/.test(s.trim());
}

/** 唤醒设备：后端发魔术包（全局广播 + 可选定向广播），返回结果文案。 */
export function sendWol(mac: string, directBroadcast?: string): Promise<string> {
  return invoke<string>("sk_wol_send", {
    mac,
    port: null,
    directBroadcast: directBroadcast || null,
  });
}
