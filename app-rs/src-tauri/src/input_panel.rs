//! 无边框呼出面板：粘贴面板（label `paste`）与命令面板（label `palette`）。
//!
//! 关键时序（Python 版真机换来的教训，硬约束③）：
//! 1. 热键命中 / 托盘点击时**先记录前台窗口**，再显示面板——面板一显示本进程
//!    就成为前台，之后再取就晚了；
//! 2. 用户确认后，面板先 hide()，Windows 才把焦点还给原窗口，因此注键必须
//!    延后一拍（异步 sleep），绝不阻塞 UI 线程；
//! 3. 连击/重呼出会 bump `seq`，在途的粘贴链发现 token 不匹配即自行作废。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};

use crate::clipboard_history as hist;
use crate::clipboard_write;
use crate::input;
use crate::w32;

pub const PANEL_LABEL: &str = "paste";
pub const PALETTE_LABEL: &str = "palette";

/// 在途粘贴链的序号守卫：新链/重呼出 bump 后旧链各步自行作废。
static PASTE_SEQ: AtomicU64 = AtomicU64::new(0);
/// 本次呼出记录的「面板显示前」的前台窗口句柄。
static TARGET_HWND: OnceLock<Mutex<isize>> = OnceLock::new();

fn target_cell() -> &'static Mutex<isize> {
    TARGET_HWND.get_or_init(|| Mutex::new(0))
}

fn target_hwnd() -> isize {
    *target_cell().lock().expect("目标句柄锁中毒")
}

fn set_target_hwnd(v: isize) {
    *target_cell().lock().expect("目标句柄锁中毒") = v;
}

/// 作废所有在途粘贴链（面板呼出/重呼出时调用）。
pub fn bump_seq() -> u64 {
    PASTE_SEQ.fetch_add(1, Ordering::SeqCst) + 1
}

fn current_seq() -> u64 {
    PASTE_SEQ.load(Ordering::SeqCst)
}

/// 面板窗口就绪（setup 阶段预建，隐藏）。
pub fn ensure_panel<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    if app.get_webview_window(PANEL_LABEL).is_some() {
        return Ok(());
    }
    WebviewWindowBuilder::new(app, PANEL_LABEL, WebviewUrl::App("index.html".into()))
        .title("快捷粘贴")
        .inner_size(560.0, 380.0)
        .decorations(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .shadow(true)
        .visible(false)
        .focused(false)
        .center()
        .build()?;
    Ok(())
}

pub fn ensure_palette<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    if app.get_webview_window(PALETTE_LABEL).is_some() {
        return Ok(());
    }
    WebviewWindowBuilder::new(app, PALETTE_LABEL, WebviewUrl::App("index.html".into()))
        .title("命令面板")
        .inner_size(600.0, 430.0)
        .decorations(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .shadow(true)
        .visible(false)
        .focused(false)
        .center()
        .build()?;
    Ok(())
}

/// 呼出/收起粘贴面板：呼出时先记前台目标，再显示。
pub fn toggle_paste_panel<R: Runtime>(app: &AppHandle<R>) {
    let Some(win) = app.get_webview_window(PANEL_LABEL) else {
        log::warn!("粘贴面板窗口不存在，呼出失败");
        return;
    };
    let visible = win.is_visible().unwrap_or(false);
    if visible {
        let _ = win.hide();
        return;
    }
    // ① 先记目标窗口（此刻前台仍是用户原来的窗口）
    set_target_hwnd(w32::foreground_hwnd_value());
    // ② 作废在途链
    bump_seq();
    let _ = win.show();
    let _ = win.set_focus();
    let _ = app.emit("sk://panel-shown", serde_json::json!({ "panel": "paste" }));
}

pub fn toggle_palette<R: Runtime>(app: &AppHandle<R>) {
    let Some(win) = app.get_webview_window(PALETTE_LABEL) else {
        log::warn!("命令面板窗口不存在，呼出失败");
        return;
    };
    if win.is_visible().unwrap_or(false) {
        let _ = win.hide();
        return;
    }
    let _ = win.show();
    let _ = win.set_focus();
    let _ = app.emit(
        "sk://panel-shown",
        serde_json::json!({ "panel": "palette" }),
    );
}

/// 面板请求隐藏（Esc / 失焦 / 执行后）。
pub fn hide_panel<R: Runtime>(app: &AppHandle<R>, label: &str) {
    if let Some(win) = app.get_webview_window(label) {
        let _ = win.hide();
    }
}

/// 粘贴请求：前端传来条目身份，后端按类型写回剪贴板并执行焦点守卫注键。
#[derive(Debug, Clone, Deserialize)]
pub struct PasteRequest {
    /// 面板呼出快照里的下标（`sk_clip_list` 返回的顺序）。
    pub index: usize,
    /// 前端已知的序号（防止陈旧请求把新链覆盖）。
    pub seq: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PasteResult {
    pub ok: bool,
    pub message: String,
    /// 是否已把内容写入剪贴板（写回失败时为 false）。
    pub written: bool,
}

/// 执行粘贴（异步：延后一拍再注键，绝不阻塞 UI 线程）。
pub async fn perform_paste<R: Runtime>(
    app: AppHandle<R>,
    req: PasteRequest,
    delay_ms: u64,
) -> PasteResult {
    // 写回剪贴板：图片读取失败时按 Python 语义重试一次（AV/索引器短暂锁定文件）
    let item = hist::with(|h| h.by_index(req.index));
    let Some(item) = item else {
        return PasteResult {
            ok: false,
            message: "粘贴失败：条目已不存在（可能被清理或裁剪）".to_string(),
            written: false,
        };
    };

    let mut written = clipboard_write::write_back(&item);
    if !written && item.kind == "image" {
        tokio_sleep(50).await;
        written = clipboard_write::write_back(&item);
    }
    if !written {
        let msg = if item.kind == "image" {
            "粘贴失败：内容不可用（图片文件可能已被清理）".to_string()
        } else {
            "粘贴失败：内容不可用".to_string()
        };
        return PasteResult {
            ok: false,
            message: msg,
            written: false,
        };
    }

    // 面板必须先隐藏，焦点才会还给目标窗口
    hide_panel(&app, PANEL_LABEL);
    tokio_sleep(delay_ms).await;

    let seq_valid = current_seq() == req.seq;
    let outcome = input::guarded_paste(target_hwnd(), &item.kind, seq_valid);
    if !outcome.message.is_empty() {
        let _ = app.emit("sk://paste-result", &outcome);
    }
    PasteResult {
        ok: outcome.ok,
        message: outcome.message,
        written: true,
    }
}

/// 快速直贴最新一条（热键 id=2）：不弹面板，直接写回 + 注键。
pub async fn quick_paste_latest<R: Runtime>(app: AppHandle<R>) -> PasteResult {
    let target = w32::foreground_hwnd_value();
    set_target_hwnd(target);
    let seq = bump_seq();
    let latest = hist::with(|h| h.by_index(0));
    let Some(item) = latest else {
        return PasteResult {
            ok: false,
            message: "剪贴板历史为空".to_string(),
            written: false,
        };
    };
    if !clipboard_write::write_back(&item) {
        return PasteResult {
            ok: false,
            message: "粘贴失败：内容不可用".to_string(),
            written: false,
        };
    }
    tokio_sleep(input::QUICK_DELAY_MS).await;
    let outcome = input::guarded_paste(target, &item.kind, current_seq() == seq);
    if !outcome.message.is_empty() {
        let _ = app.emit("sk://paste-result", &outcome);
    }
    PasteResult {
        ok: outcome.ok,
        message: outcome.message,
        written: true,
    }
}

/// 不引 tokio 直接依赖的异步睡眠（Tauri 自带运行时提供 sleep 语义）。
async fn tokio_sleep(ms: u64) {
    let _ = tauri::async_runtime::spawn_blocking(move || {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    })
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bump_seq_invalidates_in_flight_chains() {
        let a = bump_seq();
        assert!(current_seq() >= a);
        let b = bump_seq();
        assert!(b > a, "每次呼出都必须让旧链作废");
        assert_ne!(a, b);
    }

    #[test]
    fn paste_result_shape() {
        let r = PasteResult {
            ok: true,
            message: "已粘贴到原窗口".into(),
            written: true,
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("\"ok\":true"));
        assert!(s.contains("\"written\":true"));
    }

    #[test]
    fn panel_labels_are_stable() {
        assert_eq!(PANEL_LABEL, "paste");
        assert_eq!(PALETTE_LABEL, "palette");
    }
}
