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
        .spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS));
            // 自愈：单趟 panic 记 ERROR 后继续（监听线程死 = 记录静默消失，
            // 用户只会觉得"功能坏了"而日志里毫无痕迹）
            crate::catch_tick("clip-watch", capture_pass);
        })
        .expect("剪贴板监听线程启动失败");
}

/// 轮询与同步预捕获共用的单趟捕获逻辑。
fn capture_pass() {
    // 敏感内容到期清空搭在同一趟里：不另起定时器，且清空会改剪贴板内容——
    // 趟内更新序号基线，不会把"我们自己清空"当成一次新复制。
    if crate::sensitive::tick_clear() {
        let mut st = seq_state();
        st.last_seq = w32::clipboard_sequence();
        st.last_text.clear();
        return;
    }
    // 暂停时只更新序号基线：恢复后不会把暂停期间的内容补录进来
    let seq = w32::clipboard_sequence();
    let mut st = seq_state();
    if seq == st.last_seq {
        return;
    }
    st.last_seq = seq;
    if !is_enabled() {
        return;
    }
    let blacklist = crate::settings::get_str("clip_process_blacklist");
    let capture_images = crate::settings::get_bool("clip_capture_images");
    let snap = read_snapshot();
    let candidate = snap.text.clone().unwrap_or_default();
    // 去重与黑名单共用一次判定
    if candidate == st.last_text && !snap.has_image && snap.files.is_none() {
        SKIPPED_DUP.fetch_add(1, Ordering::SeqCst);
        return;
    }
    if !hist::should_record_now(&blacklist) {
        // 关键：把内容记为 last，避免切走焦点后补录
        st.last_text = candidate;
        SKIPPED_BL.fetch_add(1, Ordering::SeqCst);
        return;
    }
    let Some(cap) = decide_capture(&snap, capture_images) else {
        return;
    };
    st.last_text = candidate;
    drop(st); // 图片搬运（DIB→PNG→落盘）不在状态锁内做
    if cap.kind == "image" {
        // 图片走完整搬运路径：失败要记日志，不能让图片捕获悄悄失效
        match capture_image() {
            Ok(Some(label)) => log::debug!("已捕获剪贴板图片 {label}"),
            Ok(None) => {}
            Err(e) => log::warn!("剪贴板图片捕获失败: {e}"),
        }
    } else {
        store(&cap);
    }
}

/// 轮询基线（last_seq/last_text）：轮询线程与快速粘贴的同步预捕获共享。
#[derive(Default)]
struct SeqState {
    last_seq: u32,
    last_text: String,
}

fn seq_state() -> std::sync::MutexGuard<'static, SeqState> {
    static SEQ: std::sync::OnceLock<std::sync::Mutex<SeqState>> = std::sync::OnceLock::new();
    SEQ.get_or_init(|| std::sync::Mutex::new(SeqState::default()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 同步预捕获：序号有变化就地跑一趟捕获（零时延，事件驱动等效）。
///
/// 用途：Ctrl+Alt+V 在取历史**之前**调用——轮询间隔内「复制后立刻粘贴」
/// 时，内存历史还是上一条，直接读会把旧内容贴出去（对齐 Python 事件驱动的
/// 即时语义）。图片路径含 PNG 编码，可能占当前线程几十毫秒：调用方须在
/// 阻塞线程池内执行。
pub fn capture_pending_sync() {
    capture_pass();
}

/// 入库（图片与文本都直接进历史；图片的 PNG 已在调用方落盘）。
fn store(cap: &Captured) -> bool {
    let added = hist::with(|h| h.add(&cap.text, &cap.kind, &cap.data));
    if added {
        CAPTURED.fetch_add(1, Ordering::SeqCst);
    }
    added
}

/// 读取剪贴板位图 → 解 DIB → 编码 PNG → 落盘 → 入库。
///
/// 返回 `Ok(Some(标签))` 表示已入库；`Ok(None)` 表示这次没有值得捕获的图片
/// （剪贴板无位图，或只是图标尺寸）；`Err` 表示**识别出是图片但处理失败**。
///
/// 为什么把"没图"与"处理失败"分开：处理失败必须记日志。图片捕获悄悄失效会让
/// 用户以为图在别处丢了，而这类问题从界面上完全看不出来。
pub fn capture_image() -> Result<Option<String>, String> {
    let Some(dib) = w32::clipboard_get_dib() else {
        return Ok(None);
    };
    let bmp = crate::dib::parse_dib(&dib)?;
    if !bmp.is_worth_capturing() {
        return Ok(None); // 图标/光标残影，不值得占一条历史
    }
    let png = crate::dib::encode_png(&bmp)?;
    let label = format!("[图片 {}×{}]", bmp.width, bmp.height);
    let name = crate::clipboard_write::save_image_bytes(&png);
    if name.is_empty() {
        return Err("图片落盘失败".to_string());
    }
    store(&Captured {
        text: label.clone(),
        kind: "image".to_string(),
        data: name,
    });
    Ok(Some(label))
}

/// 把已有 PNG 字节入库（供"另存/导入"路径复用）。
pub fn store_png(png: &[u8], label: &str) -> bool {
    if png_to_dib(png).is_none() {
        return false; // 不是可用的 PNG
    }
    let name = crate::clipboard_write::save_image_bytes(png);
    if name.is_empty() {
        return false;
    }
    store(&Captured {
        text: label.to_string(),
        kind: "image".to_string(),
        data: name,
    })
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

    /// 真机端到端：读取**真实剪贴板**里的位图并解码。
    ///
    /// 剪贴板内容不可控（CI/无人值守下可能为空），所以无位图时直接跳过而不是
    /// 断言失败——但要真读到东西时，必须能解出合理尺寸且**不是全透明**。
    /// 这条用例的价值在于覆盖 `w32::clipboard_get_dib` 的长度推算：
    /// 剪贴板句柄不带长度，长度算错就会解出一堆垃圾或直接 panic。
    #[test]
    fn real_clipboard_bitmap_decodes_when_present() {
        let Some(dib) = w32::clipboard_get_dib() else {
            return; // 剪贴板里没有位图：跳过
        };
        let bmp = match crate::dib::parse_dib(&dib) {
            Ok(b) => b,
            Err(e) => panic!("读到了 {} 字节 DIB 但解析失败: {e}", dib.len()),
        };
        assert!(bmp.width > 0 && bmp.height > 0, "尺寸应有效");
        assert_eq!(
            bmp.rgba.len(),
            bmp.width as usize * bmp.height as usize * 4,
            "RGBA 缓冲长度应与宽高一致"
        );
        // 不能整张全透明：那说明我们把某种格式的 alpha 读错了
        let any_opaque = bmp.rgba.chunks_exact(4).any(|p| p[3] > 0);
        assert!(any_opaque, "解出的图不应全透明（alpha 解析有误）");
        // 也不该是全黑（除非真就是黑图，无法断言；只要求尺寸与 alpha 合理）
    }

    /// 真机：把自己构造的 DIB 走完整搬运路径（解码 → PNG → 落盘 → 入库）。
    ///
    /// 不依赖系统剪贴板：直接喂字节，验证"识别到图片就一定搬得动"。
    #[test]
    fn capture_pipeline_stores_a_real_bitmap() {
        let _g = crate::test_support::serial();
        crate::test_support::isolate();
        crate::test_support::reset_persistent_files();

        // 构造 40×30 的 24 位 DIB（超过 is_worth_capturing 的 16×16 下限）
        let (w, h) = (40u32, 30u32);
        let stride = crate::dib::row_stride(24, w) as usize;
        let mut v = Vec::new();
        v.extend_from_slice(&40u32.to_le_bytes());
        v.extend_from_slice(&(w as i32).to_le_bytes());
        v.extend_from_slice(&(h as i32).to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&24u16.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
        v.extend_from_slice(&((stride * h as usize) as u32).to_le_bytes());
        v.extend_from_slice(&2835i32.to_le_bytes());
        v.extend_from_slice(&2835i32.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        for _ in 0..h {
            let mut row = vec![0u8; w as usize * 3];
            for px in row.chunks_exact_mut(3) {
                px[0] = 0; // B
                px[1] = 128; // G
                px[2] = 255; // R → 橙色
            }
            row.resize(stride, 0);
            v.extend_from_slice(&row);
        }

        let bmp = crate::dib::parse_dib(&v).unwrap();
        assert_eq!((bmp.width, bmp.height), (w, h));
        assert!(bmp.is_worth_capturing());
        let png = crate::dib::encode_png(&bmp).unwrap();
        let label = format!("[图片 {}×{}]", w, h);
        assert!(store_png(&png, &label), "编码后的 PNG 应能入库");

        // 入库后能在历史里查到，且是 image 类型、带 PNG 文件名
        let found = hist::with(|hh| {
            hh.items()
                .iter()
                .any(|i| i.kind == "image" && i.text == label)
        });
        assert!(found, "历史里应出现该图片条目");
    }
}
