//! Wake-on-LAN：构造并发送魔术包（Magic Packet），唤醒局域网内已知 MAC 的设备。
//!
//! 协议（AMD Magic Packet 标准）：6 字节 0xFF 前导 + 目标 MAC 重复 16 次，
//! 共 102 字节，以 UDP 广播发出（默认端口 9，丢弃端口不敏感——多数网卡
//! 只认包内容不认端口）。
//!
//! MAC 解析容忍三种常见书写：`AA-BB-CC-DD-EE-FF` / `AA:BB:...` / `AABBCCDDEEFF`
//! （ARP 表输出是连字符形，用户手输常见冒号形，剪贴板复制可能无分隔）。
//!
//! 广播地址：同时向 `255.255.255.255` 与网段定向广播（如 192.168.1.255）发——
//! 部分路由器/AP 会过滤全局广播，定向广播命中率更高；两发取其一成功即可。

use std::net::UdpSocket;

/// MAC 字符串 → 6 字节。容忍 `-`/`:`/空格分隔与无分隔 12 位连写。
pub fn parse_mac(s: &str) -> Result<[u8; 6], String> {
    let cleaned: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if cleaned.len() != 12 {
        return Err(format!(
            "MAC 地址须为 12 位十六进制（收到 {} 位）：{s}",
            cleaned.len()
        ));
    }
    let mut out = [0u8; 6];
    for (i, b) in out.iter_mut().enumerate() {
        let hi = cleaned[i * 2..i * 2 + 1]
            .chars()
            .next()
            .and_then(|c| c.to_digit(16))
            .ok_or_else(|| format!("MAC 含非法字符: {s}"))? as u8;
        let lo = cleaned[i * 2 + 1..i * 2 + 2]
            .chars()
            .next()
            .and_then(|c| c.to_digit(16))
            .ok_or_else(|| format!("MAC 含非法字符: {s}"))? as u8;
        *b = (hi << 4) | lo;
    }
    Ok(out)
}

/// 构造 102 字节魔术包：6×0xFF + MAC×16。
pub fn magic_packet(mac: &[u8; 6]) -> [u8; 102] {
    let mut pkt = [0u8; 102];
    for b in pkt.iter_mut().take(6) {
        *b = 0xFF;
    }
    for i in 0..16 {
        pkt[6 + i * 6..12 + i * 6].copy_from_slice(mac);
    }
    pkt
}

/// 向单个广播地址发包。成功返回发送字节数。
fn send_to(sock: &UdpSocket, addr: &str, pkt: &[u8; 102]) -> Option<usize> {
    sock.send_to(pkt, addr).ok()
}

/// 发送 WOL：全局广播 + 定向广播（`direct_broadcast` 提供时）各发一次。
///
/// 返回人类可读结果。UDP send 极少失败（无连接不等待对端），「发出」不等于
/// 「唤醒成功」——WOL 是 fire-and-forget 协议，唤醒与否取决于目标网卡支持，
/// 结果文案如实说明。
pub fn send_wol(
    mac: &[u8; 6],
    port: u16,
    direct_broadcast: Option<&str>,
) -> Result<String, String> {
    let pkt = magic_packet(mac);
    let sock = UdpSocket::bind(("0.0.0.0", 0)).map_err(|e| format!("UDP socket 创建失败: {e}"))?;
    sock.set_broadcast(true)
        .map_err(|e| format!("广播模式开启失败: {e}"))?;

    let port = if port == 0 { 9 } else { port };
    let mut sent = 0usize;
    if let Some(n) = send_to(&sock, &format!("255.255.255.255:{port}"), &pkt) {
        sent += n;
    }
    if let Some(b) = direct_broadcast.filter(|s| !s.trim().is_empty()) {
        if let Some(n) = send_to(&sock, &format!("{b}:{port}"), &pkt) {
            sent += n;
        }
    }
    if sent == 0 {
        return Err("广播发送失败（防火墙/网卡限制？）".to_string());
    }
    Ok(format!(
        "魔术包已发送（{} 字节 × {} 个广播）——若设备支持 WOL 且为关机/休眠态，稍候数秒应已唤醒",
        pkt.len(),
        if direct_broadcast.is_some() { 2 } else { 1 },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mac_three_common_forms() {
        assert_eq!(
            parse_mac("AA-BB-CC-DD-EE-FF").unwrap(),
            [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]
        );
        assert_eq!(
            parse_mac("aa:bb:cc:dd:ee:ff").unwrap(),
            [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]
        );
        assert_eq!(
            parse_mac("AABBCCDDEEFF").unwrap(),
            [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]
        );
        // ARP 表混排分隔符也能容
        assert_eq!(
            parse_mac("aA-bb:cC dd-ee ff").unwrap(),
            [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]
        );
    }

    #[test]
    fn parse_mac_rejects_wrong_length_and_garbage() {
        assert!(parse_mac("").is_err());
        assert!(parse_mac("AA-BB-CC-DD-EE").is_err(), "11 位不足");
        assert!(parse_mac("AA-BB-CC-DD-EE-FF-00").is_err(), "14 位超长");
        assert!(parse_mac("ZZ-BB-CC-DD-EE-FF").is_err(), "非十六进制");
    }

    #[test]
    fn magic_packet_layout_matches_amd_spec() {
        let mac = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
        let pkt = magic_packet(&mac);
        assert_eq!(pkt.len(), 102);
        assert!(pkt[..6].iter().all(|&b| b == 0xFF), "前导 6 字节 0xFF");
        for i in 0..16 {
            assert_eq!(&pkt[6 + i * 6..12 + i * 6], &mac, "第 {i} 次重复");
        }
    }

    #[test]
    fn magic_packet_syncs_mac_from_parsed_input() {
        let mac = parse_mac("aabbccddeeff").unwrap();
        let pkt = magic_packet(&mac);
        assert_eq!(&pkt[6..12], &[0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
    }
}
