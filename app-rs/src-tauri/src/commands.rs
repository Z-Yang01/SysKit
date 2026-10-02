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
        serde_json::json!({
            "total": items.len(),
            "pinned": pinned,
            "images": images,
            "max_items": h.max_items,
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

/// 下载测速的**未实现说明**（前端据此显示明确提示，而不是给假结果）。
#[tauri::command]
pub fn sk_speedtest_status() -> serde_json::Value {
    serde_json::json!({
        "available": false,
        "reason": "Rust 版尚无 HTTPS 下载能力（标准库不提供 TLS，需引入 rustls/native-tls 并做依赖决策）；Python 版的测速源全是 https。为避免给出假结果，下载测速暂未接入。",
        "python_urls": [
            "https://dldir1.qq.com/weixin/Windows/WeChatSetup.exe",
            "https://dldir1.qq.com/qqfile/qq/QQNT/Windows/QQ_9.9.9_250avis_x64.exe",
            "https://mirrors.aliyun.com/debian-cd/current/amd64/iso-cd/debian-12.9.0-amd64-netinst.iso"
        ],
    })
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

// ---------------- M7 命令面板 provider ----------------

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

/// 算式直算（面板里输入 `1+1` 直接出结果）。
///
/// 返回 `{is_math, ok, display, error}`：`is_math=false` 表示这不像算式，
/// 前端据此忽略而不是弹错误。
#[tauri::command]
pub fn sk_palette_calc(input: String) -> serde_json::Value {
    if !crate::palette::looks_like_math(&input) {
        return serde_json::json!({ "is_math": false, "ok": false, "display": "", "error": "" });
    }
    // safe_calc 有意在**主线程同步**执行（面板求值本来就是同步的）：
    // 规模护栏已保证它不会长时间占用 CPU——见 palette 模块文档的两道护栏
    match crate::palette::safe_calc(&input) {
        Ok(v) => serde_json::json!({
            "is_math": true, "ok": true, "display": v.display(), "error": ""
        }),
        Err(e) => serde_json::json!({
            "is_math": true, "ok": false, "display": "", "error": e
        }),
    }
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
#[tauri::command]
pub fn sk_settings_set(key: String, value: serde_json::Value) -> Result<serde_json::Value, String> {
    let saved = crate::settings::set_one(&key, value)?;
    Ok(serde_json::json!({ "key": key, "value": saved, "message": "已保存" }))
}

/// 恢复默认值（只重置已知键，动态键保留）。
#[tauri::command]
pub fn sk_settings_reset() -> Result<String, String> {
    let n = crate::settings::reset_to_defaults()?;
    Ok(format!(
        "已恢复 {n} 项默认值（动态键如 modes_prev_volume 保留）"
    ))
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

// ---------------- M6 密码库（本阶段：加密层 + 界面骨架） ----------------

/// 密码库状态（含各能力的可用性，供界面如实展示）。
#[tauri::command]
pub fn sk_vault_status() -> serde_json::Value {
    let mut c = crate::vault_crypto::VaultCrypto::new();
    let path = c.path().to_string_lossy().to_string();
    let provisioned = c.is_provisioned();
    let meta = c.read_meta();
    let has_pw = provisioned && meta.mode == "password";
    // 免密模式可以自动解锁（DPAPI 绑定当前用户，无需输入）
    if provisioned && meta.mode == "dpapi" {
        let _ = c.unlock(None);
    }
    serde_json::json!({
        "path": path,
        "provisioned": provisioned,
        "unlocked": c.is_unlocked(),
        "has_master_password": has_pw,
        "mode": meta.mode,
        // 各能力可用性：界面据此决定按钮能不能点、要不要显示说明
        "capabilities": {
            "key_management": true,
            "password_storage": false,
            "password_storage_reason":
                "数据层（vault.db / SQLite）按你的决定暂不实现，因此目前无法存取密码条目。\
                 加密层已完成并通过双向跨语言验证：vault.key 与 Python 版互通。\
                 后续要启用存取功能，需引入 SQLite 依赖（如 rusqlite）。",
        },
    })
}

/// 初始化密码库（首次创建 `vault.key`）。`password` 为空则用 DPAPI 免密模式。
#[tauri::command]
pub async fn sk_vault_setup(password: Option<String>) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut c = crate::vault_crypto::VaultCrypto::new();
        if c.is_provisioned() {
            return Err("密码库已初始化".to_string());
        }
        let pw = password.filter(|p| !p.is_empty());
        // 600k 次 PBKDF2 在主线程会明显卡顿，所以走阻塞线程池
        c.setup(pw.as_deref())?;
        Ok(serde_json::json!({
            "ok": true,
            "mode": if pw.is_some() { "password" } else { "dpapi" },
            "message": if pw.is_some() {
                "已创建密码库（主密码模式）"
            } else {
                "已创建密码库（免密模式：密钥由 Windows DPAPI 绑定当前用户保护）"
            },
        }))
    })
    .await
    .map_err(|e| format!("初始化线程失败: {e}"))?
}

/// 解锁。失败返回可读原因（区分密码错误与文件损坏）。
#[tauri::command]
pub async fn sk_vault_unlock(password: Option<String>) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut c = crate::vault_crypto::VaultCrypto::new();
        match c.unlock(password.as_deref()) {
            Ok(true) => Ok(serde_json::json!({ "ok": true, "message": "已解锁" })),
            Ok(false) => Err("解锁失败".to_string()),
            Err(e) => Err(e.message()),
        }
    })
    .await
    .map_err(|e| format!("解锁线程失败: {e}"))?
}

/// 锁定（清空内存中的密钥）。
#[tauri::command]
pub fn sk_vault_lock() -> serde_json::Value {
    let mut c = crate::vault_crypto::VaultCrypto::new();
    c.lock();
    serde_json::json!({ "ok": true, "message": "已锁定" })
}

/// 设置主密码（免密 → 密码模式；数据密钥不变）。
#[tauri::command]
pub async fn sk_vault_set_password(password: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut c = crate::vault_crypto::VaultCrypto::new();
        // 免密模式下先自动解锁（DPAPI），否则无法重包裹数据密钥
        if !c.is_unlocked() {
            let _ = c.unlock(None);
        }
        c.set_master_password(&password)?;
        Ok("已设置主密码（数据密钥未变，已有密文仍可解开）".to_string())
    })
    .await
    .map_err(|e| format!("设置线程失败: {e}"))?
}

/// 校验主密码（不改变解锁状态）。
#[tauri::command]
pub async fn sk_vault_verify_password(password: String) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let c = crate::vault_crypto::VaultCrypto::new();
        Ok(c.verify_password(&password))
    })
    .await
    .map_err(|e| format!("校验线程失败: {e}"))?
}

/// 移除主密码（回到免密模式）。当前密码错误返回 Ok(false)。
#[tauri::command]
pub async fn sk_vault_remove_password(current: String) -> Result<bool, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut c = crate::vault_crypto::VaultCrypto::new();
        if !c.is_unlocked() {
            let _ = c.unlock(Some(&current));
        }
        c.remove_master_password(&current)
    })
    .await
    .map_err(|e| format!("移除线程失败: {e}"))?
}

/// 明文存取入口在数据层实现前**必须明确失败**，绝不假装成功。
#[tauri::command]
pub fn sk_vault_store_unavailable() -> Result<(), String> {
    Err(crate::vault_crypto::STORAGE_UNAVAILABLE.to_string())
}

// ---------------- M5 系统助手：剪贴板历史（管理动作） ----------------

/// 把一条历史保存为文件（图片 → PNG，其它 → 文本）。
///
/// 文本文件名做**路径分隔符消毒**：条目内容不可信，直接拼进默认文件名里
/// 可能带 `..\` 之类的片段；虽然最终路径由用户在对话框里确认，默认值也不该
/// 指向目录之外。
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
    serde_json::json!({
        "common_apps": crate::modes::COMMON_APPS
            .iter()
            .map(|(n, t)| serde_json::json!({ "name": n, "target": t }))
            .collect::<Vec<_>>(),
        "office_default_apps": crate::modes::OFFICE_DEFAULT_APPS,
        "whitelist": crate::modes::BACKGROUND_CLOSE_WHITELIST,
        "is_admin": crate::w32::is_elevated(),
        "notes": {
            "office": "勿扰（关通知）+ 静音 + 打开常用软件 + 备份当前配置。退出模式会恢复通知并取消静音。",
            "game": "关通知 + 高性能电源 + 暂停 Windows 更新 + 清理白名单后台。退出模式会恢复通知、平衡电源与更新策略。",
            "update_policy": "「暂停更新」通过 NoAutoUpdate 策略实现，是持久生效的（不是暂停 N 天），需退出游戏模式或手动恢复。",
            "admin": "暂停更新与部分电源操作需要管理员权限；失败步骤会如实列出，不影响其它步骤。"
        }
    })
}

/// 进入/退出场景模式（每步结果一并返回，单步失败不中断）。
#[tauri::command]
pub async fn sk_modes_run(
    mode: String,
    open_apps: Option<bool>,
    clear_bg: Option<bool>,
) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let outcome = match mode.as_str() {
            "office" => crate::modes::run_office_mode(open_apps.unwrap_or(true)),
            "office_exit" => crate::modes::restore_office_defaults(),
            "game" => crate::modes::run_game_mode(clear_bg.unwrap_or(true)),
            "game_exit" => crate::modes::restore_game_defaults(),
            other => return Err(format!("未知模式: {other}")),
        };
        let summary = outcome.summary();
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
            Some(p) => p.trim().to_string(),
            None => {
                let up = crate::telemetry::physical_up_adapters();
                let snap = crate::snapshot::take();
                let ip = up
                    .iter()
                    .find_map(|name| {
                        snap.adapters
                            .iter()
                            .find(|a| &a.name == name)
                            .and_then(|a| a.ipv4.first().cloned())
                    })
                    .ok_or_else(|| "未找到可用的本机 IPv4 地址，无法推断网段".to_string())?;
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
    fn version_payload_matches_config() {
        let v = sk_version();
        assert_eq!(v["version"], crate::paths::VERSION);
        assert_eq!(v["display_name"], crate::paths::DISPLAY_NAME);
    }
}
