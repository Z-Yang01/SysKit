//! Tauri 前端可调命令（`invoke` 接口）。所有命令都是薄封装，业务逻辑在各模块里。

use tauri::{AppHandle, Manager, Runtime};

use crate::clipboard_history::{self as hist, ClipItem};
use crate::input_panel::{self, PasteRequest, PasteResult};
use crate::settings;
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
