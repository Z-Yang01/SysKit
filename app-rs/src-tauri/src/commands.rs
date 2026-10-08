//! Tauri 前端可调命令（`invoke` 接口）。所有命令都是薄封装，业务逻辑在各模块里。

use std::sync::Arc;

use tauri::{AppHandle, Manager, Runtime, State};

use crate::clipboard_history::{self as hist, ClipItem};
use crate::input_panel::{self, PasteRequest, PasteResult};
use crate::rate_store::EventItem;
use crate::settings;
use crate::telemetry::Tick;
use crate::telemetry_hub::Hub;
use crate::tray;

/// 面板/主窗口共用的条目行（含原始下标，供写回定位）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ClipRow {
    pub idx: usize,
    pub ts: i64,
    pub text: String,
    pub kind: String,
    pub pinned: bool,
    pub data: String,
    /// 单行预览（列表展示用）。
    pub preview: String,
}

impl ClipRow {
    fn from_item(it: &ClipItem) -> Self {
        Self {
            idx: it.idx.unwrap_or(0),
            ts: it.ts,
            text: it.text.clone(),
            kind: it.kind.clone(),
            pinned: it.pinned,
            data: it.data.clone(),
            preview: ClipItem::preview(&it.text, 80),
        }
    }
}

/// 探活（M0 通路验证保留）。
#[tauri::command]
pub fn ping() -> String {
    "pong".to_string()
}

/// 读取剪贴板历史（可按关键字/类型过滤）。
#[tauri::command]
pub fn sk_clip_list(keyword: Option<String>, kind: Option<String>) -> Vec<ClipRow> {
    hist::with(|h| {
        h.search(keyword.as_deref(), kind.as_deref())
            .iter()
            .map(ClipRow::from_item)
            .collect()
    })
}

/// 条目总数（面板空态判断）。
#[tauri::command]
pub fn sk_clip_count() -> usize {
    hist::with(|h| h.len())
}

/// 面板呼出时的一次性初始化数据。
#[derive(Debug, Clone, serde::Serialize)]
pub struct PanelInit {
    pub rows: Vec<ClipRow>,
    pub max_items: i64,
    pub paste_hotkey: String,
    pub paste_hotkey_display: String,
    pub quick_hotkey: String,
    pub quick_hotkey_display: String,
    pub watch_enabled: bool,
}

#[tauri::command]
pub fn sk_panel_init() -> PanelInit {
    let rows = hist::with(|h| h.items().iter().map(ClipRow::from_item).collect());
    let paste_hotkey = settings::get_str("clip_paste_hotkey");
    let quick_hotkey = settings::get_str("clip_quick_hotkey");
    PanelInit {
        rows,
        max_items: settings::get_int("clipboard_max"),
        paste_hotkey_display: crate::hotkey::hotkey_display(&paste_hotkey),
        quick_hotkey_display: crate::hotkey::hotkey_display(&quick_hotkey),
        paste_hotkey,
        quick_hotkey,
        watch_enabled: settings::get_bool("clipboard_watch"),
    }
}

/// 把某条历史写回系统剪贴板（不下发粘贴，供托盘/命令面板「复制」用）。
#[tauri::command]
pub fn sk_clip_write_back(index: usize) -> bool {
    let item = hist::with(|h| h.by_index(index));
    match item {
        Some(it) => crate::clipboard_write::write_back(&it),
        None => false,
    }
}

#[tauri::command]
pub fn sk_clip_toggle_pin(index: usize) -> bool {
    hist::with(|h| h.toggle_pin(index))
}

#[tauri::command]
pub fn sk_clip_remove(index: usize) -> bool {
    hist::with(|h| h.remove(index))
}

#[tauri::command]
pub fn sk_clip_clear() -> bool {
    hist::with(|h| h.clear());
    true
}

/// 立即把内存态密封落盘（退出/托盘读取前调用）。
#[tauri::command]
pub fn sk_clip_flush() -> bool {
    hist::with(|h| h.flush_if_dirty())
}

/// 按保留天数清理历史，返回删除条数。
#[tauri::command]
pub fn sk_clip_purge(days: Option<i64>) -> usize {
    let d = days.unwrap_or_else(|| settings::get_int("clipboard_retain_days"));
    hist::with(|h| h.purge_older_than(d))
}

/// 前端错误上报（ErrorBoundary 兜底时调用）：落文件日志，让"前端崩了"
/// 从 devtools 独白变成可诊断的日志事件。fire-and-forget，前端不等回执。
#[tauri::command]
pub fn sk_frontend_error(message: String) {
    log::error!("[frontend] {}", message.trim());
}

/// 当前粘贴链序号（前端把它带进 `sk_paste`，防陈旧请求覆盖新链）。
#[tauri::command]
pub fn sk_paste_seq() -> u64 {
    input_panel::bump_seq()
}

/// 执行一次「写回 + 焦点守卫注键」（面板路径，延后 120ms）。
#[tauri::command]
pub async fn sk_paste<R: Runtime>(app: AppHandle<R>, req: PasteRequest) -> PasteResult {
    input_panel::perform_paste(app, req, crate::input::PASTE_DELAY_MS).await
}

/// 快速直贴最新一条（托盘菜单/面板按钮共用）。
#[tauri::command]
pub async fn sk_quick_paste<R: Runtime>(app: AppHandle<R>) -> PasteResult {
    input_panel::quick_paste_latest(app).await
}

/// 隐藏某个面板窗口（Esc / 失焦 / 执行完成）。
#[tauri::command]
pub fn sk_hide_panel<R: Runtime>(app: AppHandle<R>, label: Option<String>) {
    let label = label.unwrap_or_else(|| input_panel::PANEL_LABEL.to_string());
    input_panel::hide_panel(&app, &label);
}

/// 呼出/收起某个面板（前端快捷键亦可用）。
#[tauri::command]
pub fn sk_toggle_panel<R: Runtime>(app: AppHandle<R>, label: String) {
    if label == input_panel::PALETTE_LABEL {
        input_panel::toggle_palette(&app);
    } else {
        input_panel::toggle_paste_panel(&app);
    }
}

/// 读取设置项（前端只读消费；写入走 M7 设置页）。
#[tauri::command]
pub fn sk_setting_get(key: String) -> serde_json::Value {
    let data = settings::load();
    data.get(&key).cloned().unwrap_or(serde_json::Value::Null)
}

/// 主窗口标题栏用的版本信息。
#[tauri::command]
pub fn sk_version() -> serde_json::Value {
    serde_json::json!({
        "version": crate::paths::VERSION,
        "display_name": crate::paths::DISPLAY_NAME,
    })
}

/// 历史摘要（托盘 tooltip / 诊断用）。
#[tauri::command]
pub fn sk_clip_summary() -> serde_json::Value {
    hist::with(|h| {
        let items = h.items();
        let pinned = items.iter().filter(|i| i.pinned).count();
        let images = items.iter().filter(|i| i.kind == "image").count();
        // by_kind：类型筛选徽标消费（clipSummaryText 拼接「文本 N · HTML M」）
        let mut by_kind = std::collections::BTreeMap::new();
        for i in &items {
            *by_kind.entry(i.kind.clone()).or_insert(0u64) += 1;
        }
        serde_json::json!({
            "total": items.len(),
            // count：前端 ClipSummary.count（剪贴板页统计行「共 N/M 条」）——
            // 真机走查实锤 undefined（字段名漂移），total 保留给骨架期调用方
            "count": items.len(),
            "pinned": pinned,
            "images": images,
            "max_items": h.max_items,
            "by_kind": by_kind,
            "watch_enabled": crate::clipboard_watch::is_enabled(),
            // 后端暂停=关闭（sk_clip_watch_set 二态合并），无独立暂停态
            "watch_paused": false,
        })
    })
}

/// 托盘「最近剪贴板」子菜单的数据源（M1 之后接到真实子菜单）。
#[tauri::command]
pub fn sk_recent_items(limit: Option<usize>) -> Vec<ClipRow> {
    let n = limit.unwrap_or(5);
    hist::with(|h| {
        h.items()
            .iter()
            .take(n)
            .enumerate()
            .map(|(i, it)| {
                let mut c = it.clone();
                c.idx = Some(i);
                ClipRow::from_item(&c)
            })
            .collect()
    })
}

/// 前端启动时把托盘也准备好（重复调用安全）。
#[tauri::command]
pub fn sk_tray_ready<R: Runtime>(app: AppHandle<R>) -> bool {
    tray::flush_history();
    app.get_webview_window("main").is_some()
}

/// 面板测试辅助：把条目的载荷直接交给剪贴板写入（不查历史）。
#[tauri::command]
pub fn sk_write_item(kind: String, text: String, data: String) -> bool {
    let item = ClipItem {
        ts: 0,
        text,
        kind,
        pinned: false,
        data,
        idx: None,
    };
    crate::clipboard_write::write_back(&item)
}

// ---------------- M2 遥测 / 多粒度流量 / 日报 ----------------

/// 最近一拍遥测（前端刚加载时立即拿值，不必等下一拍事件）。
#[tauri::command]
pub fn sk_tick(hub: State<'_, Arc<Hub>>) -> Option<Tick> {
    hub.latest_tick()
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

/// 构造命令参数表（本地小工具，避免每处写 `.to_string()`）。
fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

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

// ---------------- 剪贴板内容规则动作 ----------------

/// 检测一段文本可以做什么（返回动作列表）。
///
/// **不执行任何动作**，只做识别——执行走 `sk_clip_action_run`，由用户显式点击。
#[tauri::command]
pub async fn sk_clip_detect(text: String) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        // 路径存在性检查含磁盘 IO，所以在阻塞线程里做
        let hits = crate::cliprules::detect_real(&text);
        Ok(serde_json::json!({
            "actions": hits,
            "count": hits.len(),
        }))
    })
    .await
    .map_err(|e| format!("检测线程失败: {e}"))?
}

/// 执行一个规则动作。
///
/// 只有这几类副作用：打开链接/文件/目录、复制格式化结果、发起 ping/DNS（只读）。
/// **没有"删除""写入"类动作**——所以这里不需要确认对话框；
/// 唯一有写操作的是 `json_format`，它把格式化结果**写回剪贴板**（不落盘）。
#[tauri::command]
pub async fn sk_clip_action_run(id: String, arg: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || match id.as_str() {
        "url_open" => {
            crate::w32::shell_open(&arg)?;
            Ok(format!("已在浏览器打开 {arg}"))
        }
        "file_open" => {
            crate::w32::shell_open(&arg)?;
            Ok(format!("已打开 {arg}"))
        }
        "file_reveal" => {
            // explorer /select 会打开父目录并选中该文件
            crate::w32::open_folder_selecting(&arg)?;
            Ok(format!("已在资源管理器中定位 {arg}"))
        }
        "json_format" => {
            // 把格式化结果写回剪贴板（与剪贴板页的"复制"同语义）
            if crate::w32::clipboard_set_text(&arg) {
                Ok("已格式化并复制回剪贴板".to_string())
            } else {
                Err("写回剪贴板失败".to_string())
            }
        }
        "url_qr" | "color_show" | "ip_geo" => Err(format!(
            "{id} 需要界面渲染（二维码/色块/归属地），请在前端处理"
        )),
        "ip_ping" | "domain_ping" => {
            let out = crate::w32::run_capture(&args(&["ping", "-n", "2", "-w", "1000", &arg]), 20)
                .map_err(|e| format!("ping 无法执行: {e}"))?;
            Ok(out.tail(0, 400))
        }
        "domain_dns" => {
            let ips = crate::dnsclient::resolve_system(&arg, 4.0);
            if ips.is_empty() {
                Err(format!("{arg} 解析失败或无记录"))
            } else {
                Ok(format!("{arg} → {}", ips.join(", ")))
            }
        }
        other => Err(format!("未知动作: {other}")),
    })
    .await
    .map_err(|e| format!("动作线程失败: {e}"))?
}

/// 生成二维码 SVG（`url_qr` 动作的渲染端，对应 Python `_show_qr_dialog`）。
///
/// 纯 Rust `qrcode` 本地渲染：绝不把用户复制的链接发给第三方二维码服务——
/// 那等于把剪贴板内容泄露出去。SVG 交给前端展示/导出 PNG（Python 版保存
/// 的也是图片文件，格式从 PNG 换 SVG 不改语义；前端导出仍出 PNG）。
/// 纠错级别 M 与 Python `qrcode.make` 默认一致；外圈留白取库默认 4 模块
/// （Python 版 border=2 偏紧，4 是扫码可靠性推荐值，只影响留白不影响内容）。
#[tauri::command]
pub fn sk_clip_qr_svg(data: String) -> Result<String, String> {
    use qrcode::{render::svg, EcLevel, QrCode};
    let code = QrCode::with_error_correction_level(data.as_bytes(), EcLevel::M)
        .map_err(|e| format!("二维码生成失败: {e}"))?;
    Ok(code.render::<svg::Color>().min_dimensions(280, 280).build())
}

// ---------------- 敏感内容剪贴板 ----------------

/// 复制一段敏感文本到剪贴板（密码 / WiFi 密钥等）。
///
/// 三处泄露面一起堵（少堵一处等于没堵）：
/// 1. `mark_sensitive` —— 本应用的历史在 TTL 内不记录该文本；
/// 2. `exclude_os_history` —— 不进 Win+V 历史、不上传云剪贴板；
/// 3. 到期清空剪贴板（TTL 来自设置 `clip_sensitive_ttl`，0=不清空）。
///
/// 返回说明包含"多久后清空"，让用户知道保护强度；也便于界面在
/// "不会自动清空"时提示其余保护仍在。
#[tauri::command]
pub fn sk_clip_copy_sensitive(text: String) -> Result<serde_json::Value, String> {
    copy_sensitive_text(&text)
}

/// 复制 + 三重敏感防护（copy 命令与密码库复制共用，M6）。
pub fn copy_sensitive_text(text: &str) -> Result<serde_json::Value, String> {
    if text.is_empty() {
        return Err("内容为空".to_string());
    }
    let ttl = crate::sensitive::effective_ttl(crate::settings::get_int("clip_sensitive_ttl"));
    // 1. 让监听器在 TTL 内不记录。无 TTL 时给足够长的窗口——敏感内容本就该长期
    //    不进历史（与 Python 的 `ttl if ttl > 0 else 3600` 同口径）
    crate::clipboard_history::mark_sensitive(text, ttl.map(|s| s as i64).unwrap_or(3600));
    // 2. 写入剪贴板
    if !crate::w32::clipboard_set_text(text) {
        return Err("写入剪贴板失败（可能被其他程序占用）".to_string());
    }
    // 3. 排除系统历史与云同步：必须在写入**之后**补设格式
    crate::w32::exclude_os_history();
    // 4. 安排到期清空
    crate::sensitive::arm(text, ttl);
    Ok(serde_json::json!({
        "ttl": ttl.unwrap_or(0),
        "note": crate::sensitive::clear_note(ttl),
    }))
}

/// 是否还有待清空的敏感内容（界面据此显示状态）。
#[tauri::command]
pub fn sk_clip_sensitive_pending() -> serde_json::Value {
    serde_json::json!({
        "pending": crate::sensitive::pending_text().is_some(),
        "ttl": crate::sensitive::effective_ttl(crate::settings::get_int("clip_sensitive_ttl"))
            .unwrap_or(0),
    })
}

/// 已保存的 WLAN 配置文件列表（供密码查看下拉）。
#[tauri::command]
pub async fn sk_wifi_profiles() -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let list = crate::repairs::wlan_profiles();
        Ok(serde_json::json!({ "profiles": list, "count": list.len() }))
    })
    .await
    .map_err(|e| format!("枚举线程失败: {e}"))?
}

/// 读取某个已保存 WLAN 配置文件的明文密钥（需管理员权限）。
///
/// 单独的只读查询：**不写剪贴板**。复制走 `sk_clip_copy_sensitive`——
/// 密码类内容只能经由那条路径进入剪贴板，否则会漏掉"不进历史/不进系统历史/
/// 到期清空"三重保护。这条纪律靠 `vault::password_copy_routes_through_sensitive`
/// 一类的守卫测试维持。
#[tauri::command]
pub async fn sk_wifi_profile_password(name: String) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        // netsh 超时 15s：必须在阻塞线程，不能占住命令线程
        let c = crate::wifi::profile_credential(&name)?;
        Ok(serde_json::json!({
            "name": c.name,
            "auth": c.auth,
            "password": c.password,
            "has_password": !c.password.is_empty(),
        }))
    })
    .await
    .map_err(|e| format!("读取线程失败: {e}"))?
}

/// 扫描周边 WiFi（`netsh wlan show networks mode=bssid`，BSSID 级）。
///
/// netsh 扫描可达秒级：走阻塞线程，不占住命令线程（与 sk_wifi_profiles 同款）。
#[tauri::command]
pub async fn sk_wifi_scan() -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let aps = crate::wifi::scan_networks()?;
        Ok(serde_json::json!({ "aps": aps, "count": aps.len() }))
    })
    .await
    .map_err(|e| format!("扫描线程失败: {e}"))?
}

/// 生成 WiFi 分享二维码 SVG（标准 `WIFI:T:..;S:..;P:..;;` 格式）。
///
/// **纪律：`password` 实参绝不进任何日志**——本函数与 `wifi::wifi_qr_string` /
/// `wifi::render_qr_svg` 都不 log，错误串也只有二维码库的静态错误名，不回带密码。
/// 展示与导出 PNG 全在前端完成，后端只回 SVG 文本。
///
/// `auth` 传 netsh 认证原文（如 `WPA2-Personal` / `开放式`）或手输模式直接传
/// `WPA`/`SAE`/`WEP`/`nopass`；空串时按 nopass 处理（与 Python `_qr_ready` 同口径）。
#[tauri::command]
pub fn sk_wifi_qr_svg(
    ssid: String,
    password: String,
    auth: Option<String>,
) -> Result<String, String> {
    let data = crate::wifi::wifi_qr_string(&ssid, &password, auth.as_deref().unwrap_or(""));
    crate::wifi::render_qr_svg(&data)
}

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

// ---------------- 剪贴板监听状态 ----------------

/// 监听状态（是否运行/启用、已记录与两类跳过计数）。
///
/// 计数是**可验证性**的关键：用户说"我复制了却没进历史"时，这两个计数能直接
/// 区分"没监听"、"被黑名单挡了"、"被去重挡了"三种情况。
#[tauri::command]
pub fn sk_clip_watch_state() -> serde_json::Value {
    serde_json::to_value(crate::clipboard_watch::state()).unwrap_or(serde_json::json!({}))
}

/// 暂停/恢复记录（隐私暂停：不记录新内容，历史保留）。
///
/// 同时把总开关写进设置：暂停状态要跨重启保持，否则用户以为暂停了、
/// 重启后却又开始记录——这是隐私工具里不能有的意外。
#[tauri::command]
pub fn sk_clip_watch_set(enabled: bool) -> Result<serde_json::Value, String> {
    crate::clipboard_watch::set_enabled(enabled);
    let mut patch = serde_json::Map::new();
    patch.insert(
        "clipboard_watch".to_string(),
        serde_json::Value::Bool(enabled),
    );
    crate::settings::save(&patch).map_err(|e| format!("保存设置失败: {e}"))?;
    Ok(serde_json::json!({
        "enabled": enabled,
        "message": if enabled { "已开始记录剪贴板" } else { "已暂停记录（历史保留，恢复后不补录）" },
    }))
}

/// 读一次剪贴板快照（调试/测试用：确认读取原语在真机可用）。
#[tauri::command]
pub fn sk_clip_probe() -> serde_json::Value {
    let s = crate::clipboard_watch::read_snapshot();
    serde_json::json!({
        "has_text": s.text.is_some(),
        "text_len": s.text.as_ref().map(|t| t.chars().count()).unwrap_or(0),
        "file_count": s.files.as_ref().map(|f| f.len()).unwrap_or(0),
        "has_image": s.has_image,
        "has_html": s.has_html,
    })
}

// ---------------- M7 命令面板 provider ----------------

/// 应用内 Ctrl+K 呼出/收起命令面板（对齐全局热键的行为）。
///
/// README 宣称「应用内 Ctrl+K」但此前主窗没有任何处理（CDP 实锤：dispatch
/// Ctrl+K 后 /json 无新 target）——Python 版有此行为，这里补齐：主窗键盘
/// 事件经前端 invoke 到这，复用与全局热键同一条 toggle_palette 链路
/// （定位到光标所在显示器、show+focus、发 sk://panel-shown）。
#[tauri::command]
pub fn sk_palette_toggle(app: tauri::AppHandle) {
    crate::input_panel::toggle_palette(&app);
}

/// 面板动作全集：内置页面跳转 + 自定义命令，按查询排序。
///
/// 匹配与排序在 Rust 侧做（与 Python 同口径），前端只渲染结果——这样两版的
/// **同一个查询排出同样的顺序**，用户不会觉得"搜索结果变了"。
#[tauri::command]
pub fn sk_palette_actions(query: Option<String>) -> serde_json::Value {
    let q = query.unwrap_or_default();
    let mut actions = builtin_actions();
    let raw = crate::settings::get_str("palette_commands");
    let cmds = crate::palette::parse_custom_commands(&raw);
    actions.extend(crate::palette::custom_to_actions(&cmds));
    let ranked = crate::palette::rank_actions(&q, &actions);
    serde_json::json!({
        "rows": ranked,
        "count": ranked.len(),
        "total": actions.len(),
        "custom_count": cmds.len(),
    })
}

/// 内置动作（页面跳转）。页内 Tab 由前端按当前页面结构补充。
fn builtin_actions() -> Vec<crate::palette::PaletteAction> {
    const PAGES: [(&str, &str, &str); 10] = [
        ("nav:overview", "打开 概览", "overview"),
        ("nav:clipboard", "打开 剪贴板", "clipboard"),
        ("nav:diagnose", "打开 诊断与修复", "diagnose"),
        ("nav:toolbox", "打开 工具箱", "toolbox"),
        ("nav:system", "打开 系统助手", "system"),
        ("nav:vault", "打开 密码库", "vault"),
        ("nav:settings", "打开 设置 / 日志", "settings"),
        ("nav:hosts", "打开 Hosts 编辑", "toolbox"),
        ("nav:ports", "打开 端口 / 局域网", "toolbox"),
        ("nav:dns", "打开 DNS / 子网 / ping", "toolbox"),
    ];
    PAGES
        .iter()
        .map(|(id, label, page)| crate::palette::PaletteAction {
            id: (*id).to_string(),
            label: (*label).to_string(),
            hint: "页面跳转".to_string(),
            keywords: vec![
                "goto".to_string(),
                "页面".to_string(),
                "打开".to_string(),
                (*page).to_string(),
            ],
            source: "内置".to_string(),
        })
        .collect()
}

// ---------------- M7 设置 / 日志 ----------------

/// 全部设置的当前值与界面元数据。
#[tauri::command]
pub fn sk_settings_all() -> serde_json::Value {
    serde_json::json!({
        "items": crate::settings::describe_all(),
        "count": crate::settings::defaults().len(),
    })
}

/// 写入一个设置项（类型收敛 + 范围校验都在后端做，界面可被绕过）。
///
/// 热键三键保存后即时重挂（语义对齐 Python `_rehook_hotkey`）：被占回退
/// 旧值并持久化 + 发 `sk://hotkey-reverted`；`previous` 快照在写入前捕获。
/// 保存成功一律广播 `sk://settings-changed`：主窗的 dark 状态等「设置驱动的
/// 运行时状态」以此同步（此前深色开关只写文件不改 UI 状态，勾选不变暗）。
#[tauri::command]
pub fn sk_settings_set(
    key: String,
    value: serde_json::Value,
    app: tauri::AppHandle,
) -> Result<serde_json::Value, String> {
    use tauri::Emitter;
    const HOTKEY_KEYS: [&str; 3] = ["palette_hotkey", "clip_quick_hotkey", "clip_paste_hotkey"];
    let previous = if HOTKEY_KEYS.contains(&key.as_str()) {
        Some(crate::settings::get_str(&key))
    } else {
        None
    };
    let saved = crate::settings::set_one(&key, value)?;
    let _ = app.emit(
        "sk://settings-changed",
        serde_json::json!({ "key": key, "value": saved }),
    );
    if let Some(previous) = previous {
        let prev_getter = |k: &str| -> String {
            if k == key {
                previous.clone()
            } else {
                crate::settings::get_str(k)
            }
        };
        crate::apply_hotkeys(&app, &prev_getter);
    }
    Ok(serde_json::json!({ "key": key, "value": saved, "message": "已保存" }))
}

/// 热键三键的重置回退目标快照（纯读，供 `sk_settings_reset` 与单测）。
fn hotkey_reset_fallbacks() -> [String; 3] {
    ["palette_hotkey", "clip_quick_hotkey", "clip_paste_hotkey"].map(crate::settings::get_str)
}

/// 恢复默认设置的公共体（`rehook` 注入以便单测）：**先**快照热键三键旧值，
/// **再**落默认，最后把快照交给 `rehook` 即时重挂。顺序是语义的一部分——
/// 落了默认再快照就只剩默认值，被占回退会因「回退目标==请求值」失去回退步，
/// 用户旧组合从此找不回（轮次37 变异验证锁定此顺序）。
fn reset_settings_with_rehook<R>(rehook: R) -> Result<String, String>
where
    R: FnOnce(&dyn Fn(&str) -> String),
{
    let previous = hotkey_reset_fallbacks();
    let n = crate::settings::reset_to_defaults()?;
    let prev_getter = move |k: &str| -> String {
        match k {
            "palette_hotkey" => previous[0].clone(),
            "clip_quick_hotkey" => previous[1].clone(),
            "clip_paste_hotkey" => previous[2].clone(),
            _ => String::new(),
        }
    };
    rehook(&prev_getter);
    Ok(format!(
        "已恢复 {n} 项默认值（动态键如 modes_prev_volume 保留）"
    ))
}

/// 恢复默认值（只重置已知键，动态键保留）。
///
/// 热键三键重置后**必须即时重挂**（轮次37 互审实锤）：只写设置不重挂，
/// 注册链还挂着用户旧组合，设置页却显示默认组合——按默认组合按了没反应，
/// 三方一致（注册链 × 设置页 × 侧栏速查）被 reset 路径击穿。重置**前**快照
/// 旧值作为回退目标：默认组合被占时链回退到旧值并持久化 + 发
/// `sk://hotkey-reverted`，与 `sk_settings_set` 保存路径同语义。
#[tauri::command]
pub fn sk_settings_reset(app: tauri::AppHandle) -> Result<String, String> {
    reset_settings_with_rehook(|prev| {
        crate::apply_hotkeys(&app, prev);
    })
}

/// 日志文件列表（按修改时间倒序）。
#[tauri::command]
pub fn sk_log_list() -> serde_json::Value {
    let dir = crate::paths::log_dir();
    let mut files: Vec<serde_json::Value> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            if !p.is_file() {
                continue;
            }
            let meta = e.metadata().ok();
            let name = e.file_name().to_string_lossy().to_string();
            // 只列日志类文件，避免把目录里的杂项也展示出来
            if !name.ends_with(".log") && !name.ends_with(".txt") {
                continue;
            }
            files.push(serde_json::json!({
                "name": name,
                "bytes": meta.as_ref().map(|m| m.len()).unwrap_or(0),
                "modified": meta
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
            }));
        }
    }
    files.sort_by(|a, b| b["modified"].as_u64().cmp(&a["modified"].as_u64()));
    serde_json::json!({
        "dir": dir.to_string_lossy(),
        "files": files,
        "count": files.len(),
    })
}

/// 读取日志尾部若干行（**只读尾部**：日志可能很大，整读会卡住界面）。
#[tauri::command]
pub async fn sk_log_tail(name: String, lines: Option<usize>) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        // 防目录穿越：只允许纯文件名
        if name.contains('/') || name.contains('\\') || name.contains("..") {
            return Err(format!("非法的日志文件名: {name}"));
        }
        let path = crate::paths::log_dir().join(&name);
        if !path.is_file() {
            return Err(format!("日志文件不存在: {name}"));
        }
        let n = lines.unwrap_or(500).clamp(1, 5000);
        let bytes = std::fs::read(&path).map_err(|e| format!("读取失败: {e}"))?;
        // 日志既有 UTF-8 也有 ANSI 的可能：lossy 解码保证不因个别字节整篇失败
        let text = String::from_utf8_lossy(&bytes);
        let all: Vec<&str> = text.lines().collect();
        let start = all.len().saturating_sub(n);
        let tail: Vec<&str> = all[start..].to_vec();
        Ok(serde_json::json!({
            "name": name,
            "total_lines": all.len(),
            "shown": tail.len(),
            "truncated": start > 0,
            "lines": tail,
        }))
    })
    .await
    .map_err(|e| format!("读取线程失败: {e}"))?
}

// ---------------- M6 密码库（会话式：解锁后数据命令共享同一实例） ----------------

/// 会话持有解锁后的 VaultCrypto；None = 未解锁。
/// 旧实现每个命令各 new 一个实例——unlock 与数据命令不在同一实例上，
/// 数据密钥根本传不过去（M6 接线时修正为托管会话）。
pub struct VaultSession(pub std::sync::Mutex<Option<crate::vault_crypto::VaultCrypto>>);

/// 主密码免重复输入窗口的到期时刻（sk_vault_verify_password 成功时续期）。
pub struct VaultTrust(pub std::sync::Mutex<Option<std::time::Instant>>);

fn session_lock(
    session: &VaultSession,
) -> Result<std::sync::MutexGuard<'_, Option<crate::vault_crypto::VaultCrypto>>, String> {
    // 库表确保存在：所有 vault 命令都从这里进——曾经只有 sk_vault_sites 调了
    // init_db，"设好主密码→直接添加站点"会撞 no such table: sites（真机实证
    // 2026-10-04）。init_db 幂等且带进程级短路，放在公共入口零遗漏。
    crate::vault_store::init_db().map_err(|e| format!("密码库初始化失败: {e}"))?;
    // 自动重锁：vault_autolock_min>0 且**闲置**超过窗口 → 就地收锁。
    // 「闲置」= 距上次密码库操作（unlocked_at 会被每次放行的操作 touch 刷新），
    // 与 Python 侧 _ensure_unlocked → _restart_autolock 同口径——按"解锁那一刻"
    // 起算的话，连续操作的用户也会被中途踢出，TOTP 轮询显示会反复被
    // 「密码库未解锁」打断（轮次36 跨模块审计实锤）。
    {
        let mut guard = session.0.lock().expect("vault 会话锁中毒");
        let min = crate::settings::get_int("vault_autolock_min");
        if let (Some(c), true) = (guard.as_mut(), min > 0) {
            if let Some(since) = c.unlocked_since_secs() {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0);
                if vault_autolock_due(now - since, min) {
                    c.lock();
                    *guard = None;
                }
            }
        }
        // 活跃续期：本次操作放行后把闲置计时归零（下次操作的 due 判定以此为基准）
        if let Some(c) = guard.as_mut() {
            c.touch_unlocked();
        }
    }
    let guard = session.0.lock().expect("vault 会话锁中毒");
    if guard.is_none() {
        return Err("密码库未解锁".into());
    }
    Ok(guard)
}

/// 自动重锁到期判定（纯函数，供单测/变异验证）：闲置秒数达到 `min` 分钟。
/// `min<=0` 恒不锁定（设置 0 = 不自动锁）；边界取 `>=`（与 monitor 冷却同风格）。
fn vault_autolock_due(idle_secs: f64, min: i64) -> bool {
    min > 0 && idle_secs >= min as f64 * 60.0
}

/// 取会话守卫（未解锁 → Err）。调用方以 `guard.as_mut().expect(...)` 拿实例——
/// MutexGuard::map 尚未稳定（mapped_lock_guards），不走它。
fn session_get(
    session: &VaultSession,
) -> Result<std::sync::MutexGuard<'_, Option<crate::vault_crypto::VaultCrypto>>, String> {
    session_lock(session)
}

/// DPAPI 免密模式专用：状态命令里把自动解锁的实例复制进会话
/// （本实例继续读 meta；数据密钥是随机 32B，重解锁无副作用）。
fn reopen_unlocked(source: &crate::vault_crypto::VaultCrypto) -> crate::vault_crypto::VaultCrypto {
    let mut n = crate::vault_crypto::VaultCrypto::with_path(source.path().to_path_buf());
    let _ = n.unlock(None);
    n
}

/// 密码库状态（含各能力的可用性，供界面如实展示）。
#[tauri::command]
pub fn sk_vault_status(
    session: tauri::State<'_, VaultSession>,
    trust: tauri::State<'_, VaultTrust>,
) -> serde_json::Value {
    let mut c = crate::vault_crypto::VaultCrypto::new();
    let path = c.path().to_string_lossy().to_string();
    let provisioned = c.is_provisioned();
    let meta = c.read_meta();
    let has_pw = provisioned && meta.mode == "password";
    // 免密模式可以自动解锁（DPAPI 绑定当前用户，无需输入）
    if provisioned && meta.mode == "dpapi" && c.unlock(None) == Ok(true) {
        // 自动解锁后的实例进会话：数据命令立即可用
        let auto = reopen_unlocked(&c);
        *session.0.lock().expect("vault 会话锁中毒") = Some(auto);
    }
    let session_unlocked = session.0.lock().expect("vault 会话锁中毒").is_some();
    let trusted_until = trust.0.lock().expect("vault trust 锁中毒").map(|t| {
        t.checked_duration_since(std::time::Instant::now())
            .map(|d| d.as_secs())
            .unwrap_or(0)
    });
    serde_json::json!({
        "path": path,
        "provisioned": provisioned,
        "unlocked": session_unlocked,
        "has_master_password": has_pw,
        "mode": meta.mode,
        "trusted_until": trusted_until,
        // 各能力可用性：界面据此决定按钮能不能点、要不要显示说明
        "capabilities": {
            "key_management": true,
            "password_storage": true,
        },
    })
}

/// 初始化密码库（首次创建 `vault.key`）。`password` 为空则用 DPAPI 免密模式。
#[tauri::command]
pub async fn sk_vault_setup(
    session: tauri::State<'_, VaultSession>,
    password: Option<String>,
) -> Result<serde_json::Value, String> {
    // 600k 次 PBKDF2 在主线程会明显卡顿，所以走阻塞线程池；
    // State 借用不能进 'static 闭包——本地实例跑完再进会话
    let (c, mode) = tauri::async_runtime::spawn_blocking(move || {
        let mut c = crate::vault_crypto::VaultCrypto::new();
        if c.is_provisioned() {
            return Err(("密码库已初始化".to_string(), "dpapi"));
        }
        let pw = password.filter(|p| !p.is_empty());
        let mode = if pw.is_some() { "password" } else { "dpapi" };
        c.setup(pw.as_deref()).map_err(|e| (e, mode))?;
        Ok((c, mode))
    })
    .await
    .map_err(|e| format!("初始化线程失败: {e}"))?
    .map_err(|(e, _)| e)?;
    *session.0.lock().expect("vault 会话锁中毒") = Some(c);
    Ok(serde_json::json!({
        "ok": true,
        "mode": mode,
        "message": if mode == "password" {
            "已创建密码库（主密码模式）"
        } else {
            "已创建密码库（免密模式：密钥由 Windows DPAPI 绑定当前用户保护）"
        },
    }))
}

/// 解锁。失败返回可读原因（区分密码错误与文件损坏）。
#[tauri::command]
pub async fn sk_vault_unlock(
    session: tauri::State<'_, VaultSession>,
    password: Option<String>,
) -> Result<serde_json::Value, String> {
    let unlocked = tauri::async_runtime::spawn_blocking(move || {
        let mut c = crate::vault_crypto::VaultCrypto::new();
        match c.unlock(password.as_deref()) {
            Ok(true) => Ok(c),
            Ok(false) => Err("解锁失败".to_string()),
            Err(e) => Err(e.message()),
        }
    })
    .await
    .map_err(|e| format!("解锁线程失败: {e}"))?;
    let c = unlocked?;
    *session.0.lock().expect("vault 会话锁中毒") = Some(c);
    Ok(serde_json::json!({ "ok": true, "message": "已解锁" }))
}

/// 锁定（清空内存中的密钥）。
#[tauri::command]
pub fn sk_vault_lock(session: tauri::State<VaultSession>) -> serde_json::Value {
    if let Some(mut c) = session.0.lock().expect("vault 会话锁中毒").take() {
        c.lock();
    }
    serde_json::json!({ "ok": true, "message": "已锁定" })
}

/// 设置主密码（免密 → 密码模式；数据密钥不变）。
#[tauri::command]
pub async fn sk_vault_set_password(
    session: tauri::State<'_, VaultSession>,
    password: String,
) -> Result<String, String> {
    // 会话实例先 take 出来（重挂 600k PBKDF2 期间数据命令短暂不可用，可接受），
    // 避免把 State 借用带进 'static 阻塞闭包
    let mut c = session
        .0
        .lock()
        .expect("vault 会话锁中毒")
        .take()
        .ok_or_else(|| "密码库未解锁".to_string())?;
    let result =
        tauri::async_runtime::spawn_blocking(move || match c.set_master_password(&password) {
            Ok(()) => Ok(c),
            Err(e) => Err((c, e)),
        })
        .await
        .map_err(|e| format!("设置线程失败: {e}"))?;
    // 无论成败都放回会话（实例仍持有解锁态）
    match result {
        Ok(c) => {
            *session.0.lock().expect("vault 会话锁中毒") = Some(c);
            Ok("已设置主密码（数据密钥未变，已有密文仍可解开）".to_string())
        }
        Err((c, e)) => {
            *session.0.lock().expect("vault 会话锁中毒") = Some(c);
            Err(e)
        }
    }
}

/// 校验主密码（不改变解锁状态）；成功续期免重复输入窗口。
#[tauri::command]
pub async fn sk_vault_verify_password(
    trust: tauri::State<'_, VaultTrust>,
    password: String,
) -> Result<bool, String> {
    let ok = tauri::async_runtime::spawn_blocking(move || {
        let c = crate::vault_crypto::VaultCrypto::new();
        c.verify_password(&password)
    })
    .await
    .map_err(|e| format!("校验线程失败: {e}"))?;
    if ok {
        let mins = crate::settings::get_int("vault_trust_min").max(0) as u64;
        let mut t = trust.0.lock().expect("vault trust 锁中毒");
        *t = if mins > 0 {
            Some(std::time::Instant::now() + std::time::Duration::from_secs(mins * 60))
        } else {
            None
        };
    }
    Ok(ok)
}

/// 移除主密码（回到免密模式）。当前密码错误返回 Ok(false)。
#[tauri::command]
pub async fn sk_vault_remove_password(
    session: tauri::State<'_, VaultSession>,
    current: String,
) -> Result<bool, String> {
    let mut c = session
        .0
        .lock()
        .expect("vault 会话锁中毒")
        .take()
        .ok_or_else(|| "密码库未解锁".to_string())?;
    let (c, result) = tauri::async_runtime::spawn_blocking(move || {
        let r = c.remove_master_password(&current);
        (c, r)
    })
    .await
    .map_err(|e| format!("移除线程失败: {e}"))?;
    *session.0.lock().expect("vault 会话锁中毒") = Some(c);
    result
}

// ---------------- M6 数据层命令（vault_store 薄封装；会话必须已解锁） ----------------

#[tauri::command]
pub fn sk_vault_sites(
    session: tauri::State<'_, VaultSession>,
    q: String,
) -> Result<serde_json::Value, String> {
    let _session = session_lock(&session)?;
    crate::vault_store::init_db().map_err(|e| e.to_string())?;
    let sites = crate::vault_store::search_sites(&q).map_err(|e| e.to_string())?;
    serde_json::to_value(sites).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn sk_vault_site_accounts(
    session: tauri::State<'_, VaultSession>,
    site_id: i64,
    q: String,
) -> Result<serde_json::Value, String> {
    let _session = session_lock(&session)?;
    let accs = crate::vault_store::list_accounts(site_id, &q).map_err(|e| e.to_string())?;
    serde_json::to_value(accs).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn sk_vault_add_site(
    session: tauri::State<'_, VaultSession>,
    name: String,
    domain: String,
    url: String,
    aliases: String,
    tags: String,
) -> Result<i64, String> {
    let _session = session_lock(&session)?;
    crate::vault_store::add_site(&name, &domain, &url, &aliases, &tags).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn sk_vault_update_site(
    session: tauri::State<'_, VaultSession>,
    site_id: i64,
    name: Option<String>,
    domain: Option<String>,
    url: Option<String>,
    aliases: Option<String>,
    tags: Option<String>,
) -> Result<(), String> {
    let _session = session_lock(&session)?;
    crate::vault_store::update_site(
        site_id,
        &crate::vault_store::SitePatch {
            name,
            domain,
            url,
            aliases,
            tags,
        },
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn sk_vault_delete_site(
    session: tauri::State<'_, VaultSession>,
    site_id: i64,
) -> Result<(), String> {
    let _session = session_lock(&session)?;
    crate::vault_store::delete_site(site_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn sk_vault_add_account(
    session: tauri::State<'_, VaultSession>,
    site_id: i64,
    password: String,
    label: String,
    username: String,
    totp_secret: String,
    notes: String,
) -> Result<i64, String> {
    let mut guard = session_get(&session)?;
    let crypto = guard.as_mut().expect("已确认 Some");
    crate::vault_store::add_account(
        site_id,
        &password,
        crypto,
        &label,
        &username,
        &totp_secret,
        &notes,
    )
}

#[tauri::command]
pub fn sk_vault_update_account(
    session: tauri::State<'_, VaultSession>,
    account_id: i64,
    label: Option<String>,
    username: Option<String>,
    password: Option<String>,
    totp_secret: Option<String>,
    notes: Option<String>,
) -> Result<(), String> {
    let mut guard = session_get(&session)?;
    let crypto = guard.as_mut().expect("已确认 Some");
    crate::vault_store::update_account(
        account_id,
        crypto,
        &crate::vault_store::AccountPatch {
            label,
            username,
            password,
            totp_secret,
            notes,
        },
    )
}

#[tauri::command]
pub fn sk_vault_delete_account(
    session: tauri::State<'_, VaultSession>,
    account_id: i64,
) -> Result<(), String> {
    let _session = session_lock(&session)?;
    crate::vault_store::delete_account(account_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn sk_vault_toggle_favorite(
    session: tauri::State<'_, VaultSession>,
    account_id: i64,
) -> Result<bool, String> {
    let _session = session_lock(&session)?;
    crate::vault_store::toggle_favorite(account_id).map_err(|e| e.to_string())
}

/// 解密单字段。`sensitive_ttl` 给定时同步登记敏感窗口（复制密码/TOTP 用，
/// 与 Python 版 UI 同口径：TTL 内剪贴板监听不记录 + 系统侧排除）。
#[tauri::command]
pub fn sk_vault_get_secret(
    session: tauri::State<'_, VaultSession>,
    account_id: i64,
    field: String,
    touch: Option<bool>,
) -> Result<String, String> {
    let mut guard = session_get(&session)?;
    let crypto = guard.as_mut().expect("已确认 Some");
    crate::vault_store::get_secret(account_id, crypto, &field, touch.unwrap_or(true))
}

/// 解密并复制（明文不过前端 JS；三重敏感防护与复制命令同一实现）。
#[tauri::command]
pub fn sk_vault_copy_secret(
    session: tauri::State<'_, VaultSession>,
    account_id: i64,
    field: String,
) -> Result<serde_json::Value, String> {
    let mut guard = session_get(&session)?;
    let crypto = guard.as_mut().expect("已确认 Some");
    let text = crate::vault_store::get_secret(account_id, crypto, &field, true)?;
    copy_sensitive_text(&text)
}

/// 明文复制（用户名等非敏感字段；走系统剪贴板，无敏感防护）。
#[tauri::command]
pub fn sk_clip_copy_plain(text: String) -> Result<(), String> {
    if text.is_empty() {
        return Err("内容为空".to_string());
    }
    if crate::w32::clipboard_set_text(&text) {
        Ok(())
    } else {
        Err("写入剪贴板失败（可能被其他程序占用）".to_string())
    }
}

/// 批量解密（整列显示语义；单条损坏降级缺席，不外溢）。
#[tauri::command]
pub fn sk_vault_secrets_bulk(
    session: tauri::State<'_, VaultSession>,
    account_ids: Vec<i64>,
    field: String,
) -> Result<serde_json::Value, String> {
    let mut guard = session_get(&session)?;
    let crypto = guard.as_mut().expect("已确认 Some");
    let map = crate::vault_store::get_secrets_bulk(&account_ids, crypto, &field)?;
    serde_json::to_value(map).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn sk_vault_global_search(
    session: tauri::State<'_, VaultSession>,
    q: String,
) -> Result<serde_json::Value, String> {
    let _session = session_lock(&session)?;
    let rows = crate::vault_store::global_search(&q).map_err(|e| e.to_string())?;
    serde_json::to_value(rows).map_err(|e| e.to_string())
}

/// 现算 TOTP 验证码（密文按需解密；验证码本身不进敏感窗口）。
#[tauri::command]
pub fn sk_vault_totp_code(
    session: tauri::State<'_, VaultSession>,
    account_id: i64,
) -> Result<String, String> {
    let mut guard = session_get(&session)?;
    let crypto = guard.as_mut().expect("已确认 Some");
    let secret = crate::vault_store::get_secret(account_id, crypto, "totp", false)?;
    Ok(crate::vault_store::totp_code(&secret, None, 6, 30))
}

/// 生成随机密码（表单「生成」按钮）。参数与语义见 pwgen 模块头注释：
/// 拒绝采样保证均匀、每个启用类保底一个、参数不对（长度越界/未启用任何
/// 字符类）返回 Err 由前端提示。
///
/// 不依赖密码库会话——生成是纯 CSPRNG 运算，不触碰任何密文。
/// **纪律**：生成的密码在用户点「保存」前只活在表单输入框里，但它已经是
/// 凭据材料（暴露窗口与已保存密码一致）——此时复制必须走
/// `sk_clip_copy_sensitive` 敏感通道（不进本应用历史 / 不进 Win+V 与云剪贴板 /
/// 到期清空），与 `vault::password_copy_routes_through_sensitive` 守卫维持的
/// 密码类口径相同；前端源码守卫 tests/vault.test.ts 锁死这一接线。
#[tauri::command]
pub fn sk_vault_generate_password(
    length: usize,
    upper: bool,
    lower: bool,
    digits: bool,
    symbols: bool,
    exclude_ambiguous: bool,
) -> Result<String, String> {
    crate::pwgen::generate(crate::pwgen::GenOptions {
        length,
        upper,
        lower,
        digits,
        symbols,
        exclude_ambiguous,
    })
}

// ---------------- M5 系统助手：剪贴板历史（管理动作） ----------------

/// 把一条历史保存为文件（图片 → PNG，其它 → 文本）。
///
/// 文本文件名做**路径分隔符消毒**：条目内容不可信，直接拼进默认文件名里
/// 可能带 `..\` 之类的片段；虽然最终路径由用户在对话框里确认，默认值也不该
/// 指向目录之外。
/// 解码任意图片字节里的二维码（base64）：工具箱「二维码识别」tab 的选图入口。
#[tauri::command]
pub async fn sk_qr_decode_image(data: String) -> Result<String, String> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|e| format!("图片数据不合法: {e}"))?;
    tauri::async_runtime::spawn_blocking(move || crate::qrdecode::decode_image_bytes(&bytes))
        .await
        .map_err(|e| format!("解码线程失败: {e}"))?
}

/// 识别剪贴板当前图片（CF_DIB → PNG → 解码）：工具箱「从剪贴板识别」入口。
#[tauri::command]
pub async fn sk_qr_decode_clipboard() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let dib = crate::w32::clipboard_get_dib().ok_or("剪贴板里没有图片")?;
        let bmp = crate::dib::parse_dib(&dib).map_err(|e| format!("剪贴板图片解析失败: {e}"))?;
        let png = crate::dib::encode_png(&bmp).map_err(|e| format!("图片转码失败: {e}"))?;
        crate::qrdecode::decode_image_bytes(&png)
    })
    .await
    .map_err(|e| format!("解码线程失败: {e}"))?
}

/// 识别剪贴板历史里的图片条目（index 与缩略图同语义，路径限定 clip_files）。
#[tauri::command]
pub async fn sk_qr_decode_clip_item(index: usize) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let name = hist::with(|h| {
            h.search(None, None)
                .get(index)
                .filter(|i| i.kind == "image")
                .map(|i| i.data.clone())
        })
        .ok_or_else(|| format!("找不到第 {index} 条图片历史（可能已被淘汰）"))?;
        let src =
            crate::paths::image_path(&name).ok_or_else(|| format!("图片文件名非法: {name}"))?;
        let bytes = std::fs::read(&src)
            .map_err(|e| format!("图片读取失败（{}）: {e}", src.to_string_lossy()))?;
        crate::qrdecode::decode_image_bytes(&bytes)
    })
    .await
    .map_err(|e| format!("解码线程失败: {e}"))?
}

/// 用系统默认浏览器/关联打开 URL（二维码识别结果的「打开链接」动作）。
///
/// **只放行 http/https**：shell_open 走 ShellExecuteW，任意串能触发任意
/// 协议（ms-settings、file:、自定义协议＝可执行入口），二维码内容不可信，
/// 白名单在命令边界做死。
#[tauri::command]
pub fn sk_open_url(url: String) -> Result<(), String> {
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        crate::w32::shell_open(&url)
    } else {
        Err("只允许打开 http/https 链接".to_string())
    }
}
///
/// 用 image crate 缩到最长边 96px 再编码——历史里可能几十张截图，
/// 原图直出会把列表内存/传输量顶到几十 MB。点击放大由前端 AntD Image 预览承担。
#[tauri::command]
pub async fn sk_clip_image_thumb(index: usize) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let name = hist::with(|h| {
            h.search(None, None)
                .get(index)
                .filter(|i| i.kind == "image")
                .map(|i| i.data.clone())
        })
        .ok_or_else(|| format!("找不到第 {index} 条图片历史（可能已被淘汰）"))?;
        let src =
            crate::paths::image_path(&name).ok_or_else(|| format!("图片文件名非法: {name}"))?;
        let bytes = std::fs::read(&src)
            .map_err(|e| format!("图片读取失败（{}）: {e}", src.to_string_lossy()))?;
        let img = image::load_from_memory(&bytes).map_err(|e| format!("图片解码失败: {e}"))?;
        let thumb = img.thumbnail(96, 96);
        let mut out = std::io::Cursor::new(Vec::new());
        thumb
            .write_to(&mut out, image::ImageFormat::Png)
            .map_err(|e| format!("缩略图编码失败: {e}"))?;
        use base64::Engine as _;
        Ok(format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(out.into_inner())
        ))
    })
    .await
    .map_err(|e| format!("缩略图线程失败: {e}"))?
}

#[tauri::command]
pub async fn sk_clip_save_as(index: usize) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let row = hist::with(|h| h.search(None, None).get(index).map(ClipRow::from_item))
            .ok_or_else(|| format!("找不到第 {index} 条历史（可能已被淘汰）"))?;

        if row.kind == "image" {
            let src = crate::paths::image_path(&row.data)
                .ok_or_else(|| format!("图片文件名非法: {}", row.data))?;
            if !src.is_file() {
                return Err(format!("图片文件已不存在: {}", src.to_string_lossy()));
            }
            let target = crate::w32::save_file_dialog(
                "图片另存为",
                "clip.png",
                "PNG 图片\0*.png\0所有文件\0*.*\0",
            )?;
            if target.is_empty() {
                return Ok(String::new()); // 取消
            }
            let target = ensure_ext(&target, "png");
            std::fs::copy(&src, &target).map_err(|e| format!("保存失败（路径不可写？）: {e}"))?;
            Ok(format!("图片已保存到 {target}"))
        } else {
            let default = safe_default_name(&row.text);
            let filter = if row.kind == "html" {
                "HTML 文件\0*.html\0文本文件\0*.txt\0所有文件\0*.*\0"
            } else {
                "文本文件\0*.txt\0所有文件\0*.*\0"
            };
            let target = crate::w32::save_file_dialog("另存为", &default, filter)?;
            if target.is_empty() {
                return Ok(String::new());
            }
            // 写入用与 hosts 一致的裸 \n；内容按 UTF-8
            std::fs::write(&target, row.text.as_bytes())
                .map_err(|e| format!("保存失败（路径不可写？）: {e}"))?;
            Ok(format!("已保存到 {target}"))
        }
    })
    .await
    .map_err(|e| format!("保存线程失败: {e}"))?
}

/// 目标路径没有扩展名时补上（与 Python 对 .png 的处理一致）。
fn ensure_ext(path: &str, ext: &str) -> String {
    let lower = path.to_lowercase();
    if lower.ends_with(&format!(".{ext}")) {
        path.to_string()
    } else {
        format!("{path}.{ext}")
    }
}

/// 由条目内容生成安全的默认文件名（不含路径分隔符与非法字符）。
///
/// 只用于文本/HTML 条目；图片走 `clip.png` 那套（对话框里已给固定默认名）。
pub fn safe_default_name(text: &str) -> String {
    let head: String = text
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(40)
        // Windows 文件名非法字符：\ / : * ? " < > |，另加控制字符
        .map(|c| {
            if matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    // 去掉首尾空白与点：以点结尾在 Windows 上会被静默截断
    let trimmed = head.trim().trim_matches('.').trim();
    if trimmed.is_empty() {
        "clip.txt".to_string()
    } else {
        format!("{trimmed}.txt")
    }
}

// ---------------- M5 系统助手：场景模式 ----------------

/// 场景模式清单（供前端渲染按钮与说明）。
#[tauri::command]
pub fn sk_modes_info() -> serde_json::Value {
    migrate_modes_active_keys();
    serde_json::json!({
        "common_apps": crate::modes::COMMON_APPS
            .iter()
            .map(|(n, t)| serde_json::json!({ "name": n, "target": t }))
            .collect::<Vec<_>>(),
        "office_default_apps": crate::modes::OFFICE_DEFAULT_APPS,
        "whitelist": crate::modes::BACKGROUND_CLOSE_WHITELIST,
        "is_admin": crate::w32::is_elevated(),
        // 当前生效模式（settings 双键 `modes_active_office`/`modes_active_game`，
        // 旧单键已就地迁移；run 全部步骤成功才置位，退出即清自己族）
        "active_mode": {
            "office": modes_flag_on("modes_active_office"),
            "game": modes_flag_on("modes_active_game"),
        },
        "notes": {
            "office": "勿扰（关通知）+ 静音 + 打开常用软件 + 备份当前配置。退出模式会恢复通知并取消静音。",
            "game": "关通知 + 高性能电源 + 暂停 Windows 更新 + 清理白名单后台。退出模式会恢复通知、平衡电源与更新策略。",
            "update_policy": "「暂停更新」通过 NoAutoUpdate 策略实现，是持久生效的（不是暂停 N 天），需退出游戏模式或手动恢复。",
            "admin": "暂停更新与部分电源操作需要管理员权限；失败步骤会如实列出，不影响其它步骤。"
        }
    })
}

/// 场景模式双键取值：开 = "1"。旧单键存的是模式名（"office"/"game"/""），
/// 双键拆族后每键只有 开/关 两态，"1"/"0" 与 `get_str`（缺失→空串→关）直配。
fn modes_flag_str(on: bool) -> &'static str {
    if on {
        "1"
    } else {
        "0"
    }
}

/// 场景模式双键读取（缺失/空串/脏值一律视为关）。
fn modes_flag_on(key: &str) -> bool {
    crate::settings::get_str(key) == "1"
}

/// 旧单键 `modes_active` 的值 → 双键初值 `(office, game)`（纯函数，供单测）。
pub fn modes_dual_from_legacy(legacy: &str) -> (bool, bool) {
    (legacy == "office", legacy == "game")
}

/// 一次性迁移：读到旧单键 `modes_active` 时按其值初始化双键并**清旧键**。
///
/// 幂等：旧键不存在直接返回（不动盘）；旧键显式为 ""（旧版退出写过）也走
/// 迁移——双键落 "0" 后旧键删除，此后读写面只剩双键。save 成功但清键失败时
/// 旧键仍在，下次读到会再迁一遍（写同值 + 再清），收敛。
fn migrate_modes_active_keys() {
    let Some(v) = crate::settings::load().get("modes_active").cloned() else {
        return;
    };
    let (office, game) = modes_dual_from_legacy(v.as_str().unwrap_or(""));
    let mut patch = serde_json::Map::new();
    patch.insert(
        "modes_active_office".to_string(),
        serde_json::Value::String(modes_flag_str(office).to_string()),
    );
    patch.insert(
        "modes_active_game".to_string(),
        serde_json::Value::String(modes_flag_str(game).to_string()),
    );
    if let Err(e) = crate::settings::save(&patch) {
        log::warn!("场景模式双键迁移写入失败（旧键保留，下次再迁）: {e}");
        return;
    }
    if let Err(e) = crate::settings::remove_keys(&["modes_active"]) {
        log::warn!("场景模式旧单键清除失败（下次再迁）: {e}");
    }
}

/// 场景模式执行后双键（`modes_active_office` / `modes_active_game`）的目标
/// 状态（纯函数，供单测）。`cur` = 双键当前值 `(office, game)`。
///
/// 返回 `Some((office, game))` = 双键目标；`None` = 部分失败/未知模式，系统
/// 真实状态未知，**不动旧值**——沿用轮次32 `modes_active_after_run` 的「失败
/// 不动」语义（更早的旧实现失败写空串：既抹掉仍在生效的另一模式标记，也在
/// 退出失败时谎报已还原）。
///
/// 互斥在**写入侧**强制：进入任一模式成功即清另一族（同刻只应一个场景模式
/// 生效；旧单键天然如此，双键拆族后由这里保持，显示侧只读）。退出只动自己
/// 那族——另一族标记是否真实生效未知，不能替它「顺手清」（旧前端实现任意
/// 退出清双标记，会丢仍在生效那族的「进行中」）。
pub fn modes_dual_after_run(
    mode: &str,
    fail_count: usize,
    cur: (bool, bool),
) -> Option<(bool, bool)> {
    match (mode, fail_count) {
        ("office", 0) => Some((true, false)),
        ("game", 0) => Some((false, true)),
        ("office_exit", 0) => Some((false, cur.1)),
        ("game_exit", 0) => Some((cur.0, false)),
        _ => None,
    }
}

/// 进入模式前的**互斥前置还原**判定（纯函数，供单测/变异验证）。
///
/// 标记互斥的另一面是系统态也必须跟着还：进入 office 成功会把 game 标记清成
/// "未激活"，但 `run_office_mode` 本身**不还原**游戏族的改动（NoAutoUpdate
/// 策略、高性能电源）——不补前置还原，标记说"游戏已退出"而系统仍带着游戏
/// 模式的持久策略（标记与真实系统态分叉，且 UI 的"进行中"提示从此消失，
/// 用户失去了"需要退出游戏模式来恢复更新"的线索）。
///
/// 返回 Some(还原族) = 进入前先真实执行该族的 restore；None = 无需。
pub fn mutual_entry_restore(mode: &str, cur: (bool, bool)) -> Option<&'static str> {
    match (mode, cur) {
        ("office", (_, true)) => Some("game_exit"),
        ("game", (true, _)) => Some("office_exit"),
        _ => None,
    }
}

/// 一次 run 的**执行计划**（纯函数，供单测/变异验证）：按真实执行顺序返回
/// 模式步 id 列表。另一族标记在生效时，前置还原步必须排在进入步**之前**——
/// 两族第 1 步都是「通知/勿扰」且写同一个注册表值：先进入再还原，restore 的
/// 「开启通知」会把刚进入模式的勿扰覆盖掉（轮次37 互审实锤：勿扰静默失效，
/// 结果清单的步序也和执行序相反）。未知模式返回空表（调用方报错，绝不执行
/// 任何系统动作）。
pub fn modes_run_plan(mode: &str, cur: (bool, bool)) -> Vec<&'static str> {
    if !matches!(mode, "office" | "office_exit" | "game" | "game_exit") {
        return Vec::new();
    }
    match mutual_entry_restore(mode, cur) {
        // 前置还原步在前，进入步在后（步序 = 执行序 = 清单序）
        Some(restore_id) => vec![restore_id, if mode == "office" { "office" } else { "game" }],
        None => vec![match mode {
            "office" => "office",
            "office_exit" => "office_exit",
            "game" => "game",
            _ => "game_exit",
        }],
    }
}

/// 进入/退出场景模式（每步结果一并返回，单步失败不中断）。执行顺序由
/// `modes_run_plan` 给出：另一族标记在生效时**先真实退出该族再进入**——
/// 两族第 1 步都是「通知/勿扰」且写同一个注册表值，先进入再还原会让
/// restore 的「开启通知」覆盖刚进入模式的勿扰（轮次37 互审实锤）。restore
/// 步如实并入结果清单（步序=执行序）；前置还原失败 → fail_count>0 → 双键
/// 不动旧值，与「失败不写标记」的既有语义衔接。
#[tauri::command]
pub async fn sk_modes_run(
    mode: String,
    open_apps: Option<bool>,
    clear_bg: Option<bool>,
) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        migrate_modes_active_keys();
        let cur = (
            modes_flag_on("modes_active_office"),
            modes_flag_on("modes_active_game"),
        );
        let plan = modes_run_plan(&mode, cur);
        if plan.is_empty() {
            return Err(format!("未知模式: {mode}"));
        }
        let mut mode_name = String::new();
        let mut steps = Vec::new();
        for id in plan {
            let out = match id {
                "office" => crate::modes::run_office_mode(open_apps.unwrap_or(true)),
                "office_exit" => crate::modes::restore_office_defaults(),
                "game" => crate::modes::run_game_mode(clear_bg.unwrap_or(true)),
                "game_exit" => crate::modes::restore_game_defaults(),
                other => return Err(format!("未知还原族: {other}")),
            };
            if id == mode.as_str() {
                mode_name = out.mode.clone();
            } else {
                log::info!(
                    "互斥前置还原：进入 {} 前先执行 {}（另一族标记在生效）",
                    mode,
                    id
                );
            }
            steps.extend(out.steps);
        }
        let outcome = crate::modes::ModeOutcome::new(&mode_name, steps);
        let summary = outcome.summary();
        // 当前模式持久化：跨会话可见（settings 双键）。只在**结局确定**时
        // 写：进入成功→置自己族并清另一族（互斥在写入侧强制），退出成功→
        // 清自己族（另一族保持）；部分失败时系统真实状态未知，**不动旧值**
        // （前端 applyModeRun 同语义：失败保持原标记）。旧单键 `modes_active`
        // 只能存一个模式名：office 生效中进 game（哪怕部分失败没写成）也会在
        // 下一次成功写入时丢掉 office 标记，且旧前端任意退出清双标记——双键
        // 后两族各自存退，丢态序列不再存在。
        if let Some((office, game)) = modes_dual_after_run(
            &mode,
            outcome.fail_count,
            (
                modes_flag_on("modes_active_office"),
                modes_flag_on("modes_active_game"),
            ),
        ) {
            let mut patch = serde_json::Map::new();
            patch.insert(
                "modes_active_office".to_string(),
                serde_json::Value::String(modes_flag_str(office).to_string()),
            );
            patch.insert(
                "modes_active_game".to_string(),
                serde_json::Value::String(modes_flag_str(game).to_string()),
            );
            if let Err(e) = crate::settings::save(&patch) {
                log::warn!("模式状态持久化失败（不影响本次执行）: {e}");
            }
        }
        Ok(serde_json::json!({
            "outcome": outcome,
            "summary": summary,
        }))
    })
    .await
    .map_err(|e| format!("模式线程失败: {e}"))?
}

/// 列出可清理的后台候选（白名单内正在运行的）。
#[tauri::command]
pub async fn sk_modes_bg_candidates() -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let list: Vec<serde_json::Value> = crate::modes::background_candidates()
            .into_iter()
            .map(|(pid, name)| serde_json::json!({ "pid": pid, "name": name }))
            .collect();
        Ok(serde_json::json!({ "rows": list, "count": list.len() }))
    })
    .await
    .map_err(|e| format!("扫描线程失败: {e}"))?
}

// ---------------- M5 系统助手：电池 ----------------

/// 实时电源状态（`GetSystemPowerStatus`，微秒级，可频繁调用）。
#[tauri::command]
pub fn sk_power_status() -> serde_json::Value {
    let p = crate::battery::read_power();
    serde_json::json!({
        "ac_online": p.ac_online,
        "has_battery": p.has_battery,
        "percent": p.percent,
        "charging": p.charging,
        "seconds_left": p.seconds_left,
        "remaining_human": crate::battery::human_remaining(p.seconds_left),
        "summary": crate::battery::power_summary(&p),
    })
}

/// 电池健康度（WMI，**最坏 30 秒**）——只由用户点刷新触发，不做轮询。
#[tauri::command]
pub async fn sk_battery_health() -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let h = crate::battery::query_health();
        Ok(serde_json::json!({
            "design_mwh": h.design_mwh,
            "full_mwh": h.full_mwh,
            "health_percent": h.health_percent,
            "token": crate::battery::health_token(h.health_percent),
            "available": h.is_available(),
            "note": crate::battery::health_note(&h),
        }))
    })
    .await
    .map_err(|e| format!("查询线程失败: {e}"))?
}

// ---------------- M5 系统助手：磁盘空间 / 垃圾清理 ----------------

/// 目录空间分析：递归扫描目录树（大小/文件数/类型分布/大文件 Top200）。
/// 路径校验与 sk_disk_scan_children 完全同源（diskusage::validate_scan_dir，
/// 轮次114 补齐：绝对路径/存在且为目录/拒绝设备命名空间与 DOS 保留设备名）。
/// 取消走 sk_disk_scan_cancel（同一时刻只跑一个扫描）。
#[tauri::command]
pub async fn sk_disk_scan_tree(root: String) -> Result<serde_json::Value, String> {
    let root = crate::diskusage::validate_scan_dir(&root)?;
    crate::diskusage::SCAN_STOP.reset();
    tauri::async_runtime::spawn_blocking(move || {
        let r = crate::diskusage::scan_tree(&root, &crate::diskusage::SCAN_STOP);
        serde_json::to_value(r).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("扫描线程失败: {e}"))?
}

/// 取消进行中的目录扫描（已入回收站的删除不受影响）。
#[tauri::command]
pub fn sk_disk_scan_cancel() {
    crate::diskusage::SCAN_STOP.cancel();
}

/// 目录逐级展开：扫描一个目录的直接子项（递归大小/类型分布/Top200 摘要，
/// 只返回一层——孙辈由前端再次调用按需下钻）。路径校验见
/// diskusage::validate_scan_dir（绝对路径/存在且为目录/拒绝设备命名空间与
/// DOS 保留设备名）。取消走 sk_disk_scan_cancel（与 sk_disk_scan_tree
/// 同一枚 SCAN_STOP，同一时刻只跑一个扫描）。
#[tauri::command]
pub async fn sk_disk_scan_children(dir: String) -> Result<serde_json::Value, String> {
    let dir = crate::diskusage::validate_scan_dir(&dir)?;
    crate::diskusage::SCAN_STOP.reset();
    tauri::async_runtime::spawn_blocking(move || {
        let r = crate::diskusage::scan_children(&dir, &crate::diskusage::SCAN_STOP);
        serde_json::to_value(r).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| format!("扫描线程失败: {e}"))?
}

/// 批量移入回收站（可还原），返回 (成功数, 失败路径列表)。
#[tauri::command]
pub async fn sk_disk_recycle_delete(paths: Vec<String>) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (ok, errors) = crate::diskusage::delete_to_recycle_bin(&paths);
        serde_json::json!({ "ok_count": ok, "errors": errors })
    })
    .await
    .map_err(|e| format!("删除线程失败: {e}"))
}

/// 列出本地卷的容量与使用率。
#[tauri::command]
pub async fn sk_disk_list() -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let drives = crate::disk::sort_by_pressure(crate::disk::list_drives());
        let rows: Vec<serde_json::Value> = drives
            .iter()
            .map(|d| {
                serde_json::json!({
                    "letter": d.letter,
                    "label": d.label,
                    "fs": d.fs,
                    "total": d.total,
                    "used": d.used(),
                    "free_total": d.free_total,
                    "free_to_caller": d.free_to_caller,
                    "used_percent": d.used_percent(),
                    "removable": d.removable,
                    "token": crate::disk::usage_token(d.used_percent()),
                })
            })
            .collect();
        Ok(serde_json::json!({ "rows": rows, "count": rows.len() }))
    })
    .await
    .map_err(|e| format!("枚举线程失败: {e}"))?
}

/// 清理目标清单（供前端渲染勾选项）。
#[tauri::command]
pub fn sk_clean_targets() -> serde_json::Value {
    serde_json::json!({
        "targets": crate::cleanup::CLEAN_TARGETS
            .iter()
            .map(|(t, title)| serde_json::json!({
                "key": t.key(),
                "title": title,
                "root": t.root().map(|p| p.to_string_lossy().to_string()),
            }))
            .collect::<Vec<_>>()
    })
}

/// 扫描各清理目标的大小（**只读，不删任何东西**）。
///
/// 真机 TEMP 目录可能有很多文件，这一步会慢（秒级），所以走阻塞线程池。
#[tauri::command]
pub async fn sk_clean_scan() -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let mut total = 0u64;
        let rows: Vec<serde_json::Value> = crate::cleanup::CLEAN_TARGETS
            .iter()
            .map(|(t, title)| {
                let bytes = crate::cleanup::scan_target(*t);
                total += bytes;
                serde_json::json!({
                    "key": t.key(),
                    "title": title,
                    "bytes": bytes,
                    "human": crate::disk::human_size(bytes),
                    "root": t.root().map(|p| p.to_string_lossy().to_string()),
                })
            })
            .collect();
        Ok(serde_json::json!({
            "rows": rows,
            "total_bytes": total,
            "total_human": crate::disk::human_size(total),
        }))
    })
    .await
    .map_err(|e| format!("扫描线程失败: {e}"))?
}

/// 清理选中的目标（只删文件，跳过正在使用的）。
#[tauri::command]
pub async fn sk_clean_run(keys: Vec<String>) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if keys.is_empty() {
            return Err("请先选择要清理的项目".to_string());
        }
        let mut total = crate::cleanup::CleanOutcome::empty();
        let mut per_target = Vec::new();
        for key in &keys {
            let Some(t) = crate::cleanup::CleanTarget::from_key(key) else {
                continue; // 未知 key 直接忽略，不做任何猜测
            };
            match t {
                crate::cleanup::CleanTarget::Recycle => {
                    // 回收站走 Shell COM（没有可遍历的目录路径）
                    let out = crate::w32::run_capture(
                        &[
                            "powershell".into(),
                            "-NoProfile".into(),
                            "-NonInteractive".into(),
                            "-Command".into(),
                            "Clear-RecycleBin -Force -ErrorAction SilentlyContinue".into(),
                        ],
                        60,
                    );
                    let ok = out.as_ref().map(|o| o.ok).unwrap_or(false);
                    per_target.push(serde_json::json!({
                        "key": t.key(),
                        "title": t.title(),
                        "ok": ok,
                        "message": if ok {
                            "回收站已清空（无法统计释放大小）"
                        } else {
                            "清空回收站失败或本来就是空的"
                        },
                    }));
                }
                _ => {
                    let Some(root) = t.root() else { continue };
                    let out = crate::cleanup::clean_dir(t, &root);
                    per_target.push(serde_json::json!({
                        "key": t.key(),
                        "title": t.title(),
                        "ok": true,
                        "deleted": out.deleted,
                        "freed": out.freed,
                        "failed": out.failed,
                        "message": out.summary(),
                    }));
                    total = total.merge(out);
                }
            }
        }
        Ok(serde_json::json!({
            "outcome": total,
            "summary": total.summary(),
            "per_target": per_target,
        }))
    })
    .await
    .map_err(|e| format!("清理线程失败: {e}"))?
}

// ---------------- M5 系统助手：启动项管理 ----------------

/// 枚举启动项（HKCU/HKLM 的 Run 键 + 启动文件夹 + SysKit 的禁用记录）。
#[tauri::command]
pub async fn sk_startup_list() -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let items = crate::startup::enumerate_real();
        let (on, off) = crate::startup::startup_stats(&items);
        Ok(serde_json::json!({
            "rows": items,
            "count": items.len(),
            "enabled": on,
            "disabled": off,
            "is_admin": crate::w32::is_elevated(),
        }))
    })
    .await
    .map_err(|e| format!("枚举线程失败: {e}"))?
}

/// 禁用/启用一个启动项。
///
/// 注册表项是**搬**到 `HKCU\Software\NetTool\DisabledRun`（沿用 Python 版的键名），
/// 启动文件夹项是改名加 `.disabled`。两版共享同一套标记，迁移期互不打架。
#[tauri::command]
pub async fn sk_startup_set(
    location: String,
    name: String,
    enable: bool,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::startup::set_enabled_real(&location, &name, enable)?;
        Ok(if enable {
            format!("已启用 {name}")
        } else {
            format!("已禁用 {name}（可从本页重新启用）")
        })
    })
    .await
    .map_err(|e| format!("执行线程失败: {e}"))?
}

// ---------------- M5 系统助手：Top 进程 ----------------

/// Top 进程缓存（tasklist 一次约 0.5 秒，2 秒窗口内复用同一份）。
static TOP_PROCS_CACHE: std::sync::Mutex<Option<(std::time::Instant, serde_json::Value)>> =
    std::sync::Mutex::new(None);

/// 内存占用最高的 N 个进程。
///
/// 数据源用 `tasklist /fo csv`（约 0.5 秒）而不是 `Get-CimInstance
/// Win32_PerfFormattedData_PerfProc_Process`（实测 3.3 秒，且 Name 是
/// `chrome#20` 这种性能计数器实例名而非真实进程名），也不是 `Get-Process`
/// （0.3 秒但要多一个解析分支）。选 tasklist 的决定性理由：**端口页已经在解析
/// 同一份 CSV**，复用同一份规则就没有第二处解析可能出错。
#[tauri::command]
pub async fn sk_top_procs(limit: Option<usize>) -> Result<serde_json::Value, String> {
    let n = limit.unwrap_or(15).clamp(1, 100);
    tauri::async_runtime::spawn_blocking(move || {
        const TTL_MS: u128 = 2000;
        {
            let g = TOP_PROCS_CACHE.lock().expect("进程缓存锁中毒");
            if let Some((at, v)) = g.as_ref() {
                if at.elapsed().as_millis() < TTL_MS {
                    return Ok(v.clone());
                }
            }
        }
        let out = crate::w32::run_capture(&args(&["tasklist", "/fo", "csv", "/nh"]), 30)
            .map_err(|e| format!("tasklist 无法执行: {e}"))?;
        let procs = crate::portscan::parse_tasklist_mem(&out.stdout);
        if procs.is_empty() {
            return Err("未能解析出任何进程（tasklist 输出格式可能有变）".to_string());
        }
        let (count, total) = crate::portscan::procs_summary(&procs);
        let top: Vec<serde_json::Value> = crate::portscan::top_by_mem(procs, n)
            .into_iter()
            .map(|p| {
                serde_json::json!({
                    "pid": p.pid,
                    "name": p.name,
                    "bytes": p.bytes,
                })
            })
            .collect();
        let value = serde_json::json!({
            "rows": top,
            "count": count,
            "total_bytes": total,
            "limit": n,
        });
        *TOP_PROCS_CACHE.lock().expect("进程缓存锁中毒") =
            Some((std::time::Instant::now(), value.clone()));
        Ok(value)
    })
    .await
    .map_err(|e| format!("采样线程失败: {e}"))?
}

// ---------------- M5 系统助手：WiFi 状态 ----------------

/// 读取 WiFi 状态（`wlanapi.dll`，微秒级）。
#[tauri::command]
pub fn sk_wifi_info() -> serde_json::Value {
    let w = crate::wifi::read();
    serde_json::json!({
        "connected": w.connected,
        "ssid": w.ssid,
        "signal": w.signal,
        "dbm": w.dbm,
        "bssid": w.bssid,
        "rx_kbps": w.rx_kbps,
        "tx_kbps": w.tx_kbps,
        "label": w.signal.map(crate::wifi::signal_label).unwrap_or("未连接"),
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_dual_keys_persist_only_on_decisive_outcomes() {
        // 进入成功 → 置自己族 + 清另一族（互斥在写入侧强制：同刻只应一个场景
        // 模式生效；旧单键天然如此，双键拆族后由纯函数保持）
        assert_eq!(
            modes_dual_after_run("office", 0, (false, true)),
            Some((true, false)),
            "进办公清游戏标记"
        );
        assert_eq!(
            modes_dual_after_run("game", 0, (true, false)),
            Some((false, true)),
            "进游戏清办公标记"
        );
        assert_eq!(
            modes_dual_after_run("office", 0, (false, false)),
            Some((true, false))
        );
        assert_eq!(
            modes_dual_after_run("game", 0, (false, false)),
            Some((false, true))
        );
        // 退出成功 → 只清自己那族，另一族保持（不能替它「顺手清」）
        assert_eq!(
            modes_dual_after_run("office_exit", 0, (true, false)),
            Some((false, false))
        );
        assert_eq!(
            modes_dual_after_run("game_exit", 0, (false, true)),
            Some((false, false))
        );
        assert_eq!(
            modes_dual_after_run("office_exit", 0, (true, true)),
            Some((false, true)),
            "退出办公不清游戏标记"
        );
        assert_eq!(
            modes_dual_after_run("game_exit", 0, (true, true)),
            Some((true, false)),
            "退出游戏不清办公标记"
        );
        // 部分失败：真实状态未知 → None（不动旧值）。回归锁：旧实现写空串，
        // 会抹掉仍在生效的另一模式标记（office→game 失败分叉），且退出失败
        // 时 NoAutoUpdate=1 仍生效却被清，重启后「进行中」与真实状态相反。
        assert_eq!(modes_dual_after_run("office", 1, (true, true)), None);
        assert_eq!(modes_dual_after_run("game", 3, (true, true)), None);
        assert_eq!(modes_dual_after_run("office_exit", 1, (true, false)), None);
        assert_eq!(modes_dual_after_run("game_exit", 1, (true, false)), None);
        // 未知模式一律 None（settings 不得被写进垃圾值）
        assert_eq!(modes_dual_after_run("bogus", 0, (false, false)), None);
    }

    #[test]
    fn vault_autolock_due_boundary_matches_setting() {
        // 边界含等号（与 monitor 冷却同风格）：恰满窗口即锁
        assert!(vault_autolock_due(30.0 * 60.0, 30));
        assert!(!vault_autolock_due(30.0 * 60.0 - 0.5, 30));
        // 0 与负值 = 用户显式关闭自动锁，闲置再久也不锁
        assert!(!vault_autolock_due(1e9, 0));
        assert!(!vault_autolock_due(1e9, -5));
    }

    #[test]
    fn vault_session_autolock_is_idle_based_not_since_unlock() {
        // 回归锁（无人值守轮次36）：自动重锁必须是**闲置**超时（对齐 Python
        // _ensure_unlocked → _restart_autolock）。按"解锁那一刻"起算的旧语义下，
        // 连续操作的用户也会被中途踢出，TOTP 轮询显示反复被「密码库未解锁」打断
        // ——正在进行的批量取密/复制与锁定定时交错（跨模块审计场景6）。
        let _g = crate::test_support::serial();
        let _dir = crate::test_support::isolate();
        crate::settings::set_one("vault_autolock_min", serde_json::json!(30)).unwrap();
        let now = || {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0)
        };
        let unlocked_session = |tag: &str, idle_secs: f64| {
            let mut c = crate::vault_crypto::VaultCrypto::with_path(
                crate::paths::appdata_dir().join(format!("{tag}.key")),
            );
            c.setup(Some("pw")).unwrap();
            assert!(c.unlock(Some("pw")).unwrap());
            c.set_unlocked_at_for_test(now() - idle_secs);
            VaultSession(std::sync::Mutex::new(Some(c)))
        };

        // 闲置超窗（31 分钟 > 30 分钟）→ 下一次密码库操作前就地收锁
        let session = unlocked_session("autolock-over", 31.0 * 60.0);
        let err = match session_lock(&session) {
            Err(e) => e,
            Ok(_) => panic!("闲置超窗的会话必须被拒绝"),
        };
        assert!(err.contains("未解锁"), "{err}");
        assert!(
            session.0.lock().unwrap().is_none(),
            "超窗会话必须已收锁（密钥不得驻留内存）"
        );

        // 窗口内（29 分钟）→ 放行，且**放行即续期**：闲置计时归到本次操作
        let session = unlocked_session("autolock-in", 29.0 * 60.0);
        let _guard = session_lock(&session).unwrap();
        drop(_guard);
        let since = session
            .0
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|c| c.unlocked_since_secs())
            .expect("放行后会话必须仍在");
        assert!(
            now() - since < 60.0,
            "放行后闲置计时必须已归零（旧实现保持解锁时刻，续期无从谈起）：偏移 {}s",
            now() - since
        );

        // 0 = 不自动锁：闲置数小时也不得收锁
        crate::settings::set_one("vault_autolock_min", serde_json::json!(0)).unwrap();
        let session = unlocked_session("autolock-off", 3600.0 * 5.0);
        let _guard = session_lock(&session).expect("关闭自动锁后不得因闲置收锁");
        drop(_guard);
        assert!(session.0.lock().unwrap().is_some());
    }

    #[test]
    fn modes_legacy_single_key_maps_to_dual_initials() {
        assert_eq!(modes_dual_from_legacy("office"), (true, false));
        assert_eq!(modes_dual_from_legacy("game"), (false, true));
        // 旧版退出写过空串：迁移为双关，不得臆造激活态
        assert_eq!(modes_dual_from_legacy(""), (false, false));
        // 脏值（手改 settings.json）不进双键
        assert_eq!(modes_dual_from_legacy("garbage"), (false, false));
    }

    #[test]
    fn mutual_entry_restore_requires_real_other_family_exit() {
        // 回归锁（无人值守轮次36）：标记互斥的另一面是系统态。旧实现进入
        // office 直接清掉 game 标记，但 run_office_mode 不还原游戏族的
        // NoAutoUpdate/高性能电源——标记说"游戏已退出"而持久策略仍在系统里
        // 生效（标记与真实系统态分叉）。修复：进入前先真实还原另一族。
        // 游戏标记在生效 → 进 office 前必须先退出游戏族
        assert_eq!(
            mutual_entry_restore("office", (false, true)),
            Some("game_exit")
        );
        // 办公标记在生效 → 进 game 前必须先退出办公族（静音同样不会自愈）
        assert_eq!(
            mutual_entry_restore("game", (true, false)),
            Some("office_exit")
        );
        // 另一族未激活：直接进，不画蛇添足
        assert_eq!(mutual_entry_restore("office", (false, false)), None);
        assert_eq!(mutual_entry_restore("game", (false, false)), None);
        // 退出路径永不触发前置还原（退出只动自己族，既有语义）
        assert_eq!(mutual_entry_restore("office_exit", (false, true)), None);
        assert_eq!(mutual_entry_restore("game_exit", (true, false)), None);
        // 未知模式不触发
        assert_eq!(mutual_entry_restore("bogus", (true, true)), None);
        // 与 modes_dual_after_run 的写入语义闭环：进入成功 → 自己族置位、
        // 另一族清零；前置还原已在进入动作里真实发生过，标记才配清
        let cur = (false, true);
        if mutual_entry_restore("office", cur).is_some() {
            assert_eq!(
                modes_dual_after_run("office", 0, cur),
                Some((true, false)),
                "前置还原成功后进 office：game 标记清零才与系统态一致"
            );
        }
    }

    #[test]
    fn modes_run_plan_restores_other_family_before_entry() {
        // 回归锁（无人值守轮次37）：前置还原必须排在进入**之前**。两族第 1 步
        // 都是「通知/勿扰」且写同一注册表值（NOC_GLOBAL_SETTING_TOASTS_ENABLED）
        // ——先进入再还原，restore 的「开启通知」会覆盖刚进入模式的勿扰，
        // 且结果清单步序与执行序相反（轮次37 互审实锤的旧 r36 实现行为）。
        assert_eq!(
            modes_run_plan("office", (false, true)),
            vec!["game_exit", "office"]
        );
        assert_eq!(
            modes_run_plan("game", (true, false)),
            vec!["office_exit", "game"]
        );
        // 另一族未激活：只进不还
        assert_eq!(modes_run_plan("office", (false, false)), vec!["office"]);
        assert_eq!(modes_run_plan("game", (false, false)), vec!["game"]);
        // 退出路径永不触发前置还原（既有语义）
        assert_eq!(
            modes_run_plan("office_exit", (false, true)),
            vec!["office_exit"]
        );
        assert_eq!(
            modes_run_plan("game_exit", (true, false)),
            vec!["game_exit"]
        );
        // 未知模式：空计划（调用方报错，绝不执行任何系统动作）
        assert!(modes_run_plan("bogus", (true, true)).is_empty());
        assert!(modes_run_plan("", (false, false)).is_empty());
    }

    #[test]
    fn settings_reset_captures_hotkey_fallbacks_before_defaults() {
        // 回归锁（无人值守轮次37）：reset 的热键回退目标必须在落默认**前**快照
        // ——落了默认再读就只剩默认值，被占回退会因「回退目标==请求值」失去
        // 回退步，用户旧组合从此找不回。走真实公共体（rehook 用测试替身），
        // 快照/落默认的顺序变动必须在此变红。
        let _g = crate::test_support::serial();
        let _dir = crate::test_support::isolate();
        crate::settings::set_one("palette_hotkey", serde_json::json!("Ctrl+Alt+Z")).unwrap();
        let mut given_to_rehook: Option<String> = None;
        let summary = reset_settings_with_rehook(|prev| {
            given_to_rehook = Some(prev("palette_hotkey"));
        })
        .unwrap();
        assert_eq!(
            given_to_rehook.as_deref(),
            Some("Ctrl+Alt+Z"),
            "重挂钩拿到的回退目标必须是用户旧值（快照在落默认之前）"
        );
        assert!(summary.contains("已恢复"), "{summary}");
        // 落默认确实发生了：设置里已是默认组合
        assert_ne!(
            crate::settings::get_str("palette_hotkey"),
            "Ctrl+Alt+Z",
            "reset 必须把设置落回默认，重挂钩的旧值只作回退目标"
        );
    }

    #[test]
    fn modes_legacy_key_migrates_to_dual_and_is_cleared() {
        let _g = crate::test_support::serial();
        crate::test_support::isolate();
        // 预置旧单键（旧版存过 "game"）
        let mut legacy = serde_json::Map::new();
        legacy.insert(
            "modes_active".to_string(),
            serde_json::Value::String("game".to_string()),
        );
        crate::settings::save(&legacy).unwrap();

        migrate_modes_active_keys();

        assert_eq!(crate::settings::get_str("modes_active_office"), "0");
        assert_eq!(crate::settings::get_str("modes_active_game"), "1");
        assert_eq!(
            crate::settings::load().get("modes_active"),
            None,
            "旧单键必须被删除（否则每次读都重复迁移，且旧值可能被误读）"
        );

        // 幂等：旧键已无时迁移为无操作，不炸也不覆写既有双键
        let mut on = serde_json::Map::new();
        on.insert(
            "modes_active_office".to_string(),
            serde_json::Value::String("1".to_string()),
        );
        crate::settings::save(&on).unwrap();
        migrate_modes_active_keys();
        assert_eq!(crate::settings::get_str("modes_active_office"), "1");
        assert_eq!(crate::settings::get_str("modes_active_game"), "1");

        // 显式空串（旧版退出写过）也走迁移：双键落 0 + 清旧键
        let mut empty = serde_json::Map::new();
        empty.insert(
            "modes_active".to_string(),
            serde_json::Value::String(String::new()),
        );
        crate::settings::save(&empty).unwrap();
        migrate_modes_active_keys();
        assert_eq!(
            crate::settings::get_str("modes_active_office"),
            "0",
            "旧空串迁移为双关，不得臆造激活态"
        );
        assert_eq!(crate::settings::get_str("modes_active_game"), "0");
        assert_eq!(crate::settings::load().get("modes_active"), None);
    }

    #[test]
    fn source_has_no_raw_nul_bytes() {
        // sk_diag_export 的对话框过滤器曾把 \0 写成**裸 NUL 字节**：运行时
        // 语义不变（lpstrFilter 本就是 NUL 分隔），但源文件被 file/grep/编辑
        // 工具判成二进制——审计搜索静默漏配。同类调用方（图片/文本另存为）
        // 都用 \0 转义，此处锁死源码卫生。
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands.rs");
        let src = std::fs::read(&p).expect("读 commands.rs");
        assert!(
            !src.contains(&0u8),
            "源码不得含裸 NUL 字节（应写成 \0\0 转义）"
        );
    }

    #[test]
    fn every_tauri_command_is_registered_in_lib() {
        // 接线守卫：commands.rs 里声明的每个 #[tauri::command] 都必须出现在
        // lib.rs 的 generate_handler![...] 清单里。漏注册在编译期不报错、
        // 单测全绿，只在运行时 invoke 失败——sk_vault_copy_secret /
        // sk_clip_copy_plain 曾经就这样静默失联（前端按钮全废）。
        let src_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let cmds_src =
            std::fs::read_to_string(src_dir.join("commands.rs")).expect("读 commands.rs");
        let lib_src = std::fs::read_to_string(src_dir.join("lib.rs")).expect("读 lib.rs");

        let declared: Vec<String> = cmds_src
            .split("#[tauri::command]")
            .skip(1)
            .filter_map(|chunk| {
                let idx = chunk.find("fn ")?;
                let rest = &chunk[idx + 3..];
                let name: String = rest
                    .chars()
                    .skip_while(|c| c.is_whitespace())
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if name.is_empty() {
                    None
                } else {
                    Some(name)
                }
            })
            .collect();
        assert!(
            declared.len() >= 100,
            "命令声明解析异常（只解析到 {} 个），守卫自身失效",
            declared.len()
        );

        let handler_zone = lib_src
            .split("generate_handler!")
            .nth(1)
            .expect("lib.rs 应有 generate_handler! 清单");
        let zone = &handler_zone[..handler_zone.find(']').expect("generate_handler 清单未闭合")];
        let registered: std::collections::HashSet<String> = zone
            .split("commands::")
            .skip(1)
            .map(|seg| {
                seg.chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect()
            })
            .collect();

        let missing: Vec<&str> = declared
            .iter()
            .map(String::as_str)
            .filter(|d| !registered.contains(*d))
            .collect();
        assert!(
            missing.is_empty(),
            "以下命令未在 lib.rs generate_handler 注册（运行时 invoke 必失败）: {missing:?}"
        );
    }

    #[test]
    fn clip_row_preview_and_index() {
        let long =
            "a very long clipboard entry that should be truncated in the list view".repeat(2);
        assert!(long.chars().count() > 80, "样本必须长于预览上限");
        let it = ClipItem {
            ts: 1700000000,
            text: long,
            kind: "text".into(),
            pinned: true,
            data: String::new(),
            idx: Some(3),
        };
        let row = ClipRow::from_item(&it);
        assert_eq!(row.idx, 3);
        assert!(row.pinned);
        assert!(row.preview.ends_with('…'), "超长必须截断: {}", row.preview);
        assert_eq!(row.preview.chars().count(), 80);
    }

    #[test]
    fn clip_row_without_idx_defaults_zero() {
        let it = ClipItem {
            ts: 1,
            text: "x".into(),
            kind: "text".into(),
            pinned: false,
            data: String::new(),
            idx: None,
        };
        assert_eq!(ClipRow::from_item(&it).idx, 0);
    }

    #[test]
    fn frontend_error_lands_in_syskit_log() {
        // ErrorBoundary 的上报端：错误必须进文件日志（release 下 devtools 不可见，
        // 文件日志是前端崩溃的唯一可诊断出口）
        let _g = crate::test_support::serial();
        crate::test_support::isolate();
        crate::logging::init();
        sk_frontend_error("boundary-marker-上抛测试".into());
        let dir = crate::paths::log_dir();
        let syskit = std::fs::read_to_string(dir.join("syskit.log")).unwrap_or_default();
        assert!(
            syskit.contains("[frontend] boundary-marker-上抛测试"),
            "前端错误未落 syskit.log"
        );
    }

    #[test]
    fn version_payload_matches_config() {
        let v = sk_version();
        assert_eq!(v["version"], crate::paths::VERSION);
        assert_eq!(v["display_name"], crate::paths::DISPLAY_NAME);
    }

    #[test]
    fn qr_svg_renders_local_and_scalable() {
        // url_qr 动作的渲染端：本地 SVG、内容可被浏览器 <img> 直接消费。
        // 纠错级别 M 与 Python qrcode.make 默认一致——这里锁住"能出图"与
        // "SVG 结构完整"两条底线，具体点阵形状交给 qrcode 库自己保证。
        let svg = sk_clip_qr_svg("https://example.com/rust".into()).expect("常规 URL 必须可渲染");
        assert!(
            svg.starts_with("<?xml") || svg.starts_with("<svg"),
            "必须是 SVG 文本: 首行不对"
        );
        assert!(svg.contains("<svg"), "缺 svg 根元素");
        assert!(
            svg.contains("viewBox"),
            "无 viewBox 则 <img> 无法按比例缩放"
        );
        assert!(svg.contains("</svg>"), "SVG 未闭合");

        // 确定性：同输入两次渲染必须逐字节一致（用户前后两次点同一动作
        // 看到不同的码会立刻失去信任）
        let again = sk_clip_qr_svg("https://example.com/rust".into()).expect("二次渲染失败");
        assert_eq!(svg, again, "同一输入的二维码必须逐字节一致");
    }

    #[test]
    fn qr_svg_rejects_oversized_payload() {
        // 超过 QR 码字节容量（M 级上限约 2331 字节）必须报错，而不是
        // 悄悄降级或截断——截断后的码扫出来是另一段内容，那是静默数据损坏
        let big = "a".repeat(4000);
        assert!(sk_clip_qr_svg(big).is_err(), "超容量输入必须 Err");
    }

    /// 回归锁（无人值守轮次114）：sk_disk_scan_tree 入口必须走
    /// diskusage::validate_scan_dir 同一守卫——六类拒绝（空/非绝对/设备命名
    /// 空间/DOS 保留名/不存在/非目录）逐类命中，且入口错误文案与守卫纯函数
    /// **逐字节相等**（相等断言即单一事实源锁：tree 若复制第二份清单或漏校验，
    /// 这里先红）。拒绝发生在 SCAN_STOP.reset 之前，测试不触碰全局令牌。
    #[test]
    fn tree_entry_guard_rejects_same_six_classes_as_children() {
        // 存在但不是目录（测试二进制本身是真实的绝对文件路径）
        let exe = std::env::current_exe().expect("取测试二进制路径");
        let exe_s = exe.to_string_lossy().into_owned();
        let cases: [(&str, &str); 8] = [
            // 空与纯空白
            ("", "不能为空"),
            ("   ", "不能为空"),
            // 非绝对路径
            ("relative\\dir", "绝对路径"),
            // 设备命名空间
            (r"\\.\PhysicalDrive0", "设备命名空间"),
            // DOS 保留设备名
            (r"C:\Users\NUL\sub", "保留设备名"),
            // 不存在
            (r"C:\__syskit_no_such_dir_r114__", "不存在"),
            // 存在但不是目录
            (exe_s.as_str(), "不是目录"),
            // 带首尾空白的相对路径：入口侧 trim 语义与守卫一致
            ("  ./x  ", "绝对路径"),
        ];
        for (raw, reason) in cases {
            let via_entry = tauri::async_runtime::block_on(sk_disk_scan_tree(raw.to_string()))
                .expect_err("tree 入口必须拒绝可疑路径");
            let via_fn =
                crate::diskusage::validate_scan_dir(raw).expect_err("守卫纯函数必须拒绝同一输入");
            assert!(
                via_entry.contains(reason),
                "输入 {raw:?} 的入口错误必须命中「{reason}」: {via_entry}"
            );
            assert_eq!(
                via_entry, via_fn,
                "tree 入口与 validate_scan_dir 必须是同一事实源（错误文案逐字节相等）"
            );
        }
    }

    /// 回归锁（无人值守轮次114）：合法目录放行——守卫不得误伤 tree 的正常
    /// 使用（用户自输绝对路径只读扫描），入口返回可序列化的扫描结果。
    #[test]
    fn tree_entry_accepts_real_dir_and_scans() {
        let root = std::env::temp_dir().join(format!("syskit_rs_tree_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), b"hello").unwrap();
        // SCAN_STOP 是命令共用的全局令牌：与其它可能扫描的测试串行
        let _g = crate::test_support::serial();
        let v =
            tauri::async_runtime::block_on(sk_disk_scan_tree(root.to_string_lossy().to_string()))
                .expect("真实目录必须放行并完成扫描");
        let v = v.as_object().expect("返回必须是 JSON 对象");
        let name = v["root"]["name"].as_str().expect("root.name 必须是字符串");
        assert_eq!(name, root.file_name().unwrap().to_string_lossy().as_ref());
        let _ = std::fs::remove_dir_all(&root);
    }
}
