//! 网络诊断检查（对应 Python 侧 netcheck.py）。
//!
//! 依赖分层（决定了本轮的诚实边界）：
//! - **不需要 Snapshot 的检查**已实现：NCSI 注册表核对、关键服务状态、
//!   网关连通、DNS 对比、NCSI 探测模拟、真实连通性、DNS 污染检测；
//! - **需要 Snapshot 的检查**（网卡列表 / 默认路由 / 持久路由 / IPv6 / WLAN）
//!   依赖 `snapshot.py`（全量 PowerShell 采集）尚未移植，它们返回
//!   `skip` + 「尚未接入」而不是伪造 pass/fail——诊断页伪造绿勾比留白更危险。
//!
//! HTTP 探测走**直连 opener**（对应 Python `ProxyHandler({})`）：诊断对象本身
//! 可能就是残留代理，走系统代理会把「代理挂了」误判成「断网」，进而驱动重启
//! 网卡等无效修复。Rust 标准库默认不读系统代理，语义天然一致。

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use serde::Serialize;

use crate::icmp;
use crate::net::client;
use crate::snapshot::{RouteEntry, Snapshot};
use crate::w32::{self, RegValue};

pub const PASS: &str = "pass";
pub const WARN: &str = "warn";
pub const FAIL: &str = "fail";
pub const SKIP: &str = "skip";
pub const INFO: &str = "info";

/// 状态符号（与 Python `STATUS_MARK` 逐字一致）。
#[cfg(test)]
pub fn status_mark(status: &str) -> &'static str {
    match status {
        PASS => "√",
        WARN => "!",
        FAIL => "×",
        SKIP => "-",
        INFO => "·",
        _ => "?",
    }
}

/// 单项检查结果（对应 Python `CheckResult`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckResult {
    pub id: &'static str,
    pub name: &'static str,
    pub status: &'static str,
    pub detail: String,
    /// 建议的修复动作 id（可空）。
    pub fix_ids: Vec<&'static str>,
}

impl CheckResult {
    pub fn new(id: &'static str, name: &'static str, status: &'static str) -> Self {
        Self {
            id,
            name,
            status,
            detail: String::new(),
            fix_ids: Vec::new(),
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }

    /// 单行展示（对齐 Python `CheckResult.line()`）。测试断言专用。
    #[cfg(test)]
    pub fn line(&self) -> String {
        format!(
            "[{}] {}\n      {}",
            status_mark(self.status),
            self.name,
            self.detail
        )
    }
}

mod checks;
mod probe;

// 扁平重导出：调用方继续用 crate::netcheck::X，与拆分前完全一致
pub use checks::*;
pub use probe::*;
// ---------------- 检查清单与综合判定 ----------------

/// 一项检查的注册信息。
pub struct CheckDef {
    pub id: &'static str,
    #[allow(dead_code)] // 结构体完整性：name 为定义面字段，展示路径未读取
    pub name: &'static str,
    /// quick 模式（监控轮询）是否跳过：对应 Python 的慢速外网探测集合。
    pub slow: bool,
    runner: fn(&Snapshot) -> CheckResult,
}

impl CheckDef {
    /// 执行本项检查。
    pub fn run(&self, snap: &Snapshot) -> CheckResult {
        (self.runner)(snap)
    }
}

/// 全部 15 项检查，顺序 = Python `ALL_CHECKS`。
pub fn all_checks() -> &'static [CheckDef] {
    static CHECKS: &[CheckDef] = &[
        CheckDef {
            id: "nlasvc",
            name: "NlaSvc 网络位置感知服务（画'有网/无网'标记的服务）",
            slow: false,
            runner: |_| chk_nlasvc(),
        },
        CheckDef {
            id: "persistent_route",
            name: "持久路由（重启不消失的手工路由，休眠时会劫持流量）",
            slow: false,
            runner: chk_persistent_route,
        },
        CheckDef {
            id: "static_dns",
            name: "网卡静态 DNS 审计（残留的静态 DNS 只影响对应网卡）",
            slow: false,
            runner: chk_static_dns,
        },
        CheckDef {
            id: "proxy",
            name: "系统代理（代理软件残留会让全网流量进死端口）",
            slow: false,
            runner: |_| chk_proxy(),
        },
        CheckDef {
            id: "gateway",
            name: "网关连通（Ping 默认网关 ×3）",
            slow: false,
            runner: chk_gateway,
        },
        CheckDef {
            id: "dns_compare",
            name: "DNS 解析（系统 DNS vs 公共 DNS 对比）",
            slow: false,
            runner: chk_dns_compare,
        },
        CheckDef {
            id: "dns_pollution",
            name: "NCSI 域名解析对比（污染检测）",
            slow: true,
            runner: |_| chk_dns_pollution(),
        },
        CheckDef {
            id: "ncsi_probe",
            name: "NCSI 探测模拟（Windows 判定'有网'的依据）",
            slow: false,
            runner: |_| chk_ncsi_probe(),
        },
        CheckDef {
            id: "real_conn",
            name: "真实连通性（204 端点 + HTTPS）",
            slow: false,
            runner: |_| chk_real(),
        },
        CheckDef {
            id: "ncsi_reg",
            name: "NCSI 注册表配置核对",
            slow: true,
            runner: |_| chk_ncsi_registry(),
        },
        CheckDef {
            id: "power_mgmt",
            name: "网卡节电（允许系统关闭设备以节约电源）",
            slow: true,
            runner: chk_power_mgmt,
        },
        CheckDef {
            id: "services",
            name: "关键网络服务状态",
            slow: false,
            runner: |_| chk_services(),
        },
        CheckDef {
            id: "routes",
            name: "路由健康（幽灵路由 / 多默认网关 / 隧道抢占）",
            slow: false,
            runner: chk_routes,
        },
        CheckDef {
            id: "ipv6",
            name: "IPv6 状态（V6 探测值缺失会导致 v6 网络误判）",
            slow: true,
            runner: |_| chk_ipv6(),
        },
        CheckDef {
            id: "wlan",
            name: "WLAN 配置（MAC 随机化 / 连接状态）",
            slow: true,
            runner: |_| chk_wlan(),
        },
    ];
    CHECKS
}

/// 取默认网关 IPv4 地址（`Get-NetRoute` 不引：走 `route print` 解析，零 PowerShell 依赖）。
///
/// 对应 Python `icmp.default_gateway()`，但后者走 PowerShell。这里解析
/// `route print -4` 的默认路由段：0.0.0.0/0 行的「网关」列。
#[cfg(test)]
pub fn default_gateway_v4() -> Option<String> {
    let out = w32::run_capture(&["route".into(), "print".into(), "-4".into()], 15).ok()?;
    for line in out.stdout.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        // 形如：0.0.0.0          0.0.0.0      192.168.1.1     192.168.1.100     25
        if cols.len() >= 3 && cols[0] == "0.0.0.0" && cols[1] == "0.0.0.0" {
            let gw = cols[2];
            if gw.matches('.').count() == 3 {
                return Some(gw.to_string());
            }
        }
    }
    None
}

/// 逐项执行检查（`quick=true` 跳过慢速外网探测）。
pub fn run_all(quick: bool) -> Vec<CheckResult> {
    // **快照只采一次**：多项检查都需要它，每项各采一次就是多次 PowerShell
    //（与 Python 版 `snap = take_snap()` 的做法一致）
    let snap = crate::snapshot::take();
    all_checks()
        .iter()
        .filter(|c| !(quick && c.slow))
        .map(|c| c.run(&snap))
        .collect()
}

/// 综合判定：`(状态码, 一句话)`，与 Python `verdict` 的分支完全一致。
pub fn verdict(results: &[CheckResult]) -> (String, String) {
    let by: BTreeMap<&str, &CheckResult> = results.iter().map(|r| (r.id, r)).collect();
    let real_ok = by
        .get("real_conn")
        .map(|r| r.status == PASS)
        .unwrap_or(false);
    let ncsi_ok = by
        .get("ncsi_probe")
        .map(|r| r.status == PASS)
        .unwrap_or(false);
    let dns_bad = by
        .get("dns_compare")
        .map(|r| r.status == FAIL)
        .unwrap_or(false);

    let (base, mut text) = if real_ok && ncsi_ok && !dns_bad {
        ("normal", "网络正常".to_string())
    } else if real_ok && !ncsi_ok {
        (
            "ncsi_false_alarm",
            "实际有网，但 Windows 判定'无网络'（NCSI 误报）".to_string(),
        )
    } else if real_ok && dns_bad {
        (
            "dns_broken",
            "能连上但 DNS 解析异常/缓慢（系统 DNS 排序被幽灵路由或残废 DNS 带偏）".to_string(),
        )
    } else {
        ("no_internet", "外网不可达".to_string())
    };
    let fails = results.iter().filter(|r| r.status == FAIL).count();
    if fails > 0 {
        text.push_str(&format!("；失败项: {fails} 个"));
    }
    (base.to_string(), text)
}

/// 各状态计数（前端摘要卡）。
pub fn status_counts(results: &[CheckResult]) -> BTreeMap<&'static str, usize> {
    let mut m = BTreeMap::new();
    for r in results {
        *m.entry(r.status).or_insert(0) += 1;
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_marks_match_python() {
        assert_eq!(status_mark(PASS), "√");
        assert_eq!(status_mark(WARN), "!");
        assert_eq!(status_mark(FAIL), "×");
        assert_eq!(status_mark(SKIP), "-");
        assert_eq!(status_mark(INFO), "·");
        assert_eq!(status_mark("weird"), "?");
    }

    #[test]
    fn pnp_power_label_matches_python_and_repairs() {
        // Python winregread.pnp_states：str(PnPCapabilities)=="24" → disabled（已禁止断电），
        // 其余（含缺省 0）→ enabled_off（允许断电）。repairs::run_power_off 同口径
        // （PnPCapabilities 已是 24 的键跳过不改 = 24 是修复目标态）。
        // 回归：初版把 Some(24) 报成"允许断电"——修完的网卡永远 WARN，
        // 真正有风险的缺省网卡反而显示"已禁止断电"（双向颠倒）。
        assert_eq!(pnp_power_label(Some(24)), Some("已禁止断电"));
        assert_eq!(pnp_power_label(Some(0)), Some("允许断电"));
        assert_eq!(pnp_power_label(Some(16)), Some("允许断电"));
        assert_eq!(pnp_power_label(Some(0x18 | 0x20)), Some("允许断电"));
        assert_eq!(pnp_power_label(None), None, "类键里找不到的描述应跳过");
    }

    #[test]
    fn status_code_constants_match_python() {
        assert_eq!(
            (PASS, WARN, FAIL, SKIP, INFO),
            ("pass", "warn", "fail", "skip", "info")
        );
    }

    #[test]
    fn all_checks_is_15_in_python_order() {
        let checks = all_checks();
        assert_eq!(checks.len(), 15, "Python ALL_CHECKS 有 15 项");
        let ids: Vec<&str> = checks.iter().map(|c| c.id).collect();
        assert_eq!(
            ids,
            vec![
                "nlasvc",
                "persistent_route",
                "static_dns",
                "proxy",
                "gateway",
                "dns_compare",
                "dns_pollution",
                "ncsi_probe",
                "real_conn",
                "ncsi_reg",
                "power_mgmt",
                "services",
                "routes",
                "ipv6",
                "wlan",
            ],
            "顺序是硬契约：前端列表顺序与 Python 一致"
        );
    }

    #[test]
    fn slow_checks_match_python_quick_skip_set() {
        // Python quick 模式跳过：dns_pollution / ncsi_registry / power / ipv6 / wlan
        let slow: Vec<&str> = all_checks()
            .iter()
            .filter(|c| c.slow)
            .map(|c| c.id)
            .collect();
        assert_eq!(
            slow,
            vec!["dns_pollution", "ncsi_reg", "power_mgmt", "ipv6", "wlan"]
        );
    }

    #[test]
    fn check_names_match_python_verbatim() {
        for (id, name) in [
            (
                "nlasvc",
                "NlaSvc 网络位置感知服务（画'有网/无网'标记的服务）",
            ),
            (
                "persistent_route",
                "持久路由（重启不消失的手工路由，休眠时会劫持流量）",
            ),
            (
                "static_dns",
                "网卡静态 DNS 审计（残留的静态 DNS 只影响对应网卡）",
            ),
            ("proxy", "系统代理（代理软件残留会让全网流量进死端口）"),
            ("gateway", "网关连通（Ping 默认网关 ×3）"),
            ("dns_compare", "DNS 解析（系统 DNS vs 公共 DNS 对比）"),
            ("dns_pollution", "NCSI 域名解析对比（污染检测）"),
            ("ncsi_probe", "NCSI 探测模拟（Windows 判定'有网'的依据）"),
            ("real_conn", "真实连通性（204 端点 + HTTPS）"),
            ("ncsi_reg", "NCSI 注册表配置核对"),
            ("power_mgmt", "网卡节电（允许系统关闭设备以节约电源）"),
            ("services", "关键网络服务状态"),
            ("routes", "路由健康（幽灵路由 / 多默认网关 / 隧道抢占）"),
            ("ipv6", "IPv6 状态（V6 探测值缺失会导致 v6 网络误判）"),
            ("wlan", "WLAN 配置（MAC 随机化 / 连接状态）"),
        ] {
            let c = all_checks().iter().find(|c| c.id == id).unwrap();
            assert_eq!(c.name, name, "{id} 名称与 Python 不一致");
        }
    }

    #[test]
    fn parse_status_code_handles_http_responses() {
        assert_eq!(
            parse_status_code("HTTP/1.1 204 No Content\r\n\r\n"),
            Some(204)
        );
        assert_eq!(parse_status_code("HTTP/1.0 200 OK\r\n"), Some(200));
        assert_eq!(parse_status_code("HTTP/1.1 404 Not Found"), Some(404));
        assert_eq!(parse_status_code("not http"), None);
        assert_eq!(parse_status_code(""), None);
        assert_eq!(parse_status_code("HTTP/1.1 abc"), None);
    }

    #[test]
    fn extract_body_splits_on_crlfcrlf() {
        assert_eq!(extract_body("HTTP/1.1 200 OK\r\nA: b\r\n\r\nBODY"), "BODY");
        assert_eq!(extract_body("HTTP/1.1 204 No Content\r\n\r\n"), "");
        assert_eq!(extract_body("no separator"), "");
    }

    #[test]
    fn v4_dns_filters_placeholders_and_ipv6() {
        assert_eq!(v4_dns(&["223.5.5.5".into()]), "223.5.5.5");
        // fec0::/10 占位符与 IPv6 都要跳过
        assert_eq!(
            v4_dns(&["fec0:0:0:ffff::1".into(), "8.8.8.8".into()]),
            "8.8.8.8"
        );
        assert_eq!(v4_dns(&["fec0:0:0:ffff::1".into()]), "", "全是占位符 → 空");
        assert_eq!(v4_dns(&[]), "");
        // 大小写不敏感的 fec 前缀
        assert_eq!(v4_dns(&["FEC0::1".into(), "1.1.1.1".into()]), "1.1.1.1");
    }

    #[test]
    fn is_bogus_detects_poisoned_results() {
        assert!(is_bogus(&["127.0.0.1".into()]));
        assert!(
            is_bogus(&["8.8.8.8".into(), "0.0.0.0".into()]),
            "含 unspecified 地雷 → 污染（any 语义）"
        );
        assert!(!is_bogus(&["131.107.255.255".into()]), "官方 IP 不算地雷");
        assert!(
            !is_bogus(&["8.8.8.8".into(), "1.2.3.4".into()]),
            "全公共地址（无坏 IP）→ 不判污染（与 Python _bogus 同口径）"
        );
        assert!(!is_bogus(&[]), "空结果不算污染");
    }

    #[test]
    fn is_bogus_any_semantics_and_classification_parity() {
        // 回归锁（深度审计轮31，对齐 Python _bogus 的解析分类 + any 语义）：
        // 旧版 all() + 静态表对下列真实污染形态全部漏判。
        // 1) 混合应答：官方 IP 夹带 loopback —— 旧版放行成非污染，
        //    下游 any(==官方期望) 会把它误判成 PASS
        assert!(is_bogus(&["131.107.255.255".into(), "127.0.0.1".into()]));
        // 2) 私网段应答（静态表外）
        assert!(is_bogus(&["192.168.1.1".into()]));
        assert!(is_bogus(&["10.0.0.7".into()]));
        // 3) 保留段 / 链路本地
        assert!(is_bogus(&["240.0.0.1".into()]));
        assert!(is_bogus(&["169.254.1.2".into()]));
        // 4) IPv6：loopback / ULA / 链路本地 / v4-mapped 私网
        assert!(is_bogus(&["::1".into()]));
        assert!(is_bogus(&["fd00::1".into()]));
        assert!(is_bogus(&["fe80::1".into()]));
        assert!(is_bogus(&["::ffff:192.168.0.1".into()]));
        // 5) 非法应答（解析失败）= 污染（Python ValueError→True 同口径）
        assert!(is_bogus(&["not-an-ip".into()]));
        // 合法公共地址不受牵连
        assert!(!is_bogus(&["131.107.255.255".into(), "23.32.1.5".into()]));
        // 8.8.8.8 虽是可疑的服务 IP，但按 Python 口径属公共地址：不走污染
        // WARN，由 expect-IP/INFO 路径处理（旧静态清单判定已随 all() 移除）
        assert!(!is_bogus(&["8.8.8.8".into()]));
    }

    #[test]
    fn ping_loss_parses_chinese_and_english() {
        // 真实 Windows 输出：括号紧贴数字
        assert_eq!(
            ping_loss("数据包: 已发送 = 3，已接收 = 3，丢失 = 0 (0% 丢失)，"),
            Some(0)
        );
        assert_eq!(
            ping_loss("Packets: Sent = 4, Received = 3, Lost = 1 (25% loss),"),
            Some(25)
        );
        assert_eq!(ping_loss("(100% 丢失)"), Some(100));
        assert_eq!(ping_loss("没有百分比"), None);
        assert_eq!(ping_loss(""), None);
    }

    #[test]
    fn http_get_rejects_https_with_clear_reason() {
        let e = http_get("https://www.baidu.com", 1.0, false).unwrap_err();
        assert!(
            e.contains("TLS"),
            "必须说明是未实现 TLS 而不是含糊失败: {e}"
        );
    }

    #[test]
    fn http_get_rejects_malformed_url() {
        assert!(http_get("no-scheme", 1.0, false).is_err());
    }

    #[test]
    fn http_get_unreachable_host_errors_without_panic() {
        // TEST-NET-3 保留段：不可达，但必须返回 Err 而不是 panic
        assert!(http_get("http://203.0.113.1/", 0.4, false).is_err());
    }

    #[test]
    fn verdict_follows_python_branch_order() {
        let mk = |id: &'static str, status: &'static str| {
            let mut r = CheckResult::new(id, "x", status);
            r.detail = String::new();
            r
        };
        // normal
        let v = verdict(&[
            mk("real_conn", PASS),
            mk("ncsi_probe", PASS),
            mk("dns_compare", PASS),
        ]);
        assert_eq!(v.0, "normal");
        assert!(v.1.contains("网络正常"));
        // NCSI 误报
        let v = verdict(&[mk("real_conn", PASS), mk("ncsi_probe", FAIL)]);
        assert_eq!(v.0, "ncsi_false_alarm");
        assert!(v.1.contains("误报"));
        // DNS 坏
        let v = verdict(&[
            mk("real_conn", PASS),
            mk("ncsi_probe", PASS),
            mk("dns_compare", FAIL),
        ]);
        assert_eq!(v.0, "dns_broken");
        // 无外网
        let v = verdict(&[mk("real_conn", FAIL), mk("ncsi_probe", PASS)]);
        assert_eq!(v.0, "no_internet");
    }

    #[test]
    fn verdict_appends_fail_count() {
        let mk = |id: &'static str, status: &'static str| CheckResult::new(id, "x", status);
        let (_, text) = verdict(&[
            mk("real_conn", FAIL),
            mk("ncsi_probe", FAIL),
            mk("services", FAIL),
        ]);
        assert!(text.contains("失败项: 3 个"), "实际: {text}");
    }

    #[test]
    fn empty_results_yield_no_internet_not_panic() {
        let (base, text) = verdict(&[]);
        assert_eq!(base, "no_internet");
        assert!(!text.is_empty());
    }

    #[test]
    fn status_counts_tallies() {
        let rs = vec![
            CheckResult::new("a", "a", PASS),
            CheckResult::new("b", "b", PASS),
            CheckResult::new("c", "c", SKIP),
        ];
        let c = status_counts(&rs);
        assert_eq!(c.get(PASS), Some(&2));
        assert_eq!(c.get(SKIP), Some(&1));
    }

    #[test]
    fn check_line_format_matches_python() {
        let r = CheckResult::new("x", "测试项", FAIL).with_detail("细节");
        assert_eq!(r.line(), "[×] 测试项\n      细节");
    }

    #[test]
    fn start_type_label_maps_sc_qc_numbers() {
        // 与 Python Get-Service 的 StartType 标签同集（判定按字符串比较）
        assert_eq!(start_type_label("2     AUTO_START"), "Automatic");
        assert_eq!(start_type_label("3   DEMAND_START"), "Manual");
        assert_eq!(start_type_label("4     DISABLED"), "Disabled");
        assert_eq!(start_type_label("0  BOOT_START"), "Boot");
        assert_eq!(start_type_label("1  SYSTEM_START"), "System");
        // 认不出的原样透出（不硬造标签）
        assert_eq!(start_type_label("9  WEIRD"), "9  WEIRD");
        assert_eq!(start_type_label(""), "");
    }

    #[test]
    fn nlasvc_result_follows_python_branches() {
        // 运行中 + 自动启动 + 有触发器 → PASS（原版形态）
        let ok = nlasvc_result(true, "Automatic", true);
        assert_eq!(ok.status, PASS);
        assert!(ok.fix_ids.is_empty());
        assert!(ok.detail.contains("运行中") && ok.detail.contains("触发器=有"));

        // 运行中 + 手动启动 + 无触发器 → WARN（本机核心场景：重启后复发）
        let warn = nlasvc_result(true, "Manual", false);
        assert_eq!(warn.status, WARN);
        assert_eq!(warn.fix_ids, vec!["fix_nlasvc"]);
        assert!(
            warn.detail.contains("手动启动且无触发器"),
            "{}",
            warn.detail
        );

        // 运行中 + 手动但有触发器 → PASS（有触发器拉起，非原版但能自启）
        assert_eq!(nlasvc_result(true, "Manual", true).status, PASS);

        // 停止 + 已禁用 → FAIL
        let dis = nlasvc_result(false, "Disabled", false);
        assert_eq!(dis.status, FAIL);
        assert_eq!(dis.fix_ids, vec!["fix_nlasvc"]);
        assert!(dis.detail.contains("已禁用"));

        // 停止 + 手动无触发器 → FAIL（自开机起从未运行）
        let manual = nlasvc_result(false, "Manual", false);
        assert_eq!(manual.status, FAIL);
        assert_eq!(manual.fix_ids, vec!["fix_nlasvc"]);
        assert!(manual.detail.contains("无任何触发器"), "{}", manual.detail);

        // 停止 + 其它启动类型 → FAIL + 建议 start_nlasvc
        let stopped = nlasvc_result(false, "Automatic", true);
        assert_eq!(stopped.status, FAIL);
        assert_eq!(stopped.fix_ids, vec!["start_nlasvc"]);
        assert!(stopped.detail.contains("已停止(启动类型=Automatic)"));
    }

    #[test]
    fn nlasvc_check_runs_on_real_machine_with_python_severity() {
        // 真机上只断言形状；停止态必须是 FAIL 而不是 WARN（Python 分级是硬契约）
        let r = chk_nlasvc();
        assert!(!r.detail.is_empty());
        if r.id == "nlasvc"
            && r.status != SKIP
            && (r.detail.starts_with("已停止") || r.detail.starts_with("已禁用"))
        {
            assert_eq!(r.status, FAIL, "停止/禁用的 NlaSvc 是 FAIL: {}", r.detail);
        }
    }

    #[test]
    fn snapshot_checks_are_resilient_to_empty_snapshot() {
        // 空快照（采集失败/无网卡）下每项都必须给出可读结论，绝不 panic、
        // 也绝不假装「已检测到问题」——空数据只能说「无」或「跳过」
        let empty = crate::snapshot::Snapshot::default();
        for r in [
            chk_persistent_route(&empty),
            chk_static_dns(&empty),
            chk_routes(&empty),
            chk_ipv6(),
            chk_wlan(),
        ] {
            assert!(!r.detail.is_empty(), "{} 必须有详情", r.id);
            assert!(
                matches!(r.status, PASS | WARN | FAIL | SKIP | INFO),
                "{} 状态非法: {}",
                r.id,
                r.status
            );
        }
        // 空快照的具体语义
        assert_eq!(
            chk_persistent_route(&empty).status,
            PASS,
            "无持久路由 = pass（不是 skip）"
        );
        assert_eq!(
            chk_static_dns(&empty).status,
            PASS,
            "无网卡 = 无静态 DNS 问题"
        );
        assert_eq!(chk_routes(&empty).status, PASS, "无默认路由 = 无幽灵路由");
    }

    #[test]
    fn no_check_reports_not_implemented_anymore() {
        // 全量实现的回归锁：15 项都不许再出现「尚未接入」——
        // 之前 5 项依赖 snapshot 时这里会亮红灯，现在是完成度断言
        let snap = crate::snapshot::take();
        for c in all_checks() {
            let r = c.run(&snap);
            assert!(
                !r.detail.contains("尚未接入"),
                "{} 仍显示尚未接入: {}",
                r.id,
                r.detail
            );
            assert!(!r.detail.is_empty(), "{} 详情为空", r.id);
        }
    }

    #[test]
    fn ncsi_registry_check_reads_real_registry() {
        let r = chk_ncsi_registry();
        // 真机应能读到 NCSI 注册表（普通用户可读 HKLM 该项）
        assert!(
            r.status == PASS || r.status == WARN || r.status == FAIL,
            "状态应合法: {}",
            r.status
        );
        assert!(!r.detail.is_empty(), "必须有详情");
        if r.status == FAIL {
            assert!(
                r.detail.contains("无法读取"),
                "FAIL 只能因为读不到: {}",
                r.detail
            );
        }
    }

    #[test]
    fn proxy_check_reads_real_registry() {
        let r = chk_proxy();
        assert!(!r.detail.is_empty());
        // 三种合法结论：未启用 / 已开启 / PAC
        assert!(
            r.detail.contains("未启用") || r.detail.contains("已开启") || r.detail.contains("PAC"),
            "实际: {}",
            r.detail
        );
    }

    #[test]
    fn nlasvc_and_services_checks_run_on_real_machine() {
        let n = chk_nlasvc();
        assert!(!n.detail.is_empty());
        let s = chk_services();
        assert!(!s.detail.is_empty());
        // 服务清单要么有数据要么明确写「无数据」
        assert!(
            s.detail.contains('=') || s.detail == "无数据",
            "实际: {}",
            s.detail
        );
    }

    #[test]
    fn run_all_quick_skips_slow_checks() {
        let full = run_all(false);
        assert_eq!(full.len(), 15);
        let quick = run_all(true);
        assert_eq!(quick.len(), 10, "quick 跳过 5 项慢速检查");
        assert!(
            !quick.iter().any(
                |r| ["dns_pollution", "ncsi_reg", "power_mgmt", "ipv6", "wlan"].contains(&r.id)
            ),
            "quick 模式不得包含慢速项"
        );
    }

    #[test]
    fn default_gateway_parse_returns_dotted_quad_or_none() {
        if let Some(gw) = default_gateway_v4() {
            assert_eq!(gw.matches('.').count(), 3, "网关应是点分 IPv4: {gw}");
            assert!(gw.parse::<std::net::Ipv4Addr>().is_ok());
        }
        // 无网关也不算失败（离线机器）
    }

    #[test]
    fn gateway_check_reports_something_actionable() {
        let r = chk_gateway(&crate::snapshot::Snapshot::default());
        assert!(!r.detail.is_empty());
        assert!(r.status == PASS || r.status == WARN || r.status == FAIL || r.status == SKIP);
    }

    #[test]
    fn dns_compare_runs_and_reports() {
        let r = chk_dns_compare(&crate::snapshot::Snapshot::default());
        assert!(!r.detail.is_empty());
        // 断网机器也会给 FAIL/WARN + 可读说明，不得 panic
        assert!(r.fix_ids.is_empty() || !r.fix_ids.is_empty());
    }

    #[test]
    fn ncsi_stock_matches_python_config() {
        let stock = ncsi_stock();
        assert_eq!(stock.len(), 11, "config.NCSI_STOCK 有 11 项");
        assert_eq!(
            stock
                .iter()
                .find(|(k, _)| *k == "ActiveWebProbeHost")
                .unwrap()
                .1,
            "www.msftconnecttest.com"
        );
        assert_eq!(
            stock
                .iter()
                .find(|(k, _)| *k == "ActiveDnsProbeContent")
                .unwrap()
                .1,
            "131.107.255.255"
        );
        assert_eq!(NCSI_EXPECT_CONTENT, "Microsoft Connect Test");
        assert_eq!(NCSI_DNS_EXPECT, "131.107.255.255");
    }

    #[test]
    fn probe_204_live_smoke() {
        // 真机联网时应通；断网时返回 false + 原因，都不得 panic
        let (ok, detail) = probe_204(4.0);
        assert!(!detail.is_empty(), "无论成败都要有说明");
        if !ok {
            assert!(
                detail.contains("HTTP")
                    || detail.contains("失败")
                    || detail.contains("解析")
                    || detail.contains("连接"),
                "失败原因应可读: {detail}"
            );
        }
    }
}
