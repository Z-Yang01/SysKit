//! 系统网络快照：网卡 / 路由 / 持久路由 / 服务 / WLAN 文本。
//!
//! 与 Python 侧 snapshot.py 的关系：**数据来源相同**（PowerShell `Get-Net*` +
//! `route print` + 注册表 + `netsh`），但**解析全部用 Rust 原生实现**——
//! Python 版也是「起 PowerShell 拿 JSON → 自己解析」，Rust 侧照搬这条路可以
//! 得到同样的信息，同时把解析逻辑变成可单测的纯函数。
//!
//! 为什么不在本层直接改用 GetAdaptersAddresses 之类的原生枚举：信息等价，
//! 但 `WindowsAndMessaging`/`IpHelper` 的裸指针链表解析风险高、收益只是省一次
//! PowerShell。**先把检查逻辑接通、把 5 项 skip 变成真结果**，原生枚举留作
//! 后续优化（`physical_up_adapters` 已是唯一入口，替换点集中）。
//!
//! 采集失败不 panic：`Snapshot.ok=false` + `error` 说明原因，检查项据此返回 skip。

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::w32;

/// 网卡信息（对应 Python `Adapter`）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Adapter {
    pub name: String,
    pub desc: String,
    /// `Up` / `Disconnected` / ...
    pub status: String,
    pub if_index: u32,
    pub mac: String,
    pub virtual_: bool,
    pub ipv4: Vec<String>,
    pub gw4: Vec<String>,
    pub dns: Vec<String>,
}

/// 一条路由（活动默认路由或持久路由）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RouteEntry {
    pub dest: String,
    pub next_hop: String,
    pub metric: u32,
    pub if_index: u32,
    pub if_metric: u32,
}

/// 快照（对应 Python `Snapshot`）。
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub adapters: Vec<Adapter>,
    pub default_routes: Vec<RouteEntry>,
    pub persistent_routes: Vec<RouteEntry>,
    pub if_metrics: BTreeMap<u32, u32>,
    pub wlan_text: String,
    pub ok: bool,
    pub error: String,
}

impl Snapshot {
    /// 状态 Up 且持有 IPv4 的非虚拟网卡（网关/DNS 对比的判定对象）。
    ///
    /// 排除虚拟网卡的原因与 Python 注释一致：Clash TUN（Up、198.18.0.1、fake-ip DNS）
    /// 参与网关连通与 DNS 对比会让两项核心判定的对象变成隧道假象。
    pub fn physical_up_adapters(&self) -> Vec<&Adapter> {
        self.adapters
            .iter()
            .filter(|a| a.status.eq_ignore_ascii_case("up") && !a.ipv4.is_empty() && !a.virtual_)
            .collect()
    }

    pub fn adapter_by_index(&self, idx: u32) -> Option<&Adapter> {
        self.adapters.iter().find(|a| a.if_index == idx)
    }
}

// ---------------- PowerShell 采集 ----------------

/// 采集脚本：一次调用取回全部事实（与 Python 版同源命令）。
const COLLECT_SCRIPT: &str = r#"
$ErrorActionPreference = 'SilentlyContinue'
$out = [ordered]@{}
$out.adapters = Get-NetAdapter | ForEach-Object {
  [ordered]@{ name=$_.Name; desc=$_.InterfaceDescription; status=[string]$_.Status;
    ifIndex=$_.ifIndex; mac=[string]$_.MacAddress; virtual=$_.Virtual } }
$out.ip4 = Get-NetIPAddress -AddressFamily IPv4 | ForEach-Object {
  [ordered]@{ ifIndex=[int]$_.InterfaceIndex; ip=[string]$_.IPAddress } }
$out.routes = Get-NetRoute -AddressFamily IPv4 | ForEach-Object {
  [ordered]@{ ifIndex=[int]$_.InterfaceIndex; dest=[string]$_.DestinationPrefix;
    nextHop=[string]$_.NextHop; metric=[int]$_.RouteMetric } }
$out.ifMetrics = Get-NetIPInterface -AddressFamily IPv4 | ForEach-Object {
  [ordered]@{ ifIndex=[int]$_.InterfaceIndex; name=[string]$_.InterfaceAlias;
    metric=[int]$_.InterfaceMetric } }
$out.dns = Get-DnsClientServerAddress -AddressFamily IPv4 | ForEach-Object {
  [ordered]@{ ifIndex=[int]$_.InterfaceIndex; servers=@($_.ServerAddresses) } }
$out.gw = Get-NetIPConfiguration | ForEach-Object {
  [ordered]@{ ifIndex=[int]$_.InterfaceIndex; gw=@($_.IPv4DefaultGateway.NextHop) } }
ConvertTo-Json -InputObject $out -Depth 5 -Compress
"#;

/// PowerShell 对"空值"输出显式 `null`（如断开网卡的 MAC/描述）——serde 的
/// `String` 不容 null，整份快照解析崩掉（真机实锤 2026-10-04：断开的
/// 「以太网 2」让 adapters 表全空，局域网扫描跟着报"无网卡"）。所有采集
/// String 字段统一走这里：null / 缺键 → 空串。
fn null_to_empty<'de, D>(d: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt: Option<String> = serde::Deserialize::deserialize(d)?;
    Ok(opt.unwrap_or_default())
}

/// `Vec<String>` 同款容错：外层数组在（PowerShell 总输出 `@()`），但元素是
/// null——`gw=[null]`（无默认网关的网卡，真机实证 2026-10-04，列 5431）。
/// serde 的 `Vec<String>` 碰 null 元素整份快照崩。null 元素 → 剔除。
fn null_elements_to_empty<'de, D>(d: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: Vec<Option<String>> = Vec::deserialize(d)?;
    Ok(raw.into_iter().flatten().collect())
}

/// PowerShell JSON 形状（字段名与采集脚本一一对应）。
///
/// `rename_all = "camelCase"` 是必需的：采集脚本给的是 `ifIndex` / `nextHop` /
/// `ifMetrics` 这类驼峰键，而 Rust 侧用 snake_case。漏掉这个属性时 serde 会**静默**
/// 把不认识的键丢进 default——接口跃点表恒为空且不报错（实际踩到过）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawRoot {
    #[serde(default)]
    adapters: Vec<RawAdapter>,
    #[serde(default)]
    ip4: Vec<RawIp>,
    #[serde(default)]
    routes: Vec<RawRoute>,
    /// 注意键名：采集脚本用的是 `ifMetrics`（驼峰），而 serde 的 camelCase 会把
    /// 字段 `if_metrics` 映射成 `ifMetrics`——两者的位置完全一致，但**不能**写成
    /// `ifmetrics`（第一版如此，导致接口跃点表恒为空且不报错）。
    #[serde(default)]
    if_metrics: Vec<RawMetric>,
    #[serde(default)]
    dns: Vec<RawDns>,
    #[serde(default)]
    gw: Vec<RawGw>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawAdapter {
    #[serde(default, deserialize_with = "null_to_empty")]
    name: String,
    #[serde(default, deserialize_with = "null_to_empty")]
    desc: String,
    #[serde(default, deserialize_with = "null_to_empty")]
    status: String,
    #[serde(default)]
    if_index: u32,
    #[serde(default, deserialize_with = "null_to_empty")]
    mac: String,
    #[serde(default)]
    r#virtual: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawIp {
    #[serde(default)]
    if_index: u32,
    #[serde(default, deserialize_with = "null_to_empty")]
    ip: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawRoute {
    #[serde(default)]
    if_index: u32,
    #[serde(default, deserialize_with = "null_to_empty")]
    dest: String,
    #[serde(default, deserialize_with = "null_to_empty")]
    next_hop: String,
    #[serde(default)]
    metric: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawMetric {
    #[serde(default)]
    if_index: u32,
    #[serde(default)]
    metric: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawDns {
    #[serde(default)]
    if_index: u32,
    #[serde(default, deserialize_with = "null_elements_to_empty")]
    servers: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawGw {
    #[serde(default)]
    if_index: u32,
    #[serde(default, deserialize_with = "null_elements_to_empty")]
    gw: Vec<String>,
}

/// 采集快照。
pub fn take() -> Snapshot {
    let mut snap = Snapshot::default();

    let ps = w32::run_capture(
        &[
            "powershell".into(),
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
            COLLECT_SCRIPT.into(),
        ],
        60,
    );

    match ps {
        Ok(o) if o.ok => match parse_collect(&o.stdout) {
            Ok(s) => {
                snap = s;
                snap.ok = true;
            }
            Err(e) => snap.error = format!("采集输出解析失败: {e}"),
        },
        Ok(o) => {
            snap.error = format!(
                "PowerShell 采集失败(exit={}): {}",
                o.code,
                o.tail(0, 200).trim()
            );
        }
        Err(e) => snap.error = format!("无法运行 PowerShell: {e}"),
    }

    // 持久路由与 WLAN 文本走独立轻量命令（失败不影响主快照）
    snap.persistent_routes = persistent_routes();
    snap.wlan_text = wlan_interfaces_text();
    snap
}

/// 解析采集 JSON（纯函数，可单测）。
pub fn parse_collect(json_text: &str) -> Result<Snapshot, String> {
    let raw: RawRoot =
        serde_json::from_str(json_text).map_err(|e| format!("JSON 解析失败: {e}"))?;
    let mut snap = Snapshot::default();
    for a in raw.adapters {
        snap.adapters.push(Adapter {
            name: a.name,
            desc: a.desc,
            status: a.status,
            if_index: a.if_index,
            mac: a.mac,
            virtual_: a.r#virtual,
            ipv4: Vec::new(),
            gw4: Vec::new(),
            dns: Vec::new(),
        });
    }
    // 把 IP / 网关 / DNS 按 ifIndex 归并到网卡
    for ip in raw.ip4 {
        if let Some(a) = snap.adapters.iter_mut().find(|a| a.if_index == ip.if_index) {
            if !ip.ip.is_empty() {
                a.ipv4.push(ip.ip);
            }
        }
    }
    for gw in raw.gw {
        if let Some(a) = snap.adapters.iter_mut().find(|a| a.if_index == gw.if_index) {
            a.gw4 = gw.gw.into_iter().filter(|g| !g.is_empty()).collect();
        }
    }
    for d in raw.dns {
        if let Some(a) = snap.adapters.iter_mut().find(|a| a.if_index == d.if_index) {
            a.dns = d.servers.into_iter().filter(|s| !s.is_empty()).collect();
        }
    }
    // 接口跃点表**必须先填**：默认路由要用它回填 if_metric（第一版把这段放在
    // routes 之后，导致 if_metric 恒为 0 —— 测试逮到）
    for m in raw.if_metrics {
        snap.if_metrics.insert(m.if_index, m.metric);
    }
    for r in raw.routes {
        // 只保留默认路由（0.0.0.0/0）：check 的判定对象就是它
        if r.dest == "0.0.0.0/0" {
            snap.default_routes.push(RouteEntry {
                dest: r.dest,
                next_hop: r.next_hop,
                metric: r.metric,
                if_index: r.if_index,
                if_metric: snap.if_metrics.get(&r.if_index).copied().unwrap_or(0),
            });
        }
    }
    Ok(snap)
}

/// 持久路由：从注册表 `Tcpip\Parameters\PersistentRoutes` 读取。
///
/// 该键的值形如 `"0.0.0.0,0.0.0.0,172.16.2.1,1"`（目标,掩码,网关,跃点）。
/// 走注册表而不是 `route print`：这是持久路由的**权威来源**，不需要解析
/// 本地化输出，也不会被「活动路由表」污染。
pub fn persistent_routes() -> Vec<RouteEntry> {
    let key = r"HKLM\SYSTEM\CurrentControlSet\Services\Tcpip\Parameters\PersistentRoutes";
    let vals = w32::reg_values(key);
    let mut out = Vec::new();
    for (name, value) in vals {
        // 名称通常是 "0.0.0.0,0.0.0.0,172.16.2.1,1"；数据有时为空
        let spec = if !name.contains(',') {
            value.as_compare_str()
        } else {
            name.clone()
        };
        if let Some(r) = parse_persistent_route(&spec) {
            out.push(r);
        }
    }
    out
}

/// 解析一条持久路由规格（纯函数）。
pub fn parse_persistent_route(spec: &str) -> Option<RouteEntry> {
    let cols: Vec<&str> = spec.split(',').map(|s| s.trim()).collect();
    if cols.len() < 4 {
        return None;
    }
    let dest_ip = cols[0];
    let mask = cols[1];
    let gw = cols[2];
    let metric = cols[3].parse::<u32>().unwrap_or(0);
    if dest_ip.is_empty() || mask.is_empty() {
        return None;
    }
    Some(RouteEntry {
        dest: to_cidr(dest_ip, mask),
        next_hop: gw.to_string(),
        metric,
        if_index: 0,
        if_metric: 0,
    })
}

/// IPv4 地址 + 掩码 → CIDR（如 `0.0.0.0` + `0.0.0.0` → `0.0.0.0/0`）。
///
/// 掩码按位计数得到前缀长度；掩码非法时退化为 `/32`（Python 侧由
/// `Get-NetRoute` 直接给 DestinationPrefix，不需要这一步）。
pub fn to_cidr(ip: &str, mask: &str) -> String {
    let Some(m) = parse_ipv4(mask) else {
        return format!("{ip}/32");
    };
    let prefix = m.count_ones();
    format!("{ip}/{prefix}")
}

fn parse_ipv4(s: &str) -> Option<u32> {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let mut out = 0u32;
    for p in parts {
        let n: u32 = p.parse().ok()?;
        if n > 255 {
            return None;
        }
        out = (out << 8) | n;
    }
    Some(out)
}

/// `netsh wlan show interfaces` 原文（无 WiFi 时为空串）。
pub fn wlan_interfaces_text() -> String {
    w32::run_capture(
        &[
            "netsh".into(),
            "wlan".into(),
            "show".into(),
            "interfaces".into(),
        ],
        10,
    )
    .map(|o| o.stdout)
    .unwrap_or_default()
}

/// `netsh wlan show settings` 原文（MAC 随机化判定用）。
pub fn wlan_settings_text() -> String {
    w32::run_capture(
        &[
            "netsh".into(),
            "wlan".into(),
            "show".into(),
            "settings".into(),
        ],
        10,
    )
    .map(|o| o.stdout)
    .unwrap_or_default()
}

/// 从 `netsh wlan show interfaces` 里取 SSID（对应 Python `current_ssid`）。
///
/// 只认行首锚定的 `SSID`，不认 `BSSID`（后者以 B 开头，行首锚定天然排除）。
pub fn current_ssid(wlan_text: &str) -> String {
    for line in wlan_text.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("SSID") {
            // 跳过 BSSID（strip_prefix("SSID") 不会命中 BSSID，因为行首是 B）
            let Some((_, v)) = rest.split_once(':') else {
                continue;
            };
            let v = v.trim().trim_matches('"');
            if !v.is_empty() {
                return v.to_string();
            }
        }
    }
    String::new()
}

/// WLAN 是否已断开（中英输出）。
pub fn wlan_disconnected(wlan_text: &str) -> bool {
    let lower = wlan_text.to_lowercase();
    wlan_text.contains("已断开") || lower.contains("disconnected")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_cidr_counts_mask_bits() {
        assert_eq!(to_cidr("0.0.0.0", "0.0.0.0"), "0.0.0.0/0");
        assert_eq!(to_cidr("192.168.1.0", "255.255.255.0"), "192.168.1.0/24");
        assert_eq!(to_cidr("10.0.0.0", "255.0.0.0"), "10.0.0.0/8");
        assert_eq!(to_cidr("1.2.3.4", "255.255.255.255"), "1.2.3.4/32");
        assert_eq!(to_cidr("1.2.3.4", "255.255.255.252"), "1.2.3.4/30");
        // 非法掩码 → 退化为 /32 而不是崩
        assert_eq!(to_cidr("1.2.3.4", "garbage"), "1.2.3.4/32");
        assert_eq!(to_cidr("1.2.3.4", "999.1.1.1"), "1.2.3.4/32");
    }

    #[test]
    fn parse_persistent_route_reads_registry_shape() {
        let r = parse_persistent_route("0.0.0.0,0.0.0.0,172.16.2.1,1").unwrap();
        assert_eq!(
            r.dest, "0.0.0.0/0",
            "默认持久路由必须识别为 /0（劫持流量的元凶）"
        );
        assert_eq!(r.next_hop, "172.16.2.1");
        assert_eq!(r.metric, 1);
        let r2 = parse_persistent_route("10.0.0.0,255.0.0.0,192.168.1.1,5").unwrap();
        assert_eq!(r2.dest, "10.0.0.0/8");
        assert_eq!(r2.metric, 5);
    }

    #[test]
    fn parse_persistent_route_rejects_malformed() {
        assert!(parse_persistent_route("").is_none());
        assert!(
            parse_persistent_route("0.0.0.0,0.0.0.0").is_none(),
            "少字段"
        );
        assert!(parse_persistent_route(",,,").is_none(), "空字段");
        // 跃点非法时按 0 处理而不是丢弃整条（宁可显示也不要漏掉劫持路由）
        let r = parse_persistent_route("0.0.0.0,0.0.0.0,1.2.3.4,abc").unwrap();
        assert_eq!(r.metric, 0);
    }

    #[test]
    fn parse_collect_tolerates_nulls_from_powershell() {
        // 回归（真机 2026-10-04）：PowerShell 对空值输出显式 null——
        // 断开网卡的 mac=null、无网关网卡 gw=[null]。serde 的 String/Vec<String>
        // 碰 null 整份快照崩（"采集输出解析失败: invalid type: null, expected
        // a string at column 5431"），快照空表 → 局域网扫描报"无网卡"、诊断
        // 全表盲。null 必须≈空串/剔除。
        let js = r#"{"adapters":[
            {"name":"WLAN","desc":"Intel Wi-Fi","status":"Up","ifIndex":12,"mac":null,"virtual":false},
            {"name":"以太网 2","desc":"Realtek","status":"Disconnected","ifIndex":4,"mac":null,"virtual":false}],
            "ip4":[{"ifIndex":12,"ip":"192.168.18.238"},{"ifIndex":4,"ip":"169.254.56.18"}],
            "routes":[],
            "ifMetrics":[],
            "dns":[],
            "gw":[{"ifIndex":12,"gw":["192.168.18.1"]},{"ifIndex":4,"gw":[null]}]}"#;
        let snap = parse_collect(js).expect("null 容错后必须可解析");
        assert_eq!(snap.adapters.len(), 2);
        let wlan = snap.adapter_by_index(12).unwrap();
        assert_eq!(wlan.ipv4, vec!["192.168.18.238"]);
        assert_eq!(wlan.gw4, vec!["192.168.18.1"]);
        let eth2 = snap.adapter_by_index(4).unwrap();
        assert!(eth2.mac.is_empty(), "null mac → 空串");
        assert!(eth2.gw4.is_empty(), "gw=[null] → 空数组");
        assert!(
            !snap.physical_up_adapters().is_empty(),
            "WLAN 必须被判为物理在用网卡（回归：快照崩掉后本断言恒空）"
        );
    }

    #[test]
    fn parse_collect_builds_adapters_and_joins_by_ifindex() {
        let json = r#"{
            "adapters": [
                {"name":"以太网","desc":"Realtek","status":"Up","ifIndex":13,"mac":"AA","virtual":false},
                {"name":"vgate0","desc":"Wintun","status":"Up","ifIndex":42,"mac":"","virtual":true}
            ],
            "ip4": [
                {"ifIndex":13,"ip":"192.168.1.100"},
                {"ifIndex":42,"ip":"198.18.0.1"}
            ],
            "routes": [
                {"ifIndex":13,"dest":"0.0.0.0/0","nextHop":"192.168.1.1","metric":25},
                {"ifIndex":13,"dest":"192.168.1.0/24","nextHop":"0.0.0.0","metric":1}
            ],
            "ifMetrics": [
                {"ifIndex":13,"name":"以太网","metric":25},
                {"ifIndex":42,"name":"vgate0","metric":5}
            ],
            "dns": [
                {"ifIndex":13,"servers":["223.5.5.5","114.114.114.114"]}
            ],
            "gw": [
                {"ifIndex":13,"gw":["192.168.1.1"]}
            ]
        }"#;
        let s = parse_collect(json).unwrap();
        assert_eq!(s.adapters.len(), 2);
        let eth = s.adapter_by_index(13).unwrap();
        assert_eq!(eth.name, "以太网");
        assert_eq!(eth.ipv4, vec!["192.168.1.100"]);
        assert_eq!(eth.gw4, vec!["192.168.1.1"]);
        assert_eq!(eth.dns.len(), 2);
        assert!(!eth.virtual_);
        // 只保留默认路由
        assert_eq!(s.default_routes.len(), 1);
        assert_eq!(s.default_routes[0].next_hop, "192.168.1.1");
        // if_metric 从 ifMetrics 回填
        assert_eq!(s.default_routes[0].if_metric, 25);
        assert_eq!(s.if_metrics.get(&42), Some(&5));
    }

    #[test]
    fn parse_collect_handles_missing_fields() {
        // 空对象：全部字段缺失也必须能解析（PowerShell 在无网卡机器上会这样输出）
        let s = parse_collect("{}").unwrap();
        assert!(s.adapters.is_empty());
        assert!(s.default_routes.is_empty());
        assert!(s.physical_up_adapters().is_empty());
    }

    #[test]
    fn parse_collect_rejects_garbage() {
        assert!(parse_collect("not json").is_err());
        assert!(parse_collect("").is_err());
    }

    #[test]
    fn parse_collect_ignores_unknown_extra_fields() {
        let json = r#"{"adapters":[],"newFieldFromFuture":123}"#;
        assert!(parse_collect(json).is_ok(), "未知字段不得导致解析失败");
    }

    #[test]
    fn physical_up_adapters_excludes_virtual_and_down() {
        let s = Snapshot {
            adapters: vec![
                Adapter {
                    name: "以太网".into(),
                    status: "Up".into(),
                    ipv4: vec!["192.168.1.2".into()],
                    virtual_: false,
                    if_index: 1,
                    ..Default::default()
                },
                Adapter {
                    // 虚拟但 Up 且有 IP（Clash TUN）→ 必须排除
                    name: "vgate0".into(),
                    status: "Up".into(),
                    ipv4: vec!["198.18.0.1".into()],
                    virtual_: true,
                    if_index: 2,
                    ..Default::default()
                },
                Adapter {
                    // Up 但无 IP → 排除
                    name: "空网卡".into(),
                    status: "Up".into(),
                    virtual_: false,
                    if_index: 3,
                    ..Default::default()
                },
                Adapter {
                    // Down → 排除
                    name: "断开".into(),
                    status: "Disconnected".into(),
                    ipv4: vec!["10.0.0.5".into()],
                    virtual_: false,
                    if_index: 4,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let ups = s.physical_up_adapters();
        assert_eq!(ups.len(), 1);
        assert_eq!(ups[0].name, "以太网");
    }

    #[test]
    fn current_ssid_ignores_bssid_line() {
        let text = "    名称                   : WLAN\n    SSID                   : MyWiFi\n    BSSID                  : aa:bb:cc:dd:ee:ff\n";
        assert_eq!(current_ssid(text), "MyWiFi", "不得把 BSSID 当 SSID");
    }

    #[test]
    fn current_ssid_handles_quotes_and_empty() {
        assert_eq!(
            current_ssid("    SSID                   : \"引号网\"\n"),
            "引号网"
        );
        assert_eq!(current_ssid(""), "");
        assert_eq!(current_ssid("    名称 : WLAN\n"), "");
    }

    #[test]
    fn wlan_disconnected_detects_both_languages() {
        assert!(wlan_disconnected("已断开"));
        assert!(wlan_disconnected("State : disconnected"));
        assert!(wlan_disconnected("DISCONNECTED"));
        assert!(!wlan_disconnected("    SSID : MyWiFi"));
    }

    #[test]
    fn live_snapshot_collects_something_on_this_machine() {
        // 真机采集：不断言具体网卡（CI/离线机器可能为空），但必须能跑通且不 panic
        let s = take();
        if !s.ok {
            eprintln!("提示：本机 PowerShell 采集未成功（{}），跳过断言", s.error);
            return;
        }
        assert!(!s.adapters.is_empty(), "采集成功时应至少有一个网卡");
        // 网卡名不应为空
        assert!(s.adapters.iter().all(|a| !a.name.is_empty()));
        // if_metrics 至少要有值（Get-NetIPInterface 一般都有输出）
        assert!(!s.if_metrics.is_empty(), "接口跃点表不应为空");
    }

    #[test]
    fn live_persistent_routes_parse_without_panic() {
        // 本机可能没有持久路由（返回空）；有的话 dest 必须是 CIDR 形态
        for r in persistent_routes() {
            assert!(r.dest.contains('/'), "dest 应是 CIDR: {}", r.dest);
            assert!(!r.next_hop.is_empty());
        }
    }
}
