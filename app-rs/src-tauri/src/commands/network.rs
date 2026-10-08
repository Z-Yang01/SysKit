//! M2 遥测 / 多粒度流量 / 日报 / 事件（按域拆出自 commands/mod.rs）。
use crate::rate_store::EventItem;
use crate::telemetry::Tick;
use crate::telemetry_hub::Hub;
use std::sync::Arc;
use tauri::State;

// ---------------- M2 遥测 / 多粒度流量 / 日报 ----------------

/// 最近一拍遥测（前端刚加载时立即拿值，不必等下一拍事件）。
#[tauri::command]
pub fn sk_tick(hub: State<'_, Arc<Hub>>) -> Option<Tick> {
    hub.latest_tick()
}

/// 按应用的流量汇总：今天 / 本周（ISO） / 本月 × 应用名过滤 × 降序 Top。
///
/// `available=false` 时 rows 为空并带 reason（非管理员 / ETW 会话被占），
/// 前端据此展示诚实降级提示而不是一张空表。
#[tauri::command]
pub fn sk_apptraffic_summary(
    hub: State<'_, Arc<Hub>>,
    range: String,
    q: Option<String>,
    top: Option<usize>,
) -> serde_json::Value {
    let (available, reason) = hub.app_traffic.status();
    let rows = if available {
        let agg = hub.app_traffic.agg.lock().expect("应用流量锁中毒");
        let all = agg.summary(
            &range,
            q.as_deref().unwrap_or(""),
            crate::timefmt::now_local(),
        );
        let limit = top.unwrap_or(15).clamp(1, 100);
        all.into_iter().take(limit).collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let sent: i64 = rows.iter().map(|r| r.sent).sum();
    let recv: i64 = rows.iter().map(|r| r.recv).sum();
    serde_json::json!({
        "available": available,
        "reason": reason,
        "range": range,
        "rows": rows,
        "sent": sent,
        "recv": recv,
    })
}

/// 多粒度流量序列（minute/hour/day/month × 可选网络名）。
#[tauri::command]
pub fn sk_stats_series(
    hub: State<'_, Arc<Hub>>,
    granularity: String,
    ssid: Option<String>,
) -> Result<serde_json::Value, String> {
    let series = hub.series(&granularity, ssid.as_deref())?;
    Ok(serde_json::json!({
        "granularity": granularity,
        "ssid": ssid,
        "rows": series.rows,
        "sent": series.sent,
        "recv": series.recv,
    }))
}

/// 快照中出现过的网络名（下拉过滤用）。
#[tauri::command]
pub fn sk_known_ssids(hub: State<'_, Arc<Hub>>) -> Vec<String> {
    crate::netstats::known_ssids(&hub.stats_snapshot())
}

/// 近 N 天日报（断网次数/累计时长 + 每日流量）。
#[tauri::command]
pub fn sk_daily_report(hub: State<'_, Arc<Hub>>, days: Option<i64>) -> serde_json::Value {
    let days = days.unwrap_or(7).clamp(1, 90);
    serde_json::json!({
        "days": days,
        "rows": hub.daily_report(days),
    })
}

/// 断网/恢复事件时间线（最新在前）。
#[tauri::command]
pub fn sk_events(hub: State<'_, Arc<Hub>>, limit: Option<usize>) -> Vec<EventItem> {
    hub.events(limit.unwrap_or(30))
}

/// 记录一条事件（M3 网络监控接入；前端手动检测占位也会用到）。
#[tauri::command]
pub fn sk_record_event(hub: State<'_, Arc<Hub>>, state: String, detail: Option<String>) {
    hub.record_event(&state, detail.as_deref().unwrap_or(""));
}

/// 立即把流量/多粒度桶落盘。
#[tauri::command]
pub fn sk_traffic_flush(hub: State<'_, Arc<Hub>>) {
    hub.flush();
}

/// 概览页一次性初始化：最近一拍 + 今日流量 + 最近事件 + 已知网络名。
#[tauri::command]
pub fn sk_overview_init(hub: State<'_, Arc<Hub>>) -> serde_json::Value {
    let (sent, recv) = hub.today_traffic();
    serde_json::json!({
        "tick": hub.latest_tick(),
        "today_sent": sent,
        "today_recv": recv,
        "events": hub.events(10),
        "known_ssids": crate::netstats::known_ssids(&hub.stats_snapshot()),
    })
}
