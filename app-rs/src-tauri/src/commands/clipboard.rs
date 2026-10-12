//! 剪贴板历史域：读取/管理动作/规则动作/敏感通道/监听状态 + 快捷粘贴面板窗口。

use crate::MutexExt;
use super::ClipRow;
use crate::clipboard_history::{self as hist, ClipItem};
use crate::input_panel::{self, PasteRequest, PasteResult};
use crate::settings;
use crate::tray;
use tauri::Manager;
use tauri::{AppHandle, Runtime};

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

/// 构造命令参数表（本地小工具，避免每处写 `.to_string()`）。
/// 诊断节与 toolbox 域共用（toolbox 通过 `use super::args` 引用）。
pub(crate) fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

/// 复制 + 三重敏感防护（copy 命令与密码库复制共用，M6）。
pub(crate) fn copy_sensitive_text(text: &str) -> Result<serde_json::Value, String> {
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
            "skipped_sensitive": crate::clipboard_watch::state().skipped_sensitive,
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
        dhash: None,
        ts: 0,
        text,
        kind,
        pinned: false,
        data,
        idx: None,
    };
    crate::clipboard_write::write_back(&item)
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

/// 图片条目缩略图（base64 PNG data URL）：列表内嵌显示用。
///
/// 用 image crate 缩到最长边 96px 再编码——历史里可能几十张截图，
/// 原图直出会把列表内存/传输量顶到几十 MB。点击放大由前端 AntD Image 预览承担。
///
/// **LRU 缓存**（v1.3.1）：PNG 文件名是 md5（内容寻址，不随时间变），
/// 命中即免掉整图解码+缩放+编码（几十 MB 图 ~几十 ms → 微秒级）。容量
/// 256 条 × 单条 ~几十 KB ≈ 最多几 MB 内存；超出按最近最少使用淘汰。
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

        // LRU 命中：免解码免缩放
        if let Some(cached) = thumb_cache_get(&name) {
            return Ok(cached);
        }

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
        let data_url = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(out.into_inner())
        );
        thumb_cache_put(name, data_url.clone());
        Ok(data_url)
    })
    .await
    .map_err(|e| format!("缩略图线程失败: {e}"))?
}

// ---------------- 缩略图 LRU（key = PNG 文件名，value = data URL） ----------------

const THUMB_CACHE_CAP: usize = 256;

/// 缩略图缓存：访问顺序队列 + key→dataURL 表。
type ThumbCache = (
    std::collections::VecDeque<String>,
    std::collections::HashMap<String, String>,
);

fn thumb_cache_state() -> &'static std::sync::Mutex<ThumbCache> {
    use std::sync::{Mutex, OnceLock};
    static STATE: OnceLock<Mutex<ThumbCache>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new((
            std::collections::VecDeque::new(),
            std::collections::HashMap::new(),
        ))
    })
}

fn thumb_cache_get(key: &str) -> Option<String> {
    let st = &mut *thumb_cache_state().lock_ok();
    if let Some(v) = st.1.get(key).cloned() {
        // 刷新访问顺序
        if let Some(pos) = st.0.iter().position(|k| k == key) {
            st.0.remove(pos);
        }
        st.0.push_back(key.to_string());
        Some(v)
    } else {
        None
    }
}

fn thumb_cache_put(key: String, value: String) {
    let st = &mut *thumb_cache_state().lock_ok();
    if st.1.contains_key(&key) {
        if let Some(pos) = st.0.iter().position(|k| k == &key) {
            st.0.remove(pos);
        }
    }
    st.0.push_back(key.clone());
    st.1.insert(key, value);
    while st.1.len() > THUMB_CACHE_CAP {
        if let Some(oldest) = st.0.pop_front() {
            st.1.remove(&oldest);
        } else {
            break;
        }
    }
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
