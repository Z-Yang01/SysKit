//! 端口/连接与局域网邻居解析（对应 Python 侧 toolbox_page 的 PortTab / LanTab）。
//!
//! 本模块**只做解析与判定（纯函数）**，取数由调用方注入——`netstat -ano` 与
//! `arp -a` 的文本格式跨 Windows 版本稳定，是这两个功能最可靠的来源；
//! 把它们做成纯函数，就能用固定样本断言每一种真实行形态。

/// `netstat -ano` 的一行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortRow {
    pub proto: String,
    pub local: String,
    pub remote: String,
    pub state: String,
    pub pid: u32,
}

impl PortRow {
    /// 是否处于监听态（TCP LISTENING / UDP 无状态也算监听）。
    pub fn is_listening(&self) -> bool {
        self.state.is_empty() || self.state.eq_ignore_ascii_case("LISTENING")
    }

    /// 本地端口号（取最后一个冒号后的数字；IPv6 也适用）。
    pub fn local_port(&self) -> Option<u16> {
        port_of(&self.local)
    }

    /// 复合过滤：关键字同时匹配端口/PID/地址/状态；`listen_only` 只留监听态。
    pub fn matches(&self, keyword: &str, listen_only: bool, proc_name: &str) -> bool {
        if listen_only && !self.is_listening() {
            return false;
        }
        let kw = keyword.trim().to_lowercase();
        if kw.is_empty() {
            return true;
        }
        let haystack = format!(
            "{} {} {} {} {} {}",
            self.local_port().map(|p| p.to_string()).unwrap_or_default(),
            self.pid,
            self.local,
            self.remote,
            self.state,
            proc_name
        )
        .to_lowercase();
        haystack.contains(&kw)
    }
}

/// 从 `地址:端口` 里取端口（兼容 `[::]:80`、`0.0.0.0:135`、`*:*`）。
pub fn port_of(addr: &str) -> Option<u16> {
    let tail = addr.rsplit(':').next()?;
    tail.parse::<u16>().ok()
}

/// 解析 `netstat -ano` 输出。
///
/// 真实行形如（列宽不定，用空白切分即可）：
/// ```text
///   TCP    0.0.0.0:135            0.0.0.0:0              LISTENING       1234
///   TCP    [::]:445               [::]:0                 LISTENING       4
///   UDP    0.0.0.0:5353           *:*                                    5678
/// ```
/// UDP 行没有状态列（少一列），必须按列数分支处理——这是最容易写错的地方。
pub fn parse_netstat(text: &str) -> Vec<PortRow> {
    let mut out = Vec::new();
    for line in text.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 4 {
            continue;
        }
        let proto = cols[0].to_uppercase();
        if proto != "TCP" && proto != "UDP" {
            continue; // 表头与分隔线
        }
        let (local, remote, state, pid_str) = if cols.len() >= 5 {
            // TCP：协议 本地 远程 状态 PID
            (cols[1], cols[2], cols[3], cols[4])
        } else {
            // UDP：协议 本地 远程 PID
            (cols[1], cols[2], "", cols[3])
        };
        let Ok(pid) = pid_str.parse::<u32>() else {
            continue;
        };
        out.push(PortRow {
            proto,
            local: local.to_string(),
            remote: remote.to_string(),
            state: state.to_string(),
            pid,
        });
    }
    out
}

/// 解析 `tasklist /fo csv` 的 `PID → 进程名` 映射。
///
/// 形如 `"chrome.exe","1234","Console","1","100,000 K"`；进程名可能含逗号，
/// 所以只按引号切分、不做裸 split(',')。
pub fn parse_tasklist_csv(text: &str) -> std::collections::BTreeMap<u32, String> {
    let mut out = std::collections::BTreeMap::new();
    for line in text.lines() {
        let fields = split_csv_quoted(line);
        if fields.len() < 2 {
            continue;
        }
        if let Ok(pid) = fields[1].trim().parse::<u32>() {
            let name = fields[0].trim().to_string();
            if !name.is_empty() {
                out.insert(pid, name);
            }
        }
    }
    out
}

/// 按 CSV 引号规则切分一行（支持 `""` 转义）。
pub fn split_csv_quoted(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                if in_quotes && chars.peek() == Some(&'"') {
                    cur.push('"');
                    chars.next();
                } else {
                    in_quotes = !in_quotes;
                }
            }
            ',' if !in_quotes => {
                out.push(std::mem::take(&mut cur));
            }
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

// ---------------- 局域网邻居（arp -a） ----------------

/// 一条 ARP 邻居。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArpEntry {
    pub ip: String,
    pub mac: String,
    /// `动态` / `静态`（保留原文，便于原样展示）
    pub kind: String,
}

/// 解析 `arp -a` 输出。
///
/// 真实输出分「接口: 192.168.1.100 --- 0x5」段落，行形如：
/// `  192.168.1.1           xx-xx-xx-xx-xx-xx     动态`
/// 也会出现 `  224.0.0.22            01-00-5e-00-00-16     静态`（组播）与
/// 广播地址——调用方可用 [`is_useful_neighbor`] 过滤。
pub fn parse_arp(text: &str) -> Vec<ArpEntry> {
    let mut out = Vec::new();
    for line in text.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 2 {
            continue;
        }
        let ip = cols[0];
        if ip.parse::<std::net::Ipv4Addr>().is_err() {
            continue; // 跳过「接口:」标题行与表头
        }
        let mac = cols[1];
        if !looks_like_mac(mac) {
            continue;
        }
        let kind = cols.get(2).map(|s| s.to_string()).unwrap_or_default();
        out.push(ArpEntry {
            ip: ip.to_string(),
            mac: mac.to_lowercase(),
            kind,
        });
    }
    out
}

/// MAC 形态校验：`xx-xx-xx-xx-xx-xx` 或全零（无效）。
pub fn looks_like_mac(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 6
        && parts
            .iter()
            .all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// 该邻居是否值得展示：排除组播（224.0.0.0/4）、广播、以及全零/组播 MAC。
pub fn is_useful_neighbor(e: &ArpEntry) -> bool {
    let Ok(ip) = e.ip.parse::<std::net::Ipv4Addr>() else {
        return false;
    };
    if ip.is_multicast() || ip.is_broadcast() || ip.is_unspecified() {
        return false;
    }
    // 组播 MAC 首字节最低位为 1（01-00-5e-…）
    if let Some(first) = e.mac.split('-').next() {
        if let Ok(b) = u8::from_str_radix(first, 16) {
            if b & 0x01 != 0 {
                return false;
            }
        }
    }
    e.mac != "00-00-00-00-00-00" && e.mac != "ff-ff-ff-ff-ff-ff"
}

/// 推断本机所在的 /24 网段前缀（如 `192.168.1`）；无法推断返回 None。
pub fn subnet24_of(ip: &str) -> Option<String> {
    let v4: std::net::Ipv4Addr = ip.parse().ok()?;
    let o = v4.octets();
    Some(format!("{}.{}.{}", o[0], o[1], o[2]))
}

/// 生成 /24 网段的全部主机地址（1..=254）——批量扫描的输入。
pub fn hosts_in_subnet24(prefix: &str) -> Vec<String> {
    (1..=254).map(|i| format!("{prefix}.{i}")).collect()
}

/// 解析 `ping` 输出的延迟毫秒（中英双语，`<1ms` 记 0）。
pub fn parse_ping_ms(out: &str) -> Option<u32> {
    for token in out.split_whitespace() {
        let t = token.trim();
        // 中文：时间<1ms / 时间=3ms ；英文：time<1ms / time=3ms
        let lower = t.to_lowercase();
        if lower.contains("ms") || t.contains("ms") {
            if let Some(rest) = lower.split(['=', '<']).nth(1) {
                let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                if !digits.is_empty() {
                    return digits.parse().ok();
                }
            }
        }
    }
    None
}

/// 解析 `tracert` 输出的一跳。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TraceHop {
    pub hop: u32,
    /// 三次探测的延迟（None = 该次超时）
    pub latencies: Vec<Option<u32>>,
    /// 主机名/地址；全超时为空
    pub host: String,
}

impl TraceHop {
    /// 该跳是否全超时。
    pub fn timed_out(&self) -> bool {
        self.latencies.iter().all(|l| l.is_none())
    }

    /// 最小延迟（用于展示）。
    pub fn min_latency(&self) -> Option<u32> {
        self.latencies.iter().flatten().copied().min()
    }
}

/// 解析 `tracert` 输出。
///
/// 真实行形如：
/// ```text
///   1     1 ms     1 ms     1 ms  192.168.1.1
///   2     *        *        *     请求超时。
///   3    12 ms     *       11 ms  www.example.com [93.184.216.34]
/// ```
/// 全超时行的主机部分是本地化的「请求超时。」——**不能**把它当主机名。
pub fn parse_tracert(text: &str) -> Vec<TraceHop> {
    let mut out = Vec::new();
    for line in text.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() < 5 {
            continue;
        }
        let Ok(hop) = cols[0].parse::<u32>() else {
            continue; // 表头「通过最多 30 个跃点跟踪」等
        };
        // 三个延迟段：`N ms` / `<1 ms` / `*`
        let mut latencies: Vec<Option<u32>> = Vec::new();
        let mut idx = 1usize;
        while idx < cols.len() && latencies.len() < 3 {
            let c = cols[idx];
            if c == "*" {
                latencies.push(None);
                idx += 1;
                continue;
            }
            let numeric = c.trim_start_matches('<');
            if let Ok(ms) = numeric.parse::<u32>() {
                latencies.push(Some(ms));
                idx += 1;
                if idx < cols.len() && cols[idx].eq_ignore_ascii_case("ms") {
                    idx += 1;
                }
                continue;
            }
            break;
        }
        if latencies.len() < 3 {
            continue;
        }
        let host = normalize_trace_host(&cols[idx..].join(" "));
        out.push(TraceHop {
            hop,
            latencies,
            host,
        });
    }
    out
}

/// 归一化跳点主机字段：全超时（本地化「请求超时。」等）→ 空串。
pub fn normalize_trace_host(raw: &str) -> String {
    let t = raw.trim();
    if t.is_empty() {
        return String::new();
    }
    let lower = t.to_lowercase();
    if lower.contains("请求超时") || lower.contains("request timed out") || lower.contains("超时")
    {
        return String::new();
    }
    t.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NETSTAT_TCP: &str =
        "  TCP    0.0.0.0:135            0.0.0.0:0              LISTENING       1234\n";
    const NETSTAT_UDP: &str =
        "  UDP    0.0.0.0:5353           *:*                                    5678\n";

    #[test]
    fn parse_tcp_row() {
        let rows = parse_netstat(NETSTAT_TCP);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].proto, "TCP");
        assert_eq!(rows[0].local, "0.0.0.0:135");
        assert_eq!(rows[0].state, "LISTENING");
        assert_eq!(rows[0].pid, 1234);
        assert!(rows[0].is_listening());
        assert_eq!(rows[0].local_port(), Some(135));
    }

    #[test]
    fn parse_udp_row_has_no_state() {
        // UDP 少一列：按列数分支，不能把 PID 当状态
        let rows = parse_netstat(NETSTAT_UDP);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].proto, "UDP");
        assert_eq!(rows[0].state, "", "UDP 没有状态列");
        assert_eq!(rows[0].pid, 5678, "PID 必须取到第 4 列");
        assert!(rows[0].is_listening(), "UDP 无状态视为监听");
    }

    #[test]
    fn parse_ignores_headers_and_garbage() {
        let text = "\n活动连接\n\n  协议  本地地址          外部地址        状态           PID\n  TCP   ...\n乱码行\n";
        let rows = parse_netstat(text);
        // 只有合法 TCP/UDP 行会进结果
        assert!(
            rows.iter().all(|r| r.proto == "TCP" || r.proto == "UDP"),
            "{rows:?}"
        );
    }

    #[test]
    fn parse_handles_ipv6_local_address() {
        let text = "  TCP    [::]:445               [::]:0                 LISTENING       4\n";
        let rows = parse_netstat(text);
        assert_eq!(rows[0].local, "[::]:445");
        assert_eq!(rows[0].local_port(), Some(445));
        assert_eq!(rows[0].pid, 4);
    }

    #[test]
    fn parse_established_connection() {
        let text = "  TCP    192.168.1.100:52344    93.184.216.34:443      ESTABLISHED     9876\n";
        let rows = parse_netstat(text);
        assert_eq!(rows[0].state, "ESTABLISHED");
        assert!(!rows[0].is_listening());
    }

    #[test]
    fn parse_rejects_non_numeric_pid() {
        let text = "  TCP    0.0.0.0:80    0.0.0.0:0    LISTENING    abc\n";
        assert!(parse_netstat(text).is_empty(), "PID 非数字应丢弃该行");
    }

    #[test]
    fn port_of_variants() {
        assert_eq!(port_of("0.0.0.0:135"), Some(135));
        assert_eq!(port_of("[::]:445"), Some(445));
        assert_eq!(port_of("*:*"), None);
        assert_eq!(port_of("0.0.0.0:0"), Some(0));
        assert_eq!(port_of("nonsense"), None);
        assert_eq!(port_of(""), None);
    }

    #[test]
    fn matches_filters_by_port_pid_and_proc() {
        let row = PortRow {
            proto: "TCP".into(),
            local: "0.0.0.0:135".into(),
            remote: "0.0.0.0:0".into(),
            state: "LISTENING".into(),
            pid: 1234,
        };
        assert!(row.matches("", false, "svchost.exe"));
        assert!(row.matches("135", false, ""), "按端口");
        assert!(row.matches("1234", false, ""), "按 PID");
        assert!(row.matches("svchost", false, "svchost.exe"), "按进程名");
        assert!(row.matches("LISTENING", false, ""), "按状态");
        assert!(!row.matches("9999", false, ""), "不匹配则 false");
        // listen_only 过滤非监听行
        let est = PortRow {
            state: "ESTABLISHED".into(),
            ..row.clone()
        };
        assert!(!est.matches("", true, ""), "只看监听时 ESTABLISHED 被排除");
        assert!(est.matches("", false, ""));
    }

    #[test]
    fn matches_is_case_insensitive() {
        let row = PortRow {
            proto: "TCP".into(),
            local: "0.0.0.0:135".into(),
            remote: "0.0.0.0:0".into(),
            state: "LISTENING".into(),
            pid: 1,
        };
        assert!(row.matches("SVCHOST", false, "svchost.exe"));
        assert!(row.matches("listening", false, ""));
    }

    #[test]
    fn tasklist_csv_maps_pid_to_name() {
        let text = "\"映像名称\",\"PID\",\"会话名\",\"会话#\",\"内存使用\"\n\"chrome.exe\",\"1234\",\"Console\",\"1\",\"100,000 K\"\n\"svchost.exe\",\"4\",\"Services\",\"0\",\"5,000 K\"\n";
        let m = parse_tasklist_csv(text);
        assert_eq!(m.get(&1234).map(String::as_str), Some("chrome.exe"));
        assert_eq!(m.get(&4).map(String::as_str), Some("svchost.exe"));
        assert_eq!(m.len(), 2, "表头行不该被当成进程");
    }

    #[test]
    fn split_csv_quoted_handles_commas_and_escapes() {
        assert_eq!(
            split_csv_quoted("\"a,b\",\"c\""),
            vec!["a,b".to_string(), "c".to_string()],
            "引号内的逗号不切分"
        );
        assert_eq!(
            split_csv_quoted("\"say \"\"hi\"\"\",x"),
            vec!["say \"hi\"".to_string(), "x".to_string()],
            "双引号转义"
        );
        assert_eq!(split_csv_quoted("plain"), vec!["plain".to_string()]);
    }

    // ---------------- ARP ----------------

    const ARP_SAMPLE: &str = "\n接口: 192.168.1.100 --- 0x5\n  Internet 地址         物理地址              类型\n  192.168.1.1           aa-bb-cc-dd-ee-ff     动态\n  192.168.1.20          11-22-33-44-55-66     动态\n  224.0.0.22            01-00-5e-00-00-16     静态\n  255.255.255.255       ff-ff-ff-ff-ff-ff     静态\n";

    #[test]
    fn parse_arp_extracts_neighbors() {
        let e = parse_arp(ARP_SAMPLE);
        assert_eq!(e.len(), 4, "四条形态都应被解析出来");
        assert_eq!(e[0].ip, "192.168.1.1");
        assert_eq!(e[0].mac, "aa-bb-cc-dd-ee-ff");
        assert_eq!(e[0].kind, "动态");
        // 标题行与表头行不得混入
        assert!(e.iter().all(|x| x.ip.parse::<std::net::Ipv4Addr>().is_ok()));
    }

    #[test]
    fn parse_arp_lowercases_mac() {
        let e = parse_arp("  10.0.0.1  AA-BB-CC-DD-EE-FF  动态\n");
        assert_eq!(e[0].mac, "aa-bb-cc-dd-ee-ff");
    }

    #[test]
    fn parse_arp_skips_malformed() {
        // 无效 MAC / 非 IP 首列 / 列数不足
        assert!(parse_arp("  10.0.0.1  not-a-mac  动态\n").is_empty());
        assert!(parse_arp("  接口: 1.2.3.4 --- 0x5\n").is_empty());
        assert!(parse_arp("  10.0.0.1\n").is_empty());
        assert!(parse_arp("").is_empty());
    }

    #[test]
    fn looks_like_mac_validates_shape() {
        assert!(looks_like_mac("aa-bb-cc-dd-ee-ff"));
        assert!(looks_like_mac("00-11-22-33-44-55"));
        assert!(
            !looks_like_mac("aa:bb:cc:dd:ee:ff"),
            "冒号形态不算（arp 用横线）"
        );
        assert!(!looks_like_mac("aa-bb-cc-dd-ee"));
        assert!(!looks_like_mac("gg-bb-cc-dd-ee-ff"), "非十六进制");
        assert!(!looks_like_mac(""));
    }

    #[test]
    fn is_useful_neighbor_filters_multicast_and_broadcast() {
        let good = ArpEntry {
            ip: "192.168.1.1".into(),
            mac: "aa-bb-cc-dd-ee-ff".into(),
            kind: "动态".into(),
        };
        assert!(is_useful_neighbor(&good));
        for bad_ip in [
            "224.0.0.22",
            "239.255.255.250",
            "255.255.255.255",
            "0.0.0.0",
        ] {
            let e = ArpEntry {
                ip: bad_ip.into(),
                mac: "01-00-5e-00-00-16".into(),
                kind: "静态".into(),
            };
            assert!(!is_useful_neighbor(&e), "{bad_ip} 应被过滤");
        }
        // 组播 MAC 但单播 IP：按 MAC 过滤
        let mixed = ArpEntry {
            ip: "192.168.1.5".into(),
            mac: "01-00-5e-00-00-16".into(),
            kind: "静态".into(),
        };
        assert!(!is_useful_neighbor(&mixed));
        // 全零/广播 MAC
        for mac in ["00-00-00-00-00-00", "ff-ff-ff-ff-ff-ff"] {
            let e = ArpEntry {
                ip: "192.168.1.9".into(),
                mac: mac.into(),
                kind: "静态".into(),
            };
            assert!(!is_useful_neighbor(&e), "{mac} 应被过滤");
        }
    }

    #[test]
    fn subnet24_helpers() {
        assert_eq!(subnet24_of("192.168.1.100").as_deref(), Some("192.168.1"));
        assert_eq!(subnet24_of("10.0.0.1").as_deref(), Some("10.0.0"));
        assert_eq!(subnet24_of("not-an-ip"), None);
        let hosts = hosts_in_subnet24("192.168.1");
        assert_eq!(hosts.len(), 254);
        assert_eq!(hosts[0], "192.168.1.1");
        assert_eq!(hosts[253], "192.168.1.254");
    }

    #[test]
    fn parse_ping_ms_reads_both_languages() {
        assert_eq!(
            parse_ping_ms("来自 192.168.1.1 的回复: 字节=32 时间=3ms TTL=64"),
            Some(3)
        );
        assert_eq!(
            parse_ping_ms("来自 192.168.1.1 的回复: 字节=32 时间<1ms TTL=64"),
            Some(1)
        );
        assert_eq!(
            parse_ping_ms("Reply from 192.168.1.1: bytes=32 time=12ms TTL=64"),
            Some(12)
        );
        assert_eq!(parse_ping_ms("请求超时。"), None);
        assert_eq!(parse_ping_ms(""), None);
    }
}

#[cfg(test)]
mod trace_tests {
    use super::*;

    const SAMPLE: &str = "\n通过最多 30 个跃点跟踪\n到 93.184.216.34 的路由:\n\n  1     1 ms     1 ms     1 ms  192.168.1.1\n  2     *        *        *     请求超时。\n  3    12 ms     *       11 ms  www.example.com [93.184.216.34]\n\n跟踪完成。\n";

    #[test]
    fn parses_hops_with_latencies_and_host() {
        let hops = parse_tracert(SAMPLE);
        assert_eq!(hops.len(), 3, "三跳都应有记录（含全超时那跳）");
        assert_eq!(hops[0].hop, 1);
        assert_eq!(hops[0].latencies, vec![Some(1), Some(1), Some(1)]);
        assert_eq!(hops[0].host, "192.168.1.1");
        assert!(!hops[0].timed_out());
        assert_eq!(hops[0].min_latency(), Some(1));
    }

    #[test]
    fn all_timeout_hop_keeps_empty_host() {
        let hops = parse_tracert(SAMPLE);
        let t = &hops[1];
        assert_eq!(t.hop, 2);
        assert_eq!(t.latencies, vec![None, None, None]);
        assert!(t.timed_out());
        assert_eq!(t.host, "", "「请求超时。」不得被当成主机名");
        assert_eq!(t.min_latency(), None);
    }

    #[test]
    fn partial_timeout_keeps_latencies_and_host() {
        let hops = parse_tracert(SAMPLE);
        let h = &hops[2];
        assert_eq!(h.latencies, vec![Some(12), None, Some(11)]);
        assert!(!h.timed_out());
        assert_eq!(h.min_latency(), Some(11));
        assert!(h.host.contains("www.example.com"));
        assert!(h.host.contains("93.184.216.34"));
    }

    #[test]
    fn ignores_headers_and_footer() {
        let hops = parse_tracert(SAMPLE);
        assert!(hops.iter().all(|h| h.hop >= 1 && h.latencies.len() == 3));
        assert!(parse_tracert("跟踪完成。").is_empty());
        assert!(parse_tracert("").is_empty());
    }

    #[test]
    fn parses_english_output_and_less_than_1ms() {
        let text = "  1    <1 ms    <1 ms    <1 ms  192.168.1.1\n  2     5 ms     5 ms     5 ms  gateway.local\n";
        let hops = parse_tracert(text);
        assert_eq!(hops.len(), 2, "英文/<1ms 形态应可解析: {hops:?}");
        assert_eq!(hops[0].hop, 1);
        assert_eq!(
            hops[0].latencies,
            vec![Some(1), Some(1), Some(1)],
            "<1ms 记为 1"
        );
        assert_eq!(hops[0].host, "192.168.1.1");
        assert_eq!(hops[1].host, "gateway.local");
    }

    #[test]
    fn normalize_trace_host_variants() {
        assert_eq!(normalize_trace_host("请求超时。"), "");
        assert_eq!(normalize_trace_host("Request timed out."), "");
        assert_eq!(normalize_trace_host("  "), "");
        assert_eq!(normalize_trace_host("192.168.1.1"), "192.168.1.1");
        assert_eq!(normalize_trace_host(" a.com [1.2.3.4] "), "a.com [1.2.3.4]");
    }
}
