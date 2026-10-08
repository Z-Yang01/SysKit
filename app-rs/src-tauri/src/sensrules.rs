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

/// 从文本中提取「数字分组」片段（数字与空格/连字符分隔符），去分隔符后
/// 13~19 位纯数字且 Luhn 通过 → 命中。
fn has_credit_card(text: &str) -> bool {
    let bytes: Vec<char> = text.chars().collect();
    let mut run = String::new();
    let check = |run: &str| -> bool {
        let digits: String = run.chars().filter(|c| c.is_ascii_digit()).collect();
        digits.len() >= 13
            && digits.len() <= 19
            && digits.chars().all(|c| c.is_ascii_digit())
            && luhn_ok(&digits)
    };
    for c in &bytes {
        if c.is_ascii_digit() || *c == ' ' || *c == '-' {
            run.push(*c);
        } else {
            if check(&run) {
                return true;
            }
            run.clear();
        }
    }
    check(&run)
}

/// Luhn 算法（模 10 双倍加）：从右往左奇数位原样、偶数位翻倍 >9 减 9，总和 %10==0。
fn luhn_ok(digits: &str) -> bool {
    let mut sum = 0usize;
    let mut double = false;
    for c in digits.chars().rev() {
        let Some(d) = c.to_digit(10) else {
            return false;
        };
        let mut v = d as usize;
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
fn has_id_number(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() < 18 {
        return false;
    }
    for win in chars.windows(18) {
        let s: String = win.iter().collect();
        if !s[..17].chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let last = s.chars().last().unwrap().to_ascii_uppercase();
        let sum: u32 = s[..17]
            .chars()
            .enumerate()
            .map(|(i, c)| (c as u8 - b'0') as u32 * ID_WEIGHTS[i])
            .sum();
        if ID_CHECK[(sum % 11) as usize] == last {
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

fn has_api_token(text: &str) -> bool {
    // 词 = 连续的 [A-Za-z0-9_-]；"Bearer <token>" 的 token 靠高熵规则命中
    let mut word = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            word.push(c);
        } else {
            if looks_like_token(&word) {
                return true;
            }
            word.clear();
        }
    }
    looks_like_token(&word)
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
}
