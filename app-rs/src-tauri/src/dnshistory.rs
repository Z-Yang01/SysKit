//! DNS 测速历史：落盘最近 N 轮的每台服务器平均延迟（自有格式），
//! 供测速页画趋势对比（同一台 DNS 今天 vs 上周是否劣化）。
//!
//! 纯函数核心（push/runs）+ stores::load/save 的既有 JSON 读写模式。

use serde::{Deserialize, Serialize};

/// 保留的最近轮数（每轮 6 台服务器；20 轮足够看趋势，文件 < 10KB）。
pub const KEEP_ROUNDS: usize = 20;

/// 一轮测速的紧凑记录（只存趋势需要的字段；成功率/推荐即时算更好）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DnsSpeedRound {
    /// 完成时刻（本地时区 "MM-DD HH:MM"）
    pub at: String,
    /// (服务器名, 平均延迟 ms)；全失败的台不入列（趋势上无意义）
    pub latencies: Vec<(String, f64)>,
}

/// 历史容器（有序，旧→新）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DnsSpeedHistory {
    pub rounds: Vec<DnsSpeedRound>,
}

/// 追加一轮并裁剪到 KEEP_ROUNDS（纯函数）。
pub fn push(h: &mut DnsSpeedHistory, round: DnsSpeedRound) {
    h.rounds.push(round);
    let excess = h.rounds.len().saturating_sub(KEEP_ROUNDS);
    if excess > 0 {
        h.rounds.drain(0..excess);
    }
}

/// 某台服务器的历史延迟序列（旧→新，只含有该台记录的轮）。
pub fn series_for(h: &DnsSpeedHistory, server: &str) -> Vec<(String, f64)> {
    h.rounds
        .iter()
        .filter_map(|r| {
            r.latencies
                .iter()
                .find(|(n, _)| n == server)
                .map(|(_, ms)| (r.at.clone(), *ms))
        })
        .collect()
}

/// 读取/落盘（复用 stores::load_json 的损坏回落默认语义）。
pub fn load(path: &std::path::PathBuf) -> DnsSpeedHistory {
    let raw = crate::stores::load_json(path, serde_json::json!({ "rounds": [] }));
    serde_json::from_value(raw).unwrap_or_default()
}

pub fn save(path: &std::path::PathBuf, h: &DnsSpeedHistory) -> bool {
    let v = serde_json::to_value(h).unwrap_or(serde_json::json!({ "rounds": [] }));
    crate::stores::save_json(path, &v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(at: &str, lats: &[(&str, f64)]) -> DnsSpeedRound {
        DnsSpeedRound {
            at: at.to_string(),
            latencies: lats.iter().map(|(n, m)| (n.to_string(), *m)).collect(),
        }
    }

    #[test]
    fn push_trims_to_keep_rounds() {
        let mut h = DnsSpeedHistory::default();
        for i in 0..(KEEP_ROUNDS + 5) {
            push(&mut h, round(&format!("r{i}"), &[("阿里", 10.0)]));
        }
        assert_eq!(h.rounds.len(), KEEP_ROUNDS);
        assert_eq!(h.rounds.first().unwrap().at, "r5", "最旧的被裁掉");
        assert_eq!(h.rounds.last().unwrap().at, format!("r{}", KEEP_ROUNDS + 4));
    }

    #[test]
    fn series_for_only_includes_rounds_with_that_server() {
        let mut h = DnsSpeedHistory::default();
        push(&mut h, round("t1", &[("阿里", 10.0), ("腾讯", 20.0)]));
        push(&mut h, round("t2", &[("腾讯", 25.0)])); // 阿里这轮全失败没记录
        push(&mut h, round("t3", &[("阿里", 12.0)]));
        let ali = series_for(&h, "阿里");
        assert_eq!(ali, vec![("t1".into(), 10.0), ("t3".into(), 12.0)]);
        let tx = series_for(&h, "腾讯");
        assert_eq!(tx, vec![("t1".into(), 20.0), ("t2".into(), 25.0)]);
        assert!(series_for(&h, "不存在").is_empty());
    }

    #[test]
    fn load_save_roundtrip_and_corrupt_falls_back() {
        let p = std::env::temp_dir().join(format!(
            "sk-dnshist-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut h = DnsSpeedHistory::default();
        push(&mut h, round("t1", &[("阿里", 10.0)]));
        assert!(save(&p, &h));
        let back = load(&p);
        assert_eq!(back, h);
        // 损坏文件回落空历史（不 panic）
        std::fs::write(&p, b"{ not json").unwrap();
        assert_eq!(load(&p), DnsSpeedHistory::default());
        let _ = std::fs::remove_file(&p);
    }
}
