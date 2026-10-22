//! SysKit Rust 后端：Tauri Builder + 模块聚合。
//!
//! 模块地图（v1.4 域目录化后，与下方 mod 分组注释一一对应）：
//! - 命令层：`commands`（invoke 接口，按域拆 clipboard/diag/network/palette/
//!   settings_cmds/system/toolbox/vault）；
//! - 外壳层：`tray`（托盘）、`input_panel`（无边框呼出面板）、`paste_guard`
//!   （粘贴守卫链）、`palette`（命令面板逻辑）、`hotkey`（解析+消息泵）；
//! - 网络域：`net`（DNS 客户端/测速/历史/下载测速）、`netcheck`（15 项检查）、
//!   `netquality`（质量探测+告警）、`netstats`（日报聚合）、`icmp`、`wol`；
//! - 应用网络域：`appnet`（防火墙管控 + ETW 流量）；
//! - 剪贴板域：`clipboard_history/watch/write`、`cliprules`（规则引擎）、
//!   `sensitive/sensrules`（敏感防护）、`dib/dhash`（位图/感知哈希）、`qrdecode`；
//! - 密码库域：`vault`（crypto 加密层 / store SQLite / pwgen 生成器）；
//! - 修复域：`repairs`（类型+注册表+三批执行器+backup/restore 备份回滚）；
//! - 系统域：`disk`（空间概览+占用分析）、`cleanup`、`startup`、`modes`、`battery`、`hostsed`；
//! - 遥测域：`telemetry`（core 采样纯逻辑 / hub 运行时线程）、`rate_stats`
//!   （多粒度流量桶）、`rate_store`（每日流量+事件落盘）；
//! - 基础设施：`paths`（目录拓扑）、`settings`、`jsonio`（原子 JSON）、
//!   `secure_store`（DPAPI 容器）、`logging`、`timefmt`、`snapshot`（系统快照）、
//!   `subnet`、`portscan`、`monitor`（后台监控）、`wifi`、`test_support`、
//!   `w32`（唯一 FFI 出口，input/clipboard/crypto/process/sysui 子模块）。
//!
//! 教训清单见 app-rs/README.md。

// （历史上的绞杀者迁移期 dead_code 豁免已随 M6/UNWIRED/ICMP 全部接线而移除；
// 现存 10 处 #[allow(dead_code)] 均为带注释的跨版本对齐面/预留 API，B19 复核。）

// ============ 命令层（invoke 接口，按域拆分） ============
mod commands;

// ============ 外壳层：托盘 / 面板 / 粘贴守卫 / 命令面板 / 热键 ============
mod hotkey;
mod update;
mod input_panel;
mod palette;
mod paste_guard;
mod tray;

// ============ 网络域 ============
mod icmp;
mod net;
mod netcheck;
mod netquality;
mod netstats;
mod wol;

// ============ 应用网络域（防火墙管控 + ETW 流量） ============
mod appnet;

// ============ 剪贴板域 ============
mod clipboard_history;
mod clipboard_watch;
mod clipboard_write;
mod cliprules;
mod dhash;
mod dib;
mod qrdecode;
mod sensitive;
mod sensrules;

// ============ 密码库域（加密 / 存储 / 生成器） ============
mod vault;

// ============ 修复域（执行器 + 备份回滚） ============
mod repairs;

// ============ 系统域 ============
mod battery;
mod cleanup;
mod devices;
mod disk;
mod hostsed;
mod modes;
mod startup;

// ============ 遥测域（采样纯逻辑 + 运行时线程 + 流量/事件桶） ============
mod rate_stats;
mod rate_store;
mod telemetry;

// ============ 基础设施（无业务语义的通用件） ============
mod jsonio;
mod logging;
mod monitor;
mod paths;
mod portscan;
mod secure_store;
mod settings;
mod snapshot;
mod subnet;
mod test_support;
mod timefmt;
mod w32;
mod wifi;

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
/// 恢复式 Mutex 加锁：锁中毒不再整进程退出。
///
/// release 是 `panic = "abort"`，后台线程持锁 panic 会毒化锁——原
/// `.expect("锁中毒")` 的语义是"下一个碰锁的调用点直接炸掉整个进程"，
/// 把一次局部故障放大成全局崩溃。`into_inner` 拿回数据继续跑是更好的
/// 选择：观测类数据自洽性由写入方的不变量保证；持久化变异类（剪贴板
/// 历史落盘等）调用点自己保证不在 catch_tick 恢复路径上写脏数据。
///
/// 61 处 `.lock_ok()` 已全部迁移到 `.lock_ok()`。
pub(crate) trait MutexExt<T> {
    fn lock_ok(&self) -> std::sync::MutexGuard<'_, T>;
}

impl<T> MutexExt<T> for std::sync::Mutex<T> {
    fn lock_ok(&self) -> std::sync::MutexGuard<'_, T> {
        self.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

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
        let mut hosts = state.0.lock_ok();
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
        let mut hosts = state.0.lock_ok();
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
    // 安装器协作入口：**必须在单实例互斥量之前**处理——拿不到互斥量恰恰是
    // "有实例在跑"的正常场景，若走到下面就会弹「已在运行」的框，安装器
    // 只能去强杀（用户看到的就是"装个更新还得自己关软件"）。
    if std::env::args().any(|a| a == "--quit-for-update") {
        std::process::exit(update::quit_running_instance());
    }

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
            // 安装器要装新版时用命名事件请本实例退出（见 update.rs）
            update::spawn_quit_listener(handle.clone());
            // 托盘与两个无边框面板窗口在启动时预建（避免首次呼出卡顿）
            tray::setup(&handle)?;
            input_panel::ensure_panel(&handle)?;
            input_panel::ensure_palette(&handle)?;
            startup_maintenance();
            start_hotkeys(&handle);
            // M2：遥测采样线程（1 秒一拍，60 拍落盘一次）
            let hub = crate::telemetry::hub::Hub::new();
            crate::telemetry::hub::spawn(handle.clone(), hub.clone());
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
            commands::sk_force_delete,
            commands::sk_clean_targets,
            commands::sk_clean_scan,
            commands::sk_clean_run,
            commands::sk_power_status,
            commands::sk_battery_health,
            commands::sk_devices_list,
            commands::sk_devices_battery,
            commands::sk_open_bt_settings,
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
            commands::sk_vault_contacts,
            commands::sk_vault_accounts_contacts,
            commands::sk_vault_add_contact,
            commands::sk_vault_update_contact,
            commands::sk_vault_delete_contact,
            commands::sk_vault_set_primary_contact,
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
            commands::sk_settings_export,
            commands::sk_settings_import,
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
            commands::sk_clip_image_full,
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
