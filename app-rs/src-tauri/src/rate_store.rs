//! 轻量持久化存储：每日流量（traffic.json）+ 断网事件（events.json）。
//!
//! 与 Python 侧 stores.py 的 `TrafficStore` / `EventStore` **文件格式逐字段对齐**：
//! - `traffic.json`：`{"YYYY-MM-DD": {"sent": n, "recv": n}}`，只留 62 天；
//! - `events.json`：[{ts, state, detail}]，**最新在前**，上限 500 条，detail 截 120 字。
//!
//! 为什么字节级对齐是硬约束：迁移期两版共用 `%APPDATA%\SysKit`，用户可能今天用
//! Python 版、明天用 Rust 版。任一侧把 sent/recv 写成别的键名或把事件顺序反过来，
//! 另一侧读到的就是**静默错误**的统计（不是崩溃，是悄悄算错）。

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::jsonio;
use crate::paths;
use crate::timefmt;

/// traffic.json 里单日记录。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DayTraffic {
    #[serde(default)]
    pub sent: i64,
    #[serde(default)]
    pub recv: i64,
}

/// 按天累计上下行的流量存储（脏标记 + 定期落盘，与 Python 同策略）。
pub struct TrafficStore {
    path: PathBuf,
    data: BTreeMap<String, DayTraffic>,
    dirty: bool,
    /// (day, sent, recv)：秒级热路径的无锁快照（对应 Python `_today_cache`）
    today_cache: Option<(String, i64, i64)>,
}

impl Default for TrafficStore {
    fn default() -> Self {
        Self::new()
    }
}

impl TrafficStore {
    pub fn new() -> Self {
        Self::with_path(paths::traffic_file())
    }

    pub fn with_path(path: PathBuf) -> Self {
        let raw = jsonio::load_json(&path, serde_json::json!({}));
        let data = parse_days(&raw);
        Self {
            path,
            data,
            dirty: false,
            today_cache: None,
        }
    }

    /// 累计一次采样增量。两个增量都为 0 时不标脏（避免空转写盘）。
    pub fn add(&mut self, sent_delta: i64, recv_delta: i64) {
        let ds = sent_delta.max(0);
        let dr = recv_delta.max(0);
        if ds == 0 && dr == 0 {
            return;
        }
        let day = timefmt::today();
        let rec = self.data.entry(day.clone()).or_default();
        rec.sent += ds;
        rec.recv += dr;
        self.today_cache = Some((day, rec.sent, rec.recv));
        self.dirty = true;
        // 只保留 62 天（键是 ISO 序 = 时间序，BTreeMap 天然有序）
        while self.data.len() > 62 {
            if let Some(oldest) = self.data.keys().next().cloned() {
                self.data.remove(&oldest);
            }
        }
    }

    /// 今日 (sent, recv)，走缓存快照。
    pub fn today(&mut self) -> (i64, i64) {
        let day = timefmt::today();
        if let Some((cached_day, s, r)) = &self.today_cache {
            if *cached_day == day {
                return (*s, *r);
            }
        }
        let rec = self.data.get(&day).cloned().unwrap_or_default();
        let out = (rec.sent, rec.recv);
        self.today_cache = Some((day, rec.sent, rec.recv));
        out
    }

    /// 按天数据拷贝（网络日报聚合用）。
    pub fn days(&self) -> BTreeMap<String, DayTraffic> {
        self.data.clone()
    }

    /// 有变更才写盘；返回是否发生写盘。
    pub fn save_if_dirty(&mut self) -> bool {
        if !self.dirty {
            return false;
        }
        if jsonio::save_json(&self.path, &serde_json::json!(self.data)) {
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

/// 事件条目（断网/恢复时间线）。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct EventItem {
    #[serde(default)]
    pub ts: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub detail: String,
}

/// 断网/恢复事件时间线：最新在前，上限 500 条，变更即落盘。
pub struct EventStore {
    path: PathBuf,
    limit: usize,
    data: Vec<EventItem>,
}

impl Default for EventStore {
    fn default() -> Self {
        Self::new(500)
    }
}

impl EventStore {
    pub fn new(limit: usize) -> Self {
        Self::with_path(paths::events_file(), limit)
    }

    pub fn with_path(path: PathBuf, limit: usize) -> Self {
        let raw = jsonio::load_json(&path, serde_json::json!([]));
        let data = parse_events(&raw);
        Self { path, limit, data }
    }

    /// 插入一条（最新在前），detail 按 Python 语义截到 120 字，随后立即落盘。
    pub fn add(&mut self, state: &str, detail: &str) {
        let detail: String = detail.chars().take(120).collect();
        self.data.insert(
            0,
            EventItem {
                ts: timefmt::now_fmt(timefmt::FMT_TS),
                state: state.to_string(),
                detail,
            },
        );
        self.data.truncate(self.limit);
        jsonio::save_json(&self.path, &serde_json::json!(self.data));
    }

    pub fn items(&self) -> Vec<EventItem> {
        self.data.clone()
    }

    pub fn recent(&self, n: usize) -> Vec<EventItem> {
        self.data.iter().take(n).cloned().collect()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.data.len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

/// 解析 traffic.json：容错（非对象/坏条目丢弃并留痕），与 Python 的
/// `isinstance(dict)` 兜底同语义。
fn parse_days(raw: &serde_json::Value) -> BTreeMap<String, DayTraffic> {
    let mut out = BTreeMap::new();
    let Some(obj) = raw.as_object() else {
        if !raw.is_null() {
            log::warn!("traffic.json 顶层非对象，统计已重置");
        }
        return out;
    };
    for (k, v) in obj {
        match serde_json::from_value::<DayTraffic>(v.clone()) {
            Ok(d) => {
                out.insert(k.clone(), d);
            }
            Err(_) => log::warn!("traffic.json 条目 {k} 形态异常，已丢弃"),
        }
    }
    out
}

/// 解析 events.json：非数组 → 空（Python 同款兜底）。
fn parse_events(raw: &serde_json::Value) -> Vec<EventItem> {
    let Some(arr) = raw.as_array() else {
        if !raw.is_null() {
            log::warn!("events.json 顶层非数组，事件时间线已重置");
        }
        return Vec::new();
    };
    arr.iter()
        .filter_map(|v| match serde_json::from_value::<EventItem>(v.clone()) {
            Ok(e) => Some(e),
            Err(_) => {
                log::warn!("events.json 条目形态异常，已丢弃");
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("syskit_rs_store_{tag}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn day_traffic_serializes_with_python_field_names() {
        let j = serde_json::to_string(&DayTraffic { sent: 5, recv: 9 }).unwrap();
        assert_eq!(j, r#"{"sent":5,"recv":9}"#, "字段名/顺序必须与 Python 一致");
    }

    #[test]
    fn traffic_add_accumulates_and_skips_zero() {
        let mut t = TrafficStore::with_path(temp("tr_a").join("traffic.json"));
        assert!(!t.is_dirty(), "新建不应是脏的");
        t.add(0, 0);
        assert!(!t.is_dirty(), "零增量不得标脏（否则空转写盘）");
        t.add(100, 200);
        assert!(t.is_dirty());
        t.add(-5, -7);
        assert_eq!(t.today(), (100, 200), "负增量按 0 处理，不倒退");
        t.add(10, 20);
        assert_eq!(t.today(), (110, 220));
    }

    #[test]
    fn traffic_round_trips_through_disk() {
        let path = temp("tr_rt").join("traffic.json");
        {
            let mut t = TrafficStore::with_path(path.clone());
            t.add(11, 22);
            assert!(t.save_if_dirty());
            assert!(!t.save_if_dirty(), "写完即转干净");
        }
        let t2 = TrafficStore::with_path(path.clone());
        assert_eq!(
            t2.days().get(&timefmt::today()).map(|d| (d.sent, d.recv)),
            Some((11, 22))
        );
        // 落盘形态必须是 Python 可读的 {"YYYY-MM-DD": {"sent","recv"}}
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let day = &raw[timefmt::today()];
        assert_eq!(day["sent"], 11);
        assert_eq!(day["recv"], 22);
        assert!(
            day.as_object().unwrap().len() == 2,
            "每天只允许 sent/recv 两个键"
        );
    }

    #[test]
    fn traffic_prunes_to_62_days() {
        let path = temp("tr_prune").join("traffic.json");
        // 造 80 个连续且唯一的日键（跨月，保证 ISO 序 = 时间序）
        let mut map = serde_json::Map::new();
        for i in 0..80 {
            let month = 1 + i / 28;
            let day = (i % 28) + 1;
            map.insert(
                format!("2026-{month:02}-{day:02}"),
                serde_json::json!({"sent": 1, "recv": 1}),
            );
        }
        jsonio::save_json(&path, &serde_json::Value::Object(map));
        let mut t = TrafficStore::with_path(path.clone());
        assert_eq!(t.days().len(), 80, "先原样载入（Python 只在 add 时裁剪）");
        t.add(1, 1); // 触发裁剪
        assert_eq!(t.days().len(), 62, "裁剪到 62 天");
        // 最老的键必须已被删掉，今天的仍在
        assert!(!t.days().contains_key("2026-01-01"));
        assert!(t.days().contains_key(&timefmt::today()));
    }

    #[test]
    fn corrupt_traffic_falls_back_to_empty() {
        let path = temp("tr_bad").join("traffic.json");
        std::fs::write(&path, "{ 半截").unwrap();
        let t = TrafficStore::with_path(path);
        assert!(t.days().is_empty(), "损坏文件回退为空统计（Python 同）");
        let path2 = temp("tr_list").join("traffic.json");
        std::fs::write(&path2, "[1,2,3]").unwrap();
        assert!(
            TrafficStore::with_path(path2).days().is_empty(),
            "顶层为数组同样回退"
        );
    }

    #[test]
    fn event_store_newest_first_and_truncates_detail() {
        let path = temp("ev").join("events.json");
        let mut e = EventStore::with_path(path.clone(), 500);
        e.add("ok", "第一条");
        e.add("no_net", &"长".repeat(200));
        let items = e.items();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].state, "no_net", "最新必须在前（Python insert(0)）");
        assert_eq!(items[1].state, "ok");
        assert_eq!(items[0].detail.chars().count(), 120, "detail 截 120 字");
        // ts 形态与 Python strftime 一致
        assert_eq!(items[0].ts.len(), 19);
        assert!(timefmt::parse_ts(&items[0].ts).is_some());
    }

    #[test]
    fn event_store_respects_limit_and_persists_immediately() {
        let path = temp("ev_limit").join("events.json");
        let mut e = EventStore::with_path(path.clone(), 3);
        for i in 0..10 {
            e.add("ok", &format!("第{i}条"));
        }
        assert_eq!(e.len(), 3, "超出上限裁最旧");
        assert_eq!(e.items()[0].detail, "第9条");
        // 立即落盘（Python add 里直接 _save）
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(raw.as_array().unwrap().len(), 3);
        assert_eq!(raw[0]["detail"], "第9条");
        assert_eq!(raw[0]["state"], "ok");
    }

    #[test]
    fn event_store_recent_and_corrupt_fallback() {
        let path = temp("ev_recent").join("events.json");
        let mut e = EventStore::with_path(path.clone(), 500);
        for i in 0..5 {
            e.add("ok", &format!("{i}"));
        }
        assert_eq!(e.recent(2).len(), 2);
        assert_eq!(e.recent(2)[0].detail, "4");

        let bad = temp("ev_bad").join("events.json");
        std::fs::write(&bad, r#"{"not":"a list"}"#).unwrap();
        assert!(EventStore::with_path(bad, 500).is_empty(), "非数组回退为空");
    }

    #[test]
    fn event_item_serializes_with_python_field_names() {
        let j = serde_json::to_string(&EventItem {
            ts: "2026-01-01 10:00:00".into(),
            state: "ok".into(),
            detail: "x".into(),
        })
        .unwrap();
        assert_eq!(
            j, r#"{"ts":"2026-01-01 10:00:00","state":"ok","detail":"x"}"#,
            "三字段 schema 与 Python 锁定的完全一致"
        );
        assert_eq!(
            serde_json::to_string(&EventItem::default()).unwrap(),
            r#"{"ts":"","state":"","detail":""}"#
        );
    }
}
