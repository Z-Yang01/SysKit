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
/// WiFi 读取节拍（拍数）：与 Python `self._tickno % 5 == 0`（5 秒）一致。
///
/// `wlanapi` 一次调用只有微秒级，本可以每拍都读；仍按 5 秒是为了与 Python
/// 行为对齐——信号曲线的时间分辨率变了会让两版的历史数据不可比。
pub const WIFI_EVERY_TICKS: u64 = 5;

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

    #[cfg(test)]
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
            let mut since_clip_save = 0u64;
            let mut tickno: u64 = 0;
            // WiFi 每 5 秒读一次（Python 同为 5 秒周期），但**SSID 归因每次入账都要用**，
            // 所以读数缓存在这里而不是每次重读；0 号 tick 立即读一次，
            // 避免启动后头 5 秒的流量全归到「以太网/未知」桶。
            let mut wifi = crate::wifi::WifiInfo::disconnected();
            while !hub.stopped() {
                // 自愈：单拍 panic 记 ERROR 后继续（采样线程死 = 遥测/流量全静默消失）
                crate::catch_tick("telemetry", || {
                    if tickno % WIFI_EVERY_TICKS == 0 {
                        // 带超时的后台读取：wlanapi 的 RPC 在 WLAN 服务异常时可能
                        // 阻塞数分钟（真机实测 65 分钟，16:09→17:14）——绝不能让
                        // 遥测主循环被它拖住（刷盘/事件全部停摆）。超时沿用上一拍
                        // 数据；挂起的读取线程等它自然返回后自行消亡。
                        let (tx, rx) = std::sync::mpsc::channel();
                        std::thread::Builder::new()
                            .name("sk-wifi-read".into())
                            .spawn(move || {
                                let _ = tx.send(crate::wifi::read());
                            })
                            .expect("wifi 读取线程启动失败");
                        match rx.recv_timeout(std::time::Duration::from_secs(3)) {
                            Ok(info) => {
                                wifi = info;
                                // SSID 归因：决定流量进哪个桶（未连接时空串 → 「以太网/未知」）
                                hub.set_ssid(&wifi.ssid);
                            }
                            Err(_) => {
                                log::warn!("wifi 读取超时（3s），沿用上一拍数据");
                            }
                        }
                    }
                    tickno += 1;

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
                            Some(&wifi.ssid),
                        );
                    }

                    // 今日累计要在入账之后取，前端看到的数才自洽
                    let (sent, recv) = hub.today_traffic();
                    let mut tick = tick;
                    tick.today_sent = sent;
                    tick.today_recv = recv;

                    // 信号/tick 一起带给前端（概览页信号曲线用）
                    tick.wifi_signal = wifi.signal;
                    tick.wifi_ssid = wifi.ssid.clone();
                    *hub.latest.lock().expect("遥测锁中毒") = Some(tick.clone());
                    let _ = app.emit("sk://tick", &tick);
                    // 托盘悬浮显示实时速率（内部去重 + 尊重 tray_rate_enabled 设置）；
                    // 基础段读监控实时状态（曾硬编码 "offline"——状态灯接线后两路提示一致）
                    let mstate = crate::monitor::state().state;
                    crate::tray::apply_rate(&app, tick.down, tick.up, &mstate);

                    since_save += 1;
                    if since_save >= SAVE_EVERY_TICKS {
                        hub.flush();
                        since_save = 0;
                    }
                    // 剪贴板历史独立 10s 落盘（对齐 Python QTimer 10_000；
                    // 60s 太久——崩溃丢窗口内全部历史，真机冒烟发现）
                    since_clip_save += 1;
                    if since_clip_save >= 10 {
                        crate::clipboard_history::with(|h| h.flush_if_dirty());
                        since_clip_save = 0;
                    }
                });
                std::thread::sleep(std::time::Duration::from_secs(SAMPLE_INTERVAL_SECS));
            }
            // 收尾兜底 flush：仅在线程被优雅停止时可达（进程退出时线程直接被杀，
            // 走不到这里）——真正的退出落盘在托盘「退出」菜单里做（tray.rs MENU_QUIT），
            // 否则最近一个落盘周期（≤60s）的流量统计会每次退出都丢。
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
        // 必须隔离：TrafficStore/EventStore::new() 会读真实数据目录——
        // 桌面上恰有实例在跑时（它每 60s 刷盘），不加隔离本测试就随环境假红
        let _g = crate::test_support::serial();
        crate::test_support::isolate();
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

    #[test]
    fn hub_flush_persists_dirty_traffic_and_rates() {
        // 退出落盘通路（托盘「退出」调 hub.flush()）：脏的流量/多粒度桶必须
        // 一次 flush 全部落盘，重开实例能读回。丢了这一步 = 每次退出丢
        // 最近一个落盘周期（≤60s）的统计。
        let _g = crate::test_support::serial();
        crate::test_support::reset_persistent_files();
        let dir = crate::test_support::isolate();
        let hub = Hub::new();
        {
            let mut t = hub.traffic.lock().expect("流量锁中毒");
            t.add(111, 222);
        }
        {
            let mut r = hub.rates.lock().expect("流量桶锁中毒");
            r.set_ssid("HomeWiFi");
            r.add(11, 22, None);
        }
        hub.flush();
        let tf = crate::paths::traffic_file();
        let nf = crate::paths::netstats_file();
        assert!(
            tf.starts_with(&dir) && nf.starts_with(&dir),
            "必须落在隔离目录"
        );
        let t2 = crate::rate_store::TrafficStore::with_path(tf);
        assert_eq!(
            t2.days()
                .get(&crate::timefmt::today())
                .map(|d| (d.sent, d.recv)),
            Some((111, 222)),
            "traffic.json 必须含退出前增量"
        );
        let r2 = RateStatsStore::with_path(nf);
        assert_eq!(
            r2.snapshot()
                .days
                .get(&crate::timefmt::today())
                .map(|b| (b.sent, b.recv)),
            Some((11, 22)),
            "netstats_buckets.json 必须含退出前增量"
        );
        // 收尾清盘：隔离目录是进程级共享（首设赢家），本用例写下的 traffic/
        // netstats 文件必须清掉，否则排序在后的「空 Hub」断言会读到残留数据。
        crate::test_support::reset_persistent_files();
    }
}
