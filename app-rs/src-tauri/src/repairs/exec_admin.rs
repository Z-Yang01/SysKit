//! 需管理员执行器批次 B（WLAN/网卡/还原点/NCSI 探测/电源/MAC 随机化）。
use super::*;

// ---------------- 执行器批次 B：需要管理员的 8 项 ----------------

/// 管理员门禁：非提升进程直接给出**可解释**的失败，而不是让底层 API 报
/// 模糊的「拒绝访问」。改 HKLM / sc config / netsh 改配置都要过这道门。
pub fn require_admin(id: &str) -> Option<RepairOutcome> {
    if crate::w32::is_elevated() {
        None
    } else {
        Some(RepairOutcome::fail(format!(
            "{id} 需要管理员权限：请以管理员身份重启 SysKit 后重试（当前进程未提升）"
        )))
    }
}

/// 非管理员时该动作是否会被跳过（与 `combo_plan` 同一判据，前端也用它做提示）。
#[allow(dead_code)] // 动作元数据 API 面
pub fn requires_admin(id: &str) -> bool {
    get_action(id)
        .map(|a| a.risk != Risk::Safe)
        .unwrap_or(false)
}

/// 重启 WLAN 服务并重连之前连接的 WiFi。
pub fn run_restart_wlan(_p: &BTreeMap<String, String>) -> RepairOutcome {
    if let Some(o) = require_admin("restart_wlan") {
        return o;
    }
    // 先记住当前 SSID（重连要用），必须在停服务之前读
    let cur = w32::run_capture(&args(&["netsh", "wlan", "show", "interfaces"]), 10);
    let ssid = cur
        .as_ref()
        .map(|o| crate::snapshot::current_ssid(&o.stdout))
        .unwrap_or_default();
    let _ = w32::run_capture(&args(&["net", "stop", "WlanSvc"]), 40);
    std::thread::sleep(std::time::Duration::from_secs(1));
    let start = w32::run_capture(&args(&["net", "start", "WlanSvc"]), 40);
    let started = start.as_ref().map(|o| o.ok).unwrap_or(false);
    if !started {
        return RepairOutcome {
            success: false,
            message: format!(
                "WlanSvc 启动失败: {}",
                start.map(|o| o.tail(150, 0)).unwrap_or_default()
            ),
            need_reboot: false,
            output: String::new(),
        };
    }
    let mut msg = "WlanSvc 已重启".to_string();
    if !ssid.is_empty() {
        std::thread::sleep(std::time::Duration::from_secs(2));
        let c = w32::run_capture(
            &args(&["netsh", "wlan", "connect", &format!("name={ssid}")]),
            15,
        );
        let ok = c.as_ref().map(|o| o.ok).unwrap_or(false);
        msg.push_str(&format!(
            "，已发起重连 {ssid}{}",
            if ok {
                "成功"
            } else {
                "（请手动选择网络）"
            }
        ));
    }
    RepairOutcome {
        success: true,
        message: msg,
        need_reboot: false,
        output: String::new(),
    }
}

/// 禁用并重新启用指定网卡（disable → enable，重建链路）。
pub fn run_toggle_adapter(p: &BTreeMap<String, String>) -> RepairOutcome {
    if let Some(o) = require_admin("toggle_adapter") {
        return o;
    }
    let name = match p.get("adapter").filter(|s| !s.trim().is_empty()) {
        Some(n) => n.clone(),
        None => match pick_adapter() {
            Some(n) => n,
            None => return RepairOutcome::fail("未指定网卡且无法自动选择"),
        },
    };
    let _ = w32::run_capture(
        &args(&[
            "netsh",
            "interface",
            "set",
            "interface",
            &format!("name={name}"),
            "disable",
        ]),
        20,
    );
    // 网卡下电需要时间；太早 enable 会失败（Python 同用 2 秒）
    std::thread::sleep(std::time::Duration::from_secs(2));
    let e = w32::run_capture(
        &args(&[
            "netsh",
            "interface",
            "set",
            "interface",
            &format!("name={name}"),
            "enable",
        ]),
        25,
    );
    match e {
        Ok(o) if o.ok => RepairOutcome::ok(format!("{name} 已重新启用")),
        Ok(o) => RepairOutcome {
            success: false,
            message: format!("启用失败: {}", o.tail(100, 100)),
            need_reboot: false,
            output: o.combined(),
        },
        Err(e) => RepairOutcome::fail(format!("netsh 无法执行: {e}")),
    }
}

/// 创建系统还原点（深度修复/重置前的安全网）。
pub fn run_restore_point(_p: &BTreeMap<String, String>) -> RepairOutcome {
    if let Some(o) = require_admin("restore_point") {
        return o;
    }
    // `-ErrorAction Stop` 必须带：PowerShell 非终止错误的 `-Command` 退出码恒 0，
    // 不转终止的话「系统保护未开启」这类失败会被当成已创建——还原点是组合首位
    // 安全网，假成功不可接受（Python 注释里的实机结论）。
    let r = w32::run_capture(
        &[
            "powershell".into(),
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
            "Checkpoint-Computer -Description 'SysKit 修复前快照' -RestorePointType 'MODIFY_SETTINGS' -ErrorAction Stop"
                .into(),
        ],
        180,
    );
    match r {
        Ok(o) if o.ok => RepairOutcome::ok(
            "已创建系统还原点（若 24 小时内已创建过，Windows 可能静默跳过，不影响继续修复）",
        ),
        Ok(o) => RepairOutcome {
            success: false,
            message: format!(
                "创建系统还原点失败（系统还原可能未开启，不影响继续修复）: {}",
                o.tail(150, 150).trim()
            ),
            need_reboot: false,
            output: o.combined(),
        },
        Err(e) => RepairOutcome::fail(format!("PowerShell 无法执行: {e}")),
    }
}

/// NCSI 探测恢复微软默认并启用主动探测。
pub fn run_ncsi_restore(_p: &BTreeMap<String, String>) -> RepairOutcome {
    if let Some(o) = require_admin("ncsi_restore") {
        return o;
    }
    let session = match crate::repairs::backup::BackupSession::new() {
        Ok(s) => s,
        Err(e) => return RepairOutcome::fail(format!("备份目录不可用，未做修改: {e}")),
    };
    let backup = session.export_reg(crate::netcheck::NCSI_REG_FULL).ok();
    let key = crate::netcheck::NCSI_REG_FULL;
    let mut failed = Vec::new();
    if !w32::reg_set_dword(key, "EnableActiveProbing", 1) {
        failed.push("EnableActiveProbing".to_string());
    }
    for (name, val) in crate::netcheck::ncsi_stock() {
        if name == "EnableActiveProbing" {
            continue;
        }
        if !w32::reg_set_string(key, name, val) {
            failed.push(name.to_string());
        }
    }
    if !failed.is_empty() {
        return RepairOutcome {
            success: false,
            message: format!("写注册表失败（需要管理员）: {}", failed.join(", ")),
            need_reboot: false,
            output: String::new(),
        };
    }
    let _ = w32::run_capture(&args(&["sc", "start", "NlaSvc"]), 20);
    RepairOutcome {
        success: true,
        message: format!(
            "NCSI 已恢复默认并启用主动探测{}",
            match backup {
                Some(p) => format!("；备份: {p}"),
                None => "（注册表备份失败，但修改已成功）".to_string(),
            }
        ),
        need_reboot: false,
        output: String::new(),
    }
}

/// 关闭 NCSI 主动探测（最后手段）。
pub fn run_ncsi_disable(_p: &BTreeMap<String, String>) -> RepairOutcome {
    if let Some(o) = require_admin("ncsi_disable") {
        return o;
    }
    let session = match crate::repairs::backup::BackupSession::new() {
        Ok(s) => s,
        Err(e) => return RepairOutcome::fail(format!("备份目录不可用，未做修改: {e}")),
    };
    let backup = session.export_reg(crate::netcheck::NCSI_REG_FULL).ok();
    if !w32::reg_set_dword(crate::netcheck::NCSI_REG_FULL, "EnableActiveProbing", 0) {
        return RepairOutcome::fail("写注册表失败（需要管理员）");
    }
    RepairOutcome {
        success: true,
        message: format!(
            "已关闭主动探测{}",
            match backup {
                Some(p) => format!("；备份: {p}"),
                None => String::new(),
            }
        ),
        need_reboot: false,
        output: String::new(),
    }
}

/// 应跳过电源管理的网卡描述关键字（对应 Python 的排除清单）。
pub const POWER_SKIP_KEYWORDS: [&str; 4] =
    ["wan miniport", "microsoft kernel", "bluetooth", "virtual"];

/// 该网卡描述是否应跳过电源管理写入。
pub fn power_skip(desc: &str) -> bool {
    let lower = desc.to_lowercase();
    POWER_SKIP_KEYWORDS.iter().any(|k| lower.contains(k))
}

/// 禁止系统关闭网卡省电（PnPCapabilities=24 = REG_DWORD 0x18）。
pub fn run_power_off(_p: &BTreeMap<String, String>) -> RepairOutcome {
    if let Some(o) = require_admin("power_off") {
        return o;
    }
    const NIC_CLASS_KEY: &str =
        r"HKLM\SYSTEM\CurrentControlSet\Control\Class\{4d36e972-e325-11ce-bfc1-08002be10318}";
    let session = match crate::repairs::backup::BackupSession::new() {
        Ok(s) => s,
        Err(e) => return RepairOutcome::fail(format!("备份目录不可用，未做修改: {e}")),
    };
    let mut changed = Vec::new();
    let mut skipped = Vec::new();
    for sub in 0..32 {
        let idx = format!("{sub:04}");
        let full = format!("{NIC_CLASS_KEY}\\{idx}");
        let vals = w32::reg_values(&full);
        if vals.is_empty() {
            continue;
        }
        let desc = vals
            .get("DriverDesc")
            .map(RegValue::as_compare_str)
            .unwrap_or_default();
        if desc.is_empty() || power_skip(&desc) {
            continue;
        }
        if vals.get("PnPCapabilities").and_then(RegValue::as_u32) == Some(24) {
            continue; // 已经是 24，无需改
        }
        let current = vals
            .get("PnPCapabilities")
            .map(RegValue::as_compare_str)
            .unwrap_or_else(|| "<缺省=允许断电>".to_string());
        let _ = session.save_note(
            &format!("pnp_backup_{idx}.txt"),
            &format!("{full}\nDriverDesc={desc}\nPnPCapabilities={current}"),
        );
        // 必须是 REG_DWORD 0x18：写成 REG_SZ "24" 不被 NDIS 电源策略识别，
        // 修复会**静默无效**——而读回 str()=="24" 恰好自洽，形成假成功
        if w32::reg_set_dword(&full, "PnPCapabilities", 24) {
            changed.push(desc.chars().take(30).collect::<String>());
        } else {
            skipped.push(format!(
                "{}: 写入失败",
                desc.chars().take(30).collect::<String>()
            ));
        }
    }
    // USB 选择性暂停
    let usb = w32::run_capture(
        &args(&[
            "powercfg",
            "/setacvalueindex",
            "SCHEME_CURRENT",
            "2a737441-1930-4402-8d77-b2bebba308a3",
            "48e6b7a6-50f5-4782-a5d4-53bb8f07e226",
            "0",
        ]),
        15,
    );
    let _ = w32::run_capture(&args(&["powercfg", "/setactive", "SCHEME_CURRENT"]), 15);
    let usb_ok = usb.as_ref().map(|o| o.ok).unwrap_or(false);

    if changed.is_empty() {
        return RepairOutcome {
            success: false,
            message: format!(
                "没有需要修改的网卡；USB选择性暂停:{}",
                if usb_ok { "OK" } else { "FAIL" }
            ),
            need_reboot: false,
            output: String::new(),
        };
    }
    let mut msg = format!(
        "已对 {} 个网卡禁止断电: {}；USB 选择性暂停{}",
        changed.len(),
        changed.join(", "),
        if usb_ok { "已关闭" } else { "关闭失败" }
    );
    if !skipped.is_empty() {
        msg.push_str(&format!("；跳过: {}", skipped.join(", ")));
    }
    msg.push_str("。重新启用网卡或重启后生效。");
    RepairOutcome {
        success: true,
        message: msg,
        need_reboot: false,
        output: String::new(),
    }
}

/// 改写 WiFi 配置 XML 里的 EnableRandomization（纯函数，便于单测）。
///
/// 返回 (新内容, 是否发生改动)。两级策略与 Python 一致：
/// 1. 已有 `<MacRandomization>` 块 → 替换其中的 true/false；
/// 2. 完全没有该块 → 在 `</WLANProfile>` 前插入一个（带命名空间）。
pub fn set_mac_randomization_xml(content: &str, enable: bool) -> (String, bool) {
    let want = if enable { "true" } else { "false" };
    // 手写替换：找出 MacRandomization 块，再在其中替换第一个 true/false
    let lower = content.to_lowercase();
    if let Some(start) = lower.find("<macrandomization") {
        if let Some(end_rel) = lower[start..].find("</macrandomization>") {
            let end = start + end_rel + "</macrandomization>".len();
            let block = &content[start..end];
            let rebuilt = rebuild_mac_block(block, want);
            let mut out = String::with_capacity(content.len());
            out.push_str(&content[..start]);
            out.push_str(&rebuilt);
            out.push_str(&content[end..]);
            let changed = out != content;
            return (out, changed);
        }
    }
    // 没有该块：插入到 </WLANProfile> 前
    if let Some(pos) = find_close_wlan_profile(content) {
        let insert = format!(
            "<MacRandomization xmlns=\"http://www.microsoft.com/networking/WLAN/profile/v3\"><EnableRandomization>{want}</EnableRandomization></MacRandomization>"
        );
        let mut out = String::with_capacity(content.len() + insert.len());
        out.push_str(&content[..pos]);
        out.push_str(&insert);
        out.push_str(&content[pos..]);
        return (out, true);
    }
    (content.to_string(), false)
}

/// 保留原块的属性与空白，只把 EnableRandomization 的值换成 want。
fn rebuild_mac_block(block: &str, want: &str) -> String {
    let lower = block.to_lowercase();
    let key = "<enablerandomization>";
    let Some(kpos) = lower.find(key) else {
        return block.to_string();
    };
    let val_start = kpos + key.len();
    let close = "</enablerandomization>";
    let Some(rel_end) = lower[val_start..].find(close) else {
        return block.to_string();
    };
    let val_end = val_start + rel_end;
    let mut out = String::with_capacity(block.len());
    out.push_str(&block[..val_start]);
    out.push_str(want);
    out.push_str(&block[val_end..]);
    out
}

/// 找 `</WLANProfile>`（大小写不敏感）的起始位置。
fn find_close_wlan_profile(content: &str) -> Option<usize> {
    content.to_lowercase().find("</wlanprofile>")
}

/// 当前/第一个 WiFi 配置文件名（对应 Python `_current_or_first_profile`）。
pub fn current_or_first_profile() -> String {
    let cur = w32::run_capture(&args(&["netsh", "wlan", "show", "interfaces"]), 10);
    let ssid = cur
        .as_ref()
        .map(|o| crate::snapshot::current_ssid(&o.stdout))
        .unwrap_or_default();
    if !ssid.is_empty() {
        return ssid;
    }
    wlan_profiles().into_iter().next().unwrap_or_default()
}

/// 已保存的 WiFi 配置文件名（对应 Python `wlan_profiles`）。
pub fn wlan_profiles() -> Vec<String> {
    let r = w32::run_capture(&args(&["netsh", "wlan", "show", "profiles"]), 10);
    let Ok(o) = r else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in o.stdout.lines() {
        let t = line.trim();
        for prefix in ["所有用户配置文件", "All User Profile"] {
            if let Some(rest) = t.strip_prefix(prefix) {
                if let Some((_, v)) = rest.split_once(':') {
                    let v = v.trim();
                    if !v.is_empty() {
                        out.push(v.to_string());
                    }
                }
            }
        }
    }
    out
}

/// 查找指定 SSID 的 WiFi 配置文件路径（`%ProgramData%\Microsoft\Wlansvc\Profiles\Interfaces\*\*.xml`）。
pub fn find_wlan_profile_files(profile: &str) -> Vec<std::path::PathBuf> {
    let root = std::path::PathBuf::from(
        std::env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".to_string()),
    )
    .join(r"Microsoft\Wlansvc\Profiles\Interfaces");
    let mut out = Vec::new();
    let Ok(dirs) = std::fs::read_dir(&root) else {
        return out;
    };
    let want = profile.to_lowercase();
    for d in dirs.flatten() {
        let Ok(files) = std::fs::read_dir(d.path()) else {
            continue;
        };
        for f in files.flatten() {
            let p = f.path();
            if p.extension().map(|e| e != "xml").unwrap_or(true) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&p) else {
                continue;
            };
            // 配置文件名藏在 <name>...</name> 里
            if let Some(name) = extract_xml_name(&text) {
                if name.to_lowercase() == want {
                    out.push(p);
                }
            }
        }
    }
    out
}

/// 取 WiFi 配置 XML 里的 `<name>` 值。
pub fn extract_xml_name(content: &str) -> Option<String> {
    let lower = content.to_lowercase();
    let start = lower.find("<name>")? + "<name>".len();
    let end = lower[start..].find("</name>")? + start;
    Some(content[start..end].trim().to_string())
}

/// 读写指定 WiFi 配置文件的 MAC 随机化状态：返回 (改动文件数, 错误, 备份目录)。
pub fn set_mac_random(profile: &str, enable: bool) -> (usize, String, String) {
    if profile.is_empty() {
        return (0, "未指定 WiFi 配置文件".to_string(), String::new());
    }
    let files = find_wlan_profile_files(profile);
    if files.is_empty() {
        return (0, format!("未找到名为 {profile} 的配置文件"), String::new());
    }
    let session = match crate::repairs::backup::BackupSession::new() {
        Ok(s) => s,
        Err(e) => return (0, format!("备份目录不可用，未做修改: {e}"), String::new()),
    };
    let mut changed = 0usize;
    for f in files {
        let Ok(content) = std::fs::read_to_string(&f) else {
            continue;
        };
        let base = f
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "profile.xml".to_string());
        let _ = session.save_note(&base, &content);
        let (new, did) = set_mac_randomization_xml(&content, enable);
        if did && std::fs::write(&f, new).is_ok() {
            changed += 1;
        }
    }
    (
        changed,
        String::new(),
        session.dir().to_string_lossy().to_string(),
    )
}

/// 读取指定配置文件的 MAC 随机化状态：Some(true/false) 或 None（未知）。
#[allow(dead_code)] // MAC 随机化状态读取（对齐面，当前诊断走另一路径）
pub fn mac_random_state(profile: &str) -> Option<bool> {
    for f in find_wlan_profile_files(profile) {
        let Ok(content) = std::fs::read_to_string(&f) else {
            continue;
        };
        return mac_randomization_state_xml(&content);
    }
    None
}

/// 从配置 XML 解析 EnableRandomization（纯函数）。
#[allow(dead_code)] // 同上
pub fn mac_randomization_state_xml(content: &str) -> Option<bool> {
    let lower = content.to_lowercase();
    let start = lower.find("<macrandomization")?;
    let block = &lower[start..];
    let key = "<enablerandomization>";
    let kpos = block.find(key)? + key.len();
    let rest = &block[kpos..];
    if rest.trim_start().starts_with("true") {
        Some(true)
    } else if rest.trim_start().starts_with("false") {
        Some(false)
    } else {
        None
    }
}

/// 关闭 WiFi 的 MAC 随机化（用真实地址连接）。
pub fn run_mac_random_off(p: &BTreeMap<String, String>) -> RepairOutcome {
    mac_random_apply(p, false, "关闭")
}

/// 开启 WiFi 的 MAC 随机化（恢复 Win11 默认）。
pub fn run_mac_random_on(p: &BTreeMap<String, String>) -> RepairOutcome {
    mac_random_apply(p, true, "开启")
}

fn mac_random_apply(p: &BTreeMap<String, String>, enable: bool, verb: &str) -> RepairOutcome {
    if let Some(o) = require_admin("mac_random") {
        return o;
    }
    let profile = match p.get("profile").filter(|s| !s.trim().is_empty()) {
        Some(n) => n.clone(),
        None => current_or_first_profile(),
    };
    let (changed, err, dir) = set_mac_random(&profile, enable);
    if !err.is_empty() {
        return RepairOutcome::fail(err);
    }
    if changed == 0 {
        return RepairOutcome::ok(format!(
            "{profile} 的 MAC 随机化本来就是{}的",
            if enable { "开启" } else { "关闭" }
        ));
    }
    // 改完要重连才生效（与 Python 一致：调用 restart_wlan）
    let r = run_restart_wlan(&BTreeMap::new());
    RepairOutcome {
        success: true,
        message: format!(
            "已{verb} {profile} 的 MAC 随机化（{changed} 个文件，备份: {dir}）；{}",
            r.message
        ),
        need_reboot: false,
        output: dir,
    }
}
