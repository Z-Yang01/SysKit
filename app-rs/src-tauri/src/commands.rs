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
