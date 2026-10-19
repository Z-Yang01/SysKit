//! 剪贴板敏感内容检测（纯逻辑，无 Win32）：命中 → 不入库。
//!
//! 与既有三套敏感机制的关系（互不替代）：
//! - `sensitive.rs`：应用自己复制的敏感内容到期清空；
//! - `clip_process_blacklist`：前台进程黑名单（密码管理器等）；
//! - 本模块：**内容识别**——复制的内容本身长得像凭据（信用卡/身份证/API
//!   密钥）时不进历史。命中语义与黑名单一致：静默跳过 + 计数。
//!
//! 误报控制是硬要求：
//! - 信用卡必须过 **Luhn**（订单号/时间戳等长数字串天然不通过）；
//! - 身份证必须过 **GB 11643 校验位**；
//! - 令牌类必须带已知前缀，或 ≥20 字符且**观测字符集香农熵 ≥3.5
//!   （bits/char，整串）**——32 位英文句子（熵 ~2.2）不命中，
//!   32 位随机 hex/base62 命中。

/// 检测文本是否像敏感凭据。返回命中的类别名（供日志/计数展示）。
pub fn detect_sensitive(text: &str) -> Option<&'static str> {
    if text.len() < 13 {
        return None; // 最短命中形态是 13 位信用卡
    }
    if has_credit_card(text) {
        return Some("信用卡号");
    }
    if has_id_number(text) {
        return Some("身份证号");
    }
    if has_api_token(text) {
        return Some("API 密钥");
    }
    None
}

// ---------------- 信用卡：Luhn ----------------

/// 扫描「数字/空格/连字符」连续段；段内去分隔符后 13~19 位纯数字且 Luhn 通过 → 命中。
///
/// 全程按**字节**扫描，不再 `chars().collect()` 物化整串：三类命中形态
///（信用卡数字、身份证数字+X、API 密钥的 `[A-Za-z0-9_-]`）都是 ASCII，
/// 而非 ASCII 的 UTF-8 字节（≥0x80）天然起分隔符作用——与按 char 切分**等价**。
/// 这是热路径（每次剪贴板变化都跑一遍）：旧实现在这里与 `has_id_number` 各
/// 物化一次 `Vec<char>`，复制 1MB 文本就是两次百万级分配。
fn has_credit_card(text: &str) -> bool {
    let b = text.as_bytes();
    let mut start = 0usize;
    for i in 0..=b.len() {
        let in_run = i < b.len() && (b[i].is_ascii_digit() || b[i] == b' ' || b[i] == b'-');
        if !in_run {
            if i > start && luhn_run_ok(&b[start..i]) {
                return true;
            }
            start = i + 1;
        }
    }
    false
}

/// 段内数字（去空格/连字符）13~19 位且 Luhn 通过。
///
/// 超过 19 位立即判否——与「先数完再判长度」结果相同，但不必扫完长数字串，
/// 也不必为它分配。
fn luhn_run_ok(run: &[u8]) -> bool {
    let mut digits = [0u8; 19];
    let mut n = 0usize;
    for &c in run {
        if c.is_ascii_digit() {
            if n == digits.len() {
                return false;
            }
            digits[n] = c - b'0';
            n += 1;
        }
    }
    n >= 13 && luhn_ok(&digits[..n])
}

/// Luhn 算法（模 10 双倍加）：从右往左奇数位原样、偶数位翻倍 >9 减 9，总和 %10==0。
fn luhn_ok(digits: &[u8]) -> bool {
    let mut sum = 0u32;
    let mut double = false;
    for &d in digits.iter().rev() {
        let mut v = d as u32;
        if double {
            v *= 2;
            if v > 9 {
                v -= 9;
            }
        }
        sum += v;
        double = !double;
    }
    sum % 10 == 0
}

// ---------------- 身份证：GB 11643-1999 校验位 ----------------

const ID_WEIGHTS: [u32; 17] = [7, 9, 10, 5, 8, 4, 2, 1, 6, 3, 7, 9, 10, 5, 8, 4, 2];
const ID_CHECK: [char; 11] = ['1', '0', 'X', '9', '8', '7', '6', '5', '4', '3', '2'];

/// 18 位身份证（末位可为 X/x）：前 17 位加权模 11 对应校验码。
///
/// 按**字节窗口**滑动：非 ASCII 字节不可能通过 `is_ascii_digit()`，因此与按
/// char 窗口切分等价，且不再物化 `Vec<char>`（热路径，见 `has_credit_card`）。
///
/// 这里是 `String[..n]` 字节切片 panic 的旧伤处——剪贴板文本任由多字节字符
/// （中文）开头，字节下标会落在字符中间（轮次116 回归锁
/// `multibyte_text_never_panics_id_window`）。现在只做**窗口读取**、从不按
/// 绝对下标切字符串，多字节前缀不会 panic。
fn has_id_number(text: &str) -> bool {
    let b = text.as_bytes();
    if b.len() < 18 {
        return false;
    }
    for win in b.windows(18) {
        let head = &win[..17];
        if !head.iter().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let sum: u32 = head
            .iter()
            .enumerate()
            .map(|(i, &c)| ((c - b'0') as u32) * ID_WEIGHTS[i])
            .sum();
        if ID_CHECK[(sum % 11) as usize] as u8 == win[17].to_ascii_uppercase() {
            return true;
        }
    }
    false
}

// ---------------- API 密钥：已知前缀 + 高熵段 ----------------

/// 观测字符集香农熵（bits/char，整串）。
pub fn entropy_bits_per_char(s: &str) -> f64 {
    if s.is_empty() {
        return 0.0;
    }
    let mut freq = std::collections::HashMap::new();
    for c in s.chars() {
        *freq.entry(c).or_insert(0u32) += 1;
    }
    let n = s.chars().count() as f64;
    freq.values()
        .map(|&f| {
            let p = f as f64 / n;
            -p * p.log2()
        })
        .sum()
}

fn looks_like_token(word: &str) -> bool {
    // 已知云厂商前缀：AKIA…(AWS)、sk-…(OpenAI 类)、ghp_/gho_…(GitHub)、xox…(Slack)
    let prefixed = (word.starts_with("AKIA")
        && word.len() >= 20
        && word[4..]
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()))
        || (word.starts_with("sk-") && word.len() >= 20)
        || ((word.starts_with("ghp_") || word.starts_with("gho_")) && word.len() >= 34)
        || ((word.starts_with("xoxb-") || word.starts_with("xoxp-") || word.starts_with("xoxa-"))
            && word.len() >= 20);
    if prefixed {
        return true;
    }
    // 无前缀的高熵段：≥20 字符且熵 ≥3.5（"Bearer <token>" 的 token 靠这条命中）
    word.len() >= 20 && entropy_bits_per_char(word) >= 3.5
}

/// 词 = 连续的 `[A-Za-z0-9_-]`；"Bearer <token>" 的 token 靠高熵规则命中。
///
/// 按字节取词边界、只把词切片成 `&str`，不为每个词分配 `String`
/// （长文本里词数以千计，旧实现逐词分配）。
fn has_api_token(text: &str) -> bool {
    let b = text.as_bytes();
    let mut start = 0usize;
    for i in 0..=b.len() {
        let in_word = i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'-');
        if !in_word {
            if i > start {
                // 词只含 ASCII，[start, i) 必落在 UTF-8 字符边界上
                if let Ok(w) = std::str::from_utf8(&b[start..i]) {
                    if looks_like_token(w) {
                        return true;
                    }
                }
            }
            start = i + 1;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- 信用卡 ----
    #[test]
    fn luhn_hits_valid_card_numbers_with_separators() {
        // 4111111111111111 是标准测试卡号
        assert_eq!(
            detect_sensitive("pay with 4111 1111 1111 1111 please"),
            Some("信用卡号")
        );
        assert_eq!(detect_sensitive("4111-1111-1111-1111"), Some("信用卡号"));
        assert_eq!(
            detect_sensitive("card: 4111111111111111."),
            Some("信用卡号")
        );
    }

    #[test]
    fn luhn_rejects_non_card_digit_runs() {
        // 订单号/时间戳：长度或校验位不过
        assert_eq!(detect_sensitive("order 12345678901234567890 done"), None); // 20 位超长
        assert_eq!(detect_sensitive("id 4111111111111112"), None); // Luhn 失败
        assert_eq!(detect_sensitive("202610081234"), None); // 12 位不足
    }

    // ---- 身份证 ----
    #[test]
    fn id_number_with_valid_checksum_hits() {
        // 校验位正确的合成号：11010519491231002X（GB 11643 文档示例）
        assert_eq!(detect_sensitive("id=11010519491231002X"), Some("身份证号"));
        assert_eq!(detect_sensitive("id=11010519491231002x"), Some("身份证号"));
    }

    #[test]
    fn id_number_wrong_checksum_misses() {
        assert_eq!(detect_sensitive("id=110105194912310021"), None);
    }

    // ---- API 密钥 ----
    #[test]
    fn prefixed_tokens_hit() {
        assert_eq!(
            detect_sensitive("key AKIAIOSFODNN7EXAMPLE ok"),
            Some("API 密钥")
        );
        assert_eq!(
            detect_sensitive("sk-proj-abcdefghijklmnopqrstuvwx"),
            Some("API 密钥")
        );
        assert_eq!(
            detect_sensitive("token ghp_abcdefghijklmnopqrstuvwxyz123"),
            Some("API 密钥")
        );
    }

    #[test]
    fn high_entropy_token_hits_even_bare() {
        // 32 位随机 base62（熵 >3.5）
        assert_eq!(
            detect_sensitive("Bearer 8f14e45fceea167a5a36dedd4bea2543"),
            Some("API 密钥")
        );
        // 32 位随机 hex
        assert_eq!(
            detect_sensitive("token=de7e2d4f5a9b1c0e8f3a6b5c2d1e0f97"),
            Some("API 密钥")
        );
    }

    #[test]
    fn low_entropy_english_does_not_hit() {
        // 32 位有意义的英文：熵 ~2.2（评审给的边界用例）
        assert_eq!(
            detect_sensitive("this is a perfectly normal sentence"),
            None
        );
        // 裸 Bearer 词不算（必须跟高熵段）
        assert_eq!(detect_sensitive("how to use Bearer auth"), None);
    }

    #[test]
    fn entropy_definition_matches_review_spec() {
        // bits/char、整串、观测字符集：单字符集熵 0，两等概率字符熵 1
        assert_eq!(entropy_bits_per_char(""), 0.0);
        assert!((entropy_bits_per_char("aaaa") - 0.0).abs() < 1e-9);
        assert!((entropy_bits_per_char("aabb") - 1.0).abs() < 1e-9);
        // 32 位随机 base62 理论上限 log2(62)≈5.95，随机样本应 ≥3.5
        assert!(entropy_bits_per_char("8f14e45fceea167a5a36dedd4bea2543") >= 3.5);
    }

    #[test]
    fn english_sentence_never_hits_at_word_level() {
        // 评审边界：32+ 字符英文句子不应命中。熵阈值作用在**分词后的词**上
        //（正常句子有空格 → 每词 <20 字符 → 根本不进熵评估），而不是整串——
        // 字符级整串熵对英文约 4.0+，按整串判会把普通英文误报成密钥。
        assert_eq!(
            detect_sensitive(
                "the quick brown fox jumps over the lazy dog and this is a perfectly normal sentence"
            ),
            None
        );
        assert_eq!(
            detect_sensitive(
                "please review the documentation for authentication bearer tokens usage"
            ),
            None
        );
    }

    #[test]
    fn short_and_empty_text_are_noops() {
        assert_eq!(detect_sensitive(""), None);
        assert_eq!(detect_sensitive("41111111"), None); // 太短
    }

    #[test]
    fn multibyte_text_never_panics_id_window() {
        // 回归锁（轮次116）：has_id_number 曾对 18 字符窗口做 `s[..17]`
        // **字节**切片——多字节字符（中文）落在字节 17 边界内时直接 panic。
        // 剪贴板监听线程每轮都会喂中文文本，该 panic 曾被 catch_tick 吞成
        // 「本轮捕获丢失」：内容永远不进历史（且默认开启，普通中文句子即可触发）。
        let zh = "中文中文中文中文，测试一下剪贴板敏感检测路径";
        assert_eq!(detect_sensitive(zh), None);
        // 多字节前缀 + 合法身份证：窗口滑动后仍要能命中（不因前缀失效）
        assert_eq!(
            detect_sensitive("示例文本11010519491231002X结尾"),
            Some("身份证号")
        );
    }

    #[test]
    fn byte_scan_treats_non_ascii_as_separator() {
        // 按字节扫描的等价性锁：非 ASCII 的 UTF-8 字节必须断开数字串。
        // 若被当成延续，下面两段 8 位会拼成 16 位卡号而**误命中**。
        assert_eq!(detect_sensitive("4111111111111111"), Some("信用卡号"));
        assert_eq!(detect_sensitive("41111111中11111111"), None);
    }

    #[test]
    fn large_text_scan_still_detects_at_tail() {
        // 热路径回归：整串不再物化为 Vec<char>（复制 1MB 文本曾要两次百万级
        // 分配），但语义不变——20 万字节文本尾部的卡号仍必须命中。
        let mut big = String::with_capacity(260_000);
        big.push_str(&"填充文本 ".repeat(20_000));
        big.push_str("4111 1111 1111 1111");
        assert!(big.len() >= 200_000, "构造的输入应达到 200KB 量级");
        assert_eq!(detect_sensitive(&big), Some("信用卡号"));
    }
}
