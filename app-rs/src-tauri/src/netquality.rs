//! 网络质量历史：周期探测 网关/公网 延迟丢包 + 系统 DNS 解析耗时，
//! 按分钟/小时/天三表聚合落盘，支撑「网络质量」趋势卡（P1 首项）。
//!
//! 分层与 apptraffic 相同——[`QualityStore`] 纯聚合器（伪造采样即可测），
//! 探测线程只做「发探测 + 把结果喂给 add」；持久化 `netstats_quality.json`
//! （自有格式，非跨语言契约）。
//!
//! 探测口径：每 10 秒一轮，对 网关 与 公网锚点(223.5.5.5) 各发 1 次 ICMP
//! （超时 1s），`icmp::ping` 返回 None 记丢包；DNS 计时 = 系统解析器解析
//! `www.baidu.com` A 记录的耗时（1.5s 超时，失败不计入）。
//! 分钟桶聚合后：丢包率 = lost/total，平均延迟 = sum/count。

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::timefmt;

const MINUTE_RETAIN: usize = 48 * 60;
const HOUR_RETAIN: usize = 31 * 24;
const DAY_RETAIN: usize = 365;

/// 单桶聚合量（全部累加字段；均值/丢包率展示时派生）。
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct QualityBucket {
    #[serde(default)]
    pub gw_total: u32,
    #[serde(default)]
    pub gw_lost: u32,
    #[serde(default)]
    pub gw_sum_ms: f64,
    #[serde(default)]
    pub pub_total: u32,
    #[serde(default)]
    pub pub_lost: u32,
    #[serde(default)]
    pub pub_sum_ms: f64,
    #[serde(default)]
    pub dns_count: u32,
    #[serde(default)]
    pub dns_sum_ms: f64,
}

impl QualityBucket {
    /// 网关丢包率（0~100；无样本返回 None 由调用方显示「—」）。
    pub fn gw_loss_pct(&self) -> Option<f64> {
        if self.gw_total == 0 {
            None
        } else {
            Some(self.gw_lost as f64 * 100.0 / self.gw_total as f64)
        }
    }

    pub fn gw_avg_ms(&self) -> Option<f64> {
        if self.gw_total == self.gw_lost || self.gw_total == 0 {
            None
        } else {
            Some(self.gw_sum_ms / (self.gw_total - self.gw_lost) as f64)
        }
    }

    pub fn pub_loss_pct(&self) -> Option<f64> {
        if self.pub_total == 0 {
            None
        } else {
            Some(self.pub_lost as f64 * 100.0 / self.pub_total as f64)
        }
    }

    pub fn pub_avg_ms(&self) -> Option<f64> {
        if self.pub_total == self.pub_lost || self.pub_total == 0 {
            None
        } else {
            Some(self.pub_sum_ms / (self.pub_total - self.pub_lost) as f64)
        }
    }

    pub fn dns_avg_ms(&self) -> Option<f64> {
        if self.dns_count == 0 {
            None
        } else {
            Some(self.dns_sum_ms / self.dns_count as f64)
        }
    }
}

type QualityTable = BTreeMap<String, QualityBucket>;

/// 纯聚合器：探测线程喂 add，命令层读 series。
pub struct QualityStore {
    minutes: QualityTable,
    hours: QualityTable,
    days: QualityTable,
    path: PathBuf,
    dirty: bool,
}

/// 一次探测的输入（None = 丢包/超时）。
pub struct ProbeSample {
    pub gw: Option<f64>,
    pub public: Option<f64>,
    pub dns_ms: Option<f64>,
}

impl QualityStore {
    pub fn with_path(path: PathBuf) -> Self {
        let raw = crate::stores::load_json(&path, serde_json::json!({}));
        #[derive(Deserialize)]
        struct File {
            #[serde(default)]
            minutes: QualityTable,
            #[serde(default)]
            hours: QualityTable,
            #[serde(default)]
            days: QualityTable,
        }
        let f: File = serde_json::from_value(raw.clone()).unwrap_or(File {
            minutes: BTreeMap::new(),
            hours: BTreeMap::new(),
            days: BTreeMap::new(),
        });
        Self {
            minutes: f.minutes,
            hours: f.hours,
            days: f.days,
            path,
            dirty: false,
        }
    }

    /// 累计一轮探测。三个探测点全部失败也照记（全丢包本身就是最有价值的质量信号）。
    pub fn add(&mut self, sample: &ProbeSample, now: chrono::DateTime<chrono::Local>) {
        let minute_key = timefmt::fmt(&now, timefmt::FMT_MINUTE);
        let day_key = timefmt::fmt(&now, timefmt::FMT_DAY);

        let mut bucket = QualityBucket::default();
        if let Some(ms) = sample.gw {
            bucket.gw_total += 1;
            bucket.gw_sum_ms += ms;
        } else {
            bucket.gw_total += 1;
            bucket.gw_lost += 1;
        }
        if let Some(ms) = sample.public {
            bucket.pub_total += 1;
            bucket.pub_sum_ms += ms;
        } else {
            bucket.pub_total += 1;
            bucket.pub_lost += 1;
        }
        if let Some(ms) = sample.dns_ms {
            bucket.dns_count += 1;
            bucket.dns_sum_ms += ms;
        }
        // 三点全空（如刚断网 DNS/ICMP 全超时）也入账——全丢包桶是断网证据

        let hour_key = timefmt::fmt(&now, timefmt::FMT_HOUR);
        merge(&mut self.minutes, &minute_key, &bucket);
        merge(&mut self.hours, &hour_key, &bucket);
        merge(&mut self.days, &day_key, &bucket);
        self.roll_hours(&hour_key);
        self.prune();
        self.dirty = true;
    }

    fn roll_hours(&mut self, cur_hour: &str) {
        let keys: Vec<String> = self.minutes.keys().cloned().collect();
        for k in keys {
            if k.starts_with(cur_hour) {
                continue;
            }
            let hour_key = k.chars().take(13).collect::<String>();
            let Some(bucket) = self.minutes.remove(&k) else {
                continue;
            };
            merge(&mut self.hours, &hour_key, &bucket);
        }
    }

    fn prune(&mut self) {
        while self.minutes.len() > MINUTE_RETAIN {
            if let Some(k) = self.minutes.keys().next().cloned() {
                self.minutes.remove(&k);
            } else {
                break;
            }
        }
        while self.hours.len() > HOUR_RETAIN {
            if let Some(k) = self.hours.keys().next().cloned() {
                self.hours.remove(&k);
            } else {
                break;
            }
        }
        while self.days.len() > DAY_RETAIN {
            if let Some(k) = self.days.keys().next().cloned() {
                self.days.remove(&k);
            } else {
                break;
            }
        }
    }

    /// 趋势卡序列：granularity = minute(60)/hour(48)/day(30)。
    /// 行 key 用与 netstats 相同的标签（HH:MM / HH时 / MM-DD）；
    /// 空缺失的桶不补行——前端按时间轴连续补零（与速率卡同约定）。
    pub fn series(&self, granularity: &str) -> Result<Vec<serde_json::Value>, String> {
        let (table, count) = match granularity {
            "minute" => (&self.minutes, 60usize),
            "hour" => (&self.hours, 48),
            "day" => (&self.days, 30),
            _ => return Err(format!("未知粒度: {granularity}")),
        };
        let keys: Vec<String> = table.keys().cloned().collect();
        let start = keys.len().saturating_sub(count);
        Ok(keys[start..]
            .iter()
            .map(|k| {
                let label = match granularity {
                    "minute" => crate::timefmt::minute_label(k),
                    "hour" => crate::timefmt::hour_label(k),
                    _ => crate::timefmt::day_label(k),
                };
                serde_json::json!({
                    "key": label,
                    "gw_avg_ms": table[k].gw_avg_ms(),
                    "gw_loss_pct": table[k].gw_loss_pct(),
                    "pub_avg_ms": table[k].pub_avg_ms(),
                    "pub_loss_pct": table[k].pub_loss_pct(),
                    "dns_ms": table[k].dns_avg_ms(),
                })
            })
            .collect())
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

fn merge(table: &mut QualityTable, key: &str, bucket: &QualityBucket) {
    let target = table.entry(key.to_string()).or_default();
    target.gw_total += bucket.gw_total;
    target.gw_lost += bucket.gw_lost;
    target.gw_sum_ms += bucket.gw_sum_ms;
    target.pub_total += bucket.pub_total;
    target.pub_lost += bucket.pub_lost;
    target.pub_sum_ms += bucket.pub_sum_ms;
    target.dns_count += bucket.dns_count;
    target.dns_sum_ms += bucket.dns_sum_ms;
}

// ---------------- 探测线程 ----------------

use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 探测周期（秒）：6 样本/分钟/目标，ICMP+DNS 开销可忽略。
pub const PROBE_INTERVAL_SECS: u64 = 10;
/// 网关缓存刷新周期（轮）：低频 snapshot::take（PowerShell，可能秒级阻塞）。
const GW_REFRESH_ROUNDS: u32 = 30;
/// 公网锚点：阿里 DNS，IP 直连免解析、ICMP 友好。
pub const PUBLIC_ANCHOR: &str = "223.5.5.5";
/// DNS 解析计时目标（A 记录、经系统解析器）。
const DNS_PROBE_HOST: &str = "www.baidu.com";

/// 运行态：聚合器共享（探测线程写，命令层读）。
pub type SharedQualityStore = Arc<Mutex<QualityStore>>;

/// 启动探测线程（telemetry spawn 时调用）。
/// 开关 `quality_probe_enabled` 每轮读取——关闭时只休眠不探测、不丢已有历史。
pub fn spawn_probe(store: SharedQualityStore) {
    std::thread::Builder::new()
        .name("sk-netquality".into())
        .spawn(move || {
            let mut gw_cache: Option<String> = None;
            let mut round: u32 = 0;
            loop {
                // 探测开关：关闭时跳过本轮（历史保留，重新打开即恢复）
                if !crate::settings::get_bool("quality_probe_enabled") {
                    std::thread::sleep(Duration::from_secs(PROBE_INTERVAL_SECS));
                    continue;
                }

                // 网关缓存低频刷新：snapshot::take 走 PowerShell 可能秒级阻塞，
                // 照 telemetry wifi 的超时通道模式隔离（挂起时沿用旧缓存）。
                if round % GW_REFRESH_ROUNDS == 0 {
                    let (tx, rx) = std::sync::mpsc::channel();
                    std::thread::Builder::new()
                        .name("sk-nq-gw-read".into())
                        .spawn(move || {
                            let snap = crate::snapshot::take();
                            let gw = snap
                                .physical_up_adapters()
                                .iter()
                                .find_map(|a| a.gw4.first().cloned());
                            let _ = tx.send(gw);
                        })
                        .expect("网关读取线程启动失败");
                    if let Ok(Some(gw)) = rx.recv_timeout(Duration::from_secs(3)) {
                        gw_cache = Some(gw);
                    }
                }

                // 三点探测（各自独立，互不拖累）
                let gw_ms = gw_cache
                    .as_deref()
                    .and_then(|gw| crate::icmp::ping(gw, 1000));
                let pub_ms = crate::icmp::ping(PUBLIC_ANCHOR, 1000);
                let dns_start = std::time::Instant::now();
                // resolve_system 超时返回空 Vec：空 = 解析失败，不计入 DNS 均值
                let dns_resolved =
                    !crate::dnsclient::resolve_system(DNS_PROBE_HOST, 1.5).is_empty();
                let dns_ms = dns_resolved.then(|| dns_start.elapsed().as_secs_f64() * 1000.0);

                if let Ok(mut store) = store.lock() {
                    let sample = ProbeSample {
                        gw: gw_ms,
                        public: pub_ms,
                        dns_ms,
                    };
                    store.add(&sample, timefmt::now_local());
                }
                round = round.wrapping_add(1);
                std::thread::sleep(Duration::from_secs(PROBE_INTERVAL_SECS));
            }
        })
        .expect("netquality 线程启动失败");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> QualityStore {
        QualityStore::with_path(std::env::temp_dir().join(format!(
            "sk-quality-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )))
    }

    fn sample(gw: Option<f64>, public: Option<f64>, dns: Option<f64>) -> ProbeSample {
        ProbeSample {
            gw,
            public,
            dns_ms: dns,
        }
    }

    #[test]
    fn add_aggregates_loss_and_latency() {
        let now = timefmt::now_local();
        let mut a = store();
        a.add(&sample(Some(1.0), Some(10.0), Some(5.0)), now);
        a.add(&sample(None, Some(12.0), Some(7.0)), now);
        a.add(&sample(Some(3.0), None, None), now);

        let day = timefmt::fmt(&now, timefmt::FMT_DAY);
        let b = &a.days[&day];
        assert_eq!(b.gw_total, 3);
        assert_eq!(b.gw_lost, 1);
        assert_eq!(b.pub_total, 3);
        assert_eq!(b.pub_lost, 1);
        assert_eq!(b.dns_count, 2);
        // 均值只算成功样本：gw (1+3)/2=2，pub (10+12)/2=11，dns (5+7)/2=6
        assert_eq!(b.gw_avg_ms(), Some(2.0));
        assert_eq!(b.pub_avg_ms(), Some(11.0));
        assert_eq!(b.dns_avg_ms(), Some(6.0));
        // 丢包率：1/3
        assert_eq!(b.gw_loss_pct(), Some(100.0 / 3.0));
    }

    #[test]
    fn total_outage_still_recorded() {
        let mut a = store();
        let now = timefmt::now_local();
        a.add(&sample(None, None, None), now);
        let day = timefmt::fmt(&now, timefmt::FMT_DAY);
        let b = &a.days[&day];
        assert_eq!(b.gw_total, 1);
        assert_eq!(b.gw_lost, 1);
        assert_eq!(b.gw_avg_ms(), None, "全丢包无延迟样本");
        assert_eq!(b.gw_loss_pct(), Some(100.0));
    }

    #[test]
    fn roll_hours_merges_stale_minutes() {
        let mut a = store();
        let now = timefmt::now_local();
        // 伪造一个 2 小时前的分钟桶（绕过 add 直写表，模拟跨整点遗留）
        let old = (now - chrono::Duration::hours(2))
            .format(timefmt::FMT_MINUTE)
            .to_string();
        a.minutes.insert(
            old.clone(),
            QualityBucket {
                gw_total: 6,
                gw_lost: 3,
                gw_sum_ms: 60.0,
                ..Default::default()
            },
        );
        a.add(&sample(Some(2.0), Some(2.0), None), now);
        let old_hour = old.chars().take(13).collect::<String>();
        let h = &a.hours[&old_hour];
        assert_eq!(h.gw_total, 6, "遗留分钟桶归并进小时桶");
        assert_eq!(h.gw_lost, 3);
        // 归并后原分钟桶已移走
        assert!(!a.minutes.contains_key(&old));
    }

    #[test]
    fn prune_caps_tables() {
        let mut a = store();
        let base = chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        for i in 0..(DAY_RETAIN + 10) {
            let d = base + chrono::Duration::days(i as i64);
            a.days.insert(
                d.format(timefmt::FMT_DAY).to_string(),
                QualityBucket::default(),
            );
        }
        a.prune();
        assert_eq!(a.days.len(), DAY_RETAIN);
    }

    #[test]
    fn persistence_round_trip() {
        let path = std::env::temp_dir().join(format!(
            "sk-quality-rt-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        {
            let mut a = QualityStore::with_path(path.clone());
            a.add(
                &sample(Some(4.0), Some(40.0), Some(9.0)),
                timefmt::now_local(),
            );
            assert!(a.save_if_dirty());
        }
        let b = QualityStore::with_path(path.clone());
        let now = timefmt::now_local();
        let day = timefmt::fmt(&now, timefmt::FMT_DAY);
        assert_eq!(b.days[&day].gw_sum_ms, 4.0);
        assert_eq!(b.days[&day].recv_ok_gw(), 1);
        let _ = std::fs::remove_file(path);
    }

    impl QualityBucket {
        /// 成功（非丢包）样本数——测试断言辅助。
        fn recv_ok_gw(&self) -> u32 {
            self.gw_total - self.gw_lost
        }
    }
}
