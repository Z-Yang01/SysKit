//! 剪贴板内容规则引擎：识别复制内容像什么，并给出可执行动作
//! （对应 Python 侧 `cliprules.py`）。纯逻辑，**无 Qt、无网络**。
//!
//! 动作的 `arg` 是执行时需要的参数（格式化后的 JSON / URL / 路径 / 主机名…），
//! `online=true` 表示该动作需要联网（IP 归属地、ping、DNS），界面据此提示。
//! **实际执行**由 UI 层接线到既有能力（`dnsclient`/`ping`/打开链接等）。
//!
//! ## 两处真机换来的细节
//!
//! 1. **URL 必须排除 CJK 与全角标点**：中文语境里「句号紧邻」或「全角括号包裹」
//!    会把标点一起抓进 URL，二维码/打开链接就拿到错误地址。这不是边界情况——
//!    中文用户复制链接时后面常紧跟一句中文说明。
//! 2. **`exists()` 之前先做盘型预检**：失效网络盘或未知盘符上的 `exists()` 可以
//!    阻塞数秒到数十秒，而 `detect()` 是在 UI 线程被调用的（用户选中剪贴板历史即触发）
//!    ——一次卡顿就是整个界面冻结。只对本地盘型（可移动/固定/光驱/RAM）做探测。

use serde::Serialize;

use crate::w32;

/// 检测结果里每条文本的长度上限（超过直接放弃，避免对超大内容做正则式扫描）。
pub const MAX_DETECT_LEN: usize = 1_000_000;

/// 一个可执行动作。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClipAction {
    pub id: String,
    pub label: String,
    pub arg: String,
    /// 是否需要联网（界面据此提示）
    pub online: bool,
}

fn action(id: &str, label: &str, arg: &str, online: bool) -> ClipAction {
    ClipAction {
        id: id.to_string(),
        label: label.to_string(),
        arg: arg.to_string(),
        online,
    }
}

/// 是否像 URL 里的合法字符（**排除 CJK 与全角标点**）。
///
/// 这是 `URL_RE` 那个字符类的等价物：
/// `[^\s<>"'\u3000-\u303f\uff00-\uffef\u4e00-\u9fff]`
fn url_char_ok(c: char) -> bool {
    if c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'') {
        return false;
    }
    let u = c as u32;
    // CJK 标点 3000-303F、全角形式 FF00-FFEF、CJK 汉字 4E00-9FFF
    !((0x3000..=0x303F).contains(&u)
        || (0xFF00..=0xFFEF).contains(&u)
        || (0x4E00..=0x9FFF).contains(&u))
}

/// 剥尾部标点：`.,;` 无条件剥；ASCII `)` 仅在**无配对开括号**时剥。
///
/// 最后一条是必要的：`https://en.wikipedia.org/wiki/Foo_(bar)` 的右括号是 URL 的
/// 一部分，无条件剥会打开错误地址。
pub fn trim_url(u: &str) -> String {
    let mut s = u.trim_end_matches(['.', ',', ';']).to_string();
    while s.ends_with(')') && s.matches('(').count() < s.matches(')').count() {
        s.pop();
    }
    s
}

/// 在 `haystack` 里大小写无关地找 ASCII `needle` 的字节偏移。
///
/// 必须按**原串**做字节级比较，不能拿 `to_lowercase()` 的偏移去切原串：
/// 大小写转换会改变 UTF-8 字节长度（`ẞ`→`ß` 3→2 缩短、`İ`→`i̇` 2→3 变长），
/// 转换串里的偏移会落进原串多字节字符中间（slice 直接 panic）或错位抓错地址。
/// 而 needle 全为 ASCII（<0x80），UTF-8 续字节均 ≥0x80——命中处必是字符边界，
/// 字节级搜索天然安全。
fn find_ascii_ci(haystack: &str, needle: &str) -> Option<usize> {
    if haystack.len() < needle.len() {
        return None;
    }
    haystack
        .as_bytes()
        .windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
}

/// 在文本里找第一个 URL（`http://` / `https://`，大小写不敏感）。
pub fn find_url(text: &str) -> Option<String> {
    let start = find_ascii_ci(text, "http://").or_else(|| find_ascii_ci(text, "https://"))?;
    // start 是原串里的字节偏移且必在字符边界上，取"URL 合法字符"安全
    let tail: String = text[start..]
        .chars()
        .take_while(|c| url_char_ok(*c))
        .collect();
    if tail.len() <= "https://".len() {
        return None; // 只有协议头没有内容
    }
    Some(trim_url(&tail))
}

/// 是否像 `#RGB` / `#RRGGBB`（大小写不敏感）。
pub fn is_color(text: &str) -> bool {
    let t = text.trim();
    let Some(hex) = t.strip_prefix('#') else {
        return false;
    };
    (hex.len() == 3 || hex.len() == 6) && hex.chars().all(|c| c.is_ascii_hexdigit())
}

/// 是否像 Windows 绝对路径 `X:\...`。
///
/// 等价 Python `^[A-Za-z]:\\[^<>:"|?*\r\n]+$`：盘符 + 反斜杠开头，
/// 之后至少一个字符且不含文件名非法字符。普通句子（含斜杠但非路径）不会被误判。
pub fn is_win_path(text: &str) -> bool {
    let t = text.trim();
    let b: Vec<char> = t.chars().collect();
    // 盘符 + ':' + '\' + 至少一个字符
    if b.len() < 4 {
        return false;
    }
    if !b[0].is_ascii_alphabetic() || b[1] != ':' || b[2] != '\\' {
        return false;
    }
    // 其余部分不得含非法字符（注意 `:` 在盘符后不再合法）
    b[3..]
        .iter()
        .all(|c| !matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*' | '\r' | '\n'))
}

/// 是否像域名（`example.com`，TLD 至少 2 个字母）。
pub fn is_domain(text: &str) -> bool {
    let t = text.trim();
    let n = t.chars().count();
    if !(4..=253).contains(&n) {
        return false;
    }
    let parts: Vec<&str> = t.split('.').collect();
    if parts.len() < 2 {
        return false;
    }
    for (i, p) in parts.iter().enumerate() {
        if p.is_empty() {
            return false;
        }
        let last = i == parts.len() - 1;
        if last {
            // TLD 至少两个字母
            if p.len() < 2 || !p.chars().all(|c| c.is_ascii_alphabetic()) {
                return false;
            }
        } else {
            // 每段：字母数字开头结尾、中间可含连字符、长度 ≤63
            if p.len() > 63 {
                return false;
            }
            let cs: Vec<char> = p.chars().collect();
            if !cs[0].is_ascii_alphanumeric() || !cs[cs.len() - 1].is_ascii_alphanumeric() {
                return false;
            }
            if !cs.iter().all(|c| c.is_ascii_alphanumeric() || *c == '-') {
                return false;
            }
        }
    }
    true
}

/// 盘型预检：是否值得对路径做 `exists()` 探测。
///
/// 返回 false 表示"别探"——远程盘（4）与未知类型（0/1）上的 `exists()` 会阻塞。
/// 本地盘型（2 可移动 / 3 固定 / 5 光驱 / 6 RAM）才放行。
pub fn path_probe_safe(path: &str) -> bool {
    let t = path.trim();
    let b: Vec<char> = t.chars().collect();
    if b.len() < 2 || !b[0].is_ascii_alphabetic() || b[1] != ':' {
        return true; // 无盘符：交给调用方（与 Python 同口径）
    }
    let root = format!("{}:\\", b[0].to_ascii_uppercase());
    w32::drive_type(&root)
        .map(|t| matches!(t, 2 | 3 | 5 | 6))
        .unwrap_or(false)
}

/// 按优先级返回可执行动作；不匹配返回空表。
///
/// 优先级顺序（与 Python 一致）：JSON → URL → 颜色 → 文件路径 → IP/域名。
/// 顺序对 URL 与域名尤其重要：`https://a.com` 不该再被当成"域名"给一次 DNS 动作，
/// 否则用户看到两组重复按钮。
pub fn detect(text: &str, path_exists: impl Fn(&str) -> bool) -> Vec<ClipAction> {
    let mut hits = Vec::new();
    let t = text.trim();
    if t.is_empty() || t.chars().count() > MAX_DETECT_LEN {
        return hits;
    }

    // 1. JSON：以 { 或 [ 开头且可解析 → 格式化
    if t.starts_with('{') || t.starts_with('[') {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(t) {
            if let Ok(pretty) = serde_json::to_string_pretty(&v) {
                hits.push(action("json_format", "格式化 JSON", &pretty, false));
            }
        }
    }

    // 2. URL：二维码 + 打开链接
    if let Some(url) = find_url(t) {
        hits.push(action("url_qr", "生成二维码", &url, false));
        hits.push(action("url_open", "打开链接", &url, false));
    }

    // 3. 颜色值
    if is_color(t) {
        hits.push(action("color_show", "查看色块", &t.to_uppercase(), false));
    }

    // 4. Windows 文件路径：**存在 + 盘型可探**才给动作（避免把普通句子误判）
    if is_win_path(t) && path_probe_safe(t) && path_exists(t) {
        hits.push(action("file_reveal", "打开所在目录", t, false));
        hits.push(action("file_open", "打开文件", t, false));
    }

    // 5. IP 或域名
    if let Ok(ip) = t.parse::<std::net::IpAddr>() {
        hits.push(action("ip_ping", "Ping", &ip.to_string(), true));
        // 归属地查询只对 IPv4 有意义
        if matches!(ip, std::net::IpAddr::V4(_)) {
            hits.push(action("ip_geo", "查询归属地", &ip.to_string(), true));
        }
    } else if is_domain(t) {
        let lower = t.to_lowercase();
        hits.push(action("domain_dns", "DNS 解析", &lower, true));
        hits.push(action("domain_ping", "Ping", &lower, true));
    }

    hits
}

/// 便捷入口：用真实文件系统做路径存在性检查。
pub fn detect_real(text: &str) -> Vec<ClipAction> {
    detect(text, |p| std::path::Path::new(p).exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 假的存在性检查：只有 `C:\Windows\notepad.exe` 存在。
    fn fake_exists(p: &str) -> bool {
        p.eq_ignore_ascii_case(r"C:\Windows\notepad.exe")
    }

    fn ids(hits: &[ClipAction]) -> Vec<&str> {
        hits.iter().map(|h| h.id.as_str()).collect()
    }

    #[test]
    fn empty_and_oversized_input_are_ignored() {
        assert!(detect("", fake_exists).is_empty());
        assert!(detect("   ", fake_exists).is_empty());
        let huge = "a".repeat(MAX_DETECT_LEN + 1);
        assert!(detect(&huge, fake_exists).is_empty(), "超长内容直接放弃");
    }

    #[test]
    fn json_is_formatted_and_stays_unicode() {
        let hits = detect(r#"{"name":"中文","n":1}"#, fake_exists);
        assert_eq!(ids(&hits), vec!["json_format"]);
        let arg = &hits[0].arg;
        assert!(arg.contains("中文"), "不得转义成 \\u 形式: {arg}");
        assert!(arg.contains('\n'), "应缩进格式化: {arg}");
    }

    #[test]
    fn invalid_json_starting_with_brace_gives_nothing() {
        assert!(detect("{不是 json", fake_exists).is_empty());
        assert!(detect("[1,2,", fake_exists).is_empty());
    }

    #[test]
    fn url_gives_qr_and_open() {
        let hits = detect("https://example.com/a?b=1", fake_exists);
        assert_eq!(ids(&hits), vec!["url_qr", "url_open"]);
        assert_eq!(hits[0].arg, "https://example.com/a?b=1");
        assert!(!hits[0].online, "二维码与打开链接都不需要联网");
    }

    #[test]
    fn url_embedded_in_sentence_is_extracted() {
        let hits = detect("看这个 https://example.com/x 很有意思", fake_exists);
        assert_eq!(hits[0].arg, "https://example.com/x");
    }

    #[test]
    fn url_stops_at_cjk_punctuation() {
        // 中文语境最常见：链接后紧跟句号/全角括号 → 不能把标点抓进去
        for (input, want) in [
            ("https://a.com/x。", "https://a.com/x"),
            ("https://a.com/x，后面还有", "https://a.com/x"),
            ("（https://a.com/x）", "https://a.com/x"),
            ("https://a.com/x、", "https://a.com/x"),
        ] {
            let hits = detect(input, fake_exists);
            assert_eq!(hits[0].arg, want, "输入 {input}");
        }
    }

    #[test]
    fn url_trailing_ascii_punctuation_is_trimmed() {
        assert_eq!(find_url("https://a.com/x.").unwrap(), "https://a.com/x");
        assert_eq!(find_url("https://a.com/x,").unwrap(), "https://a.com/x");
        assert_eq!(find_url("https://a.com/x;").unwrap(), "https://a.com/x");
    }

    #[test]
    fn url_keeps_balanced_parens() {
        // 维基式 URL 的右括号是地址的一部分
        let u = find_url("https://en.wikipedia.org/wiki/Foo_(bar)").unwrap();
        assert_eq!(u, "https://en.wikipedia.org/wiki/Foo_(bar)");
        // 无配对开括号时才剥
        assert_eq!(find_url("(https://a.com/x)").unwrap(), "https://a.com/x");
    }

    #[test]
    fn url_without_payload_is_rejected() {
        assert!(find_url("https://").is_none());
        assert!(find_url("http://").is_none());
    }

    #[test]
    fn url_is_case_insensitive_on_scheme() {
        assert_eq!(find_url("HTTPS://A.com/X").unwrap(), "HTTPS://A.com/X");
        assert_eq!(find_url("Http://a.com").unwrap(), "Http://a.com");
    }

    #[test]
    fn url_survives_lowercase_shrinking_unicode() {
        // 回归：`ẞ`(U+1E9E, 3 字节) 小写为 `ß`(U+00DF, 2 字节)，转换串变短后
        // 旧实现的偏移会切进原串多字节字符中间 → slice panic（UI 线程即崩应用）。
        // https 走的是 or_else 兜底分支，必须一并锁住。
        let u = find_url("ẞhttps://a.com/x").unwrap();
        assert_eq!(u, "https://a.com/x");
    }

    #[test]
    fn url_survives_lowercase_growing_unicode() {
        // 回归：`İ`(U+0130, 2 字节) 小写为 `i̇`(1+2 字节)，转换串变长后旧实现
        // 偏移错位，会把 "https://a.com" 抓成 "ttps://a.com" 这种坏地址。
        let u = find_url("İhttps://a.com/x").unwrap();
        assert_eq!(u, "https://a.com/x");
    }

    #[test]
    fn url_in_cjk_sentence_with_fullwidth_chars_still_works() {
        // 混排常见形态：全角标点紧邻 URL（截断正确性不因字节级搜索回退）
        let u = find_url("看 https://A.com/X。说明").unwrap();
        assert_eq!(u, "https://A.com/X");
    }

    #[test]
    fn color_detection() {
        for c in ["#fff", "#FFF", "#a1b2c3", "#A1B2C3"] {
            assert!(is_color(c), "{c} 应是颜色");
        }
        for c in ["#gg0000", "#12345", "#", "fff", "#1234567", ""] {
            assert!(!is_color(c), "{c} 不是颜色");
        }
    }

    #[test]
    fn color_action_uppercases() {
        let hits = detect("#a1b2c3", fake_exists);
        assert_eq!(ids(&hits), vec!["color_show"]);
        assert_eq!(hits[0].arg, "#A1B2C3");
    }

    #[test]
    fn win_path_shape() {
        assert!(is_win_path(r"C:\Windows\notepad.exe"));
        assert!(is_win_path(r"d:\a\b"));
        // 非路径形态
        assert!(!is_win_path(r"C:/Windows"), "正斜杠不是 Windows 路径形态");
        assert!(!is_win_path("C:"), "太短");
        assert!(!is_win_path(r"\\server\share"), "UNC 不在本规则内");
        assert!(!is_win_path("notepad.exe"));
        assert!(!is_win_path(r"C:\a<bad>"));
    }

    #[test]
    fn file_path_needs_existence() {
        // 存在的真实路径 → 两个动作
        let hits = detect(r"C:\Windows\notepad.exe", fake_exists);
        assert_eq!(ids(&hits), vec!["file_reveal", "file_open"]);
        // 形态正确但不存在 → **不给动作**（避免把普通句子误判成路径）
        assert!(detect(r"C:\nope\nothing.txt", fake_exists).is_empty());
    }

    #[test]
    fn ipv4_gives_ping_and_geo() {
        let hits = detect("223.5.5.5", fake_exists);
        assert_eq!(ids(&hits), vec!["ip_ping", "ip_geo"]);
        assert!(hits.iter().all(|h| h.online), "两项都需要联网");
    }

    #[test]
    fn ipv6_gives_ping_only() {
        // 归属地查询对 v6 无意义，不该出现该动作
        let hits = detect("2001:4860:4860::8888", fake_exists);
        assert_eq!(ids(&hits), vec!["ip_ping"]);
    }

    #[test]
    fn domain_gives_dns_and_ping() {
        let hits = detect("Example.COM", fake_exists);
        assert_eq!(ids(&hits), vec!["domain_dns", "domain_ping"]);
        assert_eq!(hits[0].arg, "example.com", "域名小写化");
        assert!(hits.iter().all(|h| h.online));
    }

    #[test]
    fn domain_shape_rules() {
        assert!(is_domain("example.com"));
        assert!(is_domain("a-b.example.co.uk"));
        assert!(is_domain("x1.example.com"));
        // 拒绝
        assert!(!is_domain("example"), "无点");
        assert!(!is_domain("example.c"), "TLD 少于 2 字母");
        assert!(!is_domain("example.123"), "TLD 必须字母");
        assert!(!is_domain("-bad.com"), "段不能以连字符开头");
        assert!(!is_domain("bad-.com"), "段不能以连字符结尾");
        assert!(!is_domain("a..com"), "空段");
        assert!(!is_domain("中文.com"), "非 ASCII 段");
        assert!(!is_domain(&format!("{}.com", "a".repeat(64))), "段超过 63");
    }

    #[test]
    fn url_is_not_also_treated_as_domain() {
        // 顺序保证：给了 URL 动作就不再给域名动作，否则用户看到重复按钮
        let hits = detect("https://example.com/x", fake_exists);
        assert!(!hits.iter().any(|h| h.id.starts_with("domain_")));
    }

    #[test]
    fn json_containing_a_url_gives_both() {
        // JSON 优先但不清空其它命中：{} 里常有 URL，用户两种都想用
        let hits = detect(r#"{"url":"https://example.com/x"}"#, fake_exists);
        let got = ids(&hits);
        assert!(got.contains(&"json_format"));
        assert!(got.contains(&"url_open"));
    }

    #[test]
    fn plain_sentence_gives_nothing() {
        for s in ["今天天气不错", "hello world", "这是一段普通文字。"] {
            assert!(detect(s, fake_exists).is_empty(), "{s} 不该有动作");
        }
    }

    #[test]
    fn path_probe_safe_allows_local_drives() {
        // C 盘是固定盘（3）→ 放行
        assert!(path_probe_safe(r"C:\Windows"), "固定盘应放行");
        // 无盘符 → 放行（交给调用方）
        assert!(path_probe_safe(r"relative\path"));
        assert!(path_probe_safe(""));
    }

    #[test]
    fn path_probe_safe_rejects_unmapped_drive() {
        // Z 盘通常不存在 → 类型为 0/1（未知/无根目录）→ **不该探测**，
        // 否则 exists() 在失效网络盘上会阻塞 UI 数秒
        assert!(
            !path_probe_safe(r"Z:\some\network\share"),
            "未映射盘不该探测"
        );
    }

    #[test]
    fn detect_real_finds_existing_system_file() {
        // 真机：用系统里必定存在的文件验证整条链路（含真实 exists）
        let hits = detect_real(r"C:\Windows\System32\notepad.exe");
        let got = ids(&hits);
        assert!(
            got.contains(&"file_open"),
            "真实存在的系统文件应给出打开动作，实际 {got:?}"
        );
    }
}
