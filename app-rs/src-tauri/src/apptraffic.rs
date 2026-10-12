//! 按应用的流量统计：ETW Kernel-Network 采集 (pid, 方向, 字节) → 按分钟/
//! 小时/天聚合到**进程名**，支撑「今天 / 本周 / 本月 × Top 应用」视图。
//!
//! 分层（可测性是硬要求）：
//! - [`AppTrafficAgg`] 纯聚合器：add / summary / 落盘，不碰任何 Win32；
//!   单测喂伪造事件即可锁行为
//! - [`AppTrafficState`] 采集侧：ferrisetw KernelTrace（TCP/IP provider），
//!   回调里 pid→进程名（Toolhelp32 快照 + TTL 缓存）后喂聚合器；
//!   ETW 会话启动失败（非管理员/被其他软件占用）→ `available=false + reason`，
//!   命令层诚实透传，绝不静默装死
//!
//! 持久化 `netstats_apps.json`（自有格式，非跨语言契约）：
//! `{"minutes": {ts: {app: {sent, recv}}}, "hours": …, "days": …}`，
//! days 保留 365 天（与 netstats 的 days 上限一致）。
//!
//! 事件口径（Microsoft-Windows-Kernel-Network）：
//! id 10=Send(IPv4) / 11=Recv(IPv4) / 18=Send(IPv6) / 19=Recv(IPv6)，
//! 字节数取 `size` 属性；其它 id（连接/断开/重传）不入账。

use crate::MutexExt;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::{DateTime, Datelike, Local, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::timefmt;

const DAY_RETAIN: usize = 365;
const PROC_TTL: Duration = Duration::from_secs(5);
/// ETW 事件无进程名可用时的占位（pid 已退出：快照赶不上进程消亡）。
pub const EXITED_PROC: &str = "(已退出进程)";

/// 单应用单桶的收发字节数。
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub sent: i64,
    #[serde(default)]
    pub recv: i64,
}

/// `app 名 → 用量` 的一层桶（days/hours/minutes 同形）。
type AppBucket = BTreeMap<String, BTreeMap<String, Usage>>;

/// 纯聚合器：ETW 回调喂 add，命令层读 summary。
pub struct AppTrafficAgg {
    minutes: AppBucket,
    hours: AppBucket,
    days: AppBucket,
    path: std::path::PathBuf,
    dirty: bool,
}

impl AppTrafficAgg {
    pub fn with_path(path: std::path::PathBuf) -> Self {
        let raw = crate::stores::load_json(&path, serde_json::json!({}));
        let (minutes, hours, days) = parse_buckets(&raw);
        Self {
            minutes,
            hours,
            days,
            path,
            dirty: false,
        }
    }

    /// 累计一次事件（已按 pid 解析成进程名）。
    pub fn add(&mut self, exe: &str, sent: i64, recv: i64, now: DateTime<Local>) {
        if sent <= 0 && recv <= 0 || exe.is_empty() {
            return;
        }
        let app = exe.to_string();
        for (table, key) in [
            (&mut self.minutes, timefmt::fmt(&now, timefmt::FMT_MINUTE)),
            (&mut self.hours, timefmt::fmt(&now, timefmt::FMT_HOUR)),
            (&mut self.days, timefmt::fmt(&now, timefmt::FMT_DAY)),
        ] {
            let bucket = table.entry(key).or_default();
            let usage = bucket.entry(app.clone()).or_default();
            usage.sent += sent;
            usage.recv += recv;
        }
        self.roll_hours();
        self.prune();
        self.dirty = true;
    }

    /// 把非当前小时的分钟桶归并进对应小时桶（与 rate_stats 同策略：每 add 全扫，
    /// 分钟桶 ≤1440 微秒级；时钟回拨时旧桶也能归并进「非当前」桶）。
    fn roll_hours(&mut self) {
        let cur_hour = timefmt::now_fmt(timefmt::FMT_HOUR);
        let mut done: Vec<String> = Vec::new();
        let keys: Vec<String> = self.minutes.keys().cloned().collect();
        for k in keys {
            if k.starts_with(&cur_hour) || done.iter().any(|h| k.starts_with(h.as_str())) {
                continue;
            }
            let hour_key = k.chars().take(13).collect::<String>();
            let Some(bucket) = self.minutes.remove(&k) else {
                continue;
            };
            let hour_bucket = self.hours.entry(hour_key.clone()).or_default();
            for (app, usage) in bucket {
                let target = hour_bucket.entry(app).or_default();
                target.sent += usage.sent;
                target.recv += usage.recv;
            }
            done.push(hour_key);
        }
    }

    /// days 保留最近 365 天（BTreeMap 键序 = 时间序）。
    fn prune(&mut self) {
        while self.days.len() > DAY_RETAIN {
            let oldest = self.days.keys().next().cloned();
            match oldest {
                Some(k) => {
                    self.days.remove(&k);
                    self.dirty = true;
                }
                None => break,
            }
        }
        // 小时/分钟桶按数量上限兜底（hours 24*31，minutes 60*24*2）
        while self.hours.len() > 24 * 31 {
            if let Some(k) = self.hours.keys().next().cloned() {
                self.hours.remove(&k);
            } else {
                break;
            }
        }
        while self.minutes.len() > 60 * 24 * 2 {
            if let Some(k) = self.minutes.keys().next().cloned() {
                self.minutes.remove(&k);
            } else {
                break;
            }
        }
    }

    /// 区间汇总：`day`=今天、`week`=本周（ISO，周一起）、`month`=本月；
    /// `q` 为应用名过滤（大小写不敏感，空 = 不过滤）。返回降序 Top（调用方截断）。
    pub fn summary(&self, range: &str, q: &str, now: DateTime<Local>) -> Vec<AppUsageRow> {
        let needle = q.trim().to_ascii_lowercase();
        let mut acc: BTreeMap<String, Usage> = BTreeMap::new();
        let today = timefmt::fmt(&now, timefmt::FMT_DAY);
        let month_prefix = &today[..7];
        let iy = now.iso_week().year();
        let iw = now.iso_week().week();
        for (day_key, bucket) in &self.days {
            let in_range = match range {
                "day" => day_key == &today,
                "week" => match NaiveDate::parse_from_str(day_key, timefmt::FMT_DAY) {
                    Ok(d) => {
                        let dy = d.iso_week().year();
                        let dw = d.iso_week().week();
                        dy == iy && dw == iw
                    }
                    Err(_) => false,
                },
                "month" => day_key.starts_with(month_prefix),
                _ => false,
            };
            if !in_range {
                continue;
            }
            for (app, usage) in bucket {
                if !needle.is_empty() && !app.to_ascii_lowercase().contains(&needle) {
                    continue;
                }
                let target = acc.entry(app.clone()).or_default();
                target.sent += usage.sent;
                target.recv += usage.recv;
            }
        }
        let mut rows: Vec<AppUsageRow> = acc
            .into_iter()
            .map(|(app, u)| AppUsageRow {
                app,
                sent: u.sent,
                recv: u.recv,
            })
            .collect();
        rows.sort_by(|a, b| {
            (b.sent + b.recv)
                .cmp(&(a.sent + a.recv))
                .then(a.app.cmp(&b.app))
        });
        rows
    }

    pub fn save_if_dirty(&mut self) -> bool {
        if !self.dirty {
            return true;
        }
        let payload = serde_json::json!({
            "minutes": self.minutes,
            "hours": self.hours,
            "days": self.days,
        });
        if crate::stores::save_json(&self.path, &payload) {
            self.dirty = false;
            true
        } else {
            false
        }
    }
}

/// 汇总表的一行。
#[derive(Debug, Clone, Serialize)]
pub struct AppUsageRow {
    pub app: String,
    pub sent: i64,
    pub recv: i64,
}

fn parse_buckets(raw: &serde_json::Value) -> (AppBucket, AppBucket, AppBucket) {
    #[derive(Deserialize)]
    struct File {
        #[serde(default)]
        minutes: AppBucket,
        #[serde(default)]
        hours: AppBucket,
        #[serde(default)]
        days: AppBucket,
    }
    let f: File = serde_json::from_value(raw.clone()).unwrap_or(File {
        minutes: BTreeMap::new(),
        hours: BTreeMap::new(),
        days: BTreeMap::new(),
    });
    (f.minutes, f.hours, f.days)
}

/// pid → 进程名的快照缓存（Toolhelp32，TTL 5s）。
pub struct ProcNames {
    cache: std::collections::HashMap<u32, String>,
    refreshed: Instant,
}

impl Default for ProcNames {
    fn default() -> Self {
        Self {
            cache: std::collections::HashMap::new(),
            refreshed: Instant::now() - PROC_TTL,
        }
    }
}

impl ProcNames {
    pub fn name_of(&mut self, pid: u32) -> String {
        if self.refreshed.elapsed() >= PROC_TTL {
            self.refresh();
        }
        self.cache
            .get(&pid)
            .cloned()
            .unwrap_or_else(|| EXITED_PROC.to_string())
    }

    fn refresh(&mut self) {
        #[cfg(windows)]
        {
            use windows::Win32::System::Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
                TH32CS_SNAPPROCESS,
            };
            unsafe {
                let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
                    self.refreshed = Instant::now();
                    return;
                };
                let mut entry = PROCESSENTRY32W {
                    dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                    ..Default::default()
                };
                if Process32FirstW(snap, &mut entry).is_ok() {
                    loop {
                        let name = String::from_utf16_lossy(
                            &entry.szExeFile[..entry
                                .szExeFile
                                .iter()
                                .position(|c| *c == 0)
                                .unwrap_or(entry.szExeFile.len())],
                        );
                        self.cache.insert(entry.th32ProcessID, name);
                        if Process32NextW(snap, &mut entry).is_err() {
                            break;
                        }
                    }
                }
                let _ = windows::Win32::Foundation::CloseHandle(snap);
            }
        }
        self.refreshed = Instant::now();
    }
}

/// 运行态：聚合器 + ETW 会话可用性（命令层读，采集线程写）。
pub struct AppTrafficState {
    pub agg: std::sync::Arc<Mutex<AppTrafficAgg>>,
    pub available: AtomicBool,
    pub reason: Mutex<String>,
}

impl AppTrafficState {
    pub fn new(agg: AppTrafficAgg) -> Self {
        Self {
            agg: std::sync::Arc::new(Mutex::new(agg)),
            available: AtomicBool::new(false),
            reason: Mutex::new("采集尚未启动".to_string()),
        }
    }

    pub fn set_unavailable(&self, reason: &str) {
        self.available.store(false, Ordering::Relaxed);
        *self.reason.lock_ok() = reason.to_string();
    }

    pub fn set_available(&self) {
        self.available.store(true, Ordering::Relaxed);
        *self.reason.lock_ok() = String::new();
    }

    pub fn status(&self) -> (bool, String) {
        (
            self.available.load(Ordering::Relaxed),
            self.reason
                .lock()
                .expect("apptraffic reason 锁中毒")
                .clone(),
        )
    }
}

/// 启动 ETW 采集（telemetry spawn 时调用；失败写 reason，不 panic 不重试轰炸）。
pub fn spawn_etw(state: std::sync::Arc<AppTrafficState>) {
    std::thread::Builder::new()
        .name("sk-apptraffic".into())
        .spawn(move || {
            let procs = std::sync::Arc::new(Mutex::new(ProcNames::default()));
            let agg_ref = state.agg.clone();
            let procs_cb = procs.clone();
            let callback = move |record: &ferrisetw::EventRecord,
                                 locator: &ferrisetw::schema_locator::SchemaLocator| {
                // 采集回调里绝不 panic（ETW 内部线程 panic 会拖垮整条 trace）
                let id = record.event_id();
                let sent = matches!(id, 10 | 18);
                let recv = matches!(id, 11 | 19);
                if !sent && !recv {
                    return;
                }
                let size = match locator.event_schema(record) {
                    Ok(schema) => ferrisetw::parser::Parser::create(record, &schema)
                        .try_parse::<u32>("size")
                        .unwrap_or(0),
                    Err(_) => return,
                };
                if size == 0 {
                    return;
                }
                let pid = record.process_id();
                let exe = {
                    let mut p = procs_cb.lock_ok();
                    p.name_of(pid)
                };
                if let Ok(mut agg) = agg_ref.lock() {
                    let s = if sent { i64::from(size) } else { 0 };
                    let r = if recv { i64::from(size) } else { 0 };
                    agg.add(&exe, s, r, timefmt::now_local());
                }
            };

            let provider = ferrisetw::provider::Provider::kernel(
                &ferrisetw::provider::kernel_providers::TCP_IP_PROVIDER,
            )
            .add_callback(callback)
            .build();
            match ferrisetw::trace::KernelTrace::new()
                .named(String::from("SysKit-AppTraffic"))
                .enable(provider)
                .start_and_process()
            {
                Ok(_trace) => {
                    // trace 由返回值持有（Drop 即 stop）；常驻不释放
                    std::mem::forget(_trace);
                    state.set_available();
                    log::info!("应用流量 ETW 会话已启动（SysKit-AppTraffic）");
                }
                Err(e) => {
                    state.set_unavailable(&format!(
                        "ETW 会话启动失败（{e:?}）——需要管理员身份运行，且不能与其他网络监控软件的内核会话冲突"
                    ));
                    log::warn!("应用流量 ETW 启动失败: {e:?}");
                }
            }
        })
        .expect("apptraffic 线程启动失败");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agg() -> AppTrafficAgg {
        AppTrafficAgg::with_path(std::env::temp_dir().join(format!(
            "sk-apptest-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )))
    }

    #[test]
    fn add_goes_to_all_three_tables() {
        let mut a = agg();
        let now = timefmt::now_local();
        a.add("chrome.exe", 100, 200, now);
        let day = timefmt::fmt(&now, timefmt::FMT_DAY);
        assert_eq!(a.days[&day]["chrome.exe"].sent, 100);
        assert_eq!(a.days[&day]["chrome.exe"].recv, 200);
        let hour = timefmt::fmt(&now, timefmt::FMT_HOUR);
        assert_eq!(a.hours[&hour]["chrome.exe"].recv, 200);
        let minute = timefmt::fmt(&now, timefmt::FMT_MINUTE);
        assert_eq!(a.minutes[&minute]["chrome.exe"].sent, 100);
    }

    #[test]
    fn summary_day_filters_and_sums() {
        let mut a = agg();
        let now = timefmt::now_local();
        let yesterday = now - chrono::Duration::days(1);
        a.add("a.exe", 10, 20, now);
        a.add("b.exe", 1, 2, now);
        a.add("a.exe", 5, 6, now);
        a.add("a.exe", 999, 999, yesterday); // 昨天不计入今天
        let rows = a.summary("day", "", now);
        assert_eq!(rows.len(), 2);
        let top = &rows[0];
        assert_eq!(top.app, "a.exe");
        assert_eq!(top.sent, 15);
        assert_eq!(top.recv, 26);
    }

    #[test]
    fn summary_query_is_case_insensitive_substring() {
        let mut a = agg();
        let now = timefmt::now_local();
        a.add("Chrome.exe", 10, 10, now);
        a.add("msedge.exe", 10, 10, now);
        let rows = a.summary("day", "chro", now);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].app, "Chrome.exe");
    }

    #[test]
    fn summary_month_prefix_groups_by_calendar_month() {
        let mut a = agg();
        let now = timefmt::now_local();
        // 上月最后一天：同 month 前缀判定不计入本月
        let last_month = if now.month() == 1 {
            now.with_year(now.year() - 1)
                .unwrap()
                .with_month(12)
                .unwrap()
        } else {
            now.with_month(now.month() - 1).unwrap()
        };
        a.add("a.exe", 7, 8, last_month);
        assert!(a.summary("month", "", now).is_empty(), "上月不算本月");
        a.add("a.exe", 7, 8, now);
        let rows = a.summary("month", "", now);
        assert_eq!(rows[0].sent, 7);
    }

    #[test]
    fn prune_caps_days() {
        let mut a = agg();
        let base = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        for i in 0..(DAY_RETAIN + 10) {
            let d = base + chrono::Duration::days(i as i64);
            a.days
                .insert(d.format(timefmt::FMT_DAY).to_string(), BTreeMap::new());
        }
        a.prune();
        assert_eq!(a.days.len(), DAY_RETAIN);
    }

    #[test]
    fn persistence_round_trip() {
        let path = std::env::temp_dir().join(format!(
            "sk-apptest-rt-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        {
            let mut a = AppTrafficAgg::with_path(path.clone());
            a.add("app.exe", 11, 22, timefmt::now_local());
            assert!(a.save_if_dirty());
        }
        let b = AppTrafficAgg::with_path(path.clone());
        let now = timefmt::now_local();
        let rows = b.summary("day", "app.exe", now);
        assert_eq!(rows[0].sent, 11);
        assert_eq!(rows[0].recv, 22);
        let _ = std::fs::remove_file(path);
    }
}
