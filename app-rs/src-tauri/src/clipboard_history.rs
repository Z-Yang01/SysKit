//! 剪贴板历史：多格式条目（文本/图片/文件/HTML）、置顶、搜索、敏感内容防护。
//!
//! 条目 schema：`{"ts", "text", "kind": "text|image|file|html", "pinned": bool, "data"}`
//! - image：text 为可搜索占位（如 `[图片 128×96]`），data 为 PNG 文件名，
//!   字节存 `%APPDATA%\SysKit\clipboard_files\`（md5 命名去重）；
//! - html：text 为纯文本预览，data 为原始 HTML；
//! - 落盘经 secure_store DPAPI 密封；兼容旧明文（下次保存自动升级）。
//!
//! 与 Python 逐条对齐的取舍：落盘走后台防抖（mark_dirty + flush_if_dirty），
//! 崩溃最多丢一个刷盘周期的条目，换高频复制不在 UI 线程做 DPAPI 密封。

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::paths;
use crate::secure_store;

#[allow(dead_code)] // Python 侧同名默认值的对齐基准，实际默认值由 settings 层提供
pub const DEFAULT_BLACKLIST: &str = "keepass,bitwarden,1password,enpass";

/// 一条剪贴板历史（字段名与 Python schema 完全一致）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ClipItem {
    #[serde(default)]
    pub ts: i64,
    #[serde(default)]
    pub text: String,
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub data: String,
    /// 搜索结果的原始下标（仅内存态，不落盘）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idx: Option<usize>,
}

fn default_kind() -> String {
    "text".to_string()
}

impl ClipItem {
    pub fn new(ts: i64, text: String, kind: String, data: String) -> Self {
        Self {
            ts,
            text,
            kind,
            pinned: false,
            data,
            idx: None,
        }
    }

    /// 单行预览（与 Python ClipboardHistory.preview 同规则）。
    pub fn preview(text: &str, n: usize) -> String {
        let one_line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let chars: Vec<char> = one_line.chars().collect();
        if chars.len() <= n {
            return one_line;
        }
        let mut s: String = chars[..n.saturating_sub(1)].iter().collect();
        s.push('…');
        s
    }
}

/// 敏感/抑制摘要窗口：[{digest, until}]。
#[derive(Default)]
struct DigestWindow {
    entries: Vec<(String, f64)>,
}

impl DigestWindow {
    fn prune(&mut self) {
        let now = now_secs();
        self.entries.retain(|(_, until)| *until > now);
    }

    fn mark(&mut self, text: &str, ttl: f64) {
        self.entries.push((digest(text), now_secs() + ttl));
        self.prune();
    }

    /// 读取语义对齐 Python `_text_in`：**匹配前先剪掉过期摘要**——TTL 一过
    /// 立即失效，不等下一次 mark 顺带清理（分叉修复，Step 2）。
    fn contains(&mut self, text: &str) -> bool {
        self.prune();
        let d = digest(text);
        self.entries.iter().any(|(e, _)| *e == d)
    }

    #[cfg(test)]
    fn contains_at(&mut self, text: &str, now: f64) -> bool {
        self.entries.retain(|(_, until)| *until > now);
        let d = digest(text);
        self.entries.iter().any(|(e, _)| *e == d)
    }
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn digest(text: &str) -> String {
    // 简易稳定摘要（不引第三方哈希库）：FNV-1a 64 位 + 长度，用于本进程内窗口比对
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:016x}{:x}", hash, text.len())
}

fn sensitive() -> &'static Mutex<DigestWindow> {
    static W: OnceLock<Mutex<DigestWindow>> = OnceLock::new();
    W.get_or_init(|| Mutex::new(DigestWindow::default()))
}

fn suppressed() -> &'static Mutex<DigestWindow> {
    static W: OnceLock<Mutex<DigestWindow>> = OnceLock::new();
    W.get_or_init(|| Mutex::new(DigestWindow::default()))
}

/// 标记文本为敏感：ttl 秒内不记录（密码库复制用）。
pub fn mark_sensitive(text: &str, ttl: i64) {
    let ttl = ttl.max(1) as f64;
    sensitive()
        .lock()
        .expect("sensitive 锁中毒")
        .mark(text, ttl);
}

/// 短期抑制某文本被记录（快捷粘贴自动回填时用）。
pub fn suppress_next(text: &str, ttl: f64) {
    suppressed()
        .lock()
        .expect("suppressed 锁中毒")
        .mark(text, ttl.max(0.5));
}

fn is_sensitive(text: &str) -> bool {
    sensitive().lock().expect("sensitive 锁中毒").contains(text)
}

fn is_suppressed(text: &str) -> bool {
    suppressed()
        .lock()
        .expect("suppressed 锁中毒")
        .contains(text)
}

/// 当前时刻是否应记录剪贴板：前台进程不在黑名单中。
pub fn should_record_now(blacklist: &str) -> bool {
    let names: HashSet<String> = blacklist
        .split(',')
        .map(|p| p.trim().to_ascii_lowercase())
        .filter(|p| !p.is_empty())
        .map(|p| p.trim_end_matches(".exe").to_string())
        .collect();
    if names.is_empty() {
        return true;
    }
    let fg = crate::w32::foreground_process_name();
    let fg = fg.trim_end_matches(".exe");
    !names.iter().any(|n| n == fg)
}

/// 进程级历史实例（监听器与命令共用一个，避免多份内存态分叉）。
static HISTORY: OnceLock<Mutex<History>> = OnceLock::new();

fn history_cell() -> &'static Mutex<History> {
    HISTORY.get_or_init(|| Mutex::new(History::load()))
}

/// 在历史实例上执行闭包（唯一访问入口）。
pub fn with<R>(f: impl FnOnce(&mut History) -> R) -> R {
    let mut g = history_cell().lock().expect("历史锁中毒");
    f(&mut g)
}

/// 清掉进程级历史缓存并丢弃摘要窗口（**仅测试**：隔离目录被重置后，
/// 内存里那份旧实例还指着已删除的文件，会让后续断言读到过期数据）。
#[cfg(test)]
pub fn reset_global_for_test() {
    if let Some(cell) = HISTORY.get() {
        let mut g = cell.lock().unwrap_or_else(|p| p.into_inner());
        *g = History::default();
    }
    sensitive()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .entries
        .clear();
    suppressed()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .entries
        .clear();
}

/// 剪贴板历史（内存态 + 防抖落盘）。
pub struct History {
    pub max_items: usize,
    pub max_len: usize,
    items: Vec<ClipItem>,
    dirty: bool,
}

impl Default for History {
    fn default() -> Self {
        Self {
            max_items: 50,
            max_len: 5000,
            items: Vec::new(),
            dirty: false,
        }
    }
}

impl History {
    /// 从磁盘载入（DPAPI 密封为本格式，兼容旧明文）。
    ///
    /// 容量上限跟随设置 `clipboard_max`（对齐 Python `ClipboardTab` 构造
    /// `ClipboardHistory(max_items=int(settings["clipboard_max"]))`——上限是
    /// 用户可调的设置项，若钉死默认 50，用户调大后监听侧仍按 50 淘汰，
    /// 「最多保留 N 条」就成了撒谎的摆设）。
    pub fn load() -> Self {
        let mut h = History::default();
        // Python 构造钳位同款：max(1, int(max_items)) / max(64, int(max_len))——
        // 手改配置文件写出 0/负数时退化为「至少留 1 条」而不是剪贴板功能失效
        h.max_items = crate::settings::get_int("clipboard_max").max(1) as usize;
        h.max_len = h.max_len.max(64);
        let (data, _sealed) = secure_store::load_sealed_json(&paths::clipboard_file(), json!([]));
        if let Some(arr) = data.as_array() {
            for it in arr {
                if let Some(obj) = it.as_object() {
                    let Some(text) = obj.get("text").and_then(Value::as_str) else {
                        continue; // 无 text 的条目按损坏丢弃（与 Python 一致）
                    };
                    h.items.push(ClipItem {
                        ts: obj.get("ts").and_then(Value::as_i64).unwrap_or(0),
                        text: text.to_string(),
                        kind: obj
                            .get("kind")
                            .and_then(Value::as_str)
                            .filter(|k| !k.is_empty())
                            .unwrap_or("text")
                            .to_string(),
                        pinned: obj.get("pinned").and_then(Value::as_bool).unwrap_or(false),
                        data: obj
                            .get("data")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        idx: None,
                    });
                }
            }
        }
        h
    }

    pub fn items(&self) -> Vec<ClipItem> {
        self.items.clone()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// 有待写变更才密封落盘；返回是否发生写盘。
    pub fn flush_if_dirty(&mut self) -> bool {
        if !self.dirty {
            return false;
        }
        self.dirty = false;
        self.save();
        true
    }

    /// 立即密封落盘（失败留痕，历史将不再持久化）。
    pub fn save(&self) {
        let payload = Value::Array(
            self.items
                .iter()
                .map(|it| {
                    json!({
                        "ts": it.ts,
                        "text": it.text,
                        "kind": it.kind,
                        "pinned": it.pinned,
                        "data": it.data,
                    })
                })
                .collect(),
        );
        if !secure_store::save_sealed_json(&paths::clipboard_file(), &payload) {
            log::warn!(
                "剪贴板历史密封写盘失败（DPAPI/磁盘），本次变更未持久化: {}",
                paths::display(&paths::clipboard_file())
            );
        }
    }

    /// 追加一条。空白/敏感/与最新相同/与置顶相同/同载荷重复 → 不入库。
    pub fn add(&mut self, text: &str, kind: &str, data: &str) -> bool {
        if text.trim().is_empty() {
            return false;
        }
        let mut text = text.to_string();
        if is_sensitive(&text) || is_suppressed(&text) {
            return false;
        }
        if kind == "text" && text.chars().count() > self.max_len {
            text = text.chars().take(self.max_len).collect();
        }
        if let Some(first) = self.items.first() {
            if first.text == text && first.kind == kind && first.data == data {
                return false;
            }
        }
        if !data.is_empty()
            && self
                .items
                .iter()
                .any(|it| it.data == data && it.kind == kind && it.text == text)
        {
            return false;
        }
        if self
            .items
            .iter()
            .any(|it| it.pinned && it.text == text && it.data == data)
        {
            return false;
        }
        self.items.insert(
            0,
            ClipItem::new(now_secs() as i64, text, kind.to_string(), data.to_string()),
        );
        self.resort();
        self.evict();
        self.mark_dirty();
        true
    }

    /// 置顶块在前；块内按时间倒序（稳定排序保证同秒条目相对顺序不变）。
    fn resort(&mut self) {
        self.items.sort_by_key(|x| (u8::from(!x.pinned), -x.ts));
    }

    /// 超额裁剪：只删最老的非置顶条目；置顶不受 max_items 限制。
    fn evict(&mut self) {
        let unpinned: Vec<usize> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, it)| !it.pinned)
            .map(|(i, _)| i)
            .collect();
        let excess = unpinned.len().saturating_sub(self.max_items);
        if excess == 0 {
            return;
        }
        let evicted: Vec<usize> = unpinned[unpinned.len() - excess..].to_vec();
        let removed: Vec<ClipItem> = evicted.iter().map(|i| self.items[*i].clone()).collect();
        for i in evicted.into_iter().rev() {
            self.items.remove(i);
        }
        delete_image_files(&removed);
    }

    /// 删除超过 days 天的条目（含图片载荷；置顶同样过期——按天是时间语义）。
    pub fn purge_older_than(&mut self, days: i64) -> usize {
        if days <= 0 {
            return 0;
        }
        let cutoff = now_secs() - (days as f64) * 86400.0;
        let (keep, expired): (Vec<ClipItem>, Vec<ClipItem>) =
            self.items.iter().cloned().partition(|it| {
                // ts 缺失/为 0（旧版本条目）视为时效不可判定 → 保留（宁留勿误删）
                it.ts <= 0 || (it.ts as f64) >= cutoff
            });
        if expired.is_empty() {
            return 0;
        }
        self.items = keep;
        delete_image_files(&expired);
        self.mark_dirty();
        expired.len()
    }

    /// 按关键字（大小写不敏感子串）与类型过滤；返回带 idx 的副本。
    pub fn search(&self, keyword: Option<&str>, kind: Option<&str>) -> Vec<ClipItem> {
        let kw = keyword.unwrap_or("").trim().to_lowercase();
        self.items
            .iter()
            .enumerate()
            .filter(|(_, it)| kind.map_or(true, |k| it.kind == k))
            .filter(|(_, it)| kw.is_empty() || it.text.to_lowercase().contains(&kw))
            .map(|(i, it)| {
                let mut c = it.clone();
                c.idx = Some(i);
                c
            })
            .collect()
    }

    pub fn by_index(&self, index: usize) -> Option<ClipItem> {
        self.items.get(index).cloned()
    }

    pub fn toggle_pin(&mut self, index: usize) -> bool {
        if index >= self.items.len() {
            return false;
        }
        self.items[index].pinned = !self.items[index].pinned;
        self.resort();
        self.evict();
        self.mark_dirty();
        true
    }

    pub fn remove(&mut self, index: usize) -> bool {
        if index >= self.items.len() {
            return false;
        }
        let removed = self.items.remove(index);
        delete_image_files(std::slice::from_ref(&removed));
        self.mark_dirty();
        true
    }

    pub fn clear(&mut self) {
        delete_image_files(&self.items);
        self.items.clear();
        self.mark_dirty();
    }
}

/// 删除图片条目引用的 PNG 载荷（明文截图不应在磁盘无限期残留）。
fn delete_image_files(items: &[ClipItem]) {
    for it in items {
        if it.kind == "image" && !it.data.is_empty() {
            if let Some(p) = paths::image_path(&it.data) {
                let _ = std::fs::remove_file(p);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_window_contains_prunes_expired_on_read() {
        // 对齐 Python _text_in：匹配前先剪过期——TTL 一过立即失效，
        // 不等下一次 mark 顺带清理（Step 2 分叉修复）
        let mut w = DigestWindow::default();
        let now = now_secs();
        w.entries.push((digest("secret"), now + 5.0));
        assert!(w.contains_at("secret", now), "TTL 内应命中");
        assert!(!w.contains_at("secret", now + 6.0), "过期后应立即失效");
        assert!(
            w.entries.is_empty(),
            "读时剪枝应清掉过期条目，而非仅比对跳过"
        );
    }

    fn fresh() -> History {
        // 先钉住数据目录：add/remove/evict 会经由 paths::image_path 落路径
        let _ = crate::test_support::isolate();
        let mut h = History::default();
        h.items.clear();
        h
    }

    #[test]
    fn preview_matches_python_rules() {
        assert_eq!(ClipItem::preview("hello", 10), "hello");
        assert_eq!(
            ClipItem::preview("a\nb\tc", 10),
            "a b c",
            "空白折叠成单空格"
        );
        assert_eq!(ClipItem::preview("abcdef", 4), "abc…", "截断占一位省略号");
    }

    #[test]
    fn add_rejects_blank_and_duplicate_head() {
        let mut h = fresh();
        assert!(!h.add("", "text", ""));
        assert!(!h.add("   ", "text", ""));
        assert!(h.add("第一条", "text", ""));
        assert!(!h.add("第一条", "text", ""), "与最新条相同不入库");
        assert_eq!(h.len(), 1);
    }

    #[test]
    fn add_rejects_same_payload_anywhere() {
        let mut h = fresh();
        assert!(h.add("图A", "image", "abc.png"));
        assert!(h.add("别的文本", "text", ""));
        assert!(!h.add("图A", "image", "abc.png"), "同载荷已入库不再重复");
    }

    #[test]
    fn add_rejects_pinned_duplicate() {
        let mut h = fresh();
        assert!(h.add("置顶项", "text", ""));
        assert!(h.toggle_pin(0));
        assert!(h.add("其他", "text", ""));
        assert!(!h.add("置顶项", "text", ""), "与置顶条目相同不入库");
    }

    #[test]
    fn newest_first_and_text_truncated() {
        let mut h = fresh();
        h.max_len = 5;
        assert!(h.add("1234567890", "text", ""));
        assert_eq!(h.items()[0].text, "12345", "超长文本按 max_len 截断");
    }

    #[test]
    fn pinned_sorts_first_and_is_not_evicted() {
        let mut h = fresh();
        h.max_items = 2;
        for t in ["a", "b", "c"] {
            h.add(t, "text", "");
        }
        // 目前 3 条新在前：c,b,a → 裁掉最老的非置顶 a
        let texts: Vec<String> = h.items().iter().map(|i| i.text.clone()).collect();
        assert_eq!(texts, vec!["c", "b"]);
        // 置顶 b 后再加两条：置顶项不被淘汰
        let idx_b = h.items().iter().position(|i| i.text == "b").unwrap();
        assert!(h.toggle_pin(idx_b));
        h.add("d", "text", "");
        h.add("e", "text", "");
        let texts: Vec<String> = h.items().iter().map(|i| i.text.clone()).collect();
        assert_eq!(texts[0], "b", "置顶块在最前");
        assert!(texts.contains(&"b".to_string()), "置顶不被 max_items 裁剪");
        assert_eq!(texts.len(), 3, "1 置顶 + 2 非置顶上限");
    }

    #[test]
    fn purge_older_than_keeps_zero_ts() {
        let mut h = fresh();
        assert!(h.add("新", "text", ""));
        h.items.push(ClipItem {
            ts: 1, // 很久以前
            text: "旧".into(),
            kind: "text".into(),
            pinned: false,
            data: String::new(),
            idx: None,
        });
        h.items.push(ClipItem {
            ts: 0, // 旧版本条目：时效不可判定 → 保留
            text: "无时间戳".into(),
            kind: "text".into(),
            pinned: false,
            data: String::new(),
            idx: None,
        });
        let removed = h.purge_older_than(1);
        assert_eq!(removed, 1);
        let texts: Vec<String> = h.items().iter().map(|i| i.text.clone()).collect();
        assert!(texts.contains(&"无时间戳".to_string()), "ts=0 必须保留");
        assert!(!texts.contains(&"旧".to_string()));
    }

    #[test]
    fn purge_zero_days_is_noop() {
        let mut h = fresh();
        h.add("x", "text", "");
        assert_eq!(h.purge_older_than(0), 0);
        assert_eq!(h.len(), 1);
    }

    #[test]
    fn search_filters_and_reports_index() {
        let mut h = fresh();
        h.add("Hello World", "text", "");
        h.add("图", "image", "x.png");
        h.add("hello 中文", "text", "");
        let hits = h.search(Some("HELLO"), None);
        assert_eq!(hits.len(), 2, "大小写不敏感");
        assert!(hits.iter().all(|h| h.text.to_lowercase().contains("hello")));
        assert!(hits[0].idx.is_some(), "搜索结果必须带原始下标");
        let imgs = h.search(None, Some("image"));
        assert_eq!(imgs.len(), 1);
        assert_eq!(imgs[0].kind, "image");
    }

    #[test]
    fn remove_and_clear() {
        let mut h = fresh();
        h.add("a", "text", "");
        h.add("b", "text", "");
        assert!(h.remove(0));
        assert_eq!(h.len(), 1);
        assert!(!h.remove(5), "越界删除返回 false");
        h.clear();
        assert!(h.is_empty());
    }

    #[test]
    fn sensitive_and_suppressed_block_recording() {
        let mut h = fresh();
        mark_sensitive("我的密码", 25);
        assert!(!h.add("我的密码", "text", ""), "敏感内容不记录");
        suppress_next("回填内容", 2.0);
        assert!(!h.add("回填内容", "text", ""), "自动回填不算用户复制");
        assert!(h.add("普通内容", "text", ""));
    }

    #[test]
    fn digest_is_stable_and_distinguishes() {
        assert_eq!(digest("abc"), digest("abc"));
        assert_ne!(digest("abc"), digest("abd"));
        assert_ne!(digest("abc"), digest("abcabc"));
    }

    #[test]
    fn blacklist_matching_rules() {
        // 空黑名单 → 一律记录（不依赖真实前台进程）
        assert!(should_record_now(""));
        assert!(should_record_now(",,,  ,"));
    }

    #[test]
    fn load_applies_clipboard_max_from_settings() {
        // 对齐 Python ClipboardTab 构造：ClipboardHistory(max_items=int(settings["clipboard_max"]))
        // ——上限是设置项；钉死 50 的话用户调大后监听侧仍按 50 淘汰
        let _g = crate::test_support::serial();
        crate::test_support::reset_persistent_files();
        crate::settings::set_i64("clipboard_max", 3).expect("写设置失败");
        let h = History::load();
        assert_eq!(h.max_items, 3, "容量上限必须跟随设置");
        crate::settings::set_i64("clipboard_max", 0).expect("写设置失败");
        let h = History::load();
        assert_eq!(
            h.max_items, 1,
            "0/负数按 Python 构造钳位 max(1, int(max_items)) 退化为 1"
        );
        // 收尾恢复默认，免得并行用例读到本用例留下的 clipboard_max=0
        crate::test_support::reset_persistent_files();
    }

    #[test]
    fn save_and_reload_round_trip_through_isolation() {
        // 串行 + 先清盘：否则并行的其它用例会与我们抢同一份 clipboard.json
        let _g = crate::test_support::serial();
        crate::test_support::reset_persistent_files();
        // 写盘路径必须落在隔离目录（绝不能碰真实 %APPDATA%\SysKit）
        let dir = crate::test_support::isolate();
        assert!(crate::paths::clipboard_file().starts_with(&dir));

        let mut h = History::default();
        h.add("往返条目", "text", "");
        h.save();

        let back = History::load();
        assert_eq!(back.len(), 1, "密封写盘后必须能读回");
        assert_eq!(back.items()[0].text, "往返条目");
        assert_eq!(back.items()[0].kind, "text");
        // 落盘文件必须是 DPAPI 容器而不是明文条目数组
        let text = std::fs::read_to_string(crate::paths::clipboard_file()).unwrap();
        assert!(
            text.contains("\"enc\":\"dpapi\""),
            "落盘必须是密封容器: {text}"
        );
        assert!(!text.contains("往返条目"), "明文不得出现在容器里");
    }

    #[test]
    fn flush_only_writes_when_dirty() {
        let _g = crate::test_support::serial();
        crate::test_support::reset_persistent_files();
        let mut h = History::default();
        assert!(!h.flush_if_dirty(), "无变更不应写盘");
        h.add("触发脏标记", "text", "");
        assert!(h.flush_if_dirty(), "有变更才写盘");
        assert!(!h.flush_if_dirty(), "写完即转干净");
    }
}
