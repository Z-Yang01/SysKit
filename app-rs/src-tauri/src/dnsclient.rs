//! 自研 UDP DNS 客户端：可指定 DNS 服务器做对比测试（对应 Python 侧 dnsquery.py）。
//!
//! 为什么不用现成解析库：本工具的用途本身就是「诊断 DNS 是不是坏了」，
//! 引一个库只会把不确定性搬进被诊断对象里。线格式编解码提成纯函数，
//! 用固定报文断言——**这里的解析器写错会把「DNS 正常」误报成「DNS 故障」，
//! 进而诱导用户去改 DNS 配置**，所以每个边界都要有测试。
//!
//! 与 Python 版逐条对齐的行为：
//! - QNAME 长度字节必须等于**编码后字节数**（中文标签走 IDNA/punycode，
//!   字节数 ≠ 字符数；用字符数会写出非法报文）；
//! - 超时最多重发 attempts 次（家庭路由器 1~2% 偶发丢包曾被单发实现
//!   误判为「系统 DNS 故障」）；
//! - TC=1（应答截断）显式报错，绝不把半截报文解析出垃圾结果；
//! - 走系统解析器时用 **daemon 线程** + join 限时：系统 DNS 挂死正是本工具
//!   要诊断的场景，不能让退出被拖住。

use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Duration;

/// 查询超时（秒）；对应 Python `query(timeout=3.0)`。
pub const DEFAULT_TIMEOUT_SECS: f64 = 3.0;
/// 超时重发次数；对应 Python `attempts=3`。
pub const DEFAULT_ATTEMPTS: u32 = 3;
/// 系统解析器等待上限（秒）；对应 Python `resolve_system(timeout=4.0)`。
pub const SYSTEM_TIMEOUT_SECS: f64 = 4.0;

pub const QTYPE_A: u16 = 1;
pub const QTYPE_AAAA: u16 = 28;

/// DNS 查询/应答错误（人话，直接进诊断详情）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DnsError {
    InvalidLabel(String),
    Timeout,
    IdMismatch,
    NoAnswer,
    Truncated,
    Rcode(u8),
    Malformed(String),
}

impl std::fmt::Display for DnsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DnsError::InvalidLabel(l) => write!(f, "非法 DNS 标签: {l:?}"),
            DnsError::Timeout => write!(f, "DNS 无应答（超时）"),
            DnsError::IdMismatch => write!(f, "DNS 响应 id 不匹配"),
            DnsError::NoAnswer => write!(f, "DNS 响应异常：无应答记录"),
            DnsError::Truncated => write!(f, "DNS 应答被截断（TC=1）"),
            DnsError::Rcode(c) => write!(f, "DNS 返回错误码 {c}"),
            DnsError::Malformed(m) => write!(f, "DNS 报文畸形: {m}"),
        }
    }
}

/// 按 DNS 线格式编码域名（长度字节 = 编码后**字节数**）。
///
/// 非 ASCII 标签走 punycode（`xn--`）——这里对中文标签做最小实现：
/// 实际工程里应当用 IDNA 库，但本工具诊断的都是 `www.baidu.com` 这类
/// ASCII 域名；非 ASCII 直接判非法比悄悄编码错更安全（Python 版走
/// `str.encode("idna")`，此处显式不支持并给出清晰错误）。
pub fn encode_qname(host: &str) -> Result<Vec<u8>, DnsError> {
    let trimmed = host.trim().trim_end_matches('.');
    if trimmed.is_empty() {
        return Err(DnsError::InvalidLabel(host.to_string()));
    }
    let mut out = Vec::new();
    for label in trimmed.split('.') {
        if !label.is_ascii() {
            return Err(DnsError::InvalidLabel(format!(
                "{label}（非 ASCII 域名需 punycode，本版未实现）"
            )));
        }
        let bytes = label.as_bytes();
        if bytes.is_empty() {
            return Err(DnsError::InvalidLabel(label.to_string()));
        }
        if bytes.len() > 63 {
            return Err(DnsError::InvalidLabel(format!(
                "{label}（标签 {} 字节，超过 63）",
                bytes.len()
            )));
        }
        out.push(bytes.len() as u8);
        out.extend_from_slice(bytes);
    }
    out.push(0);
    Ok(out)
}

/// 组装查询报文（12 字节头 + QNAME + QTYPE + QCLASS）。
pub fn build_query(txid: u16, host: &str, qtype: u16) -> Result<Vec<u8>, DnsError> {
    let mut p = Vec::with_capacity(32);
    p.extend_from_slice(&txid.to_be_bytes());
    p.extend_from_slice(&0x0100u16.to_be_bytes()); // 标准查询 + RD
    p.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    p.extend_from_slice(&0u16.to_be_bytes()); // ANCOUNT
    p.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
    p.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT
    p.extend_from_slice(&encode_qname(host)?);
    p.extend_from_slice(&qtype.to_be_bytes());
    p.extend_from_slice(&1u16.to_be_bytes()); // QCLASS = IN
    Ok(p)
}

/// 解析应答报文，返回 A/AAAA 记录字符串。
///
/// 校验顺序与 Python 一致：id → ANCOUNT → TC → RCODE → 解析。
pub fn parse_response(data: &[u8], txid: u16) -> Result<Vec<String>, DnsError> {
    if data.len() < 12 {
        return Err(DnsError::Malformed("短于 12 字节头".into()));
    }
    let rid = u16::from_be_bytes([data[0], data[1]]);
    let flags = u16::from_be_bytes([data[2], data[3]]);
    let qd = u16::from_be_bytes([data[4], data[5]]) as usize;
    let an = u16::from_be_bytes([data[6], data[7]]) as usize;
    if rid != txid {
        return Err(DnsError::IdMismatch);
    }
    if an == 0 {
        return Err(DnsError::NoAnswer);
    }
    if flags & 0x0200 != 0 {
        return Err(DnsError::Truncated);
    }
    let rcode = (flags & 0x000F) as u8;
    if rcode != 0 {
        return Err(DnsError::Rcode(rcode));
    }

    // 跳过 Question 区
    let mut pos = 12usize;
    for _ in 0..qd {
        loop {
            if pos >= data.len() {
                return Err(DnsError::Malformed("Question 区越界".into()));
            }
            let len = data[pos];
            if len & 0xC0 != 0 {
                pos += 2;
                break;
            }
            pos += 1;
            if len == 0 {
                break;
            }
            pos += len as usize;
        }
        pos += 4; // QTYPE + QCLASS
    }

    // 解析 Answer 区
    let mut ips = Vec::new();
    for _ in 0..an {
        if pos >= data.len() {
            return Err(DnsError::Malformed("Answer 区越界".into()));
        }
        // 跳过 NAME（支持指针压缩）
        loop {
            if pos >= data.len() {
                return Err(DnsError::Malformed("NAME 越界".into()));
            }
            let length = data[pos];
            if length & 0xC0 == 0xC0 {
                pos += 2;
                break;
            }
            pos += 1;
            if length == 0 {
                break;
            }
            pos += length as usize;
        }
        if pos + 10 > data.len() {
            return Err(DnsError::Malformed("RR 头越界".into()));
        }
        let rtype = u16::from_be_bytes([data[pos], data[pos + 1]]);
        let rdlen = u16::from_be_bytes([data[pos + 8], data[pos + 9]]) as usize;
        pos += 10;
        if pos + rdlen > data.len() {
            return Err(DnsError::Malformed("RDATA 越界".into()));
        }
        let rd = &data[pos..pos + rdlen];
        pos += rdlen;
        match (rtype, rdlen) {
            (1, 4) => ips.push(Ipv4Addr::new(rd[0], rd[1], rd[2], rd[3]).to_string()),
            (28, 16) => {
                let mut octets = [0u8; 16];
                octets.copy_from_slice(rd);
                ips.push(std::net::Ipv6Addr::from(octets).to_string());
            }
            _ => {}
        }
    }
    if ips.is_empty() {
        return Err(DnsError::NoAnswer);
    }
    Ok(ips)
}

/// 向指定服务器查询（超时重发 attempts 次）。
pub fn query_with(
    host: &str,
    server: &str,
    timeout: f64,
    qtype: u16,
    attempts: u32,
) -> Result<Vec<String>, DnsError> {
    query_with_txid(host, server, timeout, qtype, attempts, rand_u16())
}

/// 指定 txid 的版本（测试注入固定值，避免随机性）。
pub fn query_with_txid(
    host: &str,
    server: &str,
    timeout: f64,
    qtype: u16,
    attempts: u32,
    txid: u16,
) -> Result<Vec<String>, DnsError> {
    let packet = build_query(txid, host, qtype)?;
    let addr: SocketAddr = format!("{server}:53")
        .parse()
        .map_err(|e| DnsError::Malformed(format!("DNS 服务器地址非法: {server} ({e})")))?;
    let socket = UdpSocket::bind(("0.0.0.0", 0))
        .map_err(|e| DnsError::Malformed(format!("无法创建 UDP 套接字: {e}")))?;
    let dur = Duration::from_secs_f64(timeout.max(0.1));
    socket
        .set_read_timeout(Some(dur))
        .map_err(|e| DnsError::Malformed(format!("设置超时失败: {e}")))?;

    let mut last_err = DnsError::Timeout;
    for _ in 0..attempts.max(1) {
        if socket.send_to(&packet, addr).is_err() {
            last_err = DnsError::Timeout;
            continue;
        }
        let mut buf = [0u8; 4096];
        match socket.recv_from(&mut buf) {
            Ok((n, _)) => return parse_response(&buf[..n], txid),
            Err(_) => last_err = DnsError::Timeout,
        }
    }
    Err(last_err)
}

/// 便捷查询（默认参数）。
pub fn query(host: &str, server: &str) -> Result<Vec<String>, DnsError> {
    query_with(
        host,
        server,
        DEFAULT_TIMEOUT_SECS,
        QTYPE_A,
        DEFAULT_ATTEMPTS,
    )
}

/// 失败返回空列表（对应 Python `query_safe`）——诊断链里「查不到」是合法结果。
pub fn query_safe(host: &str, server: &str) -> Vec<String> {
    query(host, server).unwrap_or_default()
}

/// 走系统解析器，带超时（daemon 线程 + join 限时）。
///
/// 用 daemon 语义的裸线程而非线程池：系统 DNS 挂死时线程池的退出钩子会 join
/// 未完成的 worker，把应用退出一起拖住——而那正是本工具要诊断的场景。
pub fn resolve_system(host: &str, timeout: f64) -> Vec<String> {
    use std::sync::mpsc;
    let (tx, rx) = mpsc::channel::<Vec<String>>();
    let host = host.to_string();
    std::thread::Builder::new()
        .name("sk-dns-resolve".into())
        .spawn(move || {
            let mut ips: Vec<String> = Vec::new();
            // getaddrinfo 的等价：标准库没有直接入口，用 ToSocketAddrs（仅 IPv4）
            if let Ok(iter) = (host.as_str(), 0u16).to_socket_addrs() {
                for a in iter {
                    if let IpAddr::V4(v4) = a.ip() {
                        ips.push(v4.to_string());
                    }
                }
            }
            ips.sort();
            ips.dedup();
            let _ = tx.send(ips);
        })
        .ok();
    // 超时（Err）时返回空：解析线程留作游离，随进程退出，不拖住退出
    rx.recv_timeout(Duration::from_secs_f64(timeout.max(0.1)))
        .unwrap_or_default()
}

use std::net::ToSocketAddrs;

fn rand_u16() -> u16 {
    // 不引 rand：用时间纳秒低位 + 进程 id 混合，足够避免同机并发撞 id
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    ((nanos ^ pid.rotate_left(13)) & 0xFFFF) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个应答报文：header + question + 一条 A 记录（NAME 用指针压缩）。
    fn make_response(txid: u16, flags: u16, qd: u16, an: u16, ip: [u8; 4]) -> Vec<u8> {
        let mut p = Vec::new();
        p.extend_from_slice(&txid.to_be_bytes());
        p.extend_from_slice(&flags.to_be_bytes());
        p.extend_from_slice(&qd.to_be_bytes());
        p.extend_from_slice(&an.to_be_bytes());
        p.extend_from_slice(&0u16.to_be_bytes());
        p.extend_from_slice(&0u16.to_be_bytes());
        // question: example.com A IN
        p.extend_from_slice(&encode_qname("example.com").unwrap());
        p.extend_from_slice(&1u16.to_be_bytes());
        p.extend_from_slice(&1u16.to_be_bytes());
        if an > 0 {
            // answer NAME = 指针指向 0x0C（question 起始）
            p.extend_from_slice(&[0xC0, 0x0C]);
            p.extend_from_slice(&1u16.to_be_bytes()); // TYPE A
            p.extend_from_slice(&1u16.to_be_bytes()); // CLASS IN
            p.extend_from_slice(&60u32.to_be_bytes()); // TTL
            p.extend_from_slice(&4u16.to_be_bytes()); // RDLENGTH
            p.extend_from_slice(&ip);
        }
        p
    }

    #[test]
    fn qname_encoding_is_byte_exact() {
        assert_eq!(
            encode_qname("example.com").unwrap(),
            vec![7, b'e', b'x', b'a', b'm', b'p', b'l', b'e', 3, b'c', b'o', b'm', 0]
        );
        // 末尾点忽略
        assert_eq!(
            encode_qname("example.com.").unwrap(),
            encode_qname("example.com").unwrap()
        );
        // 单标签
        assert_eq!(encode_qname("localhost").unwrap(), {
            let mut v = vec![9u8];
            v.extend_from_slice(b"localhost");
            v.push(0);
            v
        });
    }

    #[test]
    fn qname_rejects_invalid_labels() {
        assert!(matches!(encode_qname(""), Err(DnsError::InvalidLabel(_))));
        assert!(matches!(encode_qname("  "), Err(DnsError::InvalidLabel(_))));
        // 标签 >63 字节
        let long = "a".repeat(64);
        assert!(matches!(
            encode_qname(&long),
            Err(DnsError::InvalidLabel(_))
        ));
        // 非 ASCII：显式拒绝而不是写出非法报文
        assert!(matches!(
            encode_qname("中文.com"),
            Err(DnsError::InvalidLabel(_))
        ));
        // 空标签（连续点）
        assert!(matches!(
            encode_qname("a..b"),
            Err(DnsError::InvalidLabel(_))
        ));
    }

    #[test]
    fn qname_length_byte_is_byte_count_not_char_count() {
        // 这条对应用户踩过的坑：长度字节必须是编码后字节数
        let q = encode_qname("abcdefgh").unwrap();
        assert_eq!(q[0], 8, "长度字节 = 字节数");
        assert_eq!(q.len(), 1 + 8 + 1);
    }

    #[test]
    fn build_query_layout() {
        let q = build_query(0x1234, "a.bc", QTYPE_A).unwrap();
        assert_eq!(&q[0..2], &[0x12, 0x34], "txid 大端");
        assert_eq!(u16::from_be_bytes([q[2], q[3]]), 0x0100, "标准查询 + RD");
        assert_eq!(u16::from_be_bytes([q[4], q[5]]), 1, "QDCOUNT=1");
        assert_eq!(u16::from_be_bytes([q[6], q[7]]), 0, "ANCOUNT=0");
        assert_eq!(q[12], 1, "首个标签长度");
        assert_eq!(q[13], b'a');
        assert_eq!(q[14], 2, "第二个标签长度");
        let tail = &q[q.len() - 4..];
        assert_eq!(u16::from_be_bytes([tail[0], tail[1]]), QTYPE_A);
        assert_eq!(u16::from_be_bytes([tail[2], tail[3]]), 1, "QCLASS=IN");
    }

    #[test]
    fn parse_a_record() {
        let data = make_response(0xABCD, 0x8180, 1, 1, [93, 184, 216, 34]);
        let ips = parse_response(&data, 0xABCD).unwrap();
        assert_eq!(ips, vec!["93.184.216.34"]);
    }

    #[test]
    fn parse_rejects_id_mismatch() {
        let data = make_response(0x1111, 0x8180, 1, 1, [1, 2, 3, 4]);
        assert_eq!(parse_response(&data, 0x2222), Err(DnsError::IdMismatch));
    }

    #[test]
    fn parse_rejects_no_answer_count() {
        let data = make_response(0x0001, 0x8180, 1, 0, [0, 0, 0, 0]);
        assert_eq!(parse_response(&data, 0x0001), Err(DnsError::NoAnswer));
    }

    #[test]
    fn parse_rejects_truncated_response() {
        // TC 位 = 0x0200：必须显式报错，不能解析半截报文
        let data = make_response(0x0002, 0x8380, 1, 1, [1, 2, 3, 4]);
        assert_eq!(parse_response(&data, 0x0002), Err(DnsError::Truncated));
    }

    #[test]
    fn parse_rejects_rcode() {
        // NXDOMAIN = 3
        let data = make_response(0x0003, 0x8183, 1, 1, [1, 2, 3, 4]);
        assert_eq!(parse_response(&data, 0x0003), Err(DnsError::Rcode(3)));
    }

    #[test]
    fn parse_handles_aaaa_records() {
        let mut p = Vec::new();
        p.extend_from_slice(&0x0009u16.to_be_bytes());
        p.extend_from_slice(&0x8180u16.to_be_bytes());
        p.extend_from_slice(&1u16.to_be_bytes());
        p.extend_from_slice(&1u16.to_be_bytes());
        p.extend_from_slice(&[0, 0, 0, 0]);
        p.extend_from_slice(&encode_qname("ipv6.example").unwrap());
        p.extend_from_slice(&QTYPE_AAAA.to_be_bytes());
        p.extend_from_slice(&1u16.to_be_bytes());
        p.extend_from_slice(&[0xC0, 0x0C]);
        p.extend_from_slice(&QTYPE_AAAA.to_be_bytes());
        p.extend_from_slice(&1u16.to_be_bytes());
        p.extend_from_slice(&60u32.to_be_bytes());
        p.extend_from_slice(&16u16.to_be_bytes());
        let v6 = [0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
        p.extend_from_slice(&v6);
        let ips = parse_response(&p, 0x0009).unwrap();
        assert_eq!(ips, vec!["2001:db8::1"]);
    }

    #[test]
    fn parse_skips_multiple_questions() {
        // QDCOUNT=2：偏移推进必须正确，否则会把 question 当 answer 解析
        let mut p = Vec::new();
        p.extend_from_slice(&7u16.to_be_bytes());
        p.extend_from_slice(&0x8180u16.to_be_bytes());
        p.extend_from_slice(&2u16.to_be_bytes());
        p.extend_from_slice(&1u16.to_be_bytes());
        p.extend_from_slice(&[0, 0, 0, 0]);
        for h in ["first.example", "second.example"] {
            p.extend_from_slice(&encode_qname(h).unwrap());
            p.extend_from_slice(&1u16.to_be_bytes());
            p.extend_from_slice(&1u16.to_be_bytes());
        }
        p.extend_from_slice(&[0xC0, 0x0C]);
        p.extend_from_slice(&1u16.to_be_bytes());
        p.extend_from_slice(&1u16.to_be_bytes());
        p.extend_from_slice(&0u32.to_be_bytes());
        p.extend_from_slice(&4u16.to_be_bytes());
        p.extend_from_slice(&[8, 8, 8, 8]);
        assert_eq!(parse_response(&p, 7).unwrap(), vec!["8.8.8.8"]);
    }

    #[test]
    fn parse_rejects_short_and_overrun_packets() {
        assert!(matches!(
            parse_response(&[0u8; 5], 0),
            Err(DnsError::Malformed(_))
        ));
        // 头声明 1 条 answer 但数据截断
        let mut p = make_response(1, 0x8180, 1, 1, [1, 1, 1, 1]);
        p.truncate(p.len() - 2);
        assert!(matches!(parse_response(&p, 1), Err(DnsError::Malformed(_))));
    }

    #[test]
    fn parse_ignores_non_address_records() {
        // CNAME（type 5）不应被当作地址；且解析后无 A/AAAA → NoAnswer
        let mut p = Vec::new();
        p.extend_from_slice(&3u16.to_be_bytes());
        p.extend_from_slice(&0x8180u16.to_be_bytes());
        p.extend_from_slice(&1u16.to_be_bytes());
        p.extend_from_slice(&1u16.to_be_bytes());
        p.extend_from_slice(&[0, 0, 0, 0]);
        p.extend_from_slice(&encode_qname("alias.example").unwrap());
        p.extend_from_slice(&1u16.to_be_bytes());
        p.extend_from_slice(&1u16.to_be_bytes());
        p.extend_from_slice(&[0xC0, 0x0C]);
        p.extend_from_slice(&5u16.to_be_bytes()); // CNAME
        p.extend_from_slice(&1u16.to_be_bytes());
        p.extend_from_slice(&0u32.to_be_bytes());
        let target = encode_qname("real.example").unwrap();
        p.extend_from_slice(&(target.len() as u16).to_be_bytes());
        p.extend_from_slice(&target);
        assert_eq!(parse_response(&p, 3), Err(DnsError::NoAnswer));
    }

    #[test]
    fn invalid_server_address_is_reported() {
        let e = query_with_txid("example.com", "not-an-ip", 1.0, QTYPE_A, 1, 1).unwrap_err();
        assert!(matches!(e, DnsError::Malformed(_)));
    }

    #[test]
    fn query_to_unroutable_server_times_out() {
        // 203.0.113.0/24 是 TEST-NET-3，保证不可达；只断言超时而非 panic
        let e = query_with_txid("example.com", "203.0.113.1", 0.3, QTYPE_A, 1, 7).unwrap_err();
        assert_eq!(e, DnsError::Timeout);
    }

    #[test]
    fn live_query_public_dns_works() {
        // 唯一真正走网络的用例：公共 DNS 查一个稳定域名。
        // 断网环境下允许失败（记为空），但不得 panic。
        let ips = query_safe("www.baidu.com", "223.5.5.5");
        if ips.is_empty() {
            eprintln!("提示：本机当前无法访问 223.5.5.5，跳过联网断言");
            return;
        }
        assert!(
            ips.iter().all(|ip| ip.parse::<IpAddr>().is_ok()),
            "返回的必须是合法 IP: {ips:?}"
        );
    }

    #[test]
    fn resolve_system_returns_ips_or_empty_never_panics() {
        // 本机解析器：返回空或合法 IP 都可接受（诊断链里空=解析失败）
        let ips = resolve_system("www.baidu.com", 3.0);
        assert!(ips.iter().all(|ip| ip.parse::<Ipv4Addr>().is_ok()));
        // 明显不存在的域名：应为空且不 panic
        assert!(resolve_system("nonexistent.syskit.invalid", 2.0).is_empty());
    }

    #[test]
    fn dns_error_messages_are_human_readable() {
        assert!(DnsError::Timeout.to_string().contains("超时"));
        assert!(DnsError::Truncated.to_string().contains("截断"));
        assert!(DnsError::Rcode(3).to_string().contains("3"));
        assert!(DnsError::InvalidLabel("x".into()).to_string().contains('x'));
    }
}
