//! 网络质量日报聚合 + 多粒度流量查询（纯逻辑，无 Win32/Qt）。
//!
//! 数据来源与语义（与 Python 侧 netstats.py 逐条对齐）：
//! - events：状态切换记录 `[{ts: "%Y-%m-%d %H:%M:%S", state, detail}]`，最新在前；
//!   相邻「非 ok → ok」事件对即一次断网，时长为两 ts 差。
//! - traffic：`{"YYYY-MM-DD": {"sent","recv"}}`；应用未运行的日子无键
//!   （不插值，显示层显示「—」）。
//!
//! 边界约定：
//! - 断网时长归属「断网开始日」（跨零点整段计入起始日）；
//! - 首条事件无前驱、畸形/不可解析 ts：跳过该段时长（次数照记）；
//! - `state != "ok"` 一律视为断网态（ncsi_false/offline 均如此）；
//! - **同一事故多次状态互切只计一次**：no_net/offline 互切是同一次故障
//!   （自动修复拍打网卡会让两态交替），原实现各自 +1 且时长只计最后一段，
//!   会把一次断网计成多次 / 短计。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::rate_stats::{Bucket, StatsSnapshot, WifiUsage};
use crate::rate_store::{DayTraffic, EventItem};
use crate::timefmt;

/// 断网判定：非 "ok" 一律为断网态。
pub const ST_OK: &str = "ok";

/// 日报单行。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DayReport {
    pub date: String,
    pub outages: i64,
    pub outage_sec: i64,
    pub sent: i64,
    pub recv: i64,
    /// 该日是否有真实事件记录（区分「0 次」与「无记录」）。
    pub has_events: bool,
    /// 该日是否有真实流量记录。
    pub has_traffic: bool,
}

/// 最近 `days` 天（含今天）的日粒度统计，旧 → 新排序。
pub fn daily_report(
    events: &[EventItem],
    days: i64,
    traffic: &BTreeMap<String, DayTraffic>,
) -> Vec<DayReport> {
    let days = days.max(1);
    let base = timefmt::today_midnight();
    let mut out = Vec::new();
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    for offset in (0..days).rev() {
        let key = timefmt::day_key_offset(&base, offset);
        index.insert(key.clone(), out.len());
        out.push(DayReport {
            date: key,
            outages: 0,
            outage_sec: 0,
            sent: 0,
            recv: 0,
            has_events: false,
            has_traffic: false,
        });
    }

    // 流量列
    for (key, day) in traffic {
        if let Some(&i) = index.get(key) {
            out[i].sent = day.sent.max(0);
            out[i].recv = day.recv.max(0);
            out[i].has_traffic = true;
        }
    }

    // 事件最新在前 → 反转成时间正序，以「事故」为单位维护断网段
    let mut chrono: Vec<(chrono::NaiveDateTime, String)> = Vec::new();
    for ev in events.iter().rev() {
        if let Some(st) = timefmt::parse_ts(&ev.ts) {
            chrono.push((st, ev.state.clone()));
        }
        // 畸形 ts 直接跳过（不计时长；次数也不计，与 Python continue 一致）
    }

    let mut prev_down = false;
    let mut down_start: Option<chrono::NaiveDateTime> = None;
    for (st, state) in chrono {
        let key = st.format(timefmt::FMT_DAY).to_string();
        let day = index.get(&key).copied();
        let now_down = state != ST_OK;
        if now_down {
            if !prev_down {
                down_start = Some(st); // 本次事故开始（含超窗前驱）
                if let Some(i) = day {
                    out[i].outages += 1;
                }
            }
            if let Some(i) = day {
                out[i].has_events = true;
            }
        } else {
            if let Some(i) = day {
                out[i].has_events = true;
            }
            if prev_down {
                if let Some(start) = down_start {
                    // 整段时长归属断网开始日
                    let start_key = start.format(timefmt::FMT_DAY).to_string();
                    let sec = (st - start).num_seconds().max(0);
                    if let Some(&i) = index.get(&start_key) {
                        out[i].outage_sec += sec;
                    }
                }
            }
            down_start = None;
        }
        prev_down = now_down;
    }
    out
}

/// 各粒度的统计窗口（分钟 60 / 小时 48 / 天 30 / 月 12）。
pub fn granularity_window(granularity: &str) -> Option<usize> {
    match granularity {
        "minute" => Some(60),
        "hour" => Some(48),
        "day" => Some(30),
        "month" => Some(12),
        _ => None,
    }
}

/// 快照中出现过的网络名（含「以太网/未知」），名称排序。
pub fn known_ssids(stats: &StatsSnapshot) -> Vec<String> {
    let mut names: BTreeSet<String> = BTreeSet::new();
    for table in [&stats.minutes, &stats.hours, &stats.days] {
        for rec in table.values() {
            names.extend(rec.wifi.keys().cloned());
        }
    }
    names.into_iter().collect()
}

/// 多粒度序列的一行。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SeriesRow {
    pub key: String,
    pub sent: i64,
    pub recv: i64,
}

/// 粒度区间序列（时间正序）+ 区间总量。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Series {
    pub rows: Vec<SeriesRow>,
    pub sent: i64,
    pub recv: i64,
}

/// 取某桶的 (sent, recv)：`ssid` 为 None 取总量，否则只取该网络。
///
/// 网络不存在时返回 (0, 0)——**该行仍出现**（与 Python 同：柱子显示为 0 而不是缺行）。
fn pick(rec: &Bucket, ssid: Option<&str>) -> (i64, i64) {
    match ssid {
        None => (rec.sent.max(0), rec.recv.max(0)),
        Some(name) => rec
            .wifi
            .get(name)
            .map(|w: &WifiUsage| (w.sent.max(0), w.recv.max(0)))
            .unwrap_or((0, 0)),
    }
}

/// 粒度区间序列。`granularity`：minute / hour / day / month（月由天桶派生）。
///
/// 未知粒度返回 `Err`（对应 Python 的 `ValueError`）。
pub fn range_series(
    granularity: &str,
    stats: &StatsSnapshot,
    ssid: Option<&str>,
) -> Result<Series, String> {
    let win = granularity_window(granularity).ok_or_else(|| format!("未知粒度: {granularity}"))?;
    let table: &BTreeMap<String, Bucket> = match granularity {
        "minute" => &stats.minutes,
        "hour" => &stats.hours,
        _ => &stats.days, // day / month
    };

    let rows: Vec<SeriesRow> = if granularity == "month" {
        let mut grouped: BTreeMap<String, (i64, i64)> = BTreeMap::new();
        for (key, rec) in table {
            let (s, r) = pick(rec, ssid);
            let agg = grouped.entry(timefmt::month_prefix(key)).or_insert((0, 0));
            agg.0 += s;
            agg.1 += r;
        }
        let keys: Vec<String> = grouped.keys().cloned().collect();
        let start = keys.len().saturating_sub(win);
        keys[start..]
            .iter()
            .map(|k| {
                let (s, r) = grouped[k];
                SeriesRow {
                    key: k.clone(),
                    sent: s,
                    recv: r,
                }
            })
            .collect()
    } else {
        let keys: Vec<String> = table.keys().cloned().collect();
        let start = keys.len().saturating_sub(win);
        keys[start..]
            .iter()
            .map(|k| {
                let (s, r) = pick(&table[k], ssid);
                SeriesRow {
                    key: label_for(granularity, k),
                    sent: s,
                    recv: r,
                }
            })
            .collect()
    };

    let sent = rows.iter().map(|r| r.sent).sum();
    let recv = rows.iter().map(|r| r.recv).sum();
    Ok(Series { rows, sent, recv })
}

/// 柱标签：分钟取 "HH:MM"、小时取 "HH时"、天取 "MM-DD"（月已含年月）。
fn label_for(granularity: &str, key: &str) -> String {
    match granularity {
        "minute" => timefmt::minute_label(key),
        "hour" => timefmt::hour_label(key),
        _ => timefmt::day_label(key),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rate_stats::Bucket;

    fn ev(ts: &str, state: &str) -> EventItem {
        EventItem {
            ts: ts.to_string(),
            state: state.to_string(),
            detail: String::new(),
        }
    }

    fn bucket(sent: i64, recv: i64, wifi: &[(&str, i64, i64)]) -> Bucket {
        Bucket {
            sent,
            recv,
            wifi: wifi
                .iter()
                .map(|(n, s, r)| (n.to_string(), WifiUsage { sent: *s, recv: *r }))
                .collect(),
        }
    }

    fn snapshot(
        minutes: &[(&str, Bucket)],
        hours: &[(&str, Bucket)],
        days: &[(&str, Bucket)],
    ) -> StatsSnapshot {
        StatsSnapshot {
            minutes: minutes
                .iter()
                .map(|(k, b)| (k.to_string(), b.clone()))
                .collect(),
            hours: hours
                .iter()
                .map(|(k, b)| (k.to_string(), b.clone()))
                .collect(),
            days: days
                .iter()
                .map(|(k, b)| (k.to_string(), b.clone()))
                .collect(),
        }
    }

    // ---------------- 日报 ----------------

    #[test]
    fn report_window_is_oldest_to_newest_and_covers_today() {
        let r = daily_report(&[], 7, &BTreeMap::new());
        assert_eq!(r.len(), 7);
        let base = timefmt::today_midnight();
        assert_eq!(r[6].date, timefmt::today(), "最后一行必须是今天");
        assert_eq!(
            r[0].date,
            timefmt::day_key_offset(&base, 6),
            "首行是 6 天前"
        );
        for row in &r {
            assert!(!row.has_events);
            assert!(!row.has_traffic);
            assert_eq!((row.outages, row.outage_sec), (0, 0));
        }
    }

    #[test]
    fn traffic_join_marks_presence_and_keeps_absent_days_absent() {
        let today = timefmt::today();
        let mut traffic = BTreeMap::new();
        traffic.insert(
            today.clone(),
            DayTraffic {
                sent: 1000,
                recv: 2000,
            },
        );
        let r = daily_report(&[], 3, &traffic);
        assert!(r[2].has_traffic, "今天有流量记录");
        assert_eq!((r[2].sent, r[2].recv), (1000, 2000));
        assert!(!r[0].has_traffic, "未运行的日子不插值");
        assert_eq!((r[0].sent, r[0].recv), (0, 0));
    }

    #[test]
    fn negative_traffic_is_clamped_to_zero() {
        let mut traffic = BTreeMap::new();
        traffic.insert(timefmt::today(), DayTraffic { sent: -5, recv: -9 });
        let r = daily_report(&[], 1, &traffic);
        assert_eq!((r[0].sent, r[0].recv), (0, 0));
        assert!(r[0].has_traffic, "键存在即算有记录");
    }

    #[test]
    fn outage_pair_counts_one_and_attributes_duration_to_start_day() {
        // 今天 10:00 断网 → 10:02 恢复 = 1 次、120 秒
        let today = timefmt::today();
        let events = vec![
            ev(&format!("{today} 10:02:00"), "ok"),
            ev(&format!("{today} 10:00:00"), "no_net"),
            ev(&format!("{today} 09:00:00"), "ok"),
        ];
        let r = daily_report(&events, 3, &BTreeMap::new());
        let row = r.iter().find(|x| x.date == today).unwrap();
        assert_eq!(row.outages, 1);
        assert_eq!(row.outage_sec, 120);
        assert!(row.has_events);
    }

    #[test]
    fn state_flapping_within_one_incident_counts_once() {
        // no_net ↔ offline 互切是**同一次**事故：只计 1 次，时长整段
        let today = timefmt::today();
        let events = vec![
            ev(&format!("{today} 10:05:00"), "ok"),
            ev(&format!("{today} 10:04:00"), "offline"),
            ev(&format!("{today} 10:02:00"), "no_net"),
            ev(&format!("{today} 10:00:00"), "no_net"),
            ev(&format!("{today} 09:00:00"), "ok"),
        ];
        let r = daily_report(&events, 2, &BTreeMap::new());
        let row = r.iter().find(|x| x.date == today).unwrap();
        assert_eq!(row.outages, 1, "同一事故多次互切只算一次断网");
        assert_eq!(row.outage_sec, 300, "时长是整段 10:00→10:05");
    }

    #[test]
    fn outage_spanning_midnight_is_credited_to_start_day() {
        let today = timefmt::today();
        let base = timefmt::today_midnight();
        let yesterday = timefmt::day_key_offset(&base, 1);
        // 昨天 23:58 断网 → 今天 00:03 恢复 = 300 秒，全部归昨天
        let events = vec![
            ev(&format!("{today} 00:03:00"), "ok"),
            ev(&format!("{yesterday} 23:58:00"), "no_net"),
        ];
        let r = daily_report(&events, 3, &BTreeMap::new());
        let y = r.iter().find(|x| x.date == yesterday).unwrap();
        let t = r.iter().find(|x| x.date == today).unwrap();
        assert_eq!(y.outages, 1);
        assert_eq!(y.outage_sec, 300, "跨零点整段计入起始日");
        assert_eq!(t.outages, 0, "恢复日不计次数");
        assert!(t.has_events, "恢复日仍有事件痕迹");
    }

    #[test]
    fn malformed_ts_is_skipped() {
        let today = timefmt::today();
        let events = vec![
            ev("不是时间", "ok"),
            ev(&format!("{today} 10:01:00"), "ok"),
            ev("2026-13-99 99:99:99", "no_net"),
            ev(&format!("{today} 10:00:00"), "no_net"),
        ];
        let r = daily_report(&events, 2, &BTreeMap::new());
        let row = r.iter().find(|x| x.date == today).unwrap();
        // 只有 10:00(no_net) → 10:01(ok) 这一对是合法配对
        assert_eq!(row.outages, 1);
        assert_eq!(row.outage_sec, 60);
    }

    #[test]
    fn still_down_at_window_end_scores_no_duration() {
        let today = timefmt::today();
        let events = vec![ev(&format!("{today} 10:00:00"), "no_net")];
        let r = daily_report(&events, 2, &BTreeMap::new());
        let row = r.iter().find(|x| x.date == today).unwrap();
        assert_eq!(row.outages, 1, "已开始但未恢复：次数照记");
        assert_eq!(row.outage_sec, 0, "未恢复则无时长（首条/尾条无配对）");
    }

    #[test]
    fn events_outside_window_still_pair_but_do_not_count() {
        // 断网开始于窗口外（30 天前），恢复在窗口内：时长归窗口外的起始日 → 不计数
        let today = timefmt::today();
        let base = timefmt::today_midnight();
        let old = timefmt::day_key_offset(&base, 40);
        let events = vec![
            ev(&format!("{today} 08:00:00"), "ok"),
            ev(&format!("{old} 08:00:00"), "no_net"),
        ];
        let r = daily_report(&events, 7, &BTreeMap::new());
        assert!(r.iter().all(|x| x.outages == 0), "起始日超窗 → 不计数");
        assert!(
            r.iter().all(|x| x.outage_sec == 0),
            "时长归超窗的起始日 → 丢在窗外"
        );
        // 恢复日仍在窗口内：该日算「有事件」
        assert!(r.iter().find(|x| x.date == today).unwrap().has_events);
    }

    #[test]
    fn empty_events_is_safe() {
        let r = daily_report(&[], 1, &BTreeMap::new());
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].outages, 0);
    }

    // ---------------- 多粒度序列 ----------------

    /// 期望值取自 Python 侧 `netstats.range_series` 的实跑输出（逐字比对）。
    fn fixture() -> StatsSnapshot {
        snapshot(
            &[
                ("2026-01-01T10:05", bucket(10, 100, &[("A", 1, 2)])),
                (
                    "2026-01-01T10:07",
                    bucket(20, 200, &[("A", 3, 4), ("B", 5, 6)]),
                ),
                ("2026-01-01T11:00", bucket(7, 70, &[(UNKNOWN, 7, 70)])),
            ],
            &[("2026-01-01T09", bucket(1000, 2000, &[("A", 100, 200)]))],
            &[
                ("2025-12-31", bucket(1, 2, &[])),
                ("2026-01-01", bucket(10, 20, &[("A", 5, 5)])),
                ("2026-01-02", bucket(100, 200, &[("A", 50, 50)])),
            ],
        )
    }

    const UNKNOWN: &str = crate::rate_stats::UNKNOWN_SSID;

    #[test]
    fn minute_series_matches_python() {
        let s = range_series("minute", &fixture(), None).unwrap();
        assert_eq!(
            s.rows,
            vec![
                SeriesRow {
                    key: "10:05".into(),
                    sent: 10,
                    recv: 100
                },
                SeriesRow {
                    key: "10:07".into(),
                    sent: 20,
                    recv: 200
                },
                SeriesRow {
                    key: "11:00".into(),
                    sent: 7,
                    recv: 70
                },
            ]
        );
        assert_eq!((s.sent, s.recv), (37, 370));
    }

    #[test]
    fn minute_series_filtered_by_ssid_matches_python() {
        let s = range_series("minute", &fixture(), Some("A")).unwrap();
        assert_eq!(
            s.rows,
            vec![
                SeriesRow {
                    key: "10:05".into(),
                    sent: 1,
                    recv: 2
                },
                SeriesRow {
                    key: "10:07".into(),
                    sent: 3,
                    recv: 4
                },
                // 该网络在这一分钟没有流量：仍然出行，值为 0（不删行）
                SeriesRow {
                    key: "11:00".into(),
                    sent: 0,
                    recv: 0
                },
            ]
        );
        assert_eq!((s.sent, s.recv), (4, 6));
    }

    #[test]
    fn hour_series_matches_python_including_label() {
        let s = range_series("hour", &fixture(), None).unwrap();
        assert_eq!(s.rows.len(), 1);
        assert_eq!(
            s.rows[0].key, "09时",
            "小时标签是 `HH时`（Python 逐字一致）"
        );
        assert_eq!((s.rows[0].sent, s.rows[0].recv), (1000, 2000));
        let a = range_series("hour", &fixture(), Some("A")).unwrap();
        assert_eq!((a.rows[0].sent, a.rows[0].recv), (100, 200));
    }

    #[test]
    fn day_series_matches_python() {
        let s = range_series("day", &fixture(), None).unwrap();
        assert_eq!(
            s.rows.iter().map(|r| r.key.clone()).collect::<Vec<_>>(),
            vec!["12-31", "01-01", "01-02"]
        );
        assert_eq!((s.sent, s.recv), (111, 222));
        let a = range_series("day", &fixture(), Some("A")).unwrap();
        assert_eq!((a.rows[0].sent, a.rows[0].recv), (0, 0), "12-31 无 A 桶");
        assert_eq!((a.sent, a.recv), (55, 55));
    }

    #[test]
    fn month_series_derives_from_day_buckets() {
        let s = range_series("month", &fixture(), None).unwrap();
        assert_eq!(
            s.rows,
            vec![
                SeriesRow {
                    key: "2025-12".into(),
                    sent: 1,
                    recv: 2
                },
                SeriesRow {
                    key: "2026-01".into(),
                    sent: 110,
                    recv: 220
                },
            ]
        );
        assert_eq!((s.sent, s.recv), (111, 222), "月总量 = 各日之和");
        let a = range_series("month", &fixture(), Some("A")).unwrap();
        assert_eq!((a.sent, a.recv), (55, 55));
    }

    #[test]
    fn unknown_granularity_is_an_error() {
        assert!(range_series("week", &fixture(), None).is_err());
        assert!(range_series("", &fixture(), None).is_err());
        assert_eq!(granularity_window("minute"), Some(60));
        assert_eq!(granularity_window("month"), Some(12));
    }

    #[test]
    fn series_window_truncates_to_last_n_keys() {
        let mut days: BTreeMap<String, Bucket> = BTreeMap::new();
        for i in 1..=40 {
            days.insert(
                format!("2026-02-{i:02}"),
                Bucket {
                    sent: 1,
                    recv: 1,
                    wifi: BTreeMap::new(),
                },
            );
        }
        let snap = StatsSnapshot {
            minutes: BTreeMap::new(),
            hours: BTreeMap::new(),
            days,
        };
        let s = range_series("day", &snap, None).unwrap();
        assert_eq!(s.rows.len(), 30, "天粒度窗口 30");
        assert_eq!(s.rows[29].key, "02-40", "取最后 30 个键（最新在末尾）");
        assert_eq!(s.sent, 30);
    }

    #[test]
    fn known_ssids_is_sorted_and_spans_all_tables() {
        let names = known_ssids(&fixture());
        assert_eq!(
            names,
            vec!["A".to_string(), "B".to_string(), UNKNOWN.to_string()]
        );
        assert!(known_ssids(&StatsSnapshot::default()).is_empty());
    }

    #[test]
    fn month_label_does_not_get_day_suffix() {
        // 月粒度键必须保持 YYYY-MM，不得被天标签截成 MM-DD
        let s = range_series("month", &fixture(), None).unwrap();
        assert!(s.rows.iter().all(|r| r.key.len() == 7), "月键 YYYY-MM");
    }
}
