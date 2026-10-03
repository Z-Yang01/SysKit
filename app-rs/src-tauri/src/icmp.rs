//! ICMP ping：iphlpapi `IcmpSendEcho`（等价 Python `icmp.py`，去子进程开销与
//! 本地化解析）。诊断热路径（网关连通 ×3）原先起 3 个 ping.exe——无本地化
//! 问题、无进程开销、毫秒精度。
//!
//! ReplyBuffer 必须容纳 ICMP 差错响应（IP 头 ≤60 + 8 字节），否则缓冲区溢出。

use std::net::{Ipv4Addr, ToSocketAddrs};

use windows::Win32::NetworkManagement::IpHelper::{
    IcmpCloseHandle, IcmpCreateFile, IcmpSendEcho, ICMP_ECHO_REPLY,
};

/// Python `icmp._PAYLOAD` 同款负载（无语义，仅占位）。
const PAYLOAD: &[u8] = b"SysKitPingData";
const IP_SUCCESS: u32 = 0;

/// "a.b.c.d" → IcmpSendEcho 的 destinationaddress 形态（对齐 Python
/// `struct.unpack("<I", inet_aton(ip))`：小端重解释八位组）。
fn ipv4_to_addr_u32(ip: &str) -> Option<u32> {
    let addr: Ipv4Addr = ip.parse().ok()?;
    Some(u32::from_ne_bytes(addr.octets()))
}

/// 域名/地址 → 点分 IPv4 文本（失败 None，对齐 Python gethostbyname 早退）。
fn resolve_ipv4(host: &str) -> Option<String> {
    if host.trim().is_empty() {
        return None; // 空 host 在部分系统上会被解析成 0.0.0.0，显式拒绝
    }
    let addrs: Vec<_> = (host, 0u16).to_socket_addrs().ok()?.collect();
    for a in addrs {
        if let std::net::IpAddr::V4(v4) = a.ip() {
            if !v4.is_unspecified() {
                return Some(v4.to_string());
            }
        }
    }
    None
}

/// ping 指定主机，成功返回延迟毫秒数，失败/超时/解析失败返回 None。
/// 超时语义与 Python `icmp.ping(host, timeout_ms)` 一致（单次）。
pub fn ping(host: &str, timeout_ms: u32) -> Option<f64> {
    let ip = resolve_ipv4(host)?;
    let dest = ipv4_to_addr_u32(&ip)?;
    unsafe {
        // windows-crate 包装版：CreateFile 失败返回 Err（INVALID_HANDLE 已被消化）
        let handle = IcmpCreateFile().ok()?;
        let reply_size = std::mem::size_of::<ICMP_ECHO_REPLY>() + PAYLOAD.len() + 68 + 16;
        let mut buf = vec![0u8; reply_size];
        let ret = IcmpSendEcho(
            handle,
            dest,
            PAYLOAD.as_ptr() as *const _,
            PAYLOAD.len() as u16,
            None,
            buf.as_mut_ptr() as *mut _,
            reply_size as u32,
            timeout_ms,
        );
        let out = if ret == 0 {
            None
        } else {
            let reply = &*(buf.as_ptr() as *const ICMP_ECHO_REPLY);
            if reply.Status != IP_SUCCESS {
                None
            } else {
                Some(reply.RoundTripTime as f64)
            }
        };
        // 句柄关闭失败无补救意义（探测结果已取得），忽略返回值
        let _ = IcmpCloseHandle(handle);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addr_u32_matches_python_inet_aton_le() {
        // Python: struct.unpack("<I", inet_aton("1.2.3.4"))[0] == 0x04030201
        assert_eq!(ipv4_to_addr_u32("1.2.3.4"), Some(0x0403_0201));
        assert_eq!(ipv4_to_addr_u32("192.168.1.1"), Some(0x0101_a8c0));
        assert_eq!(ipv4_to_addr_u32("not-an-ip"), None);
    }

    #[test]
    fn resolve_rejects_bogus_host() {
        // 无网络解析环境也能稳定判 None 的纯负例
        assert_eq!(resolve_ipv4(""), None);
    }
}
