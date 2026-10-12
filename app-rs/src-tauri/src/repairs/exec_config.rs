//! 配置类执行器批次 A + 持久路由纯逻辑（代理/NCSI 注册表/DNS/路由，改前备份）。
use super::*;

// ---------------- 执行器批次 A：无需管理员的 5 项 ----------------

/// HKCU 的 WinINET 设置键（代理开关所在）。
pub const INTERNET_SETTINGS_KEY: &str =
    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings";
/// 持久路由的权威来源键。
#[allow(dead_code)] // 持久路由注册表键（对齐面，执行器暂未用）
pub const PERSISTENT_ROUTES_KEY: &str =
    r"HKLM\SYSTEM\CurrentControlSet\Services\Tcpip\Parameters\PersistentRoutes";

/// 国内公共 DNS 预设（与 config.DNS_PRESETS 逐项一致）。
pub const DNS_PRESETS: [(&str, &str, &str); 4] = [
    ("阿里", "223.5.5.5", "223.6.6.6"),
    ("腾讯", "119.29.29.29", "182.254.116.116"),
    ("114", "114.114.114.114", "114.114.115.115"),
    ("360", "101.226.4.6", "218.30.118.6"),
];

pub fn dns_preset(name: &str) -> Option<(&'static str, &'static str)> {
    DNS_PRESETS
        .iter()
        .find(|(n, _, _)| *n == name)
        .map(|(_, p, s)| (*p, *s))
}

/// 关闭系统代理（含 PAC 清理）。
///
/// 与 Python `proxy.disable_proxy` 同语义：只关 `ProxyEnable` 而不清 PAC 时，
/// 诊断对「PAC 自动代理脚本」项给的修复建议会点了没效果——所以两者一并处理，
/// 且**改前必须备份成功**（备份失败就保留原配置不改）。
pub fn run_proxy_off(_p: &BTreeMap<String, String>) -> RepairOutcome {
    let key = INTERNET_SETTINGS_KEY;
    let vals = w32::reg_values(key);
    if vals.is_empty() {
        return RepairOutcome::fail(format!("无法读取系统代理注册表: {key}"));
    }
    let session = match crate::backup::BackupSession::new() {
        Ok(s) => s,
        Err(e) => return RepairOutcome::fail(format!("备份目录不可用，已保留原配置未修改: {e}")),
    };
    let snapshot = serde_json::json!({
        "ProxyEnable": vals.get("ProxyEnable").map(|v| v.as_compare_str()),
        "ProxyServer": vals.get("ProxyServer").map(|v| v.as_compare_str()),
        "AutoConfigURL": vals.get("AutoConfigURL").map(|v| v.as_compare_str()),
    });
    let backup_path = match session.save_note(
        "proxy_backup.json",
        &serde_json::to_string_pretty(&snapshot).unwrap_or_default(),
    ) {
        Ok(p) => p,
        Err(e) => return RepairOutcome::fail(format!("代理原值备份失败，已保留原配置未修改: {e}")),
    };

    if !w32::reg_set_dword(key, "ProxyEnable", 0) {
        return RepairOutcome::fail(
            "写入 ProxyEnable=0 失败（权限？），未做任何有效修改".to_string(),
        );
    }
    // PAC：能删就删（对应 Python DeleteValue），删不掉退化为写空串
    let mut pac_note = String::new();
    if vals.contains_key("AutoConfigURL") {
        match w32::reg_delete_value(key, "AutoConfigURL") {
            Ok(true) => pac_note = "，已清除 PAC".to_string(),
            Ok(false) => {}
            Err(_) => {
                if w32::reg_set_string(key, "AutoConfigURL", "") {
                    pac_note = "，PAC 已置空".to_string();
                } else {
                    pac_note = "，PAC 清理失败（需手动检查）".to_string();
                }
            }
        }
    }
    let notified = w32::notify_internet_settings_changed();
    RepairOutcome {
        success: true,
        message: format!(
            "系统代理已关闭{pac_note}{}；备份: {backup_path}",
            if notified {
                ""
            } else {
                "（设置刷新通知未发出，可能需重启应用生效）"
            }
        ),
        need_reboot: false,
        output: backup_path,
    }
}

/// 修复 NlaSvc 启动配置（设为自动并立即启动）。
///
/// `sc config NlaSvc start= auto` —— 注意 `start=` 与 `auto` 必须是**两个裸参数**
/// （sc 解析原始命令行，不能写成 `start=auto` 或带引号）。
pub fn run_fix_nlasvc(_p: &BTreeMap<String, String>) -> RepairOutcome {
    let c = w32::run_capture(&args(&["sc", "config", "NlaSvc", "start=", "auto"]), 15);
    let c = match c {
        Ok(o) => o,
        Err(e) => return RepairOutcome::fail(format!("sc config 无法执行: {e}")),
    };
    if !c.ok {
        return RepairOutcome {
            success: false,
            message: format!(
                "sc config 失败（可能需要管理员）: {}",
                c.tail(0, 120).trim()
            ),
            need_reboot: false,
            output: c.combined(),
        };
    }
    // 成败以随后的 query 为准（sc start 的退出码不足以判断）
    let _ = w32::run_capture(&args(&["sc", "start", "NlaSvc"]), 30);
    let q = w32::run_capture(&args(&["sc", "query", "NlaSvc"]), 10);
    let running = q
        .as_ref()
        .map(|o| o.stdout.contains("RUNNING"))
        .unwrap_or(false);
    RepairOutcome {
        success: running,
        message: format!(
            "NlaSvc 已设为自动启动{}",
            if running {
                "，且已成功启动"
            } else {
                "，但启动失败，请重启后再试"
            }
        ),
        need_reboot: false,
        output: q.map(|o| o.stdout).unwrap_or_default(),
    }
}

/// DNS 恢复为 DHCP 自动获取（改前备份 v4+v6 静态值）。
pub fn run_dns_auto(p: &BTreeMap<String, String>) -> RepairOutcome {
    let name = match p.get("adapter").filter(|s| !s.trim().is_empty()) {
        Some(n) => n.clone(),
        None => match static_dns_adapter() {
            Some(n) => n,
            None => return RepairOutcome::fail("未找到配置了静态 DNS 的网卡"),
        },
    };
    let session = match crate::backup::BackupSession::new() {
        Ok(s) => s,
        Err(e) => return RepairOutcome::fail(format!("备份目录不可用，已保留原配置未修改: {e}")),
    };
    // 改前备份是动作描述对用户的承诺：备份失败则保留原配置不改
    let backup = match backup_dns(&session, &name) {
        Ok(p) => p,
        Err(e) => {
            return RepairOutcome::fail(format!(
                "备份原 DNS 失败，已保留原配置未修改（检查备份目录/磁盘）: {e}"
            ))
        }
    };
    let r4 = w32::run_capture(
        &args(&[
            "netsh",
            "interface",
            "ipv4",
            "set",
            "dnsservers",
            &format!("name={name}"),
            "source=dhcp",
        ]),
        15,
    );
    let r6 = w32::run_capture(
        &args(&[
            "netsh",
            "interface",
            "ipv6",
            "set",
            "dnsservers",
            &format!("name={name}"),
            "source=dhcp",
        ]),
        15,
    );
    let _ = w32::run_capture(&args(&["ipconfig", "/flushdns"]), 10);
    let ok4 = r4.as_ref().map(|o| o.ok).unwrap_or(false);
    let ok6 = r6.as_ref().map(|o| o.ok).unwrap_or(false);
    if ok4 && ok6 {
        RepairOutcome {
            success: true,
            message: format!("{name} 的 DNS 已恢复自动获取；备份: {backup}"),
            need_reboot: false,
            output: backup,
        }
    } else {
        let mut errs = String::new();
        if !ok4 {
            errs.push_str(&r4.map(|o| o.tail(0, 100)).unwrap_or_default());
        }
        if !ok6 {
            errs.push_str(&format!(
                " v6: {}",
                r6.map(|o| o.tail(0, 100)).unwrap_or_default()
            ));
        }
        RepairOutcome {
            success: false,
            message: format!("{name} DNS 恢复自动获取失败: {errs}"),
            need_reboot: false,
            output: errs,
        }
    }
}

/// 设置静态公共 DNS。
pub fn run_dns_preset(p: &BTreeMap<String, String>) -> RepairOutcome {
    let preset = p
        .get("preset")
        .filter(|s| !s.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| crate::settings::get_str("dns_preset"));
    let Some((primary, secondary)) = dns_preset(&preset) else {
        return RepairOutcome::fail(format!(
            "未知 DNS 预设: {preset}（可用: {}）",
            DNS_PRESETS
                .iter()
                .map(|(n, _, _)| *n)
                .collect::<Vec<_>>()
                .join("/")
        ));
    };
    let name = match p.get("adapter").filter(|s| !s.trim().is_empty()) {
        Some(n) => n.clone(),
        None => match pick_adapter() {
            Some(n) => n,
            None => return RepairOutcome::fail("未指定网卡且无法自动选择"),
        },
    };
    let session = match crate::backup::BackupSession::new() {
        Ok(s) => s,
        Err(e) => return RepairOutcome::fail(format!("备份目录不可用，已保留原配置未修改: {e}")),
    };
    let backup = match backup_dns(&session, &name) {
        Ok(p) => p,
        Err(e) => return RepairOutcome::fail(format!("备份原 DNS 失败，已保留原配置未修改: {e}")),
    };
    // 与 Python 一致的三步：set 主 DNS → add 备用（index=2）→ 清 v6 静态。
    // 注意**不能**把备用写成 `set ... static primary secondary`：netsh 的
    // `set dnsservers` 只接受一个地址，多给会被当成非法参数（第一版即如此）。
    let r1 = w32::run_capture(
        &args(&[
            "netsh",
            "interface",
            "ipv4",
            "set",
            "dnsservers",
            &format!("name={name}"),
            "static",
            primary,
            "primary",
        ]),
        15,
    );
    let r2 = w32::run_capture(
        &args(&[
            "netsh",
            "interface",
            "ipv4",
            "add",
            "dnsservers",
            &format!("name={name}"),
            secondary,
            "index=2",
        ]),
        15,
    );
    // v6 残留静态 DNS 同步清掉：检测端扫的是 v4+v6 混合列表，只改 v4 会让
    // 诊断复检仍 WARN、解析仍可能走污染的 v6 DNS
    let r3 = w32::run_capture(
        &args(&[
            "netsh",
            "interface",
            "ipv6",
            "set",
            "dnsservers",
            &format!("name={name}"),
            "source=dhcp",
        ]),
        15,
    );
    let _ = w32::run_capture(&args(&["ipconfig", "/flushdns"]), 10);
    let ok1 = r1.as_ref().map(|o| o.ok).unwrap_or(false);
    let ok2 = r2.as_ref().map(|o| o.ok).unwrap_or(false);
    let ok3 = r3.as_ref().map(|o| o.ok).unwrap_or(false);
    if ok1 && ok2 {
        RepairOutcome {
            success: true,
            message: format!(
                "{name} DNS → {primary}, {secondary}（{preset}）{}；备份: {backup}",
                if ok3 {
                    "，v6 已恢复自动获取"
                } else {
                    "（v6 恢复自动获取失败）"
                }
            ),
            need_reboot: false,
            output: backup,
        }
    } else {
        let mut errs = String::new();
        if !ok1 {
            errs.push_str(&r1.map(|o| o.tail(0, 100)).unwrap_or_default());
        }
        if !ok2 {
            errs.push_str(&format!(
                " 备用DNS: {}",
                r2.map(|o| o.tail(0, 100)).unwrap_or_default()
            ));
        }
        RepairOutcome {
            success: false,
            message: format!("{name} DNS 设置失败: {errs}"),
            need_reboot: false,
            output: errs,
        }
    }
}

// ---------------- 持久路由：纯逻辑辅助（与 Python repairs.py 对齐） ----------------

/// CIDR 前缀 → 子网掩码（对应 Python `_prefix_to_mask`）。
///
/// legacy `route.exe` 不认 `0.0.0.0/0` 这类写法，删除/还原都必须拆成
/// 「IP + mask」；解析失败回退 `255.255.255.0`（Python except 分支同款）。
pub fn prefix_to_mask(prefix: &str) -> String {
    let bits: u32 = match prefix.split_once('/') {
        Some((_, b)) => match b.trim().parse::<u32>() {
            Ok(n) if n <= 32 => n,
            _ => return "255.255.255.0".to_string(),
        },
        None => return "255.255.255.0".to_string(),
    };
    // bits=0 时 `1u32 << 32` 会溢出 panic，必须特判（Python 大整数无此坑）；
    // u32 左移后高位自然丢弃，无需再 & 0xFFFF_FFFF
    let m: u32 = if bits == 0 {
        0
    } else {
        0xFFFF_FFFFu32 << (32 - bits)
    };
    format!(
        "{}.{}.{}.{}",
        m >> 24,
        (m >> 16) & 0xFF,
        (m >> 8) & 0xFF,
        m & 0xFF
    )
}

/// 备份文件里的还原命令（对应 Python 备份文本里的 `route -p add ...`）。
///
/// metric=0（注册表里「未指定跃点」）按 1 写——还原命令必须可直接粘贴执行。
pub fn route_restore_command(dest: &str, gw: &str, metric: u32) -> String {
    let ip = dest.split('/').next().unwrap_or(dest);
    format!(
        "route -p add {ip} mask {} {gw} metric {}",
        prefix_to_mask(dest),
        if metric == 0 { 1 } else { metric }
    )
}

/// legacy `route delete` 的完整参数：CIDR 拆成 ip + mask。
pub fn route_delete_args(dest: &str, gw: &str) -> Vec<String> {
    let ip = dest.split('/').next().unwrap_or(dest);
    let mask = prefix_to_mask(dest);
    args(&["route", "delete", ip, "mask", &mask, gw])
}

/// 从 `route print -4` 输出解析全部活动默认路由的网关。
///
/// 删除后复核用：不再全量 `snapshot::take()`（秒级 PowerShell 采集），
/// 复用同一条 `route print` 的解析即可回答同一个问题——「该网关还在不在活动表」。
/// 行形如 `0.0.0.0  0.0.0.0  192.168.1.1  192.168.1.100  25`；`On-link` 行被
/// 「恰 3 个点」的 IPv4 检查过滤掉。
pub fn parse_active_default_gws(route_print: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in route_print.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() >= 3 && cols[0] == "0.0.0.0" && cols[1] == "0.0.0.0" {
            let gw = cols[2];
            if gw.matches('.').count() == 3 {
                out.push(gw.to_string());
            }
        }
    }
    out
}

/// 删除后复核（对应 Python 的 gone_p/gone_a）：持久表按 (dest, next_hop) 精确匹配，
/// 活动表按网关匹配。返回 (持久表已清, 活动表已清)。
pub fn route_gone(
    persistent: &[crate::snapshot::RouteEntry],
    active_gws: &[String],
    dest: &str,
    gw: &str,
) -> (bool, bool) {
    let gone_p = !persistent
        .iter()
        .any(|r| r.dest == dest && r.next_hop == gw);
    let gone_a = !active_gws.iter().any(|g| g == gw);
    (gone_p, gone_a)
}

/// 删除劫持流量的持久路由（改前导出路由表存档）。
///
/// 与 Python `DelPersistentRoute` 逐步对齐：route delete 只动**活动表**，
/// 持久副本在 PolicyStore 里重启即复活——必须再补一刀
/// `Remove-NetRoute -PolicyStore PersistentStore`，并复核两张表后才能报成功。
/// 第一版只看 route delete 的退出码，会把「活动表删了、持久表还在」谎报成功。
pub fn run_del_persistent_route(p: &BTreeMap<String, String>) -> RepairOutcome {
    let dest_param = p
        .get("dest")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let gw_param = p
        .get("gw")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    // 目标解析（与 Python 同分支）：只给 dest 时必须沿该网段反查网关——
    // 绝不回落到自动检测，否则用户想删 10.0.0.0/8，实际删掉的却是默认路由
    let (dest, gw, metric) = match (dest_param, gw_param) {
        (Some(dest), Some(gw)) => (dest, gw, 1u32),
        (Some(dest), None) => {
            match crate::snapshot::persistent_routes()
                .into_iter()
                .find(|r| r.dest == dest)
            {
                Some(r) => (r.dest, r.next_hop, r.metric), // 携带真实 metric 供还原命令
                None => {
                    return RepairOutcome::fail(format!(
                        "持久路由中未找到目标网段 {dest}，未删除任何路由"
                    ))
                }
            }
        }
        (None, _) => match crate::snapshot::persistent_routes()
            .into_iter()
            .find(|r| r.dest == "0.0.0.0/0")
        {
            Some(r) => (r.dest, r.next_hop, r.metric),
            // 与 Python 一致：没有可删对象是「无需处理」的正常结束（幂等），不是失败
            None => return RepairOutcome::ok("未发现持久默认路由，无需处理"),
        },
    };
    let dest_ip = dest.split('/').next().unwrap_or(&dest).to_string();
    let mask = prefix_to_mask(&dest);

    let session = match crate::backup::BackupSession::new() {
        Ok(s) => s,
        Err(e) => return RepairOutcome::fail(format!("备份目录不可用，未做修改: {e}")),
    };
    // 删除前把完整路由表存档（还原命令必须是可直接粘贴执行的完整形态）
    let table = w32::run_capture(&args(&["route", "print", "-4"]), 20)
        .map(|o| o.stdout)
        .unwrap_or_default();
    let note = format!(
        "还原命令:\n  {}\n\n删除前完整路由表:\n{table}",
        route_restore_command(&dest, &gw, metric)
    );
    let backup = match session.save_note("route_backup.txt", &note) {
        Ok(p) => p,
        Err(e) => return RepairOutcome::fail(format!("路由表备份失败，未做修改: {e}")),
    };

    // ① 活动表：route.exe 不认 CIDR，必须 ip + mask。
    //    失败不在此处早退：目标可能只在持久表（接口已断开时活动表本就没有），
    //    成败交由下方双表复核统一裁决——Python 即此语义
    let r = w32::run_capture(&route_delete_args(&dest, &gw), 15);
    if let Ok(o) = &r {
        if !o.ok {
            log::warn!(
                "route delete 返回失败（{}），以双表复核为准",
                o.tail(0, 80).trim()
            );
        }
    }
    // ② 持久表：route.exe 的 -p 路由真正存放在 PolicyStore，必须用 NetRoute 清除
    //    （Python 同款兜底；失败不在此处报错，由下方双表复核统一裁决）
    let ps = format!(
        "Remove-NetRoute -DestinationPrefix {dest} -NextHop {gw} -PolicyStore PersistentStore \
         -Confirm:$false -ErrorAction SilentlyContinue; 'done'"
    );
    let _ = w32::run_capture(
        &[
            "powershell".into(),
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
            ps,
        ],
        20,
    );

    // ③ 双表复核：持久表读注册表（权威源），活动表复用 route print 解析
    let persistent_now = crate::snapshot::persistent_routes();
    let active_gws = w32::run_capture(&args(&["route", "print", "-4"]), 15)
        .map(|o| parse_active_default_gws(&o.stdout))
        .unwrap_or_default();
    let (gone_p, gone_a) = route_gone(&persistent_now, &active_gws, &dest, &gw);
    if gone_p && gone_a {
        RepairOutcome::ok(format!(
            "已删除持久路由 {dest} → {gw}（活动表+持久表）；备份/还原命令: {backup}"
        ))
    } else {
        // 格式化不支持内联表达式，先落成局部变量（对应 Python `not gone_a`）
        let residue_a = !gone_a;
        let residue_p = !gone_p;
        RepairOutcome::fail(format!(
            "删除未完全（活动表残留:{residue_a} 持久表残留:{residue_p}），请手动执行: \
             route delete {dest_ip} mask {mask} {gw} 及 \
             Remove-NetRoute -DestinationPrefix {dest} -NextHop {gw} -PolicyStore PersistentStore"
        ))
    }
}

/// 备份网卡的 DNS 配置：v1.3 起结构化 JSON（dns_backup_<适配器净化名>.json）。
///
/// 旧格式是 netsh 输出文本（随 Windows 版本漂移、无法自动还原，评审 4）——
/// 新格式记录 `{adapter, mode, entries:[{family, mode, servers}]}`，回滚侧
/// `backup_restore::restore("dns")` 直接 netsh 写回。服务器按是否含冒号
/// 分流 v4/v6（快照的 dns 列表是混合的）；mode 取 `dns_is_static`。
fn backup_dns(session: &crate::backup::BackupSession, adapter: &str) -> Result<String, String> {
    let snap = crate::snapshot::take();
    let dns: Vec<String> = snap
        .adapters
        .iter()
        .find(|a| a.name == adapter)
        .map(|a| a.dns.clone())
        .unwrap_or_default();
    let (is_static, _) = crate::netcheck::dns_is_static(adapter);
    let mode = if is_static { "static" } else { "dhcp" };
    let v4: Vec<String> = dns.iter().filter(|s| !s.contains(':')).cloned().collect();
    let v6: Vec<String> = dns.iter().filter(|s| s.contains(':')).cloned().collect();
    let mut entries = Vec::new();
    if !v4.is_empty() {
        entries.push(serde_json::json!({"family": "ipv4", "mode": mode, "servers": v4}));
    }
    if !v6.is_empty() {
        entries.push(serde_json::json!({"family": "ipv6", "mode": mode, "servers": v6}));
    }
    if entries.is_empty() {
        entries.push(serde_json::json!({"family": "ipv4", "mode": "dhcp", "servers": []}));
    }
    let payload = serde_json::json!({"adapter": adapter, "mode": mode, "entries": entries});
    let safe = crate::backup::sanitize_name(adapter);
    let fname = format!("dns_backup_{safe}.json");
    session.save_note(
        &fname,
        &serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?,
    )
}

/// 找一张配了静态 DNS 的在线物理网卡（对应 Python `_static_dns_adapter`）。
pub fn static_dns_adapter() -> Option<String> {
    let snap = crate::snapshot::take();
    for a in &snap.adapters {
        if !a.dns.is_empty()
            && !a.virtual_
            && a.status.eq_ignore_ascii_case("up")
            && crate::netcheck::dns_is_static(&a.name).0
        {
            return Some(a.name.clone());
        }
    }
    // WLAN 兜底（未连接时也可能有静态配置）
    for a in &snap.adapters {
        if a.name.eq_ignore_ascii_case("WLAN") && !a.dns.is_empty() {
            return Some(a.name.clone());
        }
    }
    None
}

/// 选一张网卡：优先无线，其次在线的物理网卡（对应 Python `_pick_adapter`）。
pub fn pick_adapter() -> Option<String> {
    let snap = crate::snapshot::take();
    let wifi_names = ["WLAN", "WI-FI", "WI FI", "WIRELESS", "无线网络连接"];
    for a in &snap.adapters {
        let upper = a.name.to_uppercase();
        let normalized = upper.replace('-', " ");
        if wifi_names.contains(&upper.as_str()) || wifi_names.contains(&normalized.as_str()) {
            return Some(a.name.clone());
        }
    }
    for a in &snap.adapters {
        if !a.virtual_ && a.status.eq_ignore_ascii_case("up") && !a.desc.contains("WAN Miniport") {
            return Some(a.name.clone());
        }
    }
    for a in &snap.adapters {
        if !a.virtual_ && !a.status.eq_ignore_ascii_case("up") && !a.desc.contains("WAN Miniport") {
            return Some(a.name.clone());
        }
    }
    None
}

pub(super) fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}
