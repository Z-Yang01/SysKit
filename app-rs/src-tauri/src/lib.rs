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
// 但它们的**调用方**属于后续里程碑（记录侧 M5、密码库 M6、设置写入 M7）。
// 覆盖范围是明确的、不掩盖错误——未使用变量/导入等仍按错误处理。
// 每个 M 接入调用方后逐条删除此豁免（见 README 里程碑表的待办）。
#![allow(dead_code)]

mod appnet;
mod backup;
mod battery;
mod cleanup;
mod clipboard_history;
mod clipboard_write;
mod commands;
mod disk;
mod dnsclient;
mod dnsspeed;
mod hostsed;
mod hotkey;
mod input;
mod input_panel;
mod modes;
mod netcheck;
mod netstats;
mod paths;
mod portscan;
mod rate_stats;
mod rate_store;
mod repairs;
mod secure_store;
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
mod w32;
mod wifi;

use tauri::{Emitter, Manager};

/// 后端就绪后要一起处理的一次性维护：历史按天过期 + 图片载荷孤儿清扫。
fn startup_maintenance() {
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
        log::info!("启动维护：过期条目 {removed} 条，孤儿图片 {orphans} 个");
    }
}

/// 注册三个全局热键；任一条被占用只记日志降级（托盘菜单仍可呼出）。
fn start_hotkeys(app: &tauri::AppHandle) {
    let bindings = hotkey::bindings(settings::get_str);
    if bindings.is_empty() {
        log::warn!("三个热键均未配置（或被显式关闭），仅托盘菜单可用");
        return;
    }
    let handle = app.clone();
    let (host, results) = hotkey::HotkeyHost::start(bindings, move |id, action| {
        log::info!("热键命中 id={id} action={action}");
        match action {
            "paste_panel" => input_panel::toggle_paste_panel(&handle),
            "palette" => input_panel::toggle_palette(&handle),
            "quick_paste" => {
                // 快速直贴不弹面板：直接走异步链（绝不在消息泵线程做阻塞工作）
                let h = handle.clone();
                tauri::async_runtime::spawn(async move {
                    let r = input_panel::quick_paste_latest(h.clone()).await;
                    if !r.message.is_empty() {
                        let _ = h.emit("sk://paste-result", &r);
                    }
                });
            }
            _ => {}
        }
    });
    for (id, ok) in &results {
        if *ok {
            log::info!("全局热键已注册 id={id}");
        } else {
            log::warn!("全局热键 id={id} 注册失败（可能被占用），功能可经托盘菜单呼出");
        }
    }
    // 句柄常驻：丢弃只 detach、泵线程照旧存活；显式 leak 表达「与进程同寿」的意图
    Box::leak(Box::new(host));
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
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
            commands::sk_subnet_calc,
            commands::sk_dns_query,
            commands::sk_ping,
            commands::sk_tracert,
            commands::sk_dns_speed,
            commands::sk_speedtest_status,
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
            commands::sk_clean_targets,
            commands::sk_clean_scan,
            commands::sk_clean_run,
            commands::sk_power_status,
            commands::sk_battery_health,
            commands::sk_modes_info,
            commands::sk_modes_run,
            commands::sk_modes_bg_candidates,
            commands::sk_clip_save_as,
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
}
