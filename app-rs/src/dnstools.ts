/**
 * DNS 查询 / 子网计算 / ping / 路由 页纯逻辑（无 React/DOM，vitest 覆盖）。
 *
 * 对应 Python 侧 DnsSubnetTab + PingRouteTab 的展示规则。
 * 计算真正在 Rust 侧（`sk_subnet_calc`），这里只做展示格式化与结果行渲染。
 */

/** 后端 `sk_dns_query` 的返回。 */
export interface DnsAnswer {
  ok: boolean;
  domain: string;
  server: string;
  ips: string[];
  ms: number;
  message: string;
}

/** 后端 `sk_subnet_calc` 的 info 字段。 */
export interface SubnetInfo {
  prefix: number;
  mask: string;
  wildcard: string;
  network: string;
  broadcast: string;
  first_host: string;
  last_host: string;
  usable_hosts: number;
  total_addresses: number;
  is_private: boolean;
}

export interface SubnetCalcResult {
  info: SubnetInfo;
  lines: [string, string][];
  presets: { label: string; server: string }[];
}

/** 后端 `sk_ping` 的返回。 */
export interface PingResult {
  ok: boolean;
  host: string;
  loss_percent: number | null;
  latency_ms: number | null;
  raw: string;
}

/** 后端 `sk_tracert` 的返回。 */
export interface TraceHop {
  hop: number;
  latencies: (number | null)[];
  host: string;
}

export interface TraceResult {
  ok: boolean;
  host: string;
  hops: TraceHop[];
  count: number;
  raw: string;
}

// ---------------- 子网 ----------------

/** 前缀长度的合法范围（与后端一致）。 */
export const PREFIX_MIN = 0;
export const PREFIX_MAX = 32;

export function isValidPrefix(prefix: number): boolean {
  return Number.isInteger(prefix) && prefix >= PREFIX_MIN && prefix <= PREFIX_MAX;
}

/** 子网结果 → 键值行（后端已给 lines，这里保留兜底以免字段缺失时空白）。 */
export function subnetRows(result: SubnetCalcResult | null): [string, string][] {
  if (!result) return [];
  if (result.lines?.length) return result.lines;
  const i = result.info;
  return [
    ["网络地址", i.network],
    ["子网掩码", i.mask],
    ["反掩码", i.wildcard],
    ["广播地址", i.broadcast],
    ["可用主机范围", `${i.first_host} ~ ${i.last_host}`],
    ["可用主机数", String(i.usable_hosts)],
    ["地址总数", String(i.total_addresses)],
    ["地址类型", i.is_private ? "私有地址" : "公网地址"],
  ];
}

/** 大整数千分位（/0 的地址总数 4294967296 需要可读）。 */
export function formatCount(n: number): string {
  return n.toLocaleString("en-US");
}

// ---------------- DNS ----------------

/** 查询失败时的展示级别：超时/无应答用 warning，其余 error。 */
export function dnsLevel(a: DnsAnswer): "success" | "warning" | "error" {
  if (a.ok) return "success";
  if (a.message.includes("超时") || a.message.includes("无应答")) return "warning";
  return "error";
}

/** DNS 结果摘要文案。 */
export function dnsSummary(a: DnsAnswer | null): string {
  if (!a) return "尚未查询";
  return a.ok
    ? `${a.domain} → ${a.ips.join(", ")}（${a.server}，${a.ms} ms）`
    : `${a.domain} 解析失败（${a.server}）：${a.message}`;
}

// ---------------- ping / 路由 ----------------

/** 丢包率 → 语义色 token。 */
export function lossToken(loss: number | null): string {
  if (loss === null) return "sub";
  if (loss === 0) return "ok";
  if (loss < 100) return "warn";
  return "err";
}

/** ping 摘要。 */
export function pingSummary(r: PingResult | null): string {
  if (!r) return "尚未 ping";
  const loss = r.loss_percent === null ? "?" : String(r.loss_percent);
  const lat = r.latency_ms === null ? "?" : String(r.latency_ms);
  return `${r.host}：丢包 ${loss}% · 延迟 ${lat} ms`;
}

/** 跳点延迟展示（全超时显示 `*`）。 */
export function formatHopLatencies(latencies: (number | null)[]): string {
  return latencies.map((l) => (l === null ? "*" : `${l} ms`)).join("  ");
}

/** 跳点主机展示（全超时显示「请求超时」）。 */
export function formatHopHost(hop: TraceHop): string {
  return hop.host && hop.host.length > 0 ? hop.host : "请求超时";
}

/** 该跳是否全超时（前端标记用）。 */
export function hopTimedOut(hop: TraceHop): boolean {
  return hop.latencies.every((l) => l === null);
}

/** 路由结果摘要。 */
export function traceSummary(r: TraceResult | null): string {
  if (!r) return "尚未跟踪";
  return `${r.host}：共 ${r.count} 跳`;
}
