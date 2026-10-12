//! 探测与纯逻辑辅助：HTTP 直连探测 / NCSI 模拟 / DNS 与服务判定。
use super::*;

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
///
/// 语义对齐（深度审计轮31修正）：Python 版逐个**解析分类**——任一 IP 解析
/// 失败或落在 loopback/unspecified/private/reserved/link-local 即判污染
/// （any 语义）。旧 Rust 版退化成「全体 ∈ 静态 8 项地雷表」的 all() 判定，
/// 两类真实污染随后者漏判：
/// - 混合应答（官方 IP 夹带 127.0.0.1）：Python WARN，旧版 all() 不过 →
///   下游 any(==官方期望) 反而放行成 PASS；
/// - 私网/保留段应答（如 192.168.x.x）、v6 ULA/链路本地：不在静态表内 → 漏判；
/// - 非法应答（解析失败）：Python ValueError→True，旧版 contains 永假 → 漏判。
///
/// 静态清单判定已整体移除（Python 无此清单，公共 IP 出现在应答里按
/// 「非污染的异常应答」走 expect-IP/INFO 路径，与 Python 同口径）。
pub fn is_bogus(ips: &[String]) -> bool {
    use std::net::IpAddr;
    // Ipv4Addr::is_reserved 在本工具链仍是 unstable 特性（E0658），手工判
    // 240.0.0.0/4（与 Python ipaddress 的 is_reserved 口径一致）。
    let v4_bogus = |v4: std::net::Ipv4Addr| {
        let o = v4.octets();
        v4.is_loopback()
            || v4.is_unspecified()
            || v4.is_private()
            || v4.is_link_local()
            || (o[0] & 0xf0) == 0xf0
    };
    ips.iter().any(|ip| match ip.parse::<IpAddr>() {
        Ok(IpAddr::V4(v4)) => v4_bogus(v4),
        Ok(IpAddr::V6(v6)) => {
            // IPv4-mapped（::ffff:a.b.c.d）按映射的 v4 分类（Python
            // ipaddress 对 mapped 私网地址 is_private 同样为 True）
            if let Some(v4) = v6.to_ipv4_mapped() {
                return v4_bogus(v4);
            }
            v6.is_loopback()
                || v6.is_unspecified()
                || (v6.segments()[0] & 0xfe00) == 0xfc00    // fc00::/7 ULA
                || (v6.segments()[0] & 0xffc0) == 0xfe80 // fe80::/10 链路本地
        }
        Err(_) => true, // 解析失败=非法应答（Python ValueError→True 同口径）
    })
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

