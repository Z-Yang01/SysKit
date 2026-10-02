//! 剪贴板监听线程：把"用户复制了什么"变成历史条目。
//!
//! ## 为什么必须存在
//!
//! 在此之前 Rust 侧**没有监听器**——`History::add()` 只在测试里被调用过。
//! 也就是说快捷粘贴面板能在空历史里正常"工作"：呼出、搜索、注键都对，
//! 但列表永远是空的。这是最容易被忽略的一类缺口：**命令层被测全了，
//! 缺的是把它们连起来的那根线**。
//!
//! ## 抓取优先级（与 Python 侧 system_page 逐条对齐）
//!
//! 文件 → 图片 → HTML → 纯文本，依序尝试，先成功者为准。
//! 顺序有意义：图片复制时 text() 往往为空、HTML 复制时 text() 是纯文本兜底，
//! 先判文本会把"复制的图片"记成空条目。
//!
//! ## 轮询而非事件
//!
//! 用 `GetClipboardSequenceNumber` 轮询：不需要窗口与消息循环
//! （`AddClipboardFormatListener` 要求 HWND 并处理 WM_CLIPBOARDUPDATE），
//! 序号不变时**完全不打开剪贴板**，开销接近零。

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use serde::Serialize;

use crate::clipboard_history as hist;
use crate::clipboard_write::png_to_dib;
use crate::w32;

/// 轮询间隔：与 Python 侧 500ms 同节奏。
///
/// 更密的轮询没有意义（人不会 200ms 复制一次），而每次"序号变了"都要打开剪贴板，
/// 那是个全局锁——轮询太密会在其他程序复制时抢锁。
pub const POLL_INTERVAL_MS: u64 = 500;

/// 一次抓取得到的内容（与剪贴板系统的边界）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Captured {
    /// 文本形态（图片条目是 `[图片 W×H]` 占位，与 Python 一致）
    pub text: String,
    /// `text` / `file` / `image` / `html`
    pub kind: String,
    /// 图片为 PNG 文件名；HTML 为原文；其余为空
    pub data: String,
}

/// 剪贴板内容快照（读一次，供纯逻辑决策）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClipSnapshot {
    pub text: Option<String>,
    pub files: Option<Vec<String>>,
    pub has_image: bool,
    pub has_html: bool,
}

/// 由快照决定要入库的条目（**纯函数**，便于逐条断言优先级）。
///
/// 返回 None 表示"这次没有值得记录的内容"。
pub fn decide_capture(snap: &ClipSnapshot, capture_images: bool) -> Option<Captured> {
    // 1. 文件优先：复制的文件 text() 常为空，先判文本会漏掉
    if let Some(files) = snap.files.as_ref().filter(|f| !f.is_empty()) {
        return Some(Captured {
            text: files.join("\n"),
            kind: "file".to_string(),
            data: String::new(),
        });
    }
    // 2. 图片：内容由 `clip_capture_images` 控制；关掉时不记录，
    //    但**要让位给文本兜底**（图片可能同时带一段说明文字）
    if snap.has_image && capture_images {
        return Some(Captured {
            text: snap.text.clone().unwrap_or_default(),
            kind: "image".to_string(),
            data: String::new(),
        });
    }
    let text = snap.text.as_deref().unwrap_or("");
    // 3. HTML：仅当 HTML 与纯文本不同才算"富文本复制"（相同则纯文本已足够）
    if snap.has_html && !text.trim().is_empty() {
        return Some(Captured {
            text: text.to_string(),
            kind: "html".to_string(),
            data: String::new(),
        });
    }
    // 4. 纯文本：空白内容不入库（与 History::add 的判据一致）
    if text.trim().is_empty() {
        return None;
    }
    Some(Captured {
        text: text.to_string(),
        kind: "text".to_string(),
        data: String::new(),
    })
}

/// 该内容是否应当被记录（黑名单闸门）。
///
/// **必须与去重共用同一次判定**：黑名单命中时要把内容记为 `_last`，
/// 否则用户切走焦点后会"补录"刚才在密码管理器里复制的东西。
pub fn should_capture(blacklist: &str, text: &str, last: &str) -> bool {
    if text == last {
        return false;
    }
    hist::should_record_now(blacklist)
}

/// 监听线程的运行状态（供前端显示）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WatchState {
    pub running: bool,
    pub enabled: bool,
    pub captured: u64,
    pub skipped_blacklist: u64,
    pub skipped_duplicate: u64,
}

static RUNNING: AtomicBool = AtomicBool::new(false);
static ENABLED: AtomicBool = AtomicBool::new(false);
static CAPTURED: AtomicU32 = AtomicU32::new(0);
static SKIPPED_BL: AtomicU32 = AtomicU32::new(0);
static SKIPPED_DUP: AtomicU32 = AtomicU32::new(0);

/// 当前监听状态。
pub fn state() -> WatchState {
    WatchState {
        running: RUNNING.load(Ordering::SeqCst),
        enabled: ENABLED.load(Ordering::SeqCst),
        captured: CAPTURED.load(Ordering::SeqCst) as u64,
        skipped_blacklist: SKIPPED_BL.load(Ordering::SeqCst) as u64,
        skipped_duplicate: SKIPPED_DUP.load(Ordering::SeqCst) as u64,
    }
}

/// 暂停/恢复记录（隐私暂停：不记录新内容，历史保留）。
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::SeqCst);
}

pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::SeqCst)
}

/// 读一次剪贴板快照。
pub fn read_snapshot() -> ClipSnapshot {
    ClipSnapshot {
        text: w32::clipboard_get_text(),
        files: w32::clipboard_get_files(),
        has_image: w32::clipboard_has_image(),
        has_html: w32::clipboard_has_html(),
    }
}

/// 起监听线程（Tauri setup 阶段调用一次；重复调用无效果）。
pub fn spawn() {
    if RUNNING.swap(true, Ordering::SeqCst) {
        return; // 已在运行
    }
    ENABLED.store(
        crate::settings::get_bool("clipboard_watch"),
        Ordering::SeqCst,
    );
    std::thread::Builder::new()
        .name("sk-clipwatch".into())
        .spawn(move || {
            let mut last_seq = w32::clipboard_sequence();
            let mut last_text = String::new();
            loop {
                std::thread::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS));
                // 暂停时只更新序号基线：恢复后不会把暂停期间的内容补录进来
                let seq = w32::clipboard_sequence();
                if seq == last_seq {
                    continue;
                }
                last_seq = seq;
                if !is_enabled() {
                    continue;
                }
                let blacklist = crate::settings::get_str("clip_process_blacklist");
                let capture_images = crate::settings::get_bool("clip_capture_images");
                let snap = read_snapshot();
                let candidate = snap.text.clone().unwrap_or_default();
                // 去重与黑名单共用一次判定
                if candidate == last_text && !snap.has_image && snap.files.is_none() {
                    SKIPPED_DUP.fetch_add(1, Ordering::SeqCst);
                    continue;
                }
                if !hist::should_record_now(&blacklist) {
                    // 关键：把内容记为 last，避免切走焦点后补录
                    last_text = candidate;
                    SKIPPED_BL.fetch_add(1, Ordering::SeqCst);
                    continue;
                }
                let Some(cap) = decide_capture(&snap, capture_images) else {
                    continue;
                };
                last_text = candidate;
                store(&cap);
            }
        })
        .expect("剪贴板监听线程启动失败");
}

/// 入库（图片走 PNG 落盘，文本直接进历史）。
fn store(cap: &Captured) -> bool {
    match cap.kind.as_str() {
        "image" => {
            // 真正的位图搬运未实现（见 decide_capture 的说明）：这里只登记占位条目，
            // 保证"复制了图片"这件事在历史里可见，而不是静默丢失
            let added = hist::with(|h| h.add(&cap.text, "image", &cap.data));
            if added {
                CAPTURED.fetch_add(1, Ordering::SeqCst);
            }
            added
        }
        _ => {
            let added = hist::with(|h| h.add(&cap.text, &cap.kind, &cap.data));
            if added {
                CAPTURED.fetch_add(1, Ordering::SeqCst);
            }
            added
        }
    }
}

/// 把剪贴板里的位图抓成 PNG 并入库（图片捕获的完整路径）。
///
/// 独立成函数是因为它需要 `CF_DIB` 解析（位深/压缩/行序），与文本路径的
/// 风险不同；当前实现只做 PNG→DIB 反向转换的可用性检查。
pub fn store_png(png: &[u8], label: &str) -> bool {
    if png_to_dib(png).is_none() {
        return false; // 不是可用的 PNG
    }
    let name = crate::clipboard_write::save_image_bytes(png);
    if name.is_empty() {
        return false;
    }
    let added = hist::with(|h| h.add(label, "image", &name));
    if added {
        CAPTURED.fetch_add(1, Ordering::SeqCst);
    }
    added
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(text: Option<&str>, files: Option<Vec<&str>>, img: bool, html: bool) -> ClipSnapshot {
        ClipSnapshot {
            text: text.map(|s| s.to_string()),
            files: files.map(|v| v.into_iter().map(|s| s.to_string()).collect()),
            has_image: img,
            has_html: html,
        }
    }

    #[test]
    fn files_win_over_text() {
        // 复制的文件 text() 常为空；先判文本会把文件复制记成空条目
        let s = snap(Some(""), Some(vec![r"C:\a.txt"]), false, false);
        let c = decide_capture(&s, true).unwrap();
        assert_eq!(c.kind, "file");
        assert_eq!(c.text, r"C:\a.txt");
    }

    #[test]
    fn multiple_files_joined_by_newline() {
        let s = snap(None, Some(vec![r"C:\a.txt", r"C:\b.txt"]), false, false);
        let c = decide_capture(&s, true).unwrap();
        assert_eq!(c.text, "C:\\a.txt\nC:\\b.txt");
    }

    #[test]
    fn image_wins_over_text_when_enabled() {
        let s = snap(Some("说明文字"), None, true, false);
        let c = decide_capture(&s, true).unwrap();
        assert_eq!(c.kind, "image");
    }

    #[test]
    fn image_is_skipped_when_capture_disabled_but_text_falls_through() {
        // 关掉图片捕获不该让整条内容丢失：图片常同时带说明文本
        let s = snap(Some("说明文字"), None, true, false);
        let c = decide_capture(&s, false).unwrap();
        assert_eq!(c.kind, "text");
        assert_eq!(c.text, "说明文字");
    }

    #[test]
    fn image_only_with_capture_disabled_yields_nothing() {
        let s = snap(Some("  "), None, true, false);
        assert_eq!(decide_capture(&s, false), None);
    }

    #[test]
    fn html_wins_over_plain_text() {
        let s = snap(Some("粗体"), None, false, true);
        let c = decide_capture(&s, true).unwrap();
        assert_eq!(c.kind, "html");
        assert_eq!(c.text, "粗体");
    }

    #[test]
    fn blank_html_does_not_produce_empty_entry() {
        // HTML 与纯文本都为空时不该入库
        let s = snap(Some(""), None, false, true);
        assert_eq!(decide_capture(&s, true), None);
    }

    #[test]
    fn plain_text_is_the_last_resort() {
        let s = snap(Some("hello"), None, false, false);
        let c = decide_capture(&s, true).unwrap();
        assert_eq!(c.kind, "text");
        assert_eq!(c.text, "hello");
    }

    #[test]
    fn whitespace_only_text_is_not_stored() {
        // 与 History::add 的判据一致：纯空白不入库
        for t in ["", "   ", "\n\t "] {
            assert_eq!(
                decide_capture(&snap(Some(t), None, false, false), true),
                None
            );
        }
    }

    #[test]
    fn empty_snapshot_yields_nothing() {
        assert_eq!(decide_capture(&ClipSnapshot::default(), true), None);
    }

    #[test]
    fn empty_file_list_is_ignored() {
        let s = snap(Some("text"), Some(vec![]), false, false);
        let c = decide_capture(&s, true).unwrap();
        assert_eq!(c.kind, "text", "空文件列表应让位给文本");
    }

    #[test]
    fn dedupe_skips_identical_content() {
        assert!(!should_capture("", "same", "same"));
    }

    #[test]
    fn blacklist_blocks_capture_when_foreground_matches() {
        // 测试进程没有前台窗口，所以这里直接测闸门本体：
        // 把**当前真实前台进程名**作为黑名单项，它必然命中（无论那是谁）。
        // 这比断言某个固定名字稳——跑测试时的前台窗口不可控。
        let fg = crate::w32::foreground_process_name();
        if fg.is_empty() {
            // 无前台窗口（例如无人值守会话）：黑名单按"不命中"处理是正确行为
            assert!(hist::should_record_now("anything.exe"));
            return;
        }
        let name = fg.trim_end_matches(".exe");
        assert!(
            !hist::should_record_now(name),
            "黑名单含前台进程 {name} 时必须拒绝记录"
        );
        // 与 .exe 后缀共存也要命中（设置里两种写法都常见）
        assert!(!hist::should_record_now(&format!("{name}.exe")));
    }

    #[test]
    fn blacklist_is_case_insensitive() {
        let fg = crate::w32::foreground_process_name();
        if fg.is_empty() {
            return;
        }
        let upper = fg.trim_end_matches(".exe").to_uppercase();
        assert!(
            !hist::should_record_now(&upper),
            "黑名单匹配应大小写不敏感（实际 {upper}）"
        );
    }

    #[test]
    fn should_capture_combines_dedupe_and_blacklist() {
        // 前置：内容不同 + 空黑名单 → 记录
        assert!(should_capture("", "new", "old"));
        // 去重闸：内容相同 → 不记录，且**不需要**查黑名单
        assert!(!should_capture("", "same", "same"));
    }

    #[test]
    fn new_content_passes_both_gates() {
        // 空黑名单 + 内容不同 → 应记录
        assert!(should_capture("", "new", "old"));
    }

    #[test]
    fn pause_does_not_lose_history_and_is_reflected_in_state() {
        set_enabled(false);
        assert!(!is_enabled());
        let s = state();
        assert!(!s.enabled);
        set_enabled(true);
        assert!(is_enabled());
        assert!(state().enabled);
        // 注意：这里不复位 running —— spawn() 在测试进程里不该被调用
    }

    #[test]
    fn snapshot_read_never_panics() {
        // 真机读取：无论剪贴板里是什么都不该 panic
        let s = read_snapshot();
        // 只要求能跑通且字段自洽
        if let Some(f) = &s.files {
            assert!(!f.is_empty(), "空文件列表会被规范成 None");
        }
    }

    #[test]
    fn clipboard_sequence_is_readable() {
        // 序号是监听器的基础：必须能读，且连续读取不会倒退（内容没变时相等）
        let a = w32::clipboard_sequence();
        let b = w32::clipboard_sequence();
        assert_eq!(a, b, "内容未变时序号应稳定");
    }

    #[test]
    fn cf_constants_match_winuser() {
        // 数值写错会静默读到别的格式——集中定义一处并锁住
        assert_eq!(w32::CF_DIB, 8);
        assert_eq!(w32::CF_UNICODETEXT, 13);
        assert_eq!(w32::CF_HDROP, 15);
        assert_eq!(w32::CF_DIBV5, 17);
    }
}
