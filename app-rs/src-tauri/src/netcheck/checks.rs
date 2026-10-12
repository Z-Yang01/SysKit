//! 15 项检查本体（chk_*）与其私有辅助。
use super::*;

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
        r.detail = format!(
            "{} —— 系统解析结果含无效/私网保留/已知地雷地址（污染特征）",
            r.detail
        );
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
/// PnPCapabilities 原始值 → 节电态标签（纯函数，回归锁核心）。
///
/// 与 Python `winregread.pnp_states` 同口径：**24 (0x18) = 电源管理已被禁用
/// =「已禁止断电」**——这正是 `repairs::run_power_off` 写入的目标态（它遇到
/// 已是 24 的键会跳过不改）；**其它值（含缺省 0）=「允许断电」**，才是风险项。
/// 诊断若把 24 报成"允许断电"，用户跑完 power_off 修复后诊断页会永远 WARN
/// 并反复推荐同一个"没有可改项"的修复——修复与诊断互相打脸。
/// `None` = 该描述在类键里找不到（跳过，不计入两种状态）。
pub fn pnp_power_label(cap: Option<u32>) -> Option<&'static str> {
    match cap {
        None => None,
        Some(24) => Some("已禁止断电"),
        Some(_) => Some("允许断电"),
    }
}

pub fn chk_power_mgmt(snap: &Snapshot) -> CheckResult {
    let mut r = CheckResult::new("power_mgmt", "网卡节电（允许系统关闭设备以节约电源）", INFO);
    let scan = pnp_capabilities_scan();
    let mut items = Vec::new();
    for a in &snap.adapters {
        if a.virtual_ || a.desc.is_empty() {
            continue;
        }
        if let Some(label) = pnp_power_label(pnp_capabilities(&a.desc, &scan)) {
            let short: String = a.desc.chars().take(24).collect();
            items.push(format!("{}({short}): {label}", a.name));
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

/// `sc qc` 的 START_TYPE 数字 → PowerShell `StartType` 风格的英文标签。
///
/// Python 侧快照直接拿 Get-Service 的 StartType（"Automatic"/"Manual"/"Disabled"），
/// 判定分支都按这些字符串比较；Rust 走 `sc qc` 拿到的是数字，必须映射到同一套
/// 标签再进判定，否则「手动启动无触发器」这个核心场景永远匹配不上。
pub fn start_type_label(raw: &str) -> String {
    // split_whitespace 自带首尾空白容忍，无需再 trim
    let first = raw.split_whitespace().next().unwrap_or("");
    match first {
        "0" => "Boot".to_string(),
        "1" => "System".to_string(),
        "2" => "Automatic".to_string(),
        "3" => "Manual".to_string(),
        "4" => "Disabled".to_string(),
        _ => raw.trim().to_string(),
    }
}

/// NlaSvc 判定（纯函数，对应 Python `chk_nlasvc` 的全部分支）。
///
/// 分级是硬对齐：停止态在 Python 是 **FAIL**（必然误报「无网络」），
/// 「运行中但手动启动+无触发器」是 WARN——那是本工具的核心目标场景
/// （重启后复发，必须建议 fix_nlasvc 而不只是 start_nlasvc）。
pub fn nlasvc_result(running: bool, start_type: &str, has_trigger: bool) -> CheckResult {
    let mut r = CheckResult::new(
        "nlasvc",
        "NlaSvc 网络位置感知服务（画'有网/无网'标记的服务）",
        PASS,
    );
    if running {
        r.detail = format!(
            "运行中; 启动类型={start_type}; 触发器={}",
            if has_trigger { "有" } else { "无" }
        );
        if start_type == "Manual" && !has_trigger {
            // 三字段（状态/详情/fix_ids）必须一起更新：二元解包曾在这里炸过
            r.status = WARN;
            r.detail
                .push_str("；本机被改为手动启动且无触发器（非原版配置，重启后仍不会自启）");
            r.fix_ids = vec!["fix_nlasvc"];
        }
        return r;
    }
    if start_type == "Disabled" {
        r.status = FAIL;
        r.detail = "已禁用 —— NCSI 探测无人执行，必然误报'无网络'".to_string();
        r.fix_ids = vec!["fix_nlasvc"];
    } else if start_type == "Manual" && !has_trigger {
        r.status = FAIL;
        r.detail = ("已停止，且启动类型=手动 + 无任何触发器 —— 无人拉起它，自开机起从未运行。\
             这是'已连接却显示无网络'的直接原因之一")
            .to_string();
        r.fix_ids = vec!["fix_nlasvc"];
    } else {
        r.status = FAIL;
        r.detail = format!("已停止(启动类型={start_type})");
        r.fix_ids = vec!["start_nlasvc"];
    }
    r
}

/// NlaSvc 是否配置了触发器（`TriggerInfo` 子键非空即有）。
///
/// 子键按数字命名（0/1/…），逐个探测非空值表即可；读不到（权限/精简系统）
/// 按无触发器处理——与 Python `subkeys()` 失败返回空表同口径，绝不误报有触发器。
fn nlasvc_has_trigger() -> bool {
    const TRIGGER_KEY: &str = r"HKLM\SYSTEM\CurrentControlSet\Services\NlaSvc\TriggerInfo";
    for i in 0..8 {
        if !w32::reg_values(&format!("{TRIGGER_KEY}\\{i}")).is_empty() {
            return true;
        }
    }
    false
}

/// NlaSvc 服务状态（id=nlasvc）：只靠 sc query 即可判断，不依赖快照。
pub fn chk_nlasvc() -> CheckResult {
    match service_state("NlaSvc") {
        Some(s) => {
            let running = s.status.eq_ignore_ascii_case("RUNNING");
            let start = start_type_label(&s.start_type);
            nlasvc_result(running, &start, nlasvc_has_trigger())
        }
        None => {
            let mut r = CheckResult::new(
                "nlasvc",
                "NlaSvc 网络位置感知服务（画'有网/无网'标记的服务）",
                SKIP,
            );
            r.detail = "无法查询 NlaSvc（sc query 无输出）".to_string();
            r
        }
    }
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

