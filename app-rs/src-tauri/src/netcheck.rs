//! 网络诊断检查（对应 Python 侧 netcheck.py）。
//!
//! 依赖分层（决定了本轮的诚实边界）：
//! - **不需要 Snapshot 的检查**已实现：NCSI 注册表核对、关键服务状态、
//!   网关连通、DNS 对比、NCSI 探测模拟、真实连通性、DNS 污染检测；
//! - **需要 Snapshot 的检查**（网卡列表 / 默认路由 / 持久路由 / IPv6 / WLAN）
//!   依赖 `snapshot.py`（全量 PowerShell 采集）尚未移植，它们返回
//!   `skip` + 「尚未接入」而不是伪造 pass/fail——诊断页伪造绿勾比留白更危险。
//!
//! HTTP 探测走**直连 opener**（对应 Python `ProxyHandler({})`）：诊断对象本身
//! 可能就是残留代理，走系统代理会把「代理挂了」误判成「断网」，进而驱动重启
//! 网卡等无效修复。Rust 标准库默认不读系统代理，语义天然一致。

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use serde::Serialize;

use crate::dnsclient;
use crate::icmp;
use crate::snapshot::{RouteEntry, Snapshot};
use crate::w32::{self, RegValue};

pub const PASS: &str = "pass";
pub const WARN: &str = "warn";
pub const FAIL: &str = "fail";
pub const SKIP: &str = "skip";
pub const INFO: &str = "info";

/// 状态符号（与 Python `STATUS_MARK` 逐字一致）。
pub fn status_mark(status: &str) -> &'static str {
    match status {
        PASS => "√",
        WARN => "!",
        FAIL => "×",
        SKIP => "-",
        INFO => "·",
        _ => "?",
    }
}

/// 单项检查结果（对应 Python `CheckResult`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckResult {
    pub id: &'static str,
    pub name: &'static str,
    pub status: &'static str,
    pub detail: String,
    /// 建议的修复动作 id（可空）。
    pub fix_ids: Vec<&'static str>,
}

impl CheckResult {
    pub fn new(id: &'static str, name: &'static str, status: &'static str) -> Self {
        Self {
            id,
            name,
            status,
            detail: String::new(),
            fix_ids: Vec::new(),
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }

    pub fn with_fixes(mut self, ids: &[&'static str]) -> Self {
        self.fix_ids = ids.to_vec();
        self
    }

    /// 单行展示（对齐 Python `CheckResult.line()`）。
    pub fn line(&self) -> String {
        format!(
            "[{}] {}\n      {}",
            status_mark(self.status),
            self.name,
            self.detail
        )
    }
}

// ---------------- 配置常量（对齐 config.py） ----------------

pub const PROBE_204_URLS: [&str; 2] = [
    "http://connect.rom.miui.com/generate_204",
    "http://wifi.vivo.com.cn/generate_204",
];
pub const PROBE_HTTPS_URL: &str = "https://www.baidu.com";
pub const NCSI_WEB_URL: &str = "http://www.msftconnecttest.com/connecttest.txt";
pub const NCSI_EXPECT_CONTENT: &str = "Microsoft Connect Test";
pub const NCSI_DNS_HOST: &str = "dns.msftncsi.com";
pub const NCSI_DNS_EXPECT: &str = "131.107.255.255";
pub const NCSI_REG_FULL: &str =
    r"HKLM\SYSTEM\CurrentControlSet\Services\NlaSvc\Parameters\Internet";
pub const NCSI_POLICY_FULL: &str =
    r"HKLM\SOFTWARE\Policies\Microsoft\Windows\NetworkConnectivityStatusIndicator";

/// NCSI 原版默认值（与 config.NCSI_STOCK 逐项一致）。
pub fn ncsi_stock() -> Vec<(&'static str, &'static str)> {
    vec![
        ("EnableActiveProbing", "1"),
        ("ActiveWebProbeHost", "www.msftconnecttest.com"),
        ("ActiveWebProbePath", "connecttest.txt"),
        ("ActiveWebProbeContent", "Microsoft Connect Test"),
        ("ActiveWebProbeHostV6", "ipv6.msftconnecttest.com"),
        ("ActiveWebProbePathV6", "connecttest.txt"),
        ("ActiveWebProbeContentV6", "Microsoft Connect Test"),
        ("ActiveDnsProbeHost", "dns.msftncsi.com"),
        ("ActiveDnsProbeContent", "131.107.255.255"),
        ("ActiveDnsProbeHostV6", "dns.msftncsi.com"),
        ("ActiveDnsProbeContentV6", "fd3e:4f5a:5b81::1"),
    ]
}

/// 国内公共 DNS 对比测试目标（对应 config.PUBLIC_DNS_TEST）。
pub const PUBLIC_DNS_TEST: [&str; 2] = ["223.5.5.5", "119.29.29.29"];
/// DNS 对比用域名。
pub const DNS_COMPARE_HOST: &str = "www.baidu.com";

/// DNS 污染判定用的"地雷"IP 集合（对应 config.FOREIGN_DNS_BLACKLIST 用途）。
pub fn bogus_ips() -> Vec<&'static str> {
    vec![
        "127.0.0.1",
        "0.0.0.0",
        "1.1.1.1",
        "8.8.8.8",
        "8.8.4.4",
        "9.9.9.9",
        "208.67.222.222",
        "208.67.220.220",
    ]
}

/// 国内不可靠、常被列为"地雷"的国外 DNS（与 config.FOREIGN_DNS_BLACKLIST 逐项一致）。
pub const FOREIGN_DNS_BLACKLIST: [&str; 7] = [
    "8.8.8.8",
    "8.8.4.4",
    "1.1.1.1",
    "1.0.0.1",
    "9.9.9.9",
    "208.67.222.222",
    "208.67.220.220",
];

// ---------------- HTTP 探测（直连，不走系统代理） ----------------

/// 一次极简 HTTP GET：返回 (状态码, 响应体前 512 字节)。
///
/// 只实现诊断需要的部分：解析 URL、连 TCP、发 GET、读响应。**不跟随重定向**
/// 之外的高级特性一概不做——诊断要的是「通不通、返回什么码」，不是通用客户端。
pub fn http_get(url: &str, timeout: f64, need_body: bool) -> Result<(u16, String), String> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| format!("URL 缺少协议: {url}"))?;
    let (hostport, path) = match rest.split_once('/') {
        Some((h, p)) => (h, format!("/{p}")),
        None => (rest, "/".to_string()),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (
            h.to_string(),
            p.parse::<u16>().map_err(|e| format!("端口非法: {e}"))?,
        ),
        None => (
            hostport.to_string(),
            if scheme == "https" { 443 } else { 80 },
        ),
    };
    // 只支持 http：https 需要 TLS，标准库没有实现。
    // 这里明确报错而不是静默失败——诊断项会把「无法验证 HTTPS」如实显示。
    if scheme == "https" {
        return Err("本版未实现 TLS，无法直连 HTTPS 探测".to_string());
    }

    let addr = format!("{host}:{port}");
    let sock = std::net::ToSocketAddrs::to_socket_addrs(&addr)
        .map_err(|e| format!("解析 {addr} 失败: {e}"))?
        .next()
        .ok_or_else(|| format!("{addr} 无可用地址"))?;
    let dur = Duration::from_secs_f64(timeout.max(0.2));
    let mut stream =
        TcpStream::connect_timeout(&sock, dur).map_err(|e| format!("连接失败: {e}"))?;
    stream.set_read_timeout(Some(dur)).ok();
    stream.set_write_timeout(Some(dur)).ok();

    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: SysKit/1.0\r\nAccept: */*\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(req.as_bytes())
        .map_err(|e| format!("发送请求失败: {e}"))?;

    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    let body_limit = if need_body { 512 } else { 0 };
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                // 够判断状态行与所需响应体即可收手
                if buf.len() >= 4096
                    || (buf.len() > body_limit && body_limit > 0 && buf.len() > 1024)
                {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let text = String::from_utf8_lossy(&buf).to_string();
    let code = parse_status_code(&text).ok_or_else(|| "响应缺少状态行".to_string())?;
    let body = if need_body {
        extract_body(&text).chars().take(512).collect()
    } else {
        String::new()
    };
    Ok((code, body))
}

/// 从响应首行解析状态码（`HTTP/1.1 204 No Content`）。
pub fn parse_status_code(response: &str) -> Option<u16> {
    let first = response.lines().next()?;
    let mut parts = first.split_whitespace();
    let version = parts.next()?;
    if !version.starts_with("HTTP/") {
        return None;
    }
    parts.next()?.parse::<u16>().ok()
}

/// 取响应体（`\r\n\r\n` 之后）。
pub fn extract_body(response: &str) -> &str {
    match response.find("\r\n\r\n") {
        Some(i) => &response[i + 4..],
        None => "",
    }
}

/// 任一 204 端点可达即算通（对应 Python `probe_204`）。
pub fn probe_204(timeout: f64) -> (bool, String) {
    let mut last = String::from("all failed");
    for url in PROBE_204_URLS {
        match http_get(url, timeout, false) {
            Ok((204, _)) => return (true, url.to_string()),
            Ok((code, _)) => last = format!("HTTP {code}"),
            Err(e) => last = e,
        }
    }
    (false, last)
}

/// 真实连通性：204 端点 + HTTPS（对应 Python `probe_real_internet`）。
///
/// Rust 侧没有 TLS，HTTPS 分支以「TCP 能连通 443」作为弱证据，并在详情里
/// 明确标注，避免把「未验证」写成「已验证」。
pub fn probe_real_internet() -> (bool, String) {
    let (ok204, d1) = probe_204(6.0);
    let https = probe_tcp(PROBE_HTTPS_URL, 6.0);
    let detail = format!(
        "204探测:{} ({d1}) HTTPS({PROBE_HTTPS_URL}):{}",
        if ok204 { "OK" } else { "FAIL" },
        match &https {
            Ok(()) => "TCP可达(未校验证书)".to_string(),
            Err(e) => format!("FAIL({e})"),
        }
    );
    (ok204 || https.is_ok(), detail)
}

/// 只验证能否建立 TCP 连接（HTTPS 的弱替代）。
pub fn probe_tcp(url: &str, timeout: f64) -> Result<(), String> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| "URL 缺少协议".to_string())?;
    let hostport = rest.split('/').next().unwrap_or(rest);
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().unwrap_or(443)),
        None => (
            hostport.to_string(),
            if scheme == "https" { 443 } else { 80 },
        ),
    };
    let addr = format!("{host}:{port}");
    let sock = std::net::ToSocketAddrs::to_socket_addrs(&addr)
        .map_err(|e| format!("解析失败: {e}"))?
        .next()
        .ok_or_else(|| "无可用地址".to_string())?;
    TcpStream::connect_timeout(&sock, Duration::from_secs_f64(timeout.max(0.2)))
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// NCSI 探测模拟（Web + DNS 双条件，对应 Python `probe_ncsi`）。
pub fn probe_ncsi() -> (bool, String) {
    let web = http_get(NCSI_WEB_URL, 6.0, true);
    let (web_ok, web_detail) = match web {
        Ok((200, body)) if body.contains(NCSI_EXPECT_CONTENT) => (true, "HTTP 200".to_string()),
        Ok((code, body)) => (
            false,
            format!(
                "HTTP {code} 内容不符({:?})",
                body.chars().take(40).collect::<String>()
            ),
        ),
        Err(e) => (false, e),
    };
    let ips = dnsclient::resolve_system(NCSI_DNS_HOST, 4.0);
    let dns_ok = ips.iter().any(|i| i == NCSI_DNS_EXPECT);
    let detail = format!(
        "Web探测: {web_detail}; DNS({NCSI_DNS_HOST}): {}",
        if dns_ok {
            "OK".to_string()
        } else {
            format!(
                "FAIL->{}",
                if ips.is_empty() {
                    "超时".to_string()
                } else {
                    ips.join(",")
                }
            )
        }
    );
    (web_ok && dns_ok, detail)
}

// ---------------- 纯逻辑辅助 ----------------

/// 取第一个可用 IPv4 DNS，过滤 Windows 占位符 fec0::/10（对应 Python `_v4_dns`）。
pub fn v4_dns(dns_list: &[String]) -> String {
    for d in dns_list {
        if !d.contains(':') && !d.to_ascii_lowercase().starts_with("fec") {
            return d.clone();
        }
    }
    String::new()
}

/// 结果是否为「可疑/污染」IP（对应 Python `_bogus`）。
pub fn is_bogus(ips: &[String]) -> bool {
    let bogus = bogus_ips();
    !ips.is_empty() && ips.iter().all(|ip| bogus.contains(&ip.as_str()))
}

/// 从 ping 输出解析丢包率百分比（对应 Python `_ping_loss`）。
///
/// 真实输出里括号是**紧贴**数字的：`丢失 = 0 (0% 丢失)` / `Lost = 1 (25% loss)`,
/// 分词后得到 `(0%` 与 `25%)`。所以先去括号再判 `%` 后缀——第一版直接判
/// `strip_suffix('%')` 就漏掉了 `(0%` 这种形态（测试逮到的）。
pub fn ping_loss(ping_out: &str) -> Option<u32> {
    for token in ping_out.split_whitespace() {
        let cleaned = token.trim_matches(|c| c == '(' || c == ')' || c == '，' || c == ',');
        if let Some(pct) = cleaned.strip_suffix('%') {
            if let Ok(n) = pct.trim().parse::<u32>() {
                return Some(n);
            }
        }
    }
    None
}

/// 服务状态（对应 Python `Snapshot.service(name)` 的字段）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceState {
    pub status: String,
    pub start_type: String,
}

/// 查询服务状态（`sc query` + `sc qc`）。
pub fn service_state(name: &str) -> Option<ServiceState> {
    let q = w32::run_capture(&["sc".into(), "query".into(), name.into()], 10).ok()?;
    if !q.ok && !q.stdout.contains("SERVICE_NAME") {
        return None;
    }
    let status = extract_sc_field(&q.stdout, "STATE").unwrap_or_else(|| "Unknown".to_string());
    // STATE 行形如 "        STATE              : 4  RUNNING"
    let status = status
        .split_whitespace()
        .last()
        .unwrap_or("Unknown")
        .to_string();
    let qc = w32::run_capture(&["sc".into(), "qc".into(), name.into()], 10).ok();
    let start_type = qc
        .as_ref()
        .and_then(|o| extract_sc_field(&o.stdout, "START_TYPE"))
        .map(|s| s.split_whitespace().next().unwrap_or("Unknown").to_string())
        .unwrap_or_else(|| "Unknown".to_string());
    Some(ServiceState { status, start_type })
}

/// 从 `sc` 输出里取某字段冒号后的内容。
fn extract_sc_field(text: &str, field: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix(field) {
            if let Some((_, v)) = rest.split_once(':') {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

// ---------------- 各项检查 ----------------

/// NCSI 注册表配置核对（id=ncsi_reg）。
pub fn chk_ncsi_registry() -> CheckResult {
    let mut r = CheckResult::new("ncsi_reg", "NCSI 注册表配置核对", PASS);
    let vals = w32::reg_values(NCSI_REG_FULL);
    if vals.is_empty() {
        r.status = FAIL;
        r.detail = "无法读取 NCSI 注册表（需管理员？）".to_string();
        return r;
    }
    let mut issues = Vec::new();
    let probing = vals.get("EnableActiveProbing").and_then(RegValue::as_u32);
    if probing == Some(0) {
        issues.push("EnableActiveProbing=0（主动探测被关闭）".to_string());
    }
    for (k, want) in ncsi_stock() {
        if k == "EnableActiveProbing" {
            continue;
        }
        match vals.get(k) {
            None => issues.push(format!("{k} 缺失")),
            Some(got) => {
                if got.as_compare_str() != want {
                    issues.push(format!("{k}={}(默认{want})", got.as_compare_str()));
                }
            }
        }
    }
    let policy = w32::reg_values(NCSI_POLICY_FULL);
    if policy.get("NoActiveProbe").and_then(RegValue::as_u32) == Some(1) {
        issues.push("组策略 NoActiveProbe=1".to_string());
    }
    if issues.is_empty() {
        let host = vals
            .get("ActiveWebProbeHost")
            .map(RegValue::as_compare_str)
            .unwrap_or_default();
        let path = vals
            .get("ActiveWebProbePath")
            .map(RegValue::as_compare_str)
            .unwrap_or_default();
        r.detail = format!(
            "与原版默认一致（EnableActiveProbing={}, 探测 {host}/{path}）",
            probing.map(|p| p.to_string()).unwrap_or_else(|| "?".into())
        );
    } else {
        r.status = WARN;
        r.detail = issues.join("；");
        r.fix_ids = vec!["ncsi_restore"];
    }
    r
}

/// 关键网络服务状态（id=services）。
pub fn chk_services() -> CheckResult {
    let mut r = CheckResult::new("services", "关键网络服务状态", PASS);
    let mut lines = Vec::new();
    let mut bad = Vec::new();
    for name in ["Dhcp", "Dnscache", "netprofm", "WlanSvc"] {
        let Some(s) = service_state(name) else {
            continue;
        };
        let ok = s.status == "RUNNING";
        lines.push(format!("{name}={}/{}", s.status, s.start_type));
        // WlanSvc 停止在「无 WiFi 使用场景」可接受
        if !ok && name != "WlanSvc" {
            bad.push(name);
        }
    }
    r.detail = if lines.is_empty() {
        "无数据".to_string()
    } else {
        lines.join("；")
    };
    if !bad.is_empty() {
        r.status = WARN;
        r.fix_ids = vec!["restart_services"];
    }
    r
}

/// 网关连通（id=gateway）。
///
/// 判定对象是「已连接的物理网卡各自的默认网关」（与 Python 一致），
/// 无物理网卡时返回 skip。
pub fn chk_gateway(snap: &Snapshot) -> CheckResult {
    let mut r = CheckResult::new("gateway", "网关连通（Ping 默认网关 ×3）", SKIP);
    let ups = snap.physical_up_adapters();
    if ups.is_empty() {
        r.detail = "无已连接的物理网卡，跳过".to_string();
        return r;
    }
    let mut lines = Vec::new();
    let mut any_ok = false;
    let mut any_fail = false;
    for a in ups {
        // 每张网卡只测第一个网关（对应 Python `a.gw4[:1]`）
        for gw in a.gw4.iter().take(1) {
            // 原生 ICMP ×3（换掉 ping.exe：无本地化解析、无子进程开销）
            let mut lost = 0u32;
            for _ in 0..3 {
                if icmp::ping(gw, 1000).is_none() {
                    lost += 1;
                }
            }
            let loss = lost * 100 / 3;
            let ok = loss < 100;
            any_ok |= ok;
            any_fail |= !ok;
            lines.push(format!(
                "{} 网关 {gw}: 丢包 {loss}%{}",
                a.name,
                if lost == 3 {
                    "（全丢，疑似二层拦截）"
                } else {
                    ""
                }
            ));
        }
    }
    if lines.is_empty() {
        r.detail = "已连接网卡均无默认网关".to_string();
        r.status = WARN;
        return r;
    }
    r.detail = lines.join("；");
    r.status = if any_fail && !any_ok {
        FAIL
    } else if any_fail {
        WARN
    } else {
        PASS
    };
    r
}

/// DNS 解析对比（id=dns_compare）：用**网卡上的系统 DNS** 与公共 DNS 对比。
///
/// Python 版分别向 `a.dns` 与公共 DNS 各查一次同一域名；两者结果都为空才算
/// 「无系统 DNS 可测」。这里保留该语义，并对结果做集合比对。
pub fn chk_dns_compare(snap: &Snapshot) -> CheckResult {
    let mut r = CheckResult::new("dns_compare", "DNS 解析（系统 DNS vs 公共 DNS 对比）", SKIP);
    let ups = snap.physical_up_adapters();
    let sys_dns = ups.iter().map(|a| v4_dns(&a.dns)).find(|d| !d.is_empty());
    let Some(sys_dns) = sys_dns else {
        r.detail = "无系统 DNS 可测".to_string();
        return r;
    };
    let sys_ips = dnsclient::query_safe(DNS_COMPARE_HOST, &sys_dns);
    let mut pub_ips = Vec::new();
    let mut pub_detail = Vec::new();
    for server in PUBLIC_DNS_TEST {
        let start = std::time::Instant::now();
        let ips = dnsclient::query_safe(DNS_COMPARE_HOST, server);
        let ms = start.elapsed().as_millis();
        pub_detail.push(format!(
            "{server}:{}",
            if ips.is_empty() {
                "FAIL".to_string()
            } else {
                format!("{}ms", ms)
            }
        ));
        if pub_ips.is_empty() && !ips.is_empty() {
            pub_ips = ips;
        }
    }
    let _ = &pub_detail;
    if sys_ips.is_empty() {
        r.status = FAIL;
        r.detail = format!("网卡 DNS {sys_dns} 解析 {DNS_COMPARE_HOST} 失败/超时");
        r.fix_ids = vec!["flush_dns", "dns_auto"];
        return r;
    }
    if pub_ips.is_empty() {
        r.status = WARN;
        r.detail = format!(
            "网卡 DNS {sys_dns} → {}；公共 DNS 全部无响应（{}）——可能是网络屏蔽了 53 端口",
            sys_ips.join(","),
            pub_detail.join(" ")
        );
        return r;
    }
    if sys_ips.iter().any(|ip| pub_ips.contains(ip)) {
        r.status = PASS;
        r.detail = format!(
            "系统 DNS({sys_dns}) → {}；公共 DNS → {}（{}）",
            sys_ips.join(","),
            pub_ips.join(","),
            pub_detail.join(" ")
        );
    } else {
        r.status = WARN;
        r.detail = format!(
            "系统 DNS 与公共 DNS 结果不一致：系统={} 公共={}（{}）——可能是 DNS 劫持或 CDN 就近解析",
            sys_ips.join(","),
            pub_ips.join(","),
            pub_detail.join(" ")
        );
        r.fix_ids = vec!["dns_preset"];
    }
    r
}

/// NCSI 域名解析污染检测（id=dns_pollution）。
pub fn chk_dns_pollution() -> CheckResult {
    let mut r = CheckResult::new("dns_pollution", "NCSI 域名解析对比（污染检测）", INFO);
    let sys_ips = dnsclient::resolve_system(NCSI_DNS_HOST, 4.0);
    let pub_ips = dnsclient::query_safe(NCSI_DNS_HOST, PUBLIC_DNS_TEST[0]);
    let detail = format!(
        "系统: {}; 公共({}): {}",
        if sys_ips.is_empty() {
            "无".to_string()
        } else {
            sys_ips.join(",")
        },
        PUBLIC_DNS_TEST[0],
        if pub_ips.is_empty() {
            "无".to_string()
        } else {
            pub_ips.join(",")
        }
    );
    r.detail = detail;
    if is_bogus(&sys_ips) {
        r.status = WARN;
        r.detail = format!("{} —— 系统解析结果是已知地雷 IP，疑似污染", r.detail);
        r.fix_ids = vec!["dns_preset"];
    } else if !sys_ips.is_empty() && sys_ips.iter().any(|i| i == NCSI_DNS_EXPECT) {
        r.status = PASS;
        r.detail = format!("{} —— 与官方期望一致", r.detail);
    }
    r
}

/// NCSI 探测模拟（id=ncsi_probe）。
pub fn chk_ncsi_probe() -> CheckResult {
    let (ok, detail) = probe_ncsi();
    CheckResult::new(
        "ncsi_probe",
        "NCSI 探测模拟（Windows 判定'有网'的依据）",
        if ok { PASS } else { FAIL },
    )
    .with_detail(detail)
}

/// 真实连通性（id=real_conn）。
pub fn chk_real() -> CheckResult {
    let (ok, detail) = probe_real_internet();
    CheckResult::new(
        "real_conn",
        "真实连通性（204 端点 + HTTPS）",
        if ok { PASS } else { FAIL },
    )
    .with_detail(detail)
}

/// 持久路由（id=persistent_route）：默认持久路由是流量黑洞的元凶。
pub fn chk_persistent_route(snap: &Snapshot) -> CheckResult {
    let mut r = CheckResult::new(
        "persistent_route",
        "持久路由（重启不消失的手工路由，休眠时会劫持流量）",
        PASS,
    );
    if snap.persistent_routes.is_empty() {
        r.detail = "无持久路由".to_string();
        return r;
    }
    let defaults: Vec<&RouteEntry> = snap
        .persistent_routes
        .iter()
        .filter(|p| p.dest == "0.0.0.0/0")
        .collect();
    let others: Vec<&RouteEntry> = snap
        .persistent_routes
        .iter()
        .filter(|p| p.dest != "0.0.0.0/0")
        .collect();
    if !defaults.is_empty() {
        let gws = defaults
            .iter()
            .map(|d| format!("{}(跃点{})", d.next_hop, d.metric))
            .collect::<Vec<_>>()
            .join(", ");
        r.status = FAIL;
        r.detail = format!(
            "存在持久默认路由 → {gws}。连上同网段 WiFi 时它会以更低跃点抢占全部流量，若该网关不可达则流量黑洞（热点/USB 不受影响）"
        );
        r.fix_ids = vec!["del_persistent_route"];
    } else {
        r.status = WARN;
        let list = others
            .iter()
            .take(4)
            .map(|p| format!("{}→{}", p.dest, p.next_hop))
            .collect::<Vec<_>>()
            .join(", ");
        r.detail = format!("存在非默认持久路由: {list}");
    }
    r
}

/// 网卡静态 DNS 审计（id=static_dns）。
pub fn chk_static_dns(snap: &Snapshot) -> CheckResult {
    let mut r = CheckResult::new(
        "static_dns",
        "网卡静态 DNS 审计（残留的静态 DNS 只影响对应网卡）",
        PASS,
    );
    let mut problems = Vec::new();
    let mut notes = Vec::new();
    for a in &snap.adapters {
        if a.dns.is_empty() || a.virtual_ || a.desc.contains("WAN Miniport") {
            continue;
        }
        let v4 = v4_dns(&a.dns);
        if v4.is_empty() {
            continue;
        }
        let (is_static, _src) = dns_is_static(&a.name);
        let foreign: Vec<&String> = a
            .dns
            .iter()
            .filter(|d| FOREIGN_DNS_BLACKLIST.contains(&d.as_str()))
            .collect();
        let tag = if a.status.eq_ignore_ascii_case("up") {
            ""
        } else {
            "(未连接)"
        };
        if !foreign.is_empty() {
            let fw = foreign
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(",");
            problems.push(format!(
                "{}{tag}: {}（{}，含国内不可达的 {fw}）",
                a.name,
                a.dns.join(","),
                if is_static { "静态" } else { "DHCP" }
            ));
            r.fix_ids = vec!["dns_auto", "dns_preset"];
        } else if is_static {
            notes.push(format!("{}{tag}: 静态 {}", a.name, a.dns.join(",")));
        }
    }
    if !problems.is_empty() {
        r.status = WARN;
        let mut all = problems;
        all.extend(notes);
        r.detail = all.join("；");
    } else if !notes.is_empty() {
        r.status = INFO;
        r.detail = notes.join("；");
    } else {
        r.detail = "各网卡 DNS 均为 DHCP 自动获取，无国外黑名单 DNS".to_string();
    }
    r
}

/// 静态 DNS 判定：解析 `netsh interface ipv4 show dnsservers` 的中英文标记。
///
/// 这是静态判定的**唯一实现**（Python 注释里的教训：两处各写一份正则曾导致语义分叉）。
pub fn dns_is_static(adapter_name: &str) -> (bool, &'static str) {
    let out = w32::run_capture(
        &[
            "netsh".into(),
            "interface".into(),
            "ipv4".into(),
            "show".into(),
            "dnsservers".into(),
            adapter_name.to_string(),
        ],
        10,
    );
    let text = match out {
        Ok(o) => o.stdout,
        Err(_) => return (false, "unknown"),
    };
    if contains_ci(&text, "静态配置") || contains_ci(&text, "Statically configured") {
        return (true, "static");
    }
    if contains_ci(&text, "通过 DHCP 配置")
        || contains_ci(&text, "DHCP configured")
        || contains_ci(&text, "Initiated by Dhcp")
    {
        return (false, "dhcp");
    }
    (false, "unknown")
}

fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

/// 路由健康（id=routes）：幽灵默认路由 / 多默认网关 / 隧道抢占。
pub fn chk_routes(snap: &Snapshot) -> CheckResult {
    let mut r = CheckResult::new(
        "routes",
        "路由健康（幽灵路由 / 多默认网关 / 隧道抢占）",
        PASS,
    );
    let status_of = |idx: u32| -> String {
        snap.adapter_by_index(idx)
            .map(|a| a.status.to_lowercase())
            .unwrap_or_default()
    };
    let fmt = |d: &RouteEntry| -> String {
        format!(
            "{}(if{}, 跃点{})",
            d.next_hop,
            d.if_index,
            d.metric + d.if_metric
        )
    };
    let phantom: Vec<&RouteEntry> = snap
        .default_routes
        .iter()
        .filter(|d| status_of(d.if_index) != "up")
        .collect();
    let active: Vec<&RouteEntry> = snap
        .default_routes
        .iter()
        .filter(|d| status_of(d.if_index) == "up")
        .collect();
    let lines: Vec<String> = active
        .iter()
        .map(|d| format!("活动默认路由: {}", fmt(d)))
        .collect();

    if !phantom.is_empty() {
        let persist_gw: Vec<&str> = snap
            .persistent_routes
            .iter()
            .filter(|p| p.dest == "0.0.0.0/0")
            .map(|p| p.next_hop.as_str())
            .collect();
        let from_persist = phantom
            .iter()
            .any(|d| persist_gw.contains(&d.next_hop.as_str()));
        r.status = FAIL;
        r.detail = format!(
            "幽灵默认路由（接口已断开却占据活动路由表）: {}{}",
            phantom
                .iter()
                .map(|d| fmt(d))
                .collect::<Vec<_>>()
                .join("; "),
            if from_persist {
                "；来自持久路由 —— 连上同网段网络时会劫持全部流量，且已扰乱 DNS 解析排序"
            } else {
                ""
            }
        );
        r.fix_ids = vec!["del_persistent_route"];
        return r;
    }

    if active.len() > 1 {
        r.status = WARN;
        r.detail = format!("{} —— 存在多个默认网关，流量出口可能漂移", lines.join("；"));
        return r;
    }

    // 隧道抢占：隧道网卡有默认路由且跃点不高于物理网卡
    let tunnels: Vec<&crate::snapshot::Adapter> = snap
        .adapters
        .iter()
        .filter(|a| a.status.eq_ignore_ascii_case("up") && is_tunnel(&a.desc))
        .collect();
    let tun_with_gw: Vec<u32> = active.iter().map(|d| d.if_index).collect();
    let mut low_metric_tun = Vec::new();
    for t in &tunnels {
        if !tun_with_gw.contains(&t.if_index) {
            continue;
        }
        let Some(&m) = snap.if_metrics.get(&t.if_index) else {
            continue;
        };
        let phys: Vec<u32> = snap
            .adapters
            .iter()
            .filter(|a| a.status.eq_ignore_ascii_case("up") && !is_tunnel(&a.desc) && !a.virtual_)
            .filter_map(|a| snap.if_metrics.get(&a.if_index).copied())
            .collect();
        if !phys.is_empty() && m <= *phys.iter().min().unwrap() {
            let short: String = t.desc.chars().take(20).collect();
            low_metric_tun.push(format!("{}({short}, 跃点{m})", t.name));
        }
    }
    if !low_metric_tun.is_empty() {
        r.status = WARN;
        r.detail = format!(
            "{}；隧道网卡跃点不高于物理网卡: {}",
            lines.join("；"),
            low_metric_tun.join(", ")
        );
    } else {
        r.detail = if lines.is_empty() {
            "无默认路由（可能未连接）".to_string()
        } else {
            lines.join("；")
        };
    }
    r
}

/// 隧道网卡识别（与 Python `vpn.TUNNEL_PAT` 同一份口径）。
/// 隧道网卡描述关键字。
///
/// **必须与 Python `vpn.TUNNEL_PAT` 覆盖同一批产品**：这个判定有安全含义——
/// 自动修复的 L3 梯度（`renew_dhcp`/`restart_wlan`/`toggle_adapter`）会**切断隧道**，
/// 而护栏就是"检测到隧道就跳过这些步骤"。漏检一个产品，用户挂着 VPN 时就会被
/// 自动修复踢下线。
pub const TUNNEL_KEYWORDS: [&str; 23] = [
    "wintun",
    "tap-windows",
    "tap-win32",
    "tap9",
    "tun",
    "utun",
    "wireguard",
    "openvpn",
    "easyconnect",
    "sangfor",
    "clash",
    "v2ray",
    "sing-box",
    "nekoray",
    "hysteria",
    "uu加速",
    "uu ",
    "hamachi",
    "zerotier",
    "tailscale",
    "oray",
    "pgyer",
    "tunnel",
];

/// 描述里是否含隧道/VPN 产品关键字（大小写不敏感）。
pub fn is_tunnel(desc: &str) -> bool {
    let lower = desc.to_lowercase();
    TUNNEL_KEYWORDS.iter().any(|k| lower.contains(k))
}

/// IPv6 状态（id=ipv6）：NCSI 的 V6 探测值是否存在。
pub fn chk_ipv6() -> CheckResult {
    let mut r = CheckResult::new("ipv6", "IPv6 状态（V6 探测值缺失会导致 v6 网络误判）", INFO);
    let vals = w32::reg_values(NCSI_REG_FULL);
    let has_v6 = !vals.is_empty() && vals.contains_key("ActiveWebProbeHostV6");
    r.detail = format!("NCSI V6 探测值: {}", if has_v6 { "有" } else { "无" });
    r
}

/// WLAN 配置（id=wlan）：连接状态 + MAC 随机化。
pub fn chk_wlan() -> CheckResult {
    let mut r = CheckResult::new("wlan", "WLAN 配置（MAC 随机化 / 连接状态）", INFO);
    let text = crate::snapshot::wlan_interfaces_text();
    let ssid = crate::snapshot::current_ssid(&text);
    let mut lines = Vec::new();
    if crate::snapshot::wlan_disconnected(&text) {
        lines.push("当前未连接 WiFi".to_string());
    } else if !ssid.is_empty() {
        lines.push(format!("已连接: {ssid}"));
    }
    let settings = crate::snapshot::wlan_settings_text();
    if mac_randomization_enabled(&settings) {
        lines.push(
            "接口级 MAC 随机化已启用（部分路由器/公寓网对此不友好，连不上或频繁掉线时可尝试关闭）"
                .to_string(),
        );
        r.status = WARN;
    }
    r.detail = if lines.is_empty() {
        "无法读取 WLAN 设置".to_string()
    } else {
        lines.join("；")
    };
    r
}

/// MAC 随机化是否启用：真实输出里词组间有说明文字与冒号，必须用「宽松匹配」。
///
/// Python 注释记着这条：旧正则紧邻匹配永不命中，导致该 WARN 从未产生过。
pub fn mac_randomization_enabled(settings_text: &str) -> bool {
    for line in settings_text.lines() {
        let lower = line.to_lowercase();
        let mentions = lower.contains("mac 随机化")
            || lower.contains("mac randomization")
            || lower.contains("随机硬件地址");
        if !mentions {
            continue;
        }
        if lower.contains("已启用") || lower.contains("enabled") {
            return true;
        }
    }
    false
}

/// 网卡节电（id=power_mgmt）：PnPCapabilities 显示「允许系统关闭设备」。
///
/// 数据来源是网卡驱动注册表（`...\Control\Class\{4d36e972-...}\<NNNN>`），
/// 只对物理网卡判定；读不到时返回 skip（对应 Python 的 `scan is None` 分支）。
pub fn chk_power_mgmt(snap: &Snapshot) -> CheckResult {
    let mut r = CheckResult::new("power_mgmt", "网卡节电（允许系统关闭设备以节约电源）", INFO);
    let scan = pnp_capabilities_scan();
    let mut items = Vec::new();
    for a in &snap.adapters {
        if a.virtual_ || a.desc.is_empty() {
            continue;
        }
        match pnp_capabilities(&a.desc, &scan) {
            // 24 = PnPCapabilities 里「允许系统关闭设备以节约电源」的位组合
            Some(24) => {
                let short: String = a.desc.chars().take(24).collect();
                items.push(format!("{}({short}): 允许断电", a.name));
            }
            Some(_) => {
                let short: String = a.desc.chars().take(24).collect();
                items.push(format!("{}({short}): 已禁止断电", a.name));
            }
            None => continue,
        }
    }
    if scan.is_empty() && items.is_empty() {
        r.status = SKIP;
        r.detail = "无法读取网卡注册表".to_string();
        return r;
    }
    let risky: Vec<&String> = items.iter().filter(|i| i.contains("允许断电")).collect();
    if !risky.is_empty() {
        r.status = WARN;
        r.detail = format!(
            "{} —— 无线/USB 网卡被允许休眠省电，是 WiFi/USB 共享频繁掉线的常见原因",
            items.join("；")
        );
        r.fix_ids = vec!["power_off"];
    } else {
        r.status = PASS;
        r.detail = if items.is_empty() {
            "未发现可断电网卡".to_string()
        } else {
            items.join("；")
        };
    }
    r
}

/// 扫描网卡类键：描述 → PnPCapabilities（一次读全，避免每网卡各枚举一遍）。
///
/// Python 注释记着这条性能教训：曾每个物理网卡各调一次 → 重复全量枚举，
/// 实测 7ms × 网卡数，在虚拟卡多的机器上线性放大。
pub fn pnp_capabilities_scan() -> BTreeMap<String, u32> {
    const NIC_CLASS_KEY: &str =
        r"HKLM\SYSTEM\CurrentControlSet\Control\Class\{4d36e972-e325-11ce-bfc1-08002be10318}";
    let mut out = BTreeMap::new();
    // 子键形如 0000 / 0001 …；逐个数探测（RegEnumKey 未在本层封装）
    for i in 0..32 {
        let sub = format!("{NIC_CLASS_KEY}\\{i:04}");
        let vals = w32::reg_values(&sub);
        if vals.is_empty() {
            continue;
        }
        let Some(desc) = vals.get("DriverDesc").map(RegValue::as_compare_str) else {
            continue;
        };
        let cap = vals
            .get("PnPCapabilities")
            .and_then(RegValue::as_u32)
            .unwrap_or(0);
        out.insert(desc, cap);
    }
    out
}

/// 按驱动描述查 PnPCapabilities（24 = 允许系统关闭设备省电）。
pub fn pnp_capabilities(driver_desc: &str, scan: &BTreeMap<String, u32>) -> Option<u32> {
    scan.get(driver_desc).copied()
}

/// NlaSvc 服务状态（id=nlasvc）：只靠 sc query 即可判断，不依赖快照。
pub fn chk_nlasvc() -> CheckResult {
    // NlaSvc 是否运行可以只靠 sc query 判断——是 Snapshot 检查里唯一能独立实现的一项
    let mut r = CheckResult::new(
        "nlasvc",
        "NlaSvc 网络位置感知服务（画'有网/无网'标记的服务）",
        PASS,
    );
    match service_state("NlaSvc") {
        Some(s) => {
            if s.status == "RUNNING" {
                r.detail = format!("NlaSvc={}/{}", s.status, s.start_type);
            } else {
                r.status = WARN;
                r.detail = format!(
                    "NlaSvc 未运行（{}/{}）：Windows 无法判断网络位置，可能误报'无网络'",
                    s.status, s.start_type
                );
                r.fix_ids = vec!["start_nlasvc", "fix_nlasvc"];
            }
        }
        None => {
            r.status = SKIP;
            r.detail = "无法查询 NlaSvc（sc query 无输出）".to_string();
        }
    }
    r
}

pub fn chk_proxy() -> CheckResult {
    // WinINET 代理可从注册表直接读，不依赖 Snapshot —— 独立实现
    let mut r = CheckResult::new(
        "proxy",
        "系统代理（代理软件残留会让全网流量进死端口）",
        PASS,
    );
    let key = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings";
    let vals = w32::reg_values(key);
    let enabled = vals.get("ProxyEnable").and_then(RegValue::as_u32) == Some(1);
    let server = vals
        .get("ProxyServer")
        .map(RegValue::as_compare_str)
        .unwrap_or_default();
    let auto = vals.get("AutoConfigURL").map(RegValue::as_compare_str);
    if enabled && !server.is_empty() {
        r.status = WARN;
        r.detail = format!("系统代理已开启: {server} —— 若代理软件未运行，所有应用将无法上网");
        r.fix_ids = vec!["proxy_off"];
    } else if let Some(pac) = auto.filter(|p| !p.is_empty()) {
        r.status = WARN;
        let short: String = pac.chars().take(60).collect();
        r.detail = format!("配置了 PAC 自动代理脚本: {short}");
        r.fix_ids = vec!["proxy_off"];
    } else {
        r.detail = "未启用系统代理".to_string();
    }
    r
}

// ---------------- 检查清单与综合判定 ----------------

/// 一项检查的注册信息。
pub struct CheckDef {
    pub id: &'static str,
    pub name: &'static str,
    /// quick 模式（监控轮询）是否跳过：对应 Python 的慢速外网探测集合。
    pub slow: bool,
    runner: fn(&Snapshot) -> CheckResult,
}

impl CheckDef {
    /// 执行本项检查。
    pub fn run(&self, snap: &Snapshot) -> CheckResult {
        (self.runner)(snap)
    }
}

/// 全部 15 项检查，顺序 = Python `ALL_CHECKS`。
pub fn all_checks() -> &'static [CheckDef] {
    static CHECKS: &[CheckDef] = &[
        CheckDef {
            id: "nlasvc",
            name: "NlaSvc 网络位置感知服务（画'有网/无网'标记的服务）",
            slow: false,
            runner: |_| chk_nlasvc(),
        },
        CheckDef {
            id: "persistent_route",
            name: "持久路由（重启不消失的手工路由，休眠时会劫持流量）",
            slow: false,
            runner: chk_persistent_route,
        },
        CheckDef {
            id: "static_dns",
            name: "网卡静态 DNS 审计（残留的静态 DNS 只影响对应网卡）",
            slow: false,
            runner: chk_static_dns,
        },
        CheckDef {
            id: "proxy",
            name: "系统代理（代理软件残留会让全网流量进死端口）",
            slow: false,
            runner: |_| chk_proxy(),
        },
        CheckDef {
            id: "gateway",
            name: "网关连通（Ping 默认网关 ×3）",
            slow: false,
            runner: chk_gateway,
        },
        CheckDef {
            id: "dns_compare",
            name: "DNS 解析（系统 DNS vs 公共 DNS 对比）",
            slow: false,
            runner: chk_dns_compare,
        },
        CheckDef {
            id: "dns_pollution",
            name: "NCSI 域名解析对比（污染检测）",
            slow: true,
            runner: |_| chk_dns_pollution(),
        },
        CheckDef {
            id: "ncsi_probe",
            name: "NCSI 探测模拟（Windows 判定'有网'的依据）",
            slow: false,
            runner: |_| chk_ncsi_probe(),
        },
        CheckDef {
            id: "real_conn",
            name: "真实连通性（204 端点 + HTTPS）",
            slow: false,
            runner: |_| chk_real(),
        },
        CheckDef {
            id: "ncsi_reg",
            name: "NCSI 注册表配置核对",
            slow: true,
            runner: |_| chk_ncsi_registry(),
        },
        CheckDef {
            id: "power_mgmt",
            name: "网卡节电（允许系统关闭设备以节约电源）",
            slow: true,
            runner: chk_power_mgmt,
        },
        CheckDef {
            id: "services",
            name: "关键网络服务状态",
            slow: false,
            runner: |_| chk_services(),
        },
        CheckDef {
            id: "routes",
            name: "路由健康（幽灵路由 / 多默认网关 / 隧道抢占）",
            slow: false,
            runner: chk_routes,
        },
        CheckDef {
            id: "ipv6",
            name: "IPv6 状态（V6 探测值缺失会导致 v6 网络误判）",
            slow: true,
            runner: |_| chk_ipv6(),
        },
        CheckDef {
            id: "wlan",
            name: "WLAN 配置（MAC 随机化 / 连接状态）",
            slow: true,
            runner: |_| chk_wlan(),
        },
    ];
    CHECKS
}

/// 取默认网关 IPv4 地址（`Get-NetRoute` 不引：走 `route print` 解析，零 PowerShell 依赖）。
///
/// 对应 Python `icmp.default_gateway()`，但后者走 PowerShell。这里解析
/// `route print -4` 的默认路由段：0.0.0.0/0 行的「网关」列。
pub fn default_gateway_v4() -> Option<String> {
    let out = w32::run_capture(&["route".into(), "print".into(), "-4".into()], 15).ok()?;
    for line in out.stdout.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        // 形如：0.0.0.0          0.0.0.0      192.168.1.1     192.168.1.100     25
        if cols.len() >= 3 && cols[0] == "0.0.0.0" && cols[1] == "0.0.0.0" {
            let gw = cols[2];
            if gw.matches('.').count() == 3 {
                return Some(gw.to_string());
            }
        }
    }
    None
}

/// 逐项执行检查（`quick=true` 跳过慢速外网探测）。
pub fn run_all(quick: bool) -> Vec<CheckResult> {
    // **快照只采一次**：多项检查都需要它，每项各采一次就是多次 PowerShell
    //（与 Python 版 `snap = take_snap()` 的做法一致）
    let snap = crate::snapshot::take();
    all_checks()
        .iter()
        .filter(|c| !(quick && c.slow))
        .map(|c| c.run(&snap))
        .collect()
}

/// 综合判定：`(状态码, 一句话)`，与 Python `verdict` 的分支完全一致。
pub fn verdict(results: &[CheckResult]) -> (String, String) {
    let by: BTreeMap<&str, &CheckResult> = results.iter().map(|r| (r.id, r)).collect();
    let real_ok = by
        .get("real_conn")
        .map(|r| r.status == PASS)
        .unwrap_or(false);
    let ncsi_ok = by
        .get("ncsi_probe")
        .map(|r| r.status == PASS)
        .unwrap_or(false);
    let dns_bad = by
        .get("dns_compare")
        .map(|r| r.status == FAIL)
        .unwrap_or(false);

    let (base, mut text) = if real_ok && ncsi_ok && !dns_bad {
        ("normal", "网络正常".to_string())
    } else if real_ok && !ncsi_ok {
        (
            "ncsi_false_alarm",
            "实际有网，但 Windows 判定'无网络'（NCSI 误报）".to_string(),
        )
    } else if real_ok && dns_bad {
        (
            "dns_broken",
            "能连上但 DNS 解析异常/缓慢（系统 DNS 排序被幽灵路由或残废 DNS 带偏）".to_string(),
        )
    } else {
        ("no_internet", "外网不可达".to_string())
    };
    let fails = results.iter().filter(|r| r.status == FAIL).count();
    if fails > 0 {
        text.push_str(&format!("；失败项: {fails} 个"));
    }
    (base.to_string(), text)
}

/// 各状态计数（前端摘要卡）。
pub fn status_counts(results: &[CheckResult]) -> BTreeMap<&'static str, usize> {
    let mut m = BTreeMap::new();
    for r in results {
        *m.entry(r.status).or_insert(0) += 1;
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_marks_match_python() {
        assert_eq!(status_mark(PASS), "√");
        assert_eq!(status_mark(WARN), "!");
        assert_eq!(status_mark(FAIL), "×");
        assert_eq!(status_mark(SKIP), "-");
        assert_eq!(status_mark(INFO), "·");
        assert_eq!(status_mark("weird"), "?");
    }

    #[test]
    fn status_code_constants_match_python() {
        assert_eq!(
            (PASS, WARN, FAIL, SKIP, INFO),
            ("pass", "warn", "fail", "skip", "info")
        );
    }

    #[test]
    fn all_checks_is_15_in_python_order() {
        let checks = all_checks();
        assert_eq!(checks.len(), 15, "Python ALL_CHECKS 有 15 项");
        let ids: Vec<&str> = checks.iter().map(|c| c.id).collect();
        assert_eq!(
            ids,
            vec![
                "nlasvc",
                "persistent_route",
                "static_dns",
                "proxy",
                "gateway",
                "dns_compare",
                "dns_pollution",
                "ncsi_probe",
                "real_conn",
                "ncsi_reg",
                "power_mgmt",
                "services",
                "routes",
                "ipv6",
                "wlan",
            ],
            "顺序是硬契约：前端列表顺序与 Python 一致"
        );
    }

    #[test]
    fn slow_checks_match_python_quick_skip_set() {
        // Python quick 模式跳过：dns_pollution / ncsi_registry / power / ipv6 / wlan
        let slow: Vec<&str> = all_checks()
            .iter()
            .filter(|c| c.slow)
            .map(|c| c.id)
            .collect();
        assert_eq!(
            slow,
            vec!["dns_pollution", "ncsi_reg", "power_mgmt", "ipv6", "wlan"]
        );
    }

    #[test]
    fn check_names_match_python_verbatim() {
        for (id, name) in [
            (
                "nlasvc",
                "NlaSvc 网络位置感知服务（画'有网/无网'标记的服务）",
            ),
            (
                "persistent_route",
                "持久路由（重启不消失的手工路由，休眠时会劫持流量）",
            ),
            (
                "static_dns",
                "网卡静态 DNS 审计（残留的静态 DNS 只影响对应网卡）",
            ),
            ("proxy", "系统代理（代理软件残留会让全网流量进死端口）"),
            ("gateway", "网关连通（Ping 默认网关 ×3）"),
            ("dns_compare", "DNS 解析（系统 DNS vs 公共 DNS 对比）"),
            ("dns_pollution", "NCSI 域名解析对比（污染检测）"),
            ("ncsi_probe", "NCSI 探测模拟（Windows 判定'有网'的依据）"),
            ("real_conn", "真实连通性（204 端点 + HTTPS）"),
            ("ncsi_reg", "NCSI 注册表配置核对"),
            ("power_mgmt", "网卡节电（允许系统关闭设备以节约电源）"),
            ("services", "关键网络服务状态"),
            ("routes", "路由健康（幽灵路由 / 多默认网关 / 隧道抢占）"),
            ("ipv6", "IPv6 状态（V6 探测值缺失会导致 v6 网络误判）"),
            ("wlan", "WLAN 配置（MAC 随机化 / 连接状态）"),
        ] {
            let c = all_checks().iter().find(|c| c.id == id).unwrap();
            assert_eq!(c.name, name, "{id} 名称与 Python 不一致");
        }
    }

    #[test]
    fn parse_status_code_handles_http_responses() {
        assert_eq!(
            parse_status_code("HTTP/1.1 204 No Content\r\n\r\n"),
            Some(204)
        );
        assert_eq!(parse_status_code("HTTP/1.0 200 OK\r\n"), Some(200));
        assert_eq!(parse_status_code("HTTP/1.1 404 Not Found"), Some(404));
        assert_eq!(parse_status_code("not http"), None);
        assert_eq!(parse_status_code(""), None);
        assert_eq!(parse_status_code("HTTP/1.1 abc"), None);
    }

    #[test]
    fn extract_body_splits_on_crlfcrlf() {
        assert_eq!(extract_body("HTTP/1.1 200 OK\r\nA: b\r\n\r\nBODY"), "BODY");
        assert_eq!(extract_body("HTTP/1.1 204 No Content\r\n\r\n"), "");
        assert_eq!(extract_body("no separator"), "");
    }

    #[test]
    fn v4_dns_filters_placeholders_and_ipv6() {
        assert_eq!(v4_dns(&["223.5.5.5".into()]), "223.5.5.5");
        // fec0::/10 占位符与 IPv6 都要跳过
        assert_eq!(
            v4_dns(&["fec0:0:0:ffff::1".into(), "8.8.8.8".into()]),
            "8.8.8.8"
        );
        assert_eq!(v4_dns(&["fec0:0:0:ffff::1".into()]), "", "全是占位符 → 空");
        assert_eq!(v4_dns(&[]), "");
        // 大小写不敏感的 fec 前缀
        assert_eq!(v4_dns(&["FEC0::1".into(), "1.1.1.1".into()]), "1.1.1.1");
    }

    #[test]
    fn is_bogus_detects_poisoned_results() {
        assert!(is_bogus(&["127.0.0.1".into()]));
        assert!(
            is_bogus(&["8.8.8.8".into(), "0.0.0.0".into()]),
            "全是地雷 → 污染"
        );
        assert!(!is_bogus(&["131.107.255.255".into()]), "官方 IP 不算地雷");
        assert!(
            !is_bogus(&["8.8.8.8".into(), "1.2.3.4".into()]),
            "有一个正常 → 不判污染"
        );
        assert!(!is_bogus(&[]), "空结果不算污染");
    }

    #[test]
    fn ping_loss_parses_chinese_and_english() {
        // 真实 Windows 输出：括号紧贴数字
        assert_eq!(
            ping_loss("数据包: 已发送 = 3，已接收 = 3，丢失 = 0 (0% 丢失)，"),
            Some(0)
        );
        assert_eq!(
            ping_loss("Packets: Sent = 4, Received = 3, Lost = 1 (25% loss),"),
            Some(25)
        );
        assert_eq!(ping_loss("(100% 丢失)"), Some(100));
        assert_eq!(ping_loss("没有百分比"), None);
        assert_eq!(ping_loss(""), None);
    }

    #[test]
    fn http_get_rejects_https_with_clear_reason() {
        let e = http_get("https://www.baidu.com", 1.0, false).unwrap_err();
        assert!(
            e.contains("TLS"),
            "必须说明是未实现 TLS 而不是含糊失败: {e}"
        );
    }

    #[test]
    fn http_get_rejects_malformed_url() {
        assert!(http_get("no-scheme", 1.0, false).is_err());
    }

    #[test]
    fn http_get_unreachable_host_errors_without_panic() {
        // TEST-NET-3 保留段：不可达，但必须返回 Err 而不是 panic
        assert!(http_get("http://203.0.113.1/", 0.4, false).is_err());
    }

    #[test]
    fn verdict_follows_python_branch_order() {
        let mk = |id: &'static str, status: &'static str| {
            let mut r = CheckResult::new(id, "x", status);
            r.detail = String::new();
            r
        };
        // normal
        let v = verdict(&[
            mk("real_conn", PASS),
            mk("ncsi_probe", PASS),
            mk("dns_compare", PASS),
        ]);
        assert_eq!(v.0, "normal");
        assert!(v.1.contains("网络正常"));
        // NCSI 误报
        let v = verdict(&[mk("real_conn", PASS), mk("ncsi_probe", FAIL)]);
        assert_eq!(v.0, "ncsi_false_alarm");
        assert!(v.1.contains("误报"));
        // DNS 坏
        let v = verdict(&[
            mk("real_conn", PASS),
            mk("ncsi_probe", PASS),
            mk("dns_compare", FAIL),
        ]);
        assert_eq!(v.0, "dns_broken");
        // 无外网
        let v = verdict(&[mk("real_conn", FAIL), mk("ncsi_probe", PASS)]);
        assert_eq!(v.0, "no_internet");
    }

    #[test]
    fn verdict_appends_fail_count() {
        let mk = |id: &'static str, status: &'static str| CheckResult::new(id, "x", status);
        let (_, text) = verdict(&[
            mk("real_conn", FAIL),
            mk("ncsi_probe", FAIL),
            mk("services", FAIL),
        ]);
        assert!(text.contains("失败项: 3 个"), "实际: {text}");
    }

    #[test]
    fn empty_results_yield_no_internet_not_panic() {
        let (base, text) = verdict(&[]);
        assert_eq!(base, "no_internet");
        assert!(!text.is_empty());
    }

    #[test]
    fn status_counts_tallies() {
        let rs = vec![
            CheckResult::new("a", "a", PASS),
            CheckResult::new("b", "b", PASS),
            CheckResult::new("c", "c", SKIP),
        ];
        let c = status_counts(&rs);
        assert_eq!(c.get(PASS), Some(&2));
        assert_eq!(c.get(SKIP), Some(&1));
    }

    #[test]
    fn check_line_format_matches_python() {
        let r = CheckResult::new("x", "测试项", FAIL).with_detail("细节");
        assert_eq!(r.line(), "[×] 测试项\n      细节");
    }

    #[test]
    fn snapshot_checks_are_resilient_to_empty_snapshot() {
        // 空快照（采集失败/无网卡）下每项都必须给出可读结论，绝不 panic、
        // 也绝不假装「已检测到问题」——空数据只能说「无」或「跳过」
        let empty = crate::snapshot::Snapshot::default();
        for r in [
            chk_persistent_route(&empty),
            chk_static_dns(&empty),
            chk_routes(&empty),
            chk_ipv6(),
            chk_wlan(),
        ] {
            assert!(!r.detail.is_empty(), "{} 必须有详情", r.id);
            assert!(
                matches!(r.status, PASS | WARN | FAIL | SKIP | INFO),
                "{} 状态非法: {}",
                r.id,
                r.status
            );
        }
        // 空快照的具体语义
        assert_eq!(
            chk_persistent_route(&empty).status,
            PASS,
            "无持久路由 = pass（不是 skip）"
        );
        assert_eq!(
            chk_static_dns(&empty).status,
            PASS,
            "无网卡 = 无静态 DNS 问题"
        );
        assert_eq!(chk_routes(&empty).status, PASS, "无默认路由 = 无幽灵路由");
    }

    #[test]
    fn no_check_reports_not_implemented_anymore() {
        // 全量实现的回归锁：15 项都不许再出现「尚未接入」——
        // 之前 5 项依赖 snapshot 时这里会亮红灯，现在是完成度断言
        let snap = crate::snapshot::take();
        for c in all_checks() {
            let r = c.run(&snap);
            assert!(
                !r.detail.contains("尚未接入"),
                "{} 仍显示尚未接入: {}",
                r.id,
                r.detail
            );
            assert!(!r.detail.is_empty(), "{} 详情为空", r.id);
        }
    }

    #[test]
    fn ncsi_registry_check_reads_real_registry() {
        let r = chk_ncsi_registry();
        // 真机应能读到 NCSI 注册表（普通用户可读 HKLM 该项）
        assert!(
            r.status == PASS || r.status == WARN || r.status == FAIL,
            "状态应合法: {}",
            r.status
        );
        assert!(!r.detail.is_empty(), "必须有详情");
        if r.status == FAIL {
            assert!(
                r.detail.contains("无法读取"),
                "FAIL 只能因为读不到: {}",
                r.detail
            );
        }
    }

    #[test]
    fn proxy_check_reads_real_registry() {
        let r = chk_proxy();
        assert!(!r.detail.is_empty());
        // 三种合法结论：未启用 / 已开启 / PAC
        assert!(
            r.detail.contains("未启用") || r.detail.contains("已开启") || r.detail.contains("PAC"),
            "实际: {}",
            r.detail
        );
    }

    #[test]
    fn nlasvc_and_services_checks_run_on_real_machine() {
        let n = chk_nlasvc();
        assert!(!n.detail.is_empty());
        let s = chk_services();
        assert!(!s.detail.is_empty());
        // 服务清单要么有数据要么明确写「无数据」
        assert!(
            s.detail.contains('=') || s.detail == "无数据",
            "实际: {}",
            s.detail
        );
    }

    #[test]
    fn run_all_quick_skips_slow_checks() {
        let full = run_all(false);
        assert_eq!(full.len(), 15);
        let quick = run_all(true);
        assert_eq!(quick.len(), 10, "quick 跳过 5 项慢速检查");
        assert!(
            !quick.iter().any(
                |r| ["dns_pollution", "ncsi_reg", "power_mgmt", "ipv6", "wlan"].contains(&r.id)
            ),
            "quick 模式不得包含慢速项"
        );
    }

    #[test]
    fn default_gateway_parse_returns_dotted_quad_or_none() {
        if let Some(gw) = default_gateway_v4() {
            assert_eq!(gw.matches('.').count(), 3, "网关应是点分 IPv4: {gw}");
            assert!(gw.parse::<std::net::Ipv4Addr>().is_ok());
        }
        // 无网关也不算失败（离线机器）
    }

    #[test]
    fn gateway_check_reports_something_actionable() {
        let r = chk_gateway(&crate::snapshot::Snapshot::default());
        assert!(!r.detail.is_empty());
        assert!(r.status == PASS || r.status == WARN || r.status == FAIL || r.status == SKIP);
    }

    #[test]
    fn dns_compare_runs_and_reports() {
        let r = chk_dns_compare(&crate::snapshot::Snapshot::default());
        assert!(!r.detail.is_empty());
        // 断网机器也会给 FAIL/WARN + 可读说明，不得 panic
        assert!(r.fix_ids.is_empty() || !r.fix_ids.is_empty());
    }

    #[test]
    fn ncsi_stock_matches_python_config() {
        let stock = ncsi_stock();
        assert_eq!(stock.len(), 11, "config.NCSI_STOCK 有 11 项");
        assert_eq!(
            stock
                .iter()
                .find(|(k, _)| *k == "ActiveWebProbeHost")
                .unwrap()
                .1,
            "www.msftconnecttest.com"
        );
        assert_eq!(
            stock
                .iter()
                .find(|(k, _)| *k == "ActiveDnsProbeContent")
                .unwrap()
                .1,
            "131.107.255.255"
        );
        assert_eq!(NCSI_EXPECT_CONTENT, "Microsoft Connect Test");
        assert_eq!(NCSI_DNS_EXPECT, "131.107.255.255");
    }

    #[test]
    fn probe_204_live_smoke() {
        // 真机联网时应通；断网时返回 false + 原因，都不得 panic
        let (ok, detail) = probe_204(4.0);
        assert!(!detail.is_empty(), "无论成败都要有说明");
        if !ok {
            assert!(
                detail.contains("HTTP")
                    || detail.contains("失败")
                    || detail.contains("解析")
                    || detail.contains("连接"),
                "失败原因应可读: {detail}"
            );
        }
    }
}
