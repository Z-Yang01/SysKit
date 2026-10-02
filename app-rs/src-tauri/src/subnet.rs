//! 子网计算器与 DNS 查询预设（对应 Python 侧 DnsSubnetTab）。
//!
//! 子网计算**自己实现**而不是引 `ipnet`/`cidr`：IPv4 的前缀运算就是位运算，
//! 手写反而能把边界（/31、/32、/0）逐条断言清楚，也少一个依赖。
//! 计算全部是纯函数，UI 与测试共用同一份实现。

use std::net::Ipv4Addr;

use serde::Serialize;

/// DNS 服务器下拉项（对应 Python 的 cmb_server 预设）。
pub const DNS_SERVER_PRESETS: [(&str, &str); 5] = [
    ("系统默认", ""),
    ("223.5.5.5（阿里）", "223.5.5.5"),
    ("119.29.29.29（腾讯）", "119.29.29.29"),
    ("114.114.114.114（114）", "114.114.114.114"),
    ("8.8.8.8（Google）", "8.8.8.8"),
];

/// 由下拉显示文本取服务器地址；"系统默认" 返回空串（表示走系统解析器）。
pub fn server_from_label(label: &str) -> &str {
    DNS_SERVER_PRESETS
        .iter()
        .find(|(l, _)| *l == label)
        .map(|(_, s)| *s)
        .unwrap_or("")
}

/// 子网计算结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SubnetInfo {
    pub prefix: u8,
    pub mask: String,
    pub wildcard: String,
    pub network: String,
    pub broadcast: String,
    /// 第一个可用主机（/31、/32 无可用主机时为空）
    pub first_host: String,
    /// 最后一个可用主机
    pub last_host: String,
    /// 可用主机数（/31 按 RFC 3021 算 2，/32 算 1）
    pub usable_hosts: u64,
    pub total_addresses: u64,
    /// IPv4 私有地址段判定
    pub is_private: bool,
}

/// 计算 IPv4 子网信息。`prefix` 必须在 0..=32。
pub fn subnet_info(ip: &str, prefix: u8) -> Result<SubnetInfo, String> {
    if prefix > 32 {
        return Err(format!("前缀长度非法: /{prefix}（应在 0~32）"));
    }
    let addr: Ipv4Addr = ip
        .trim()
        .parse()
        .map_err(|_| format!("IPv4 地址非法: {ip}"))?;
    let bits = u32::from(addr);
    // /0 时 mask_bits 移位 32 位是 UB，单独处理
    let mask_bits: u32 = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix as u32)
    };
    let network = bits & mask_bits;
    let broadcast = network | !mask_bits;
    let total: u64 = 1u64 << (32 - prefix as u32);

    // 可用主机：/0~/30 去掉网络号与广播；/31 点对点（RFC 3021）算 2；/32 算 1
    let (usable, first, last) = match prefix {
        32 => (1u64, Ipv4Addr::from(network), Ipv4Addr::from(network)),
        31 => (2u64, Ipv4Addr::from(network), Ipv4Addr::from(network + 1)),
        _ => (
            total.saturating_sub(2),
            Ipv4Addr::from(network + 1),
            Ipv4Addr::from(broadcast - 1),
        ),
    };

    Ok(SubnetInfo {
        prefix,
        mask: Ipv4Addr::from(mask_bits).to_string(),
        wildcard: Ipv4Addr::from(!mask_bits).to_string(),
        network: format!("{}/{}", Ipv4Addr::from(network), prefix),
        broadcast: Ipv4Addr::from(broadcast).to_string(),
        first_host: first.to_string(),
        last_host: last.to_string(),
        usable_hosts: usable,
        total_addresses: total,
        is_private: is_private_v4(addr),
    })
}

/// RFC 1918 私有地址 + 回环 + 链路本地 + CGNAT（100.64/10）。
pub fn is_private_v4(addr: Ipv4Addr) -> bool {
    let o = addr.octets();
    addr.is_loopback()
        || addr.is_link_local()
        || o[0] == 10
        || (o[0] == 172 && (16..=31).contains(&o[1]))
        || (o[0] == 192 && o[1] == 168)
        || (o[0] == 100 && (64..=127).contains(&o[1]))
        || addr.is_unspecified()
}

/// 由 IP + 前缀求网络地址字符串（便于 UI 直接展示）。
pub fn network_of(ip: &str, prefix: u8) -> Result<String, String> {
    subnet_info(ip, prefix).map(|s| s.network)
}

/// 结果 → 便于展示的「标签/值」行（顺序与 Python 侧列表一致）。
pub fn subnet_lines(info: &SubnetInfo) -> Vec<(&'static str, String)> {
    vec![
        ("网络地址", info.network.clone()),
        ("子网掩码", info.mask.clone()),
        ("反掩码", info.wildcard.clone()),
        ("广播地址", info.broadcast.clone()),
        (
            "可用主机范围",
            format!("{} ~ {}", info.first_host, info.last_host),
        ),
        ("可用主机数", info.usable_hosts.to_string()),
        ("地址总数", info.total_addresses.to_string()),
        (
            "地址类型",
            if info.is_private {
                "私有地址".to_string()
            } else {
                "公网地址".to_string()
            },
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_case_matches_expected() {
        let s = subnet_info("192.168.1.10", 24).unwrap();
        assert_eq!(s.mask, "255.255.255.0");
        assert_eq!(s.wildcard, "0.0.0.255");
        assert_eq!(s.network, "192.168.1.0/24");
        assert_eq!(s.broadcast, "192.168.1.255");
        assert_eq!(s.first_host, "192.168.1.1");
        assert_eq!(s.last_host, "192.168.1.254");
        assert_eq!(s.usable_hosts, 254);
        assert_eq!(s.total_addresses, 256);
        assert!(s.is_private);
    }

    #[test]
    fn prefix_30_has_two_usable() {
        let s = subnet_info("10.0.0.5", 30).unwrap();
        assert_eq!(s.network, "10.0.0.4/30");
        assert_eq!(s.first_host, "10.0.0.5");
        assert_eq!(s.last_host, "10.0.0.6");
        assert_eq!(s.usable_hosts, 2);
        assert_eq!(s.broadcast, "10.0.0.7");
    }

    #[test]
    fn prefix_31_is_point_to_point() {
        // /31 没有网络号/广播之分（RFC 3021），两端都可用
        let s = subnet_info("10.0.0.1", 31).unwrap();
        assert_eq!(s.usable_hosts, 2);
        assert_eq!(s.first_host, "10.0.0.0");
        assert_eq!(s.last_host, "10.0.0.1");
        assert_eq!(s.total_addresses, 2);
    }

    #[test]
    fn prefix_32_is_single_host() {
        let s = subnet_info("8.8.8.8", 32).unwrap();
        assert_eq!(s.usable_hosts, 1);
        assert_eq!(s.network, "8.8.8.8/32");
        assert_eq!(s.first_host, "8.8.8.8");
        assert!(!s.is_private);
    }

    #[test]
    fn prefix_0_does_not_shift_by_32() {
        // 左移 32 位是 UB——必须单独处理，否则结果随机
        let s = subnet_info("1.2.3.4", 0).unwrap();
        assert_eq!(s.mask, "0.0.0.0");
        assert_eq!(s.wildcard, "255.255.255.255");
        assert_eq!(s.network, "0.0.0.0/0");
        assert_eq!(s.total_addresses, 1u64 << 32);
        assert_eq!(s.usable_hosts, (1u64 << 32) - 2);
    }

    #[test]
    fn mask_and_wildcard_are_complements() {
        for p in [8u8, 12, 16, 20, 24, 25, 26, 27, 28, 29, 30] {
            let s = subnet_info("172.16.5.9", p).unwrap();
            let m: u32 = s.mask.parse::<Ipv4Addr>().unwrap().into();
            let w: u32 = s.wildcard.parse::<Ipv4Addr>().unwrap().into();
            assert_eq!(m | w, u32::MAX, "/{p} 掩码与反掩码应互补");
            assert_eq!(m & w, 0, "/{p} 掩码与反掩码不应重叠");
        }
    }

    #[test]
    fn host_range_within_network() {
        for p in [24u8, 26, 28, 30] {
            let s = subnet_info("192.168.100.77", p).unwrap();
            let net: u32 = s
                .network
                .split('/')
                .next()
                .unwrap()
                .parse::<Ipv4Addr>()
                .unwrap()
                .into();
            let first: u32 = s.first_host.parse::<Ipv4Addr>().unwrap().into();
            let last: u32 = s.last_host.parse::<Ipv4Addr>().unwrap().into();
            let bc: u32 = s.broadcast.parse::<Ipv4Addr>().unwrap().into();
            assert!(net < first, "/{p} 网络号须小于首主机");
            assert!(first <= last, "/{p} 范围有序");
            assert!(last < bc, "/{p} 末主机须小于广播");
        }
    }

    #[test]
    fn private_ranges_match_rfc1918_plus_extras() {
        for ip in [
            "10.0.0.1",
            "10.255.255.254",
            "172.16.0.1",
            "172.31.255.254",
            "192.168.0.1",
            "127.0.0.1",
            "169.254.1.1",
            "100.64.0.1",
            "100.127.255.254",
            "0.0.0.0",
        ] {
            let a: Ipv4Addr = ip.parse().unwrap();
            assert!(is_private_v4(a), "{ip} 应判为私有/保留");
        }
        for ip in [
            "8.8.8.8",
            "1.1.1.1",
            "172.15.0.1",
            "172.32.0.1",
            "192.169.0.1",
            "100.63.0.1",
            "100.128.0.1",
        ] {
            let a: Ipv4Addr = ip.parse().unwrap();
            assert!(!is_private_v4(a), "{ip} 应判为公网");
        }
    }

    #[test]
    fn rejects_invalid_input() {
        assert!(subnet_info("not-an-ip", 24).is_err());
        assert!(subnet_info("", 24).is_err());
        assert!(subnet_info("192.168.1.1", 33).is_err());
        assert!(subnet_info("::1", 24).is_err(), "IPv6 不走这条路径");
        // 错误信息必须可读
        let e = subnet_info("bad", 24).unwrap_err();
        assert!(e.contains("非法"), "{e}");
        let e2 = subnet_info("1.2.3.4", 40).unwrap_err();
        assert!(e2.contains("前缀"), "{e2}");
    }

    #[test]
    fn network_of_helper() {
        assert_eq!(network_of("192.168.1.10", 24).unwrap(), "192.168.1.0/24");
        assert_eq!(network_of("10.1.2.3", 8).unwrap(), "10.0.0.0/8");
        assert!(network_of("bad", 24).is_err());
    }

    #[test]
    fn subnet_lines_order_and_content() {
        let info = subnet_info("192.168.1.10", 24).unwrap();
        let lines = subnet_lines(&info);
        assert_eq!(lines.len(), 8);
        assert_eq!(lines[0].0, "网络地址");
        assert_eq!(lines[0].1, "192.168.1.0/24");
        assert_eq!(lines[5].0, "可用主机数");
        assert_eq!(lines[5].1, "254");
        assert_eq!(lines[7].1, "私有地址");
    }

    #[test]
    fn dns_presets_match_python_dropdown() {
        assert_eq!(DNS_SERVER_PRESETS.len(), 5);
        assert_eq!(DNS_SERVER_PRESETS[0].0, "系统默认");
        assert_eq!(DNS_SERVER_PRESETS[0].1, "", "系统默认无地址");
        assert_eq!(server_from_label("223.5.5.5（阿里）"), "223.5.5.5");
        assert_eq!(server_from_label("119.29.29.29（腾讯）"), "119.29.29.29");
        assert_eq!(
            server_from_label("114.114.114.114（114）"),
            "114.114.114.114"
        );
        assert_eq!(server_from_label("8.8.8.8（Google）"), "8.8.8.8");
        assert_eq!(server_from_label("系统默认"), "");
        assert_eq!(server_from_label("未知项"), "", "未知项退回系统默认");
    }
}
