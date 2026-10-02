//! 本地时间格式化：**逐格式串对齐 Python 侧 `time.strftime`**。
//!
//! 为什么单独成模块：两版共用同一批 JSON 文件，而桶键/事件 ts 都是本地时间
//! 字符串键（`%Y-%m-%d`、`%Y-%m-%dT%H:%M`、`%Y-%m-%dT%H`、`%Y-%m-%d %H:%M:%S`）。
//! 键格式一旦漂移，同一份文件在两版眼里就成了两套互不相认的数据。
//! 全部格式串集中在此，改一处即可全局对齐。

use chrono::{DateTime, Datelike, Duration, Local, NaiveDateTime, TimeZone, Timelike};

/// `%Y-%m-%d` —— traffic.json 的日键、netstats 的 days 桶键。
pub const FMT_DAY: &str = "%Y-%m-%d";
/// `%Y-%m-%dT%H:%M` —— netstats 的 minutes 桶键。
pub const FMT_MINUTE: &str = "%Y-%m-%dT%H:%M";
/// `%Y-%m-%dT%H` —— netstats 的 hours 桶键。
pub const FMT_HOUR: &str = "%Y-%m-%dT%H";
/// `%Y-%m-%d %H:%M:%S` —— events.json 的 ts。
pub const FMT_TS: &str = "%Y-%m-%d %H:%M:%S";

pub fn now_local() -> DateTime<Local> {
    Local::now()
}

pub fn fmt(dt: &DateTime<Local>, fmt: &str) -> String {
    dt.format(fmt).to_string()
}

/// 当前本地时间的指定格式。
pub fn now_fmt(fmt_str: &str) -> String {
    fmt(&now_local(), fmt_str)
}

/// 今天（`%Y-%m-%d`）。
pub fn today() -> String {
    now_fmt(FMT_DAY)
}

/// 解析 `%Y-%m-%d %H:%M:%S`（事件 ts）；畸形返回 None（对应 Python `_parse_ts`）。
pub fn parse_ts(text: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(text, FMT_TS).ok()
}

/// 把本地 naive 时间转成「由该本地日历时刻确定的 DateTime<Local>」。
///
/// 夏令时歧义（本地时间重复/缺失）时取最早的那个候选，与 Python
/// `time.mktime` 取当前生效偏移的行为在实用范围内一致；中国无夏令时。
pub fn local_from_naive(naive: NaiveDateTime) -> Option<DateTime<Local>> {
    match Local.from_local_datetime(&naive) {
        chrono::LocalResult::Single(dt) => Some(dt),
        chrono::LocalResult::Ambiguous(a, _) => Some(a),
        chrono::LocalResult::None => None,
    }
}

/// 本地当天零点。
pub fn today_midnight() -> DateTime<Local> {
    let now = now_local();
    let naive = now
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap_or_else(|| now.naive_local());
    local_from_naive(naive).unwrap_or(now)
}

/// 本地当天零点往前 `offset` 天的日键（日报窗口用）。
pub fn day_key_offset(base_midnight: &DateTime<Local>, offset: i64) -> String {
    let d = *base_midnight - Duration::days(offset);
    fmt(&d, FMT_DAY)
}

/// 取 `%Y-%m-%d` 日键的年月前缀（月份桶由天桶派生）。
pub fn month_prefix(day_key: &str) -> String {
    day_key.chars().take(7).collect()
}

/// 天键的 `MM-DD` 标签（天粒度柱标签）。
pub fn day_label(day_key: &str) -> String {
    day_key.chars().skip(5).collect()
}

/// 分钟桶键的 `HH:MM` 标签。
pub fn minute_label(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() < 5 {
        return key.to_string();
    }
    chars[chars.len() - 5..].iter().collect()
}

/// 小时桶键的 `HH时` 标签（Python: `key[11:] + "时"`）。
pub fn hour_label(key: &str) -> String {
    let tail: String = key.chars().skip(11).collect();
    format!("{tail}时")
}

/// 小时桶键的前缀（分钟桶键去掉 `:MM`，即 `key[:-3]`）。
pub fn hour_of_minute_key(minute_key: &str) -> String {
    let chars: Vec<char> = minute_key.chars().collect();
    if chars.len() <= 3 {
        return String::new();
    }
    chars[..chars.len() - 3].iter().collect()
}

/// 本地日历日序号（用于按天归组比较）。
pub fn day_ordinal(dt: &DateTime<Local>) -> i32 {
    dt.date_naive().num_days_from_ce()
}

/// 本地年份与月份（月份键派生用）。
pub fn year_month(dt: &DateTime<Local>) -> (i32, u32) {
    (dt.year(), dt.month())
}

/// 本地时分秒（无其他用途，供测试断言可读性）。
pub fn hms(dt: &DateTime<Local>) -> (u32, u32, u32) {
    (dt.hour(), dt.minute(), dt.second())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_constants_match_python_strftime() {
        assert_eq!(FMT_DAY, "%Y-%m-%d");
        assert_eq!(FMT_MINUTE, "%Y-%m-%dT%H:%M");
        assert_eq!(FMT_HOUR, "%Y-%m-%dT%H");
        assert_eq!(FMT_TS, "%Y-%m-%d %H:%M:%S");
    }

    #[test]
    fn today_has_expected_shape() {
        let t = today();
        assert_eq!(t.len(), 10, "YYYY-MM-DD 共 10 字符: {t}");
        assert_eq!(t.matches('-').count(), 2);
        // 年份必须与本地时间一致（不是 UTC 年——跨零点的时区差异会写错日键）
        assert!(t.starts_with(&now_local().year().to_string()));
    }

    #[test]
    fn now_fmt_shapes() {
        let m = now_fmt(FMT_MINUTE);
        assert_eq!(m.len(), 16, "YYYY-MM-DDTHH:MM: {m}");
        assert_eq!(
            m.chars().nth(10),
            Some('T'),
            "分钟键必须以 T 分隔（Python 同）"
        );
        let h = now_fmt(FMT_HOUR);
        assert_eq!(h.len(), 13, "YYYY-MM-DDTHH: {h}");
        let ts = now_fmt(FMT_TS);
        assert_eq!(ts.len(), 19, "YYYY-MM-DD HH:MM:SS: {ts}");
        assert_eq!(
            ts.chars().nth(10),
            Some(' '),
            "事件 ts 用空格分隔（Python 同）"
        );
    }

    #[test]
    fn parse_ts_round_trips_and_rejects_garbage() {
        let ts = now_fmt(FMT_TS);
        assert!(parse_ts(&ts).is_some(), "自家格式必须能解析回: {ts}");
        assert!(parse_ts("").is_none());
        assert!(parse_ts("2026-01-01").is_none(), "缺时分秒必须判畸形");
        assert!(parse_ts("2026-13-45 99:99:99").is_none());
        assert!(parse_ts("not a time").is_none());
    }

    #[test]
    fn labels_match_python_slicing() {
        assert_eq!(minute_label("2026-01-01T10:05"), "10:05");
        assert_eq!(hour_label("2026-01-01T09"), "09时");
        assert_eq!(day_label("2026-01-02"), "01-02");
        assert_eq!(month_prefix("2026-01-02"), "2026-01");
        assert_eq!(hour_of_minute_key("2026-01-01T10:05"), "2026-01-01T10");
        assert_eq!(hour_of_minute_key("2026-01-01T10:00"), "2026-01-01T10");
    }

    #[test]
    fn hour_of_minute_key_is_robust() {
        assert_eq!(hour_of_minute_key(""), "");
        assert_eq!(hour_of_minute_key("ab"), "");
        assert_eq!(hour_of_minute_key("abcd"), "a", "长度退化时不得 panic");
    }

    #[test]
    fn day_key_offset_walks_backwards() {
        let base = today_midnight();
        assert_eq!(day_key_offset(&base, 0), fmt(&base, FMT_DAY));
        let yesterday = day_key_offset(&base, 1);
        assert_ne!(yesterday, day_key_offset(&base, 0), "N 天前必须早于今天");
        // 自洽：今天零点减一天 = 昨天的日键
        let back: DateTime<Local> = base - Duration::days(1);
        assert_eq!(yesterday, fmt(&back, FMT_DAY));
    }

    #[test]
    fn day_ordinal_is_monotonic() {
        let base = today_midnight();
        let a = day_ordinal(&base);
        let b = day_ordinal(&(base - Duration::days(1)));
        assert_eq!(a - b, 1, "相邻两天序号差 1");
    }
}
