//! M4 工具箱命令：Hosts / 端口进程 / DNS 子网 ping 路由 / DNS 测速 / 应用联网 / 局域网。
use super::args;

// ---------------- M4 工具箱：Hosts 编辑 ----------------

/// hosts 文件路径与可写性（前端展示用）。
#[tauri::command]
pub fn sk_hosts_info() -> serde_json::Value {
    let path = std::path::Path::new(crate::hostsed::HOSTS_PATH);
    let exists = path.exists();
    let writable = std::fs::metadata(path)
        .map(|m| !m.permissions().readonly())
        .unwrap_or(false);
    serde_json::json!({
        "path": crate::hostsed::HOSTS_PATH,
        "exists": exists,
        "writable": writable,
        "is_admin": crate::w32::is_elevated(),
    })
}

/// 读取并解析 hosts。
#[tauri::command]
pub async fn sk_hosts_read() -> Vec<crate::hostsed::HostEntry> {
    tauri::async_runtime::spawn_blocking(|| crate::hostsed::read_entries(None))
        .await
        .unwrap_or_default()
}

/// 追加一条映射（仅内存，返回全量列表供前端刷新）。
#[tauri::command]
pub async fn sk_hosts_add(
    ip: String,
    host: String,
    comment: Option<String>,
    aliases: Option<Vec<String>>,
) -> Vec<crate::hostsed::HostEntry> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut entries = crate::hostsed::read_entries(None);
        crate::hostsed::add_entry(
            &mut entries,
            &ip,
            &host,
            comment.as_deref().unwrap_or(""),
            &aliases.unwrap_or_default(),
        );
        entries
    })
    .await
    .unwrap_or_default()
}

/// 删除第 index 条（仅内存）。
#[tauri::command]
pub async fn sk_hosts_remove(index: usize) -> Vec<crate::hostsed::HostEntry> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut entries = crate::hostsed::read_entries(None);
        crate::hostsed::remove_entry(&mut entries, index);
        entries
    })
    .await
    .unwrap_or_default()
}

/// 启用/禁用第 index 条（仅内存）。
#[tauri::command]
pub async fn sk_hosts_toggle(index: usize) -> Vec<crate::hostsed::HostEntry> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut entries = crate::hostsed::read_entries(None);
        crate::hostsed::toggle_entry(&mut entries, index);
        entries
    })
    .await
    .unwrap_or_default()
}

/// 写回 hosts（自动备份；编码可疑时返回 Err 拒绝写入）。
#[tauri::command]
pub async fn sk_hosts_write(
    entries: Vec<crate::hostsed::HostEntry>,
    backup: Option<bool>,
) -> Result<serde_json::Value, String> {
    let do_backup = backup.unwrap_or(true);
    tauri::async_runtime::spawn_blocking(move || {
        crate::hostsed::write_entries(&entries, None, do_backup).map(|backup_path| {
            serde_json::json!({
                "ok": true,
                "backup": backup_path,
                "message": if backup_path.is_empty() {
                    "已写入（原文件不存在，无备份）".to_string()
                } else {
                    format!("已写入；备份: {backup_path}")
                },
            })
        })
    })
    .await
    .map_err(|e| format!("写入线程失败: {e}"))?
}

/// 只做备份（手动「备份一次」按钮）。
#[tauri::command]
pub async fn sk_hosts_backup() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(|| {
        crate::hostsed::backup_hosts(None).map(|p| p.unwrap_or_default())
    })
    .await
    .map_err(|e| format!("备份线程失败: {e}"))?
}

// ---------------- M4 工具箱：端口 / 进程 ----------------

/// 端口与进程列表：`netstat -ano` + `tasklist` 合并。
#[tauri::command]
pub async fn sk_ports_list() -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let ns = crate::w32::run_capture(&args(&["netstat", "-ano"]), 30)
            .map_err(|e| format!("netstat 无法执行: {e}"))?;
        // tasklist 失败不致命：只是进程名缺失
        let tl = crate::w32::run_capture(&args(&["tasklist", "/fo", "csv", "/nh"]), 30)
            .map(|o| o.stdout)
            .unwrap_or_default();
        let procs = crate::portscan::parse_tasklist_csv(&tl);
        let rows: Vec<serde_json::Value> = crate::portscan::parse_netstat(&ns.stdout)
            .into_iter()
            .map(|r| {
                let pid = r.pid;
                serde_json::json!({
                    "proto": r.proto,
                    "local": r.local,
                    "remote": r.remote,
                    "state": r.state,
                    "pid": pid,
                    "process": procs.get(&pid).cloned().unwrap_or_default(),
                    "listening": r.is_listening(),
                    "port": r.local_port(),
                })
            })
            .collect();
        Ok(serde_json::json!({
            "rows": rows,
            "total": rows.len(),
        }))
    })
    .await
    .map_err(|e| format!("采集线程失败: {e}"))?
}

/// 结束指定进程（危险操作，前端必须二次确认）。
#[tauri::command]
pub async fn sk_kill_process(pid: u32) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if pid == 0 {
            return Err("PID 0 是系统空闲进程，不允许结束".to_string());
        }
        if pid == 4 {
            return Err("PID 4 是系统进程，不允许结束".to_string());
        }
        if pid == std::process::id() {
            return Err("不允许结束 SysKit 自身".to_string());
        }
        let out = crate::w32::run_capture(&args(&["taskkill", "/PID", &pid.to_string(), "/F"]), 15)
            .map_err(|e| format!("taskkill 无法执行: {e}"))?;
        if out.ok {
            Ok(format!("已结束进程 PID {pid}"))
        } else {
            Err(format!("结束失败: {}", out.tail(0, 150).trim()))
        }
    })
    .await
    .map_err(|e| format!("执行线程失败: {e}"))?
}

// ---------------- M4 工具箱：DNS 查询 / 子网计算 / ping / 路由 ----------------

/// 子网计算（纯计算，秒回）。
#[tauri::command]
pub fn sk_subnet_calc(ip: String, prefix: u8) -> Result<serde_json::Value, String> {
    let info = crate::subnet::subnet_info(&ip, prefix)?;
    let lines = crate::subnet::subnet_lines(&info);
    Ok(serde_json::json!({
        "info": info,
        "lines": lines,
        "presets": crate::subnet::DNS_SERVER_PRESETS
            .iter()
            .map(|(l, s)| serde_json::json!({ "label": l, "server": s }))
            .collect::<Vec<_>>(),
    }))
}

/// DNS 查询：`server` 为空走系统解析器，否则直查指定服务器（自研 UDP 客户端）。
#[tauri::command]
pub async fn sk_dns_query(
    domain: String,
    server: Option<String>,
) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let domain = domain.trim().to_string();
        if domain.is_empty() {
            return Err("域名不能为空".to_string());
        }
        let server = server.unwrap_or_default();
        let start = std::time::Instant::now();
        if server.trim().is_empty() {
            let ips = crate::dnsclient::resolve_system(&domain, 4.0);
            let ms = start.elapsed().as_millis();
            if ips.is_empty() {
                return Ok(serde_json::json!({
                    "ok": false, "domain": domain, "server": "系统默认",
                    "ips": [], "ms": ms,
                    "message": "系统解析失败/超时（可能是 DNS 故障或域名不存在）",
                }));
            }
            Ok(serde_json::json!({
                "ok": true, "domain": domain, "server": "系统默认",
                "ips": ips, "ms": ms, "message": format!("解析成功，用时 {ms} ms"),
            }))
        } else {
            let s = server.trim().to_string();
            match crate::dnsclient::query_with(
                &domain,
                &s,
                crate::dnsclient::DEFAULT_TIMEOUT_SECS,
                crate::dnsclient::QTYPE_A,
                crate::dnsclient::DEFAULT_ATTEMPTS,
            ) {
                Ok(ips) => {
                    let ms = start.elapsed().as_millis();
                    Ok(serde_json::json!({
                        "ok": true, "domain": domain, "server": s,
                        "ips": ips, "ms": ms, "message": format!("解析成功，用时 {ms} ms"),
                    }))
                }
                Err(e) => {
                    let ms = start.elapsed().as_millis();
                    Ok(serde_json::json!({
                        "ok": false, "domain": domain, "server": s,
                        "ips": [], "ms": ms, "message": e.to_string(),
                    }))
                }
            }
        }
    })
    .await
    .map_err(|e| format!("查询线程失败: {e}"))?
}

/// ping 一个主机（`ping.exe`，复用 netcheck 的丢包率解析）。
#[tauri::command]
pub async fn sk_ping(host: String, count: Option<u32>) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let host = host.trim().to_string();
        if host.is_empty() {
            return Err("主机不能为空".to_string());
        }
        let n = count.unwrap_or(4).clamp(1, 20).to_string();
        let out = crate::w32::run_capture(&args(&["ping", "-n", &n, "-w", "1000", &host]), 40)
            .map_err(|e| format!("ping 无法执行: {e}"))?;
        let combined = out.combined();
        let loss = crate::netcheck::ping_loss(&combined);
        let latency = crate::portscan::parse_ping_ms(&combined);
        Ok(serde_json::json!({
            "ok": out.ok,
            "host": host,
            "loss_percent": loss,
            "latency_ms": latency,
            "raw": combined,
        }))
    })
    .await
    .map_err(|e| format!("ping 线程失败: {e}"))?
}

/// 路由跟踪（`tracert`），返回逐跳解析结果。
#[tauri::command]
pub async fn sk_tracert(host: String, max_hops: Option<u32>) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let host = host.trim().to_string();
        if host.is_empty() {
            return Err("主机不能为空".to_string());
        }
        let hops = max_hops.unwrap_or(15).clamp(1, 30).to_string();
        let out = crate::w32::run_capture(
            &args(&["tracert", "-d", "-h", &hops, "-w", "1000", &host]),
            120,
        )
        .map_err(|e| format!("tracert 无法执行: {e}"))?;
        let parsed = crate::portscan::parse_tracert(&out.stdout);
        Ok(serde_json::json!({
            "ok": out.ok,
            "host": host,
            "hops": parsed,
            "count": parsed.len(),
            "raw": out.combined(),
        }))
    })
    .await
    .map_err(|e| format!("tracert 线程失败: {e}"))?
}

// ---------------- M4 工具箱：DNS 测速 ----------------

/// DNS 测速：并发测各公共 DNS 的解析延迟/成功率，并推荐最优。
#[tauri::command]
pub async fn sk_dns_speed(domain: String, times: Option<u32>) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let domain = domain.trim().to_string();
        if domain.is_empty() {
            return Err("测试域名不能为空".to_string());
        }
        let times = times.unwrap_or(3).clamp(1, 5);
        let results = crate::dnsspeed::run_all(&domain, times);
        let (usable, failed) = crate::dnsspeed::summarize(&results);

        // 测速历史：紧凑记录可用台的延迟（全失败台不入列），落盘最近 20 轮
        {
            let path = crate::paths::dns_speed_history_file();
            let mut hist = crate::dnshistory::load(&path);
            crate::dnshistory::push(
                &mut hist,
                crate::dnshistory::DnsSpeedRound {
                    at: crate::timefmt::now_fmt(crate::timefmt::FMT_MINUTE_COMPACT),
                    latencies: results
                        .iter()
                        .filter_map(|p| p.avg_ms.map(|ms| (p.name.clone(), ms)))
                        .collect(),
                },
            );
            if !crate::dnshistory::save(&path, &hist) {
                log::warn!("DNS 测速历史落盘失败（不影响本次结果）");
            }
        }

        Ok(serde_json::json!({
            "domain": domain,
            "times": times,
            "results": results,
            "usable": usable,
            "failed": failed,
            "recommendation": crate::dnsspeed::recommendation_text(&results),
        }))
    })
    .await
    .map_err(|e| format!("测速线程失败: {e}"))?
}

/// DNS 测速历史（最近 N 轮，测速页趋势对比用）。
#[tauri::command]
pub fn sk_dns_speed_history() -> serde_json::Value {
    let h = crate::dnshistory::load(&crate::paths::dns_speed_history_file());
    serde_json::to_value(&h.rounds).unwrap_or_else(|_| serde_json::json!([]))
}

/// 下载测速能力状态（WinInet 接入后恒可用）。
#[tauri::command]
pub fn sk_speedtest_status() -> serde_json::Value {
    serde_json::json!({
        "available": true,
        "urls": crate::netdl::TEST_URLS,
    })
}

/// 命令面板查询（算式直算 + 内置/自定义动作排序）。
#[tauri::command]
pub fn sk_palette_query(q: String) -> Vec<serde_json::Value> {
    crate::palette::query(&q, &crate::settings::get_str("palette_commands"))
        .into_iter()
        .map(|(id, label, hint)| serde_json::json!({ "id": id, "label": label, "hint": hint }))
        .collect()
}

/// 中止进行中的下载测速：当前 URL 尽快收尾（已计量部分照常返回），
/// 前端同时停发后续源。新开一轮前必须调 `sk_download_reset` 清标志，
/// 否则首个源会立刻中断。
#[tauri::command]
pub fn sk_download_cancel() {
    crate::netdl::cancel();
}

/// 清除测速中止标志（前端「开始测速」时调用，确保新一轮不受上轮取消影响）。
#[tauri::command]
pub fn sk_download_reset() {
    crate::netdl::reset_cancel();
}

/// 单源下载测速（前端逐源调用实现真进度：每源完成即点亮一行）。
#[tauri::command]
pub async fn sk_download_test_one(
    url: String,
    seconds: u64,
) -> Result<crate::netdl::UrlResult, String> {
    let secs = seconds.clamp(2, 30);
    let r = tauri::async_runtime::spawn_blocking(move || crate::netdl::probe_one(&url, secs))
        .await
        .map_err(|e| format!("测速线程失败: {e}"))?;
    Ok(r)
}

/// 下载测速：逐 URL 独立开窗 `seconds` 秒（WinInet 系统栈，零新依赖）。
/// 阻塞池执行；`netdl::cancel()` 支持中止（已计量 URL 照常返回）。
#[tauri::command]
pub async fn sk_download_test(seconds: u64) -> Result<serde_json::Value, String> {
    let secs = seconds.clamp(2, 30);
    let results = tauri::async_runtime::spawn_blocking(move || crate::netdl::run_probe_all(secs))
        .await
        .map_err(|e| format!("测速线程失败: {e}"))?;
    let ok = results.iter().filter(|r| r.ok).count();
    let best = results
        .iter()
        .filter(|r| r.ok)
        .map(|r| r.peak_bps)
        .fold(0.0f64, f64::max);
    Ok(serde_json::json!({
        "results": results,
        "ok_count": ok,
        "best_peak_bps": best,
    }))
}

// ---------------- M4 工具箱：应用联网 ----------------

/// 台账 + 实际规则状态合并（三态：on/off/drift/missing）。
#[tauri::command]
pub async fn sk_appnet_list() -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let entries = crate::appnet::load_entries();
        let programs: Vec<String> = entries.iter().map(|e| e.program.clone()).collect();
        let states = query_states(&programs);
        let rows: Vec<serde_json::Value> = entries
            .iter()
            .map(|e| {
                let state = states
                    .get(&e.rule)
                    .copied()
                    .unwrap_or(crate::appnet::RuleState::Missing);
                serde_json::json!({
                    "program": e.program,
                    "rule": e.rule,
                    "blocked": e.blocked,
                    "added": e.added,
                    "state": state.as_str(),
                    "blocking": state.is_blocking(),
                })
            })
            .collect();
        Ok(serde_json::json!({
            "rows": rows,
            "count": rows.len(),
            "is_admin": crate::w32::is_elevated(),
        }))
    })
    .await
    .map_err(|e| format!("查询线程失败: {e}"))?
}

/// 批量查规则状态：PS 一次问完（避免每条规则起一次 powershell）。
///
/// 没有哨兵 = `Get-NetFirewallRule` 跑不动（模块缺失/被策略禁用），此时**退回 netsh
/// 逐条查**——不能把"查不到"当成"规则不存在"，否则界面会显示"未阻断"而实际在拦。
fn query_states(
    programs: &[String],
) -> std::collections::BTreeMap<String, crate::appnet::RuleState> {
    if programs.is_empty() {
        return std::collections::BTreeMap::new();
    }
    let script = crate::appnet::ps_status_script(programs);
    if let Ok(out) = crate::w32::run_capture(
        &[
            "powershell".into(),
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
            script,
        ],
        40,
    ) {
        let combined = out.combined();
        if combined.contains(crate::appnet::PS_PROBE) {
            return crate::appnet::parse_status_output(&combined);
        }
    }
    // 退回 netsh（较慢，但至少能给出答案）
    let mut map = std::collections::BTreeMap::new();
    for p in programs {
        let cmd = crate::appnet::cmd_show(p);
        let state = match crate::w32::run_capture(&cmd, 15) {
            Ok(o) => match crate::appnet::parse_netsh_state(&o.combined()) {
                Some((enabled, action)) => {
                    if !crate::appnet::action_is_block(&action) {
                        crate::appnet::RuleState::Drift
                    } else if enabled {
                        crate::appnet::RuleState::On
                    } else {
                        crate::appnet::RuleState::Off
                    }
                }
                None => crate::appnet::RuleState::Missing,
            },
            Err(_) => crate::appnet::RuleState::Missing,
        };
        map.insert(crate::appnet::rule_name(p), state);
    }
    map
}

/// 添加（或对已存在条目改状态）一个受控程序。
#[tauri::command]
pub async fn sk_appnet_add(program: String, blocked: bool) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if !crate::w32::is_elevated() {
            return Err(crate::appnet::ADMIN_MSG.to_string());
        }
        let path = crate::appnet::real_path(&program);
        let exists = std::path::Path::new(&path).is_file();
        crate::appnet::validate_program(&program, exists)?;

        let mut entries = crate::appnet::load_entries();
        let existing = crate::appnet::find_entry(&entries, &path);
        // 台账里已有 → 只切启用状态（不重复 add，否则 netsh 报规则已存在）
        let cmd = match existing {
            Some(_) => crate::appnet::cmd_set_enable(&path, blocked),
            None => crate::appnet::cmd_add(&path, blocked),
        };
        let out = crate::w32::run_capture(&cmd, 30).map_err(|e| format!("netsh 无法执行: {e}"))?;
        if !out.ok {
            return Err(format!("添加规则失败: {}", out.tail(0, 200).trim()));
        }
        // 确认规则真的按预期生效（不自证：重新查一次）
        let states = query_states(std::slice::from_ref(&path));
        let state = states
            .get(&crate::appnet::rule_name(&path))
            .copied()
            .unwrap_or(crate::appnet::RuleState::Missing);
        let expect = if blocked {
            crate::appnet::RuleState::On
        } else {
            crate::appnet::RuleState::Off
        };
        if state != expect {
            return Err(format!(
                "规则已下发但状态不符（期望 {}，实际 {}）——可能被组策略覆盖",
                expect.as_str(),
                state.as_str()
            ));
        }
        if existing.is_none() {
            entries.push(crate::appnet::AppNetEntry {
                program: path.clone(),
                rule: crate::appnet::rule_name(&path),
                blocked,
                added: crate::timefmt::now_fmt(crate::timefmt::FMT_TS),
            });
            crate::appnet::save_entries(&entries);
        } else if let Some(i) = existing {
            entries[i].blocked = blocked;
            crate::appnet::save_entries(&entries);
        }
        Ok(serde_json::json!({
            "ok": true,
            "program": path,
            "state": state.as_str(),
            "message": if blocked {
                format!("{path} 已禁止联网（出站阻断）")
            } else {
                format!("{path} 已放行")
            },
        }))
    })
    .await
    .map_err(|e| format!("执行线程失败: {e}"))?
}

/// 移除受控程序：删规则 + 从台账移除。
#[tauri::command]
pub async fn sk_appnet_remove(program: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if !crate::w32::is_elevated() {
            return Err(crate::appnet::ADMIN_MSG.to_string());
        }
        let out = crate::w32::run_capture(&crate::appnet::cmd_delete(&program), 30)
            .map_err(|e| format!("netsh 无法执行: {e}"))?;
        // 规则本来就不在也算成功（用户目标已达成），但必须确认它确实不在
        let states = query_states(std::slice::from_ref(&program));
        let still = states
            .get(&crate::appnet::rule_name(&program))
            .copied()
            .unwrap_or(crate::appnet::RuleState::Missing);
        if still != crate::appnet::RuleState::Missing {
            return Err(format!(
                "删除未生效（规则仍在，状态 {}）：{}",
                still.as_str(),
                out.tail(0, 150).trim()
            ));
        }
        let mut entries = crate::appnet::load_entries();
        if let Some(i) = crate::appnet::find_entry(&entries, &program) {
            entries.remove(i);
            crate::appnet::save_entries(&entries);
        }
        Ok(format!(
            "已移除 {} 的联网控制规则",
            crate::appnet::real_path(&program)
        ))
    })
    .await
    .map_err(|e| format!("执行线程失败: {e}"))?
}

/// 校验路径（前端在提交前调用，给出即时反馈）。
#[tauri::command]
pub fn sk_appnet_validate(program: String) -> Result<serde_json::Value, String> {
    let path = crate::appnet::real_path(&program);
    let exists = std::path::Path::new(&path).is_file();
    Ok(serde_json::json!({
        "program": path,
        "exists": exists,
        "rule": crate::appnet::rule_name(&program),
        "error": crate::appnet::validate_program(&program, exists).err().unwrap_or_default(),
        "is_admin": crate::w32::is_elevated(),
        "admin_msg": crate::appnet::ADMIN_MSG,
    }))
}

/// 打开原生文件选择器选一个 exe（避免为一次选择引入 tauri-plugin-dialog 依赖）。
#[tauri::command]
pub async fn sk_appnet_pick_exe() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(|| {
        crate::w32::pick_file_dialog(
            "选择要控制的程序",
            "可执行文件\0*.exe;*.bat;*.cmd;*.com\0所有文件\0*.*\0",
        )
    })
    .await
    .map_err(|e| format!("选择线程失败: {e}"))?
}

// ---------------- M4 工具箱：局域网设备 ----------------

/// 扫描本机所在 /24 网段：并发 ping 唤醒邻居 → 读 ARP 表列出存活设备。
#[tauri::command]
pub async fn sk_lan_scan(prefix: Option<String>) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let prefix = match prefix.filter(|p| !p.trim().is_empty()) {
            Some(p) => crate::portscan::normalize_prefix(p.trim()),
            None => {
                // 用快照自洽的物理网卡（单一 PowerShell 数据源，IP 归并天然一致）。
                // 此前跨源 join（GetIfTable2 别名 ↔ 快照按名相等）在本机对不上，
                // 有 IPv4 也报"未找到"——真机功能扫实锤 2026-10-04。
                let snap = crate::snapshot::take();
                let ip = snap
                    .physical_up_adapters()
                    .first()
                    .and_then(|a| a.ipv4.first().cloned())
                    .ok_or_else(|| {
                        // 带上采集自身错误：快照失败（PowerShell 异常/解析失败）
                        // 与"真的没有物理网卡"是两回事，混在一起没法排障
                        format!(
                            "未找到可用的本机 IPv4 地址，无法推断网段（快照：{}）",
                            if snap.error.is_empty() {
                                "无错误，快照正常"
                            } else {
                                &snap.error
                            }
                        )
                    })?;
                crate::portscan::subnet24_of(&ip)
                    .ok_or_else(|| format!("无法从 {ip} 推断 /24 网段"))?
            }
        };
        let hosts = crate::portscan::hosts_in_subnet24(&prefix);
        // 并发 ping：256 个线程太重，用 32 路分批（每批 ~1 秒，总 ~8 秒）
        const BATCH: usize = 32;
        for chunk in hosts.chunks(BATCH) {
            let handles: Vec<_> = chunk
                .iter()
                .map(|h| {
                    let h = h.clone();
                    std::thread::spawn(move || {
                        let _ = crate::w32::run_capture(
                            &args(&["ping", "-n", "1", "-w", "500", &h]),
                            3,
                        );
                    })
                })
                .collect();
            for h in handles {
                let _ = h.join();
            }
        }
        let arp = crate::w32::run_capture(&args(&["arp", "-a"]), 20)
            .map_err(|e| format!("arp 无法执行: {e}"))?;
        let mut devices: Vec<serde_json::Value> = crate::portscan::parse_arp(&arp.stdout)
            .into_iter()
            .filter(crate::portscan::is_useful_neighbor)
            .filter(|e| e.ip.starts_with(&format!("{prefix}.")))
            .map(|e| {
                // 再 ping 一次拿延迟（ARP 表已暖，这次很快）
                let latency =
                    crate::w32::run_capture(&args(&["ping", "-n", "1", "-w", "800", &e.ip]), 3)
                        .ok()
                        .filter(|o| o.ok)
                        .and_then(|o| crate::portscan::parse_ping_ms(&o.combined()));
                serde_json::json!({
                    "ip": e.ip,
                    "mac": e.mac,
                    "kind": e.kind,
                    "latency_ms": latency,
                })
            })
            .collect();
        devices.sort_by_key(|d| {
            d["ip"]
                .as_str()
                .and_then(|s| s.rsplit('.').next())
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0)
        });
        Ok(serde_json::json!({
            "prefix": prefix,
            "scanned": hosts.len(),
            "devices": devices,
            "count": devices.len(),
        }))
    })
    .await
    .map_err(|e| format!("扫描线程失败: {e}"))?
}

