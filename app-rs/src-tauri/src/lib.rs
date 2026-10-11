//! SysKit Rust 后端：Tauri Builder + 模块聚合。
//!
//! 里程碑：M0 脚手架/门禁（已完成）；**M1 托盘 + 三热键 + 快捷粘贴闭环**（本版）。
//! 模块地图：
//! - 数据层：`paths`（目录拓扑）、`settings`（DEFAULTS + 类型收敛）、`stores`
//!   （原子 JSON）、`secure_store`（DPAPI 容器，与 Python 逐字节对齐）、
//!   `clipboard_history`（条目 schema + 置顶/淘汰/搜索）；
//! - Win32 层：`w32`（唯一 FFI 出口）、`hotkey`（解析 + 消息泵）、`input`
//!   （焦点守卫 + SendInput）、`clipboard_write`（text/html/image/file 写回）；
//! - 外壳层：`tray`、`input_panel`（无边框呼出面板）、`commands`（invoke 接口）。
//!
//! 教训清单见 app-rs/README.md（M1 起点必读）。

// 绞杀者迁移期的跨里程碑契约：这些函数/常量已按 Python 版逐条实现并带单测，
// 但它们的**调用方**属于剩余工作——M6 密码库数据层、Step4 的 UNWIRED 五项
// 接线与原生 ICMP（实测移除本豁免现存 63 处，全部属上述两批）。
// 覆盖范围是明确的、不掩盖错误——未使用变量/导入等仍按错误处理。
// 上述两批落地后本豁免必须收窄（README「已知缺口」有跟踪项）。

mod appnet;
mod apptraffic;
mod backup;
mod backup_restore;
mod battery;
mod cleanup;
mod clipboard_history;
mod clipboard_watch;
mod clipboard_write;
mod cliprules;
mod commands;
mod dhash;
mod dib;
mod disk;
mod diskusage;
mod dnsclient;
mod dnshistory;
mod dnsspeed;
mod hostsed;
mod hotkey;
mod icmp;
mod input;
mod input_panel;
mod logging;
mod modes;
mod monitor;
mod netcheck;
mod netdl;
mod netquality;
mod netstats;
mod palette;
mod paths;
mod portscan;
mod pwgen;
mod qrdecode;
mod rate_stats;
mod rate_store;
mod repairs;
mod secure_store;
mod sensitive;
mod sensrules;
mod settings;
mod snapshot;
mod startup;
mod stores;
mod subnet;
mod telemetry;
mod telemetry_hub;
mod test_support;
mod timefmt;
mod tray;
mod vault_crypto;
mod vault_store;
mod w32;
mod wifi;
mod wol;

use tauri::{Emitter, Manager};

/// 启动自愈：`X` 缺失而 `X.json.migrated` 存在时改回原名。
///
/// 背景（真机事故 2026-10-04）：Python 便携迁移把 `%APPDATA%\SysKit` 的
/// clipboard/traffic/events/settings/vault.db/vault.key 全部改成 `.migrated`
/// 退役（重命名无损，数据都在），而新格式文件又没落到本目录——正在运行的
/// 实例从此读到空历史/空密码库。Python 侧设计意图是"保守可逆：改回原名即可
/// 回退"；这里把回退自动化：**仅当原名文件缺失**（绝不覆盖新数据）才改回。
/// 两边今后谁做迁移都必须遵守这条不变式：退役改名前确保数据已可用。
fn restore_migrated_files() {
    use crate::paths::{appdata_dir, localappdata_dir};
    let mut restored = Vec::new();
    let appdata = appdata_dir();
    for name in [
        "clipboard.json",
        "traffic.json",
        "events.json",
        "netstats_buckets.json",
        "settings.json",
        "vault.db",
        "vault.key",
    ] {
        let cur = appdata.join(name);
        let old = appdata.join(format!("{name}.migrated"));
        if !cur.exists() && old.exists() {
            match std::fs::rename(&old, &cur) {
                Ok(()) => restored.push(name.to_string()),
                Err(e) => log::warn!("[migrated 自愈] {name} 改回失败: {e}"),
            }
        }
    }
    if !restored.is_empty() {
        log::warn!(
            "[migrated 自愈] 检测到数据文件处于 .migrated 退役态且原名缺失，已改回: {}",
            restored.join(", ")
        );
    }
    // LOCALAPPDATA 侧（当前机制只在 APPDATA 侧发生，兜一手同规则）
    let local = localappdata_dir();
    let cur = local.join("crash.log");
    let old = local.join("crash.log.migrated");
    if !cur.exists() && old.exists() {
        let _ = std::fs::rename(&old, &cur);
    }
}

/// 后端就绪后要一起处理的一次性维护：migrated 退役态自愈 + 历史按天过期 + 图片载荷孤儿清扫。
fn startup_maintenance() {
    restore_migrated_files();
    run_history_maintenance();
    // 24h 周期重跑过期清理（对齐 Python 每 24h 重跑 purge_expired）：长驻会话
    // 只在启动清一次的话，跨天使用的过期条目永远清不掉
    std::thread::Builder::new()
        .name("sk-daily-maintenance".into())
        .spawn(|| loop {
            std::thread::sleep(std::time::Duration::from_secs(24 * 3600));
            catch_tick("daily-maintenance", run_history_maintenance);
        })
        .expect("日常维护线程启动失败");
}

/// 历史按天过期 + 图片孤儿清扫（启动与每 24h 共用）。
fn run_history_maintenance() {
    let (removed, orphans) = clipboard_history::with(|h| {
        let days = settings::get_int("clipboard_retain_days");
        let removed = h.purge_older_than(days);
        h.flush_if_dirty();
        let referenced: Vec<String> = h
            .items()
            .iter()
            .filter(|i| i.kind == "image" && !i.data.is_empty())
            .map(|i| i.data.clone())
            .collect();
        let orphans = clipboard_write::sweep_orphan_files(&referenced, 3600.0);
        (removed, orphans)
    });
    if removed > 0 || orphans > 0 {
        log::info!("历史维护：过期条目 {removed} 条，孤儿图片 {orphans} 个");
    }
}

/// 三把全局热键的宿主状态：重挂时 drain 旧 host（反注册）再压入新的。
pub struct HotkeyHosts(pub std::sync::Mutex<Vec<hotkey::HotkeyHost>>);

/// 常驻线程循环体的自愈包装：单次 panic 记 ERROR 后返回，循环继续下一拍，
/// 而不是整条线程静默死亡。panic 遗留的锁中毒会让循环内的 `.expect` 继续
/// 炸——同样被这里捕获并以 ERROR 日志呈现：**响着降级**好过**哑着死亡**
/// （真机教训：wifi RPC 卡住曾让遥测静默停摆 65 分钟才被发现）。
///
/// 只用于"状态/观测"类循环；剪贴板历史等持久化数据的**变异**不在此列——
/// 中毒后继续写可能落盘脏数据，那里宁可 fail-fast。
pub(crate) fn catch_tick<F: FnOnce()>(name: &str, f: F) {
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    if let Err(payload) = r {
        let msg = payload
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "未知 panic".to_string());
        log::error!("[{name}] 单次循环 panic 已恢复（循环继续，状态可能降级）: {msg}");
    }
}

/// 注册/重挂三个全局热键。`previous` 提供各键保存前旧值（回退步来源）；
/// 启动路径传 get_str 本身 → 旧值==当前值 → 无回退步。
///
/// 返回是否全部链注册成功（false = 有键处于「不可用/回退」态）。
///
/// 回退生效时：持久化实际生效值到设置 + 发 `sk://hotkey-reverted` 事件
/// （前端刷新组合框），语义对齐 Python `_rehook_hotkey/_rehook_paste_hotkey`。
pub fn apply_hotkeys(app: &tauri::AppHandle, previous: &dyn Fn(&str) -> String) -> bool {
    let mut all_ok = true;
    let chains = hotkey::build_chains(settings::get_str, previous);
    let handle = app.clone();
    {
        // 先卸旧再挂新（对齐 Python _rehook 的卸载重挂序）：RegisterHotKey
        // 全系统互斥，旧宿主还持着同组合时新注册必失败——会把自己的键误报成
        // "被占"。真机日志实证过：palette+quick 成对"全链注册失败"，重启才恢复。
        let state = app.state::<HotkeyHosts>();
        let mut hosts = state.0.lock().expect("热键宿主锁中毒");
        for h in hosts.drain(..) {
            let mut h = h;
            h.stop(); // 阻塞到泵线程收尾、热键已反注册
        }
    }
    let (host, results) = hotkey::HotkeyHost::start_chains(chains, move |id, action| {
        log::info!("热键命中 id={id} action={action}");
        match action {
            "paste_panel" => input_panel::toggle_paste_panel(&handle),
            "palette" => input_panel::toggle_palette(&handle),
            "quick_paste" => {
                // 快速直贴不弹面板：直接走异步链（绝不在消息泵线程做阻塞工作）
                let h = handle.clone();
                tauri::async_runtime::spawn(async move {
                    let r = input_panel::quick_paste_latest(h.clone()).await;
                    // 粘贴结果可观测性：真机排障的关键证据（成功/降级原因）
                    log::info!("[quick-paste] {}", r.message);
                    if !r.message.is_empty() {
                        let _ = h.emit("sk://paste-result", &r);
                    }
                });
            }
            _ => {}
        }
    });
    {
        let state = app.state::<HotkeyHosts>();
        let mut hosts = state.0.lock().expect("热键宿主锁中毒");
        hosts.push(host);
    }
    for r in &results {
        if !r.ok {
            all_ok = false;
            log::warn!(
                "全局热键 {} 全链注册失败（可能被占用），功能可经托盘菜单呼出",
                r.key
            );
            continue;
        }
        if r.value_used != r.requested {
            // 回退/禁用生效：持久化实际值 + 通知前端刷新（不再标 needs_restart）
            let _ = settings::set_one(r.key, serde_json::json!(r.value_used));
            log::warn!(
                "热键 {} 请求值「{}」被占，已回退为「{}」",
                r.key,
                r.requested,
                if r.value_used.is_empty() {
                    "禁用"
                } else {
                    &r.value_used
                }
            );
            let _ = app.emit(
                "sk://hotkey-reverted",
                serde_json::json!({ "key": r.key, "value": r.value_used }),
            );
        } else {
            log::info!(
                "全局热键已生效 key={} id={} 值={}",
                r.key,
                r.id,
                r.value_used
            );
        }
    }
    all_ok
}

/// 注册三个全局热键；任一条被占用按链回退（对齐 Python _rehook 语义）。
///
/// 启动注册失败（真机实证 06:36：瞬时占用者让三个 Ctrl+Alt 组合同时失败、
/// 半分钟后全部空闲）会让热键死到下次设置保存——与 Python 行为一致但体验差，
/// 因此做一次**有界自动重试**：30 秒后重挂（幂等，先卸旧再挂新），仅此一次，
/// 不循环。重试仍失败则保持回退态，托盘菜单永远可用。
fn start_hotkeys(app: &tauri::AppHandle) {
    app.manage(HotkeyHosts(std::sync::Mutex::new(Vec::new())));
    let ok = apply_hotkeys(app, &settings::get_str);
    if ok {
        return;
    }
    // 有界重试：3 次 × 60s（覆盖约 3 分钟的持续占用——真机实证 00:38 有
    // 占用者持续超过 30 秒，单次 30s 重试不够；再失败保持回退态，托盘菜单
    // 永远可用，用户重启应用也能重来）。
    let handle = app.clone();
    std::thread::Builder::new()
        .name("sk-hotkey-retry".into())
        .spawn(move || {
            for attempt in 1..=3 {
                std::thread::sleep(std::time::Duration::from_secs(60));
                log::info!("热键启动注册失败，第 {attempt}/3 次自动重挂");
                if apply_hotkeys(&handle, &settings::get_str) {
                    return;
                }
            }
            log::warn!("热键 3 次自动重挂均失败——请到「设置 / 日志」检查热键是否被其他程序占用");
        })
        .expect("热键重试线程启动失败");
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 文件日志先行：其后任何初始化失败都有迹可循
    logging::init();
    // 单实例互斥（与 Python 版同名：兼防本版多开与两版同跑——多实例各自
    // 刷盘会互相覆盖 clipboard.json/traffic.json，热键也会互抢）
    if !crate::w32::acquire_single_instance_mutex("SysKit_SingleInstance_Mutex") {
        unsafe {
            use windows::core::w;
            use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONINFORMATION, MB_OK};
            let text = w!("SysKit 已在运行（含 Python 旧版）。
请先退出已有实例（托盘右键 → 退出）再启动。");
            let title = w!("SysKit");
            MessageBoxW(None, text, title, MB_OK | MB_ICONINFORMATION);
        }
        log::warn!("已有实例在运行，本实例退出");
        return;
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            let handle = app.handle().clone();
            // 托盘与两个无边框面板窗口在启动时预建（避免首次呼出卡顿）
            tray::setup(&handle)?;
            input_panel::ensure_panel(&handle)?;
            input_panel::ensure_palette(&handle)?;
            startup_maintenance();
            start_hotkeys(&handle);
            // M2：遥测采样线程（1 秒一拍，60 拍落盘一次）
            let hub = telemetry_hub::Hub::new();
            telemetry_hub::spawn(handle.clone(), hub.clone());
            app.manage(hub);
            // 剪贴板监听：把"用户复制了什么"变成历史条目。
            // 没有它，快捷粘贴面板会在空历史里"正常"工作——命令全对、列表恒空。
            clipboard_watch::spawn();
            // M6 密码库会话：解锁后的数据密钥只驻留在托管会话里
            app.manage(commands::VaultSession(std::sync::Mutex::new(None)));
            app.manage(commands::VaultTrust(std::sync::Mutex::new(None)));
            // minimize_to_tray（Step4 接线）：关窗 = 隐藏到托盘（真退出走托盘菜单-退出，
            // app.exit 不触发 CloseRequested，两条路径互不干扰）
            if let Some(win) = app.get_webview_window("main") {
                let w = win.clone();
                win.on_window_event(move |e| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = e {
                        if crate::settings::get_bool("minimize_to_tray") {
                            api.prevent_close();
                            let _ = w.hide();
                            log::info!("窗口关闭：按设置最小化到托盘（退出请用托盘菜单）");
                        }
                    }
                });
            }
            // 后台监控 + 保守自动修复梯度（消费那六项 auto_repair_* 设置）；
            // 注入句柄供状态切换时更新托盘状态灯
            monitor::spawn(handle.clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::ping,
            commands::sk_clip_list,
            commands::sk_clip_count,
            commands::sk_panel_init,
            commands::sk_clip_write_back,
            commands::sk_clip_toggle_pin,
            commands::sk_clip_remove,
            commands::sk_clip_clear,
            commands::sk_clip_flush,
            commands::sk_clip_purge,
            commands::sk_paste_seq,
            commands::sk_paste,
            commands::sk_quick_paste,
            commands::sk_hide_panel,
            commands::sk_toggle_panel,
            commands::sk_setting_get,
            commands::sk_version,
            commands::sk_clip_summary,
            commands::sk_recent_items,
            commands::sk_tray_ready,
            commands::sk_write_item,
            commands::sk_tick,
            commands::sk_stats_series,
            commands::sk_apptraffic_summary,
            commands::sk_known_ssids,
            commands::sk_daily_report,
            commands::sk_events,
            commands::sk_record_event,
            commands::sk_traffic_flush,
            commands::sk_overview_init,
            commands::sk_is_admin,
            commands::sk_repair_list,
            commands::sk_repair_plan,
            commands::sk_repair_run,
            commands::sk_backup_list,
            commands::sk_backup_restore,
            commands::sk_backup_list,
            commands::sk_backup_restore,
            commands::sk_log_event,
            commands::sk_diag_run,
            commands::sk_diag_one,
            commands::sk_hosts_info,
            commands::sk_hosts_read,
            commands::sk_hosts_add,
            commands::sk_hosts_remove,
            commands::sk_hosts_toggle,
            commands::sk_hosts_write,
            commands::sk_hosts_backup,
            commands::sk_ports_list,
            commands::sk_kill_process,
            commands::sk_lan_scan,
            commands::sk_wol_send,
            commands::sk_quality_series,
            commands::sk_subnet_calc,
            commands::sk_dns_query,
            commands::sk_ping,
            commands::sk_tracert,
            commands::sk_dns_speed,
            commands::sk_dns_speed_history,
            commands::sk_speedtest_status,
            commands::sk_download_test,
            commands::sk_appnet_list,
            commands::sk_appnet_add,
            commands::sk_appnet_remove,
            commands::sk_appnet_validate,
            commands::sk_appnet_pick_exe,
            commands::sk_wifi_info,
            commands::sk_top_procs,
            commands::sk_startup_list,
            commands::sk_startup_set,
            commands::sk_disk_list,
            commands::sk_disk_scan_tree,
            commands::sk_disk_scan_children,
            commands::sk_disk_scan_cancel,
            commands::sk_disk_recycle_delete,
            commands::sk_clean_targets,
            commands::sk_clean_scan,
            commands::sk_clean_run,
            commands::sk_power_status,
            commands::sk_battery_health,
            commands::sk_modes_info,
            commands::sk_modes_run,
            commands::sk_modes_bg_candidates,
            commands::sk_clip_save_as,
            commands::sk_vault_status,
            commands::sk_vault_setup,
            commands::sk_vault_unlock,
            commands::sk_vault_lock,
            commands::sk_vault_set_password,
            commands::sk_vault_verify_password,
            commands::sk_vault_remove_password,
            commands::sk_vault_sites,
            commands::sk_vault_site_accounts,
            commands::sk_vault_add_site,
            commands::sk_vault_update_site,
            commands::sk_vault_delete_site,
            commands::sk_vault_add_account,
            commands::sk_vault_update_account,
            commands::sk_vault_delete_account,
            commands::sk_vault_toggle_favorite,
            commands::sk_vault_get_secret,
            commands::sk_vault_secrets_bulk,
            commands::sk_vault_global_search,
            commands::sk_vault_totp_code,
            commands::sk_vault_copy_secret,
            commands::sk_vault_generate_password,
            commands::sk_vault_reused_passwords,
            commands::sk_clip_copy_plain,
            commands::sk_settings_all,
            commands::sk_settings_set,
            commands::sk_settings_reset,
            commands::sk_log_list,
            commands::sk_log_tail,
            commands::sk_palette_actions,
            commands::sk_clip_watch_state,
            commands::sk_clip_watch_set,
            commands::sk_clip_probe,
            commands::sk_monitor_state,
            commands::sk_monitor_force_check,
            commands::sk_monitor_preview,
            commands::sk_clip_detect,
            commands::sk_clip_action_run,
            commands::sk_clip_qr_svg,
            commands::sk_clip_image_thumb,
            commands::sk_qr_decode_image,
            commands::sk_qr_decode_clipboard,
            commands::sk_qr_decode_clip_item,
            commands::sk_open_url,
            commands::sk_download_test_one,
            commands::sk_download_cancel,
            commands::sk_download_reset,
            commands::sk_diag_export,
            commands::sk_palette_query,
            commands::sk_palette_toggle,
            commands::sk_frontend_error,
            commands::sk_clip_copy_sensitive,
            commands::sk_clip_sensitive_pending,
            commands::sk_wifi_profile_password,
            commands::sk_wifi_profiles,
            commands::sk_wifi_scan,
            commands::sk_wifi_qr_svg,
        ])
        .run(tauri::generate_context!())
        .expect("SysKit 启动失败");
}

#[cfg(test)]
mod tests {
    #[test]
    fn ping_is_pong() {
        assert_eq!(super::commands::ping(), "pong");
    }

    #[test]
    fn restore_migrated_brings_back_missing_data_files() {
        let _g = crate::test_support::serial();
        let dir = crate::test_support::isolate();
        let appdata = crate::paths::appdata_dir();
        std::fs::create_dir_all(&appdata).unwrap();
        // 场景 1：X 缺失 + X.migrated 在 → 改回
        std::fs::write(appdata.join("clipboard.json.migrated"), b"[1]").unwrap();
        std::fs::write(appdata.join("vault.key.migrated"), b"K").unwrap();
        // 场景 2：X 已在（新数据）→ 绝不覆盖，.migrated 原样保留
        std::fs::write(appdata.join("settings.json"), b"{\"new\":true}").unwrap();
        std::fs::write(appdata.join("settings.json.migrated"), b"{\"old\":true}").unwrap();
        super::restore_migrated_files();
        assert_eq!(
            std::fs::read(appdata.join("clipboard.json")).unwrap(),
            b"[1]",
            "退役态数据必须改回原名"
        );
        assert!(!appdata.join("clipboard.json.migrated").exists());
        assert_eq!(std::fs::read(appdata.join("vault.key")).unwrap(), b"K");
        assert_eq!(
            std::fs::read(appdata.join("settings.json")).unwrap(),
            b"{\"new\":true}",
            "原名已存在的（更新）数据绝不能被 .migrated 覆盖"
        );
        assert!(appdata.join("settings.json.migrated").exists());
        let _ = dir;
    }

    #[test]
    fn catch_tick_runs_normal_body_and_swallows_panic() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        let ran = Arc::new(AtomicBool::new(false));
        let r = ran.clone();
        super::catch_tick("test-ok", move || r.store(true, Ordering::SeqCst));
        assert!(ran.load(Ordering::SeqCst), "正常循环体必须被执行");
        // 走到这里即 panic 未逃逸（否则本测试线程已死、用例报失败）
        super::catch_tick("test-panic", || panic!("必须被吞掉的自愈测试"));
    }
}
