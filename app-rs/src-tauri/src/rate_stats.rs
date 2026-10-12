//! 多粒度流量统计：分钟/小时桶 + 按天分 WiFi 累计（`netstats_buckets.json`）。
//!
//! 文件形态与 Python 侧 `RateStatsStore` 逐字段对齐：
//! ```text
//! {"minutes": {"YYYY-MM-DDTHH:MM": {"sent","recv","wifi":{ssid:{"sent","recv"}}}},
//!  "hours":   {"YYYY-MM-DDTHH":    ...同形...},
//!  "days":    {"YYYY-MM-DD":       ...同形...}}
//! ```
//! 保留：分钟 48h、小时 60d、天 365d，各自剪枝（键是 ISO 序 = 时间序）。
//!
//! 两个必须照搬的细节（照抄 Python 的注释，因为都是踩过的坑）：
//! 1. **整点归并**：整点后的首次 add 把上一小时的分钟桶归并进小时桶（免定时器）。
//!    且**每次 add 都扫一遍**，不能只在小时变化时滚一次——跨整点无流量 + 新小时
//!    冷启动时，盘上遗留的旧分钟桶会永不归并。
//! 2. **未知网络名**：SSID 取不到时统一归入 "以太网/未知"，绝不丢数据。

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::jsonio;
use crate::paths;
use crate::timefmt;

/// 未知/非 WiFi 网络的统一归属名（与 Python `RateStatsStore.UNKNOWN` 一致）。
pub const UNKNOWN_SSID: &str = "以太网/未知";

pub const MINUTE_RETAIN: usize = 48 * 60;
pub const HOUR_RETAIN: usize = 60 * 24;
pub const DAY_RETAIN: usize = 365;

/// 单网络（SSID）用量。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WifiUsage {
    #[serde(default)]
    pub sent: i64,
    #[serde(default)]
    pub recv: i64,
}

/// 一个时间桶：总上下行 + 按网络拆分。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Bucket {
    #[serde(default)]
    pub sent: i64,
    #[serde(default)]
    pub recv: i64,
    #[serde(default)]
    pub wifi: BTreeMap<String, WifiUsage>,
}

/// 三表快照（UI 查询用）。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct StatsSnapshot {
    #[serde(default)]
    pub minutes: BTreeMap<String, Bucket>,
    #[serde(default)]
    pub hours: BTreeMap<String, Bucket>,
    #[serde(default)]
    pub days: BTreeMap<String, Bucket>,
}

/// 多粒度流量桶存储（脏标记 + 定期落盘）。
pub struct RateStatsStore {
    path: PathBuf,
    minutes: BTreeMap<String, Bucket>,
    hours: BTreeMap<String, Bucket>,
    days: BTreeMap<String, Bucket>,
    /// 最近已知网络名（netsh 刷新周期之外沿用）。
    ssid: String,
    dirty: bool,
}

impl Default for RateStatsStore {
    fn default() -> Self {
        Self::new()
    }
}

impl RateStatsStore {
    pub fn new() -> Self {
        Self::with_path(paths::netstats_file())
    }

    pub fn with_path(path: PathBuf) -> Self {
        let raw = jsonio::load_json(&path, serde_json::json!({}));
        let snap = parse_snapshot(&raw);
        Self {
            path,
            minutes: snap.minutes,
            hours: snap.hours,
            days: snap.days,
            ssid: String::new(),
            dirty: false,
        }
    }

    /// 记录当前网络名（telemetry 的 netsh 刷新周期调用；空串 = 未知）。
    pub fn set_ssid(&mut self, ssid: &str) {
        self.ssid = ssid.trim().to_string();
    }

    /// 规范化网络名：空 → "以太网/未知"（与 Python `_norm_ssid` 一致）。
    pub fn norm_ssid(ssid: &str) -> String {
        let s = ssid.trim();
        if s.is_empty() {
            UNKNOWN_SSID.to_string()
        } else {
            s.to_string()
        }
    }

    /// 累计一次采样增量（字节）到分钟桶/天桶，并按需滚动小时桶。
    ///
    /// `ssid` 为 None 时沿用最近一次 `set_ssid` 的值。
    pub fn add(&mut self, sent_delta: i64, recv_delta: i64, ssid: Option<&str>) {
        let ds = sent_delta.max(0);
        let dr = recv_delta.max(0);
        if ds == 0 && dr == 0 {
            return;
        }
        let name = Self::norm_ssid(ssid.unwrap_or(&self.ssid));
        let now = timefmt::now_local();
        let minute_key = timefmt::fmt(&now, timefmt::FMT_MINUTE);
        let day_key = timefmt::fmt(&now, timefmt::FMT_DAY);
        let hour_key = timefmt::fmt(&now, timefmt::FMT_HOUR);

        let mut wifi = BTreeMap::new();
        wifi.insert(name, WifiUsage { sent: ds, recv: dr });
        merge(&mut self.minutes, &minute_key, ds, dr, &wifi);
        merge(&mut self.days, &day_key, ds, dr, &wifi);
        // 每次都扫（分钟桶 ≤2881，微秒级）
        self.roll_hours(&hour_key);
        self.prune();
        self.dirty = true;
    }

    /// 把非当前小时的分钟桶归并进对应小时桶（键序 = 插入序，与 Python 一致）。
    fn roll_hours(&mut self, cur_hour: &str) {
        let stale: Vec<String> = self
            .minutes
            .keys()
            .filter(|k| timefmt::hour_of_minute_key(k) != cur_hour)
            .cloned()
            .collect();
        for k in stale {
            if let Some(rec) = self.minutes.remove(&k) {
                let hour_key = timefmt::hour_of_minute_key(&k);
                merge(&mut self.hours, &hour_key, rec.sent, rec.recv, &rec.wifi);
            }
        }
    }

    /// 各自按上限剪枝（键 ISO 序 = 时间序）。
    fn prune(&mut self) {
        prune_table(&mut self.minutes, MINUTE_RETAIN);
        prune_table(&mut self.hours, HOUR_RETAIN);
        prune_table(&mut self.days, DAY_RETAIN);
    }

    /// 三表深拷贝（UI 查询用）。
    pub fn snapshot(&self) -> StatsSnapshot {
        StatsSnapshot {
            minutes: self.minutes.clone(),
            hours: self.hours.clone(),
            days: self.days.clone(),
        }
    }

    /// 有变更才写盘；返回是否发生写盘。
    pub fn save_if_dirty(&mut self) -> bool {
        if !self.dirty {
            return false;
        }
        let payload = serde_json::json!({
            "minutes": self.minutes,
            "hours": self.hours,
            "days": self.days,
        });
        if jsonio::save_json(&self.path, &payload) {
            self.dirty = false;
            true
        } else {
            false
        }
    }

    #[cfg(test)]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }
}

/// 把一个增量并入指定桶（含按网络拆分）。
fn merge(
    table: &mut BTreeMap<String, Bucket>,
    key: &str,
    sent: i64,
    recv: i64,
    wifi: &BTreeMap<String, WifiUsage>,
) {
    let rec = table.entry(key.to_string()).or_default();
    rec.sent += sent;
    rec.recv += recv;
    for (name, w) in wifi {
        let dst = rec.wifi.entry(name.clone()).or_default();
        dst.sent += w.sent.max(0);
        dst.recv += w.recv.max(0);
    }
}

/// 超出上限时删最早的键。
fn prune_table(table: &mut BTreeMap<String, Bucket>, cap: usize) {
    while table.len() > cap {
        let Some(oldest) = table.keys().next().cloned() else {
            break;
        };
        table.remove(&oldest);
    }
}

fn parse_snapshot(raw: &serde_json::Value) -> StatsSnapshot {
    let mut snap = StatsSnapshot::default();
    let Some(obj) = raw.as_object() else {
        if !raw.is_null() {
            log::warn!("netstats_buckets.json 顶层非对象，多粒度统计已重置");
        }
        return snap;
    };
    for (field, target) in [
        ("minutes", &mut snap.minutes),
        ("hours", &mut snap.hours),
        ("days", &mut snap.days),
    ] {
        if let Some(t) = obj.get(field) {
            match serde_json::from_value::<BTreeMap<String, Bucket>>(t.clone()) {
                Ok(m) => *target = m,
                Err(_) => log::warn!("netstats_buckets.json 的 {field} 形态异常，已重置"),
            }
        }
    }
    snap
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("syskit_rs_netstats_{tag}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn bucket_serializes_with_python_field_names() {
        let mut wifi = BTreeMap::new();
        wifi.insert("A".to_string(), WifiUsage { sent: 1, recv: 2 });
        let b = Bucket {
            sent: 10,
            recv: 20,
            wifi,
        };
        assert_eq!(
            serde_json::to_string(&b).unwrap(),
            r#"{"sent":10,"recv":20,"wifi":{"A":{"sent":1,"recv":2}}}"#
        );
    }

    #[test]
    fn unknown_ssid_normalization() {
        assert_eq!(RateStatsStore::norm_ssid(""), UNKNOWN_SSID);
        assert_eq!(RateStatsStore::norm_ssid("   "), UNKNOWN_SSID);
        assert_eq!(RateStatsStore::norm_ssid(" MyWiFi "), "MyWiFi");
        assert_eq!(
            UNKNOWN_SSID, "以太网/未知",
            "归属名是跨版本硬契约（写进文件键）"
        );
    }

    #[test]
    fn add_skips_zero_and_marks_dirty() {
        let mut s = RateStatsStore::with_path(temp("zero").join("n.json"));
        assert!(!s.is_dirty());
        s.add(0, 0, None);
        assert!(!s.is_dirty(), "零增量不标脏");
        s.add(-3, -4, None);
        assert!(!s.is_dirty(), "负增量按 0 处理 → 仍不标脏");
        s.add(5, 6, None);
        assert!(s.is_dirty());
    }

    #[test]
    fn add_writes_minute_and_day_buckets_with_wifi() {
        let mut s = RateStatsStore::with_path(temp("add").join("n.json"));
        s.set_ssid("HomeWiFi");
        s.add(100, 200, None);
        let snap = s.snapshot();
        let mk = timefmt::now_fmt(timefmt::FMT_MINUTE);
        let dk = timefmt::today();
        assert_eq!(
            snap.minutes.get(&mk).map(|b| (b.sent, b.recv)),
            Some((100, 200))
        );
        assert_eq!(
            snap.days.get(&dk).map(|b| (b.sent, b.recv)),
            Some((100, 200))
        );
        // 网络名沿用 set_ssid
        assert_eq!(
            snap.minutes[&mk]
                .wifi
                .get("HomeWiFi")
                .map(|w| (w.sent, w.recv)),
            Some((100, 200))
        );
        // 显式 ssid 覆盖沿用值
        s.add(1, 2, Some("Other"));
        let snap2 = s.snapshot();
        assert!(snap2.minutes[&mk].wifi.contains_key("Other"));
        assert!(snap2.minutes[&mk].wifi.contains_key("HomeWiFi"));
    }

    #[test]
    fn empty_ssid_goes_to_unknown_bucket() {
        let mut s = RateStatsStore::with_path(temp("unknown").join("n.json"));
        s.add(7, 8, Some(""));
        let snap = s.snapshot();
        let mk = timefmt::now_fmt(timefmt::FMT_MINUTE);
        assert!(
            snap.minutes[&mk].wifi.contains_key(UNKNOWN_SSID),
            "取不到 SSID 时必须落到统一归属名，不能丢数据"
        );
    }

    #[test]
    fn roll_hours_merges_stale_minute_buckets() {
        let mut s = RateStatsStore::with_path(temp("roll").join("n.json"));
        // 直接注入两个「上一小时」的分钟桶（模拟盘上遗留 + 与新小时冷启动）
        s.minutes.insert(
            "2026-01-01T09:10".into(),
            Bucket {
                sent: 10,
                recv: 20,
                wifi: BTreeMap::from([("A".into(), WifiUsage { sent: 1, recv: 2 })]),
            },
        );
        s.minutes.insert(
            "2026-01-01T09:20".into(),
            Bucket {
                sent: 30,
                recv: 40,
                wifi: BTreeMap::from([("A".into(), WifiUsage { sent: 3, recv: 4 })]),
            },
        );
        // 当前小时与它们不同 → add 应把它们归并进 09 小时桶
        s.add(1, 1, Some("X"));
        let snap = s.snapshot();
        assert!(
            !snap.minutes.contains_key("2026-01-01T09:10"),
            "旧分钟桶必须被归并掉"
        );
        let h = snap.hours.get("2026-01-01T09").expect("应生成 09 小时桶");
        assert_eq!((h.sent, h.recv), (40, 60), "两个分钟桶累计进入小时桶");
        assert_eq!(
            h.wifi.get("A").map(|w| (w.sent, w.recv)),
            Some((4, 6)),
            "wifi 子映射必须一并归并"
        );
    }

    #[test]
    fn current_hour_minute_buckets_are_not_rolled() {
        let mut s = RateStatsStore::with_path(temp("noroll").join("n.json"));
        s.add(50, 60, Some("A"));
        let mk = timefmt::now_fmt(timefmt::FMT_MINUTE);
        let hk = timefmt::now_fmt(timefmt::FMT_HOUR);
        // 再 add 一次：当前小时的分钟桶必须留下，小时桶里不该出现它的影子
        s.add(1, 1, Some("A"));
        let snap = s.snapshot();
        assert!(snap.minutes.contains_key(&mk), "当前小时分钟桶不得被滚走");
        assert!(
            !snap.hours.contains_key(&hk),
            "当前小时还没结束，不得提前产生小时桶"
        );
    }

    #[test]
    fn prune_caps_each_table() {
        let path = temp("prune").join("n.json");
        let mut minutes = serde_json::Map::new();
        for i in 0..(MINUTE_RETAIN + 50) {
            let h = 10 + i / 60;
            minutes.insert(
                format!("2026-01-{:02}T{h:02}:{:02}", 1 + i / 1440, i % 60),
                serde_json::json!({"sent": 1, "recv": 1, "wifi": {}}),
            );
        }
        let days: serde_json::Map<String, serde_json::Value> = (0..(DAY_RETAIN + 10))
            .map(|i| {
                (
                    format!("20{:02}-01-01", 20 + i / 365),
                    serde_json::json!({"sent": 1, "recv": 1, "wifi": {}}),
                )
            })
            .collect();
        jsonio::save_json(
            &path,
            &serde_json::json!({
                "minutes": minutes,
                "hours": {},
                "days": days,
            }),
        );
        let mut s = RateStatsStore::with_path(path);
        s.add(1, 1, Some("A")); // 触发剪枝
        let snap = s.snapshot();
        assert!(
            snap.minutes.len() <= MINUTE_RETAIN,
            "分钟桶超限: {}",
            snap.minutes.len()
        );
        assert!(
            snap.days.len() <= DAY_RETAIN,
            "天桶超限: {}",
            snap.days.len()
        );
    }

    #[test]
    fn snapshot_is_a_deep_copy() {
        let mut s = RateStatsStore::with_path(temp("copy").join("n.json"));
        s.add(5, 5, Some("A"));
        let before = s.snapshot();
        s.add(5, 5, Some("A"));
        let after = s.snapshot();
        let mk = timefmt::now_fmt(timefmt::FMT_MINUTE);
        assert_eq!((before.minutes[&mk].sent, before.minutes[&mk].recv), (5, 5));
        assert_eq!((after.minutes[&mk].sent, after.minutes[&mk].recv), (10, 10));
    }

    #[test]
    fn round_trip_and_corrupt_fallback() {
        let path = temp("rt").join("netstats_buckets.json");
        {
            let mut s = RateStatsStore::with_path(path.clone());
            s.add(100, 200, Some("A"));
            assert!(s.save_if_dirty());
            assert!(!s.save_if_dirty(), "写完即转干净");
        }
        let s2 = RateStatsStore::with_path(path.clone());
        let mk = timefmt::now_fmt(timefmt::FMT_MINUTE);
        assert_eq!(s2.snapshot().minutes[&mk].sent, 100);
        // 落盘三表键名必须是 Python 认得的 minutes/hours/days
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        for k in ["minutes", "hours", "days"] {
            assert!(
                raw.get(k).map(|v| v.is_object()).unwrap_or(false),
                "缺 {k} 表"
            );
        }

        let bad = temp("bad").join("netstats_buckets.json");
        std::fs::write(&bad, "not json").unwrap();
        let s3 = RateStatsStore::with_path(bad);
        assert!(s3.snapshot().minutes.is_empty(), "损坏回退为空");
    }
}
