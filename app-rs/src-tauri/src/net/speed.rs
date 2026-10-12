//! DNS 测速：并发测各公共 DNS 的解析延迟/成功率，推荐最优（对应 Python 侧 DnsSpeedTab）。
//!
//! 与 Python 同语义：**每台服务器串行 N 次**（避免自己打自己的并发干扰），
//! 服务器之间并发（单台超时不拖总时长）；建议值在"至少成功一次"的服务器里取平均
//! 延迟最小者。
//!
//! 说明：`下载测速`（SpeedTestTab）依赖 HTTPS 下载，Rust 侧**尚无 TLS**
//! （标准库不提供，需引入 rustls/native-tls 并做依赖决策），故本轮只交付 DNS 测速；
//! 下载测速待 TLS 决策后接入，界面会明确标注而不是给个假结果。

use std::time::Instant;

use serde::Serialize;

use crate::net::client;

/// 被测 DNS 服务器（与 Python `_DNS_SPEED_SERVERS` 逐项一致）。
pub const DNS_SPEED_SERVERS: [(&str, &str); 6] = [
    ("223.5.5.5", "阿里"),
    ("119.29.29.29", "腾讯"),
    ("114.114.114.114", "114"),
    ("180.76.76.76", "百度"),
    ("1.1.1.1", "Cloudflare"),
    ("8.8.8.8", "Google"),
];

/// 单台 DNS 的探测结果。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DnsProbe {
    pub server: String,
    pub name: String,
    /// 平均延迟（仅统计成功的次数）；全失败为 None
    pub avg_ms: Option<f64>,
    pub ok_count: u32,
    pub total: u32,
    /// 是否被推荐为最优
    pub recommended: bool,
}

/// 结果是否可用（至少成功一次）。
pub fn is_usable(p: &DnsProbe) -> bool {
    p.avg_ms.is_some() && p.ok_count > 0
}

/// 成功率（0.0~1.0）。
pub fn success_rate(p: &DnsProbe) -> f64 {
    if p.total == 0 {
        0.0
    } else {
        p.ok_count as f64 / p.total as f64
    }
}

/// 在可用服务器里标出平均延迟最小者。返回被推荐的下标（无可用时 None）。
///
/// 平手时取先出现的（与 Python 的 `min` 稳定语义一致）。
pub fn mark_recommended(results: &mut [DnsProbe]) -> Option<usize> {
    for r in results.iter_mut() {
        r.recommended = false;
    }
    let best = results
        .iter()
        .enumerate()
        .filter(|(_, p)| is_usable(p))
        .min_by(|(_, a), (_, b)| {
            a.avg_ms
                .partial_cmp(&b.avg_ms)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i);
    if let Some(i) = best {
        results[i].recommended = true;
    }
    best
}

/// 探测一台服务器（串行 `times` 次）。
///
/// 单次超时按失败计入（**不重试**——测速要的是真实一次往返的表现，
/// 重发会把丢包服务器测成"慢但成功"，与 Python `attempts=1` 同口径）。
pub fn probe_server(server: &str, name: &str, domain: &str, times: u32) -> DnsProbe {
    let times = times.clamp(1, 5);
    let mut latencies: Vec<f64> = Vec::new();
    for _ in 0..times {
        let start = Instant::now();
        let ok = client::query_with(domain, server, 1.5, client::QTYPE_A, 1).is_ok();
        if ok {
            latencies.push(start.elapsed().as_secs_f64() * 1000.0);
        }
    }
    let ok_count = latencies.len() as u32;
    let avg_ms = if latencies.is_empty() {
        None
    } else {
        Some(latencies.iter().sum::<f64>() / latencies.len() as f64)
    };
    DnsProbe {
        server: server.to_string(),
        name: name.to_string(),
        avg_ms,
        ok_count,
        total: times,
        recommended: false,
    }
}

/// 并发测全部预设服务器，返回按平均延迟升序排列的结果（全失败的排在最后）。
pub fn run_all(domain: &str, times: u32) -> Vec<DnsProbe> {
    let domain = domain.trim().to_string();
    if domain.is_empty() {
        return Vec::new();
    }
    let handles: Vec<_> = DNS_SPEED_SERVERS
        .iter()
        .map(|(server, name)| {
            let d = domain.clone();
            let s = server.to_string();
            let n = name.to_string();
            std::thread::spawn(move || probe_server(&s, &n, &d, times))
        })
        .collect();
    let mut results: Vec<DnsProbe> = handles.into_iter().filter_map(|h| h.join().ok()).collect();
    sort_results(&mut results);
    mark_recommended(&mut results);
    results
}

/// 排序：可用的按平均延迟升序在前，全失败的在后（保持原顺序）。
pub fn sort_results(results: &mut [DnsProbe]) {
    results.sort_by(|a, b| match (a.avg_ms, b.avg_ms) {
        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
}

/// 推荐文案（对应 Python 的 lbl_recommend）。
pub fn recommendation_text(results: &[DnsProbe]) -> String {
    if results.is_empty() {
        return "尚未测速".to_string();
    }
    let usable: Vec<&DnsProbe> = results.iter().filter(|p| is_usable(p)).collect();
    if usable.is_empty() {
        return "全部 DNS 均无响应——可能是网络屏蔽了 53 端口，或当前无网络".to_string();
    }
    let best = usable[0];
    format!(
        "推荐使用 {}（{}），平均 {:.0} ms，成功率 {:.0}%",
        best.server,
        best.name,
        best.avg_ms.unwrap_or(0.0),
        success_rate(best) * 100.0
    )
}

/// 是否为"可用"的 DNS（供前端标注）。
pub fn summarize(results: &[DnsProbe]) -> (usize, usize) {
    let usable = results.iter().filter(|p| is_usable(p)).count();
    (usable, results.len() - usable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    fn probe(server: &str, avg: Option<f64>, ok: u32, total: u32) -> DnsProbe {
        DnsProbe {
            server: server.into(),
            name: "x".into(),
            avg_ms: avg,
            ok_count: ok,
            total,
            recommended: false,
        }
    }

    #[test]
    fn server_list_matches_python() {
        assert_eq!(DNS_SPEED_SERVERS.len(), 6);
        let expect = [
            ("223.5.5.5", "阿里"),
            ("119.29.29.29", "腾讯"),
            ("114.114.114.114", "114"),
            ("180.76.76.76", "百度"),
            ("1.1.1.1", "Cloudflare"),
            ("8.8.8.8", "Google"),
        ];
        for (i, (s, n)) in expect.iter().enumerate() {
            assert_eq!(DNS_SPEED_SERVERS[i].0, *s, "第 {i} 台服务器地址");
            assert_eq!(DNS_SPEED_SERVERS[i].1, *n, "第 {i} 台显示名");
        }
        // 全部必须是合法 IP（拼错会静默测不通）
        for (s, _) in DNS_SPEED_SERVERS {
            assert!(s.parse::<IpAddr>().is_ok(), "{s} 不是合法 IP");
        }
    }

    #[test]
    fn success_rate_basic() {
        assert_eq!(success_rate(&probe("a", Some(10.0), 3, 3)), 1.0);
        assert_eq!(success_rate(&probe("a", Some(10.0), 1, 4)), 0.25);
        assert_eq!(success_rate(&probe("a", None, 0, 3)), 0.0);
        assert_eq!(
            success_rate(&probe("a", None, 0, 0)),
            0.0,
            "total=0 不得除零"
        );
    }

    #[test]
    fn usable_requires_success() {
        assert!(is_usable(&probe("a", Some(1.0), 1, 3)));
        assert!(!is_usable(&probe("a", None, 0, 3)));
        assert!(
            !is_usable(&probe("a", Some(1.0), 0, 3)),
            "有平均但零成功属自相矛盾"
        );
    }

    #[test]
    fn mark_recommended_picks_lowest_average() {
        let mut rs = vec![
            probe("slow", Some(90.0), 3, 3),
            probe("fast", Some(12.0), 3, 3),
            probe("mid", Some(40.0), 3, 3),
        ];
        let idx = mark_recommended(&mut rs);
        assert_eq!(idx, Some(1));
        assert!(rs[1].recommended);
        assert!(!rs[0].recommended && !rs[2].recommended);
    }

    #[test]
    fn mark_recommended_ignores_all_failed() {
        let mut rs = vec![probe("dead", None, 0, 3), probe("alive", Some(200.0), 1, 3)];
        let idx = mark_recommended(&mut rs);
        assert_eq!(idx, Some(1), "全失败的不能因为「没有延迟」被当最优");
        assert!(!rs[0].recommended);
    }

    #[test]
    fn mark_recommended_none_when_all_failed() {
        let mut rs = vec![probe("a", None, 0, 3), probe("b", None, 0, 3)];
        assert_eq!(mark_recommended(&mut rs), None);
        assert!(rs.iter().all(|p| !p.recommended));
    }

    #[test]
    fn mark_recommended_is_idempotent_and_clears_stale_flags() {
        let mut rs = vec![probe("a", Some(10.0), 3, 3), probe("b", Some(20.0), 3, 3)];
        mark_recommended(&mut rs);
        assert!(rs[0].recommended);
        // 把 a 变慢后重算：旧推荐标记必须被清掉
        rs[0].avg_ms = Some(99.0);
        mark_recommended(&mut rs);
        assert!(!rs[0].recommended, "陈旧的推荐标记必须清除");
        assert!(rs[1].recommended);
    }

    #[test]
    fn mark_recommended_tie_takes_first() {
        let mut rs = vec![probe("a", Some(10.0), 3, 3), probe("b", Some(10.0), 3, 3)];
        assert_eq!(mark_recommended(&mut rs), Some(0), "平手取先出现的");
    }

    #[test]
    fn sort_puts_usable_first_then_failures() {
        let mut rs = vec![
            probe("dead1", None, 0, 3),
            probe("slow", Some(90.0), 3, 3),
            probe("dead2", None, 0, 3),
            probe("fast", Some(12.0), 3, 3),
        ];
        sort_results(&mut rs);
        assert_eq!(
            rs.iter().map(|p| p.server.as_str()).collect::<Vec<_>>(),
            vec!["fast", "slow", "dead1", "dead2"],
            "可用的按延迟升序在前，全失败的保持相对顺序在后"
        );
    }

    #[test]
    fn recommendation_text_variants() {
        let mut rs = vec![
            probe("fast", Some(12.4), 3, 3),
            probe("slow", Some(90.0), 1, 3),
        ];
        mark_recommended(&mut rs);
        rs[0].name = "阿里".into();
        let t = recommendation_text(&rs);
        assert!(t.contains("223.5.5.5") || t.contains("fast"), "{t}");
        assert!(t.contains("阿里"), "{t}");
        assert!(t.contains("12 ms"), "平均延迟取整: {t}");
        assert!(t.contains("100%"), "成功率: {t}");

        let dead = vec![probe("a", None, 0, 3)];
        assert!(recommendation_text(&dead).contains("全部 DNS 均无响应"));
        assert_eq!(recommendation_text(&[]), "尚未测速");
    }

    #[test]
    fn summarize_counts() {
        let rs = vec![
            probe("a", Some(10.0), 3, 3),
            probe("b", Some(20.0), 1, 3),
            probe("c", None, 0, 3),
        ];
        assert_eq!(summarize(&rs), (2, 1));
        assert_eq!(summarize(&[]), (0, 0));
    }

    #[test]
    fn run_all_rejects_empty_domain() {
        assert!(run_all("", 3).is_empty());
        assert!(run_all("   ", 3).is_empty());
    }

    #[test]
    fn probe_server_times_clamped() {
        // 次数被夹到 1..=5：既不为 0 也不无限跑
        let p = probe_server("203.0.113.1", "TEST-NET", "example.com", 0);
        assert_eq!(p.total, 1, "0 次应夹到 1");
        assert_eq!(p.ok_count, 0);
        assert!(p.avg_ms.is_none());
    }

    #[test]
    fn probe_server_unreachable_fails_without_panic() {
        // TEST-NET-3 保证不可达：应为全失败而不是 panic 或假成功
        let p = probe_server("203.0.113.1", "TEST-NET", "example.com", 2);
        assert_eq!(p.total, 2);
        assert_eq!(p.ok_count, 0);
        assert!(p.avg_ms.is_none());
        assert!(!is_usable(&p));
    }

    #[test]
    fn live_dns_speed_run_produces_ranked_results() {
        // 唯一真正走网络的用例：断网时允许全失败，但不得 panic，
        // 且排序/推荐必须自洽
        let rs = run_all("www.baidu.com", 2);
        assert_eq!(rs.len(), 6, "六台服务器都要有结果行");
        // 排序自洽：可用的都在前面
        let first_failed = rs.iter().position(|p| !is_usable(p));
        if let Some(i) = first_failed {
            assert!(
                rs[i..].iter().all(|p| !is_usable(p)),
                "全失败的必须都排在可用项之后"
            );
        }
        // 推荐标记唯一
        assert!(rs.iter().filter(|p| p.recommended).count() <= 1);
        if let Some(rec) = rs.iter().find(|p| p.recommended) {
            assert!(is_usable(rec), "推荐项必须是可用项");
            assert_eq!(rec, &rs[0], "推荐项应排在首位");
        }
    }
}
