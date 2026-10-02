//! 遥测运行时：1 秒采样线程 + 前端查询接口（对应 Python 侧 telemetry.py + stores）。
//!
//! 与 Python 的分工差异：Python 用 Qt Signal `tick(dict)` 推送；这里用
//! Tauri 事件 `sk://tick` 推送，同时把最近一拍缓存在共享状态里，
//! 让「前端刚加载/刚切页」能立刻拿到当前值而不是等下一拍。
//!
//! 落盘策略与 Python 一致：每 60 拍把流量/多粒度桶 flush 一次（脏标记 + 原子写），
//! 高频复制场景不在热路径上做序列化与写盘。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter, Runtime};

use crate::netstats::{self, DayReport, Series};
use crate::rate_stats::{RateStatsStore, StatsSnapshot};
use crate::rate_store::{EventItem, EventStore, TrafficStore};
use crate::telemetry::{Sampler, Tick};

/// 采样间隔（秒）：与 Python `self._stop_evt.wait(1)` 一致。
pub const SAMPLE_INTERVAL_SECS: u64 = 1;
/// 落盘节拍（拍数）：与 Python `save_counter >= 60` 一致。
pub const SAVE_EVERY_TICKS: u64 = 60;

/// 遥测共享状态：采样线程写，命令线程读。
pub struct Hub {
    latest: Mutex<Option<Tick>>,
    traffic: Mutex<TrafficStore>,
    rates: Mutex<RateStatsStore>,
    events: Mutex<EventStore>,
    stop: AtomicBool,
}

impl Hub {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            latest: Mutex::new(None),
            traffic: Mutex::new(TrafficStore::new()),
            rates: Mutex::new(RateStatsStore::new()),
            events: Mutex::new(EventStore::new(500)),
            stop: AtomicBool::new(false),
        })
    }

    pub fn latest_tick(&self) -> Option<Tick> {
        self.latest.lock().expect("遥测锁中毒").clone()
    }

    pub fn stats_snapshot(&self) -> StatsSnapshot {
        self.rates.lock().expect("流量桶锁中毒").snapshot()
    }

    pub fn events(&self, limit: usize) -> Vec<EventItem> {
        self.events.lock().expect("事件锁中毒").recent(limit)
    }

    /// 记录一次状态切换（M3 的网络监控接入；现在也供命令手动写入）。
    pub fn record_event(&self, state: &str, detail: &str) {
        self.events.lock().expect("事件锁中毒").add(state, detail);
    }

    pub fn daily_report(&self, days: i64) -> Vec<DayReport> {
        let events = self.events.lock().expect("事件锁中毒").items();
        let traffic = self.traffic.lock().expect("流量锁中毒").days();
        netstats::daily_report(&events, days, &traffic)
    }

    pub fn series(&self, granularity: &str, ssid: Option<&str>) -> Result<Series, String> {
        netstats::range_series(granularity, &self.stats_snapshot(), ssid)
    }

    pub fn today_traffic(&self) -> (i64, i64) {
        self.traffic.lock().expect("流量锁中毒").today()
    }

    pub fn set_ssid(&self, ssid: &str) {
        self.rates.lock().expect("流量桶锁中毒").set_ssid(ssid);
    }

    /// 立即落盘（退出路径/命令调用）。
    pub fn flush(&self) {
        self.traffic.lock().expect("流量锁中毒").save_if_dirty();
        self.rates.lock().expect("流量桶锁中毒").save_if_dirty();
    }

    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }
}

/// 起采样线程（Tauri setup 阶段调用一次）。
pub fn spawn<R: Runtime>(app: AppHandle<R>, hub: Arc<Hub>) {
    std::thread::Builder::new()
        .name("sk-telemetry".into())
        .spawn(move || {
            let mut sampler = Sampler::new();
            let mut since_save = 0u64;
            while !hub.stopped() {
                let today = hub.today_traffic();
                let tick = sampler.sample(today);

                // 速率增量入流量统计（上行/下行分开传，与 Python 调用顺序一致）
                if tick.down > 0 || tick.up > 0 {
                    hub.traffic
                        .lock()
                        .expect("流量锁中毒")
                        .add(tick.up as i64, tick.down as i64);
                    hub.rates.lock().expect("流量桶锁中毒").add(
                        tick.up as i64,
                        tick.down as i64,
                        None,
                    );
                }

                // 今日累计要在入账之后取，前端看到的数才自洽
                let (sent, recv) = hub.today_traffic();
                let mut tick = tick;
                tick.today_sent = sent;
                tick.today_recv = recv;

                *hub.latest.lock().expect("遥测锁中毒") = Some(tick.clone());
                let _ = app.emit("sk://tick", &tick);
                // 托盘悬浮显示实时速率（内部去重 + 尊重 tray_rate_enabled 设置；
                // 状态灯暂固定 offline，M3 的网络监控接管）
                crate::tray::apply_rate(&app, tick.down, tick.up, "offline");

                since_save += 1;
                if since_save >= SAVE_EVERY_TICKS {
                    hub.flush();
                    since_save = 0;
                }
                std::thread::sleep(std::time::Duration::from_secs(SAMPLE_INTERVAL_SECS));
            }
            // 退出前最后一拍务必落盘（Python 靠 atexit；这里靠线程收尾）
            hub.flush();
        })
        .expect("遥测线程启动失败");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hub_starts_empty_and_records_events() {
        let hub = Hub::new();
        assert!(hub.latest_tick().is_none(), "未采样前应为空");
        assert_eq!(hub.today_traffic(), (0, 0));
        assert!(hub.events(10).is_empty());
        hub.record_event("no_net", "测试事件");
        let evs = hub.events(10);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].state, "no_net");
        assert_eq!(evs[0].detail, "测试事件");
    }

    #[test]
    fn hub_series_rejects_unknown_granularity() {
        let hub = Hub::new();
        assert!(hub.series("hour", None).is_ok());
        assert!(hub.series("week", None).is_err());
    }

    #[test]
    fn hub_daily_report_defaults_to_empty_zeroes() {
        let hub = Hub::new();
        let r = hub.daily_report(7);
        assert_eq!(r.len(), 7);
        assert!(r
            .iter()
            .all(|d| d.outages == 0 && d.outage_sec == 0 && !d.has_traffic));
    }

    #[test]
    fn hub_stop_flag_round_trips() {
        let hub = Hub::new();
        assert!(!hub.stopped());
        hub.request_stop();
        assert!(hub.stopped());
    }

    #[test]
    fn cadence_constants_match_python() {
        assert_eq!(SAMPLE_INTERVAL_SECS, 1);
        assert_eq!(SAVE_EVERY_TICKS, 60, "Python: save_counter >= 60");
    }
}
