//! 诊断 / 修复域：15 项检查执行、修复动作库调用、组合修复、备份回滚命令、
//! 后台监控状态与事件入账（按域拆自 commands/mod.rs 与 clipboard.rs）。
use std::sync::Arc;

use tauri::State;

use crate::telemetry::hub::Hub;

// ---------------- 后台监控 / 自动修复 ----------------

/// 监控状态（状态码、失败连击、已执行修复次数、最近梯度）。
#[tauri::command]
pub fn sk_monitor_state() -> serde_json::Value {
    serde_json::to_value(crate::monitor::state()).unwrap_or(serde_json::json!({}))
}

/// 请求立即检查一次（不必等满 `poll_interval_sec`）。
#[tauri::command]
pub fn sk_monitor_force_check() -> serde_json::Value {
    crate::monitor::force_check();
    serde_json::json!({ "ok": true, "message": "已请求立即检查" })
}

/// 预览当前梯度（按配置与 VPN 护栏算出，**不执行任何动作**）。
///
/// 让用户在自动修复真的动网卡之前就能看到"会发生什么、哪些被跳过"。
#[tauri::command]
pub fn sk_monitor_preview() -> serde_json::Value {
    let cfg = crate::monitor::MonitorConfig::from_settings();
    let snap = crate::snapshot::take();
    let vpn_up = snap.adapters.iter().any(|a| {
        a.status.eq_ignore_ascii_case("up")
            && (crate::netcheck::is_tunnel(&a.desc) || crate::netcheck::is_tunnel(&a.name))
    });
    let (keep, skip) = crate::monitor::build_ladder(cfg.max_level, vpn_up);
    serde_json::json!({
        "enabled": cfg.enabled,
        "max_level": cfg.max_level,
        "poll_interval_sec": cfg.poll_interval_sec,
        "fail_threshold": cfg.fail_threshold,
        "cooldown_sec": cfg.cooldown_sec,
        "vpn_up": vpn_up,
        "ladder": keep,
        "skipped_by_vpn": skip,
        "vpn_note": if vpn_up { crate::monitor::vpn_guard_note() } else { "" },
        "suggestions": crate::monitor::suggestions()
            .iter()
            .map(|(id, reason)| serde_json::json!({ "id": id, "reason": reason }))
            .collect::<Vec<_>>(),
    })
}

// ---------------- M3 诊断 / 修复 ----------------

/// 当前是否管理员（修复页据此禁用改配置类动作）。
#[tauri::command]
pub fn sk_is_admin() -> bool {
    crate::w32::is_elevated()
}

/// 修复动作清单 + 组合 + 管理员状态（修复页一次拿全）。
#[tauri::command]
pub fn sk_repair_list() -> serde_json::Value {
    use crate::repairs::{catalog, COMBOS};
    let actions: Vec<serde_json::Value> = catalog()
        .iter()
        .map(|a| {
            serde_json::json!({
                "id": a.id,
                "title": a.title,
                "risk": a.risk,
                "risk_label": a.risk.label(),
                "description": a.description,
                "restore_hint": a.restore_hint,
                "brief": a.brief,
                "need_reboot": a.need_reboot,
                "params": a.params,
            })
        })
        .collect();
    let combos: Vec<serde_json::Value> = COMBOS
        .iter()
        .map(|c| serde_json::json!({ "key": c.key, "title": c.title, "actions": c.actions }))
        .collect();
    serde_json::json!({
        "actions": actions,
        "combos": combos,
        "is_admin": crate::w32::is_elevated(),
    })
}

/// 组合展开计划（含每步是否被跳过及原因；跳过必须显式告知）。
#[tauri::command]
pub fn sk_repair_plan(combo: String, is_admin: Option<bool>) -> Vec<crate::repairs::ComboStep> {
    let admin = is_admin.unwrap_or_else(crate::w32::is_elevated);
    crate::repairs::combo_plan(&combo, admin)
}

/// 执行单个修复动作（在阻塞线程池里跑，绝不冻 UI）。
#[tauri::command]
pub async fn sk_repair_run(
    id: String,
    params: Option<serde_json::Value>,
) -> Result<crate::repairs::RepairOutcome, String> {
    let action = crate::repairs::get_action(&id).ok_or_else(|| format!("未知修复动作: {id}"))?;
    let mut map = std::collections::BTreeMap::new();
    if let Some(serde_json::Value::Object(obj)) = params {
        for (k, v) in obj {
            if let Some(s) = v.as_str() {
                map.insert(k, s.to_string());
            }
        }
    }
    let out = tauri::async_runtime::spawn_blocking(move || action.execute(&map))
        .await
        .map_err(|e| format!("修复线程失败: {e}"))?;
    log::info!("修复动作 {id} -> success={} {}", out.success, out.message);
    Ok(out)
}

/// 记录一条诊断/修复事件到时间线（修复页执行后调用，供概览页展示）。
#[tauri::command]
pub fn sk_log_event(hub: State<'_, Arc<Hub>>, state: String, detail: String) {
    hub.record_event(&state, &detail);
}

/// 运行网络诊断（`quick=true` 跳过慢速外网探测，供监控轮询）。
///
/// 15 项检查里含 ping/HTTP/DNS 等秒级操作，必须放阻塞线程池，绝不能冻 UI。
#[tauri::command]
pub async fn sk_diag_run(quick: Option<bool>) -> serde_json::Value {
    let quick = quick.unwrap_or(false);
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let results = crate::netcheck::run_all(quick);
        let (verdict_code, verdict_text) = crate::netcheck::verdict(&results);
        let counts = crate::netcheck::status_counts(&results);
        (results, verdict_code, verdict_text, counts)
    })
    .await;
    match outcome {
        Ok((results, code, text, counts)) => serde_json::json!({
            "results": results,
            "verdict": code,
            "verdict_text": text,
            "counts": counts,
            "quick": quick,
        }),
        Err(e) => serde_json::json!({
            "results": [],
            "verdict": "error",
            "verdict_text": format!("诊断线程失败: {e}"),
            "counts": {},
            "quick": quick,
        }),
    }
}

/// 单项检查（前端逐项刷新用）。
#[tauri::command]
pub async fn sk_diag_one(id: String) -> Result<crate::netcheck::CheckResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::netcheck::all_checks()
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.run(&crate::snapshot::take()))
            .ok_or_else(|| format!("未知检查项: {id}"))
    })
    .await
    .map_err(|e| format!("诊断线程失败: {e}"))?
}

/// 备份会话索引：修复页「备份与回滚」卡（新会话在前，legacy 存量随后）。
#[tauri::command]
pub fn sk_backup_list() -> Vec<crate::repairs::restore::BackupSessionInfo> {
    crate::repairs::restore::list_sessions()
}

/// 回滚一项结构化备份（kind = proxy | dns | ncsi；session 为目录名）。
/// 危险操作：前端确认弹窗后调用；错误原样透传给用户。
#[tauri::command]
pub fn sk_backup_restore(kind: String, session: String) -> Result<String, String> {
    crate::repairs::restore::restore(&kind, &session)
}

/// 诊断报告导出为 Markdown 文件（save_file_dialog 原语，与图片另存为同路）。
/// 前端已把全文拼好（formatReportForCopy 同一文本），这里只落盘。
#[tauri::command]
pub async fn sk_diag_export(content: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let target = crate::w32::save_file_dialog(
            "导出诊断报告",
            "diag_report.md",
            "Markdown\0*.md\0文本文件\0*.txt\0所有文件\0*.*\0",
        )?;
        if target.is_empty() {
            return Ok(String::new()); // 用户取消
        }
        std::fs::write(&target, content.as_bytes())
            .map_err(|e| format!("写入失败（路径不可写？）: {e}"))?;
        Ok(target)
    })
    .await
    .map_err(|e| format!("导出线程失败: {e}"))?
}
