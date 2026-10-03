//! 后台监控线程：周期探测 + 保守自动修复梯度（对应 Python 侧 `monitor.py`）。
//!
//! ## 为什么这是最后一个大缺口
//!
//! Rs 侧的 19 个修复动作执行器早已完备，缺的是**调度**它们的那层：没有任何后台
//! 轮询，用户只能手动点「开始诊断」。于是六项设置（`auto_repair_enabled` /
//! `poll_interval_sec` / `auto_repair_max_level` / `fail_threshold` /
//! `cooldown_sec` / `level3_cooldown_sec`）都没有消费方。
//!
//! ## 保守梯度（自动修复只走固定白名单）
//!
//! L1 `flush_dns` → L2 +`start_nlasvc`/`renew_dhcp` → L3 +`restart_wlan`/`toggle_adapter`。
//! 后三项属"改配置/断网"类动作，**这里是唯一自动执行它们的路径**，靠三道闸约束：
//! L3 限频（`level3_cooldown_sec`）+ 通用冷却（`cooldown_sec`）+ **VPN 护栏**。
//! 其余改配置类动作只通过"建议"提示，绝不自动执行。
//!
//! ## 三处实测换来的细节
//!
//! 1. **先探连通、后抓快照**：`snapshot.take()` 是全量 PowerShell 采集（4~6 秒），
//!    而它在这里只服务一件事——判断有没有活动物理网卡，那只有探测失败时才有意义。
//!    健康时每轮省掉一次采集，托盘常驻不再空转。
//! 2. **无物理网卡时 `fail_streak` 归零**：纯虚拟网卡（Clash TUN 等）场景下
//!    "探测失败"不等于断网，计进 streak 会触发无意义的自动修复。
//! 3. **L3 时间戳必须用单调时钟**：Python 注释专门警示过——写入 epoch 秒会让
//!    与单调时钟的差值为巨负，L3 冷却**永久命中**，自动修复被静默降级到 L2。

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;

/// 网络状态码（与 Python 同名，便于日志对照）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NetState {
    Ok,
    NoNet,
    #[allow(dead_code)] // 状态机对齐 Python ST_NCSI_FALSE（Python 同样只定义未赋值）
    NcsiFalse,
    Offline,
}

impl NetState {
    pub fn as_str(self) -> &'static str {
        match self {
            NetState::Ok => "ok",
            NetState::NoNet => "no_net",
            NetState::NcsiFalse => "ncsi_false",
            NetState::Offline => "offline",
        }
    }
}

/// 一轮监控的配置快照（已做**下限夹紧**）。
///
/// 夹紧很重要：`poll_interval_sec=0` 会让线程忙等空转（设置层已限 ≥10，
/// 这里是第二道防线——配置文件可被手工编辑）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorConfig {
    pub poll_interval_sec: u64,
    pub fail_threshold: u32,
    pub max_level: u32,
    pub enabled: bool,
    pub cooldown_sec: u64,
    pub level3_cooldown_sec: u64,
}

impl MonitorConfig {
    /// 从设置读取并夹紧。
    pub fn from_settings() -> Self {
        Self {
            poll_interval_sec: crate::settings::get_int("poll_interval_sec").max(10) as u64,
            fail_threshold: crate::settings::get_int("fail_threshold").max(1) as u32,
            max_level: crate::settings::get_int("auto_repair_max_level").clamp(1, 3) as u32,
            enabled: crate::settings::get_bool("auto_repair_enabled"),
            cooldown_sec: crate::settings::get_int("cooldown_sec").max(0) as u64,
            level3_cooldown_sec: crate::settings::get_int("level3_cooldown_sec").max(0) as u64,
        }
    }
}

/// 本轮修复上限：距上次 L3 不足 `level3_cooldown_sec` 时压到 L2。
///
/// 这是"避免频繁重启网卡"的核心机制：L2 的动作是安全的，L3 会短暂断网。
pub fn repair_level(cfg: &MonitorConfig, since_last_l3: Duration) -> u32 {
    if since_last_l3 < Duration::from_secs(cfg.level3_cooldown_sec) {
        2
    } else {
        cfg.max_level
    }
}

/// 是否应当触发一轮修复。
///
/// 三个条件缺一不可：已启用、连续失败达阈值、距上次修复超过冷却。
pub fn should_repair(cfg: &MonitorConfig, fail_streak: u32, since_last_repair: Duration) -> bool {
    if !cfg.enabled {
        return false;
    }
    if fail_streak < cfg.fail_threshold {
        return false;
    }
    since_last_repair >= Duration::from_secs(cfg.cooldown_sec)
}

/// 一次性选择要联网的动作（VPN 护栏）。
pub const VPN_UNSAFE_ACTIONS: [&str; 3] = ["renew_dhcp", "restart_wlan", "toggle_adapter"];

/// 该动作是否会被 VPN 护栏跳过。
pub fn skipped_by_vpn_guard(action: &str) -> bool {
    VPN_UNSAFE_ACTIONS.contains(&action)
}

/// 构造修复梯度并按 VPN 护栏过滤。
///
/// 返回 `(可执行动作, 被跳过的动作)`。VPN 在线时仍保留 `flush_dns` 与
/// `start_nlasvc`——它们不会断隧道。
pub fn build_ladder(level: u32, vpn_up: bool) -> (Vec<&'static str>, Vec<&'static str>) {
    let mut ladder = vec!["flush_dns"];
    if level >= 2 {
        ladder.extend(["start_nlasvc", "renew_dhcp"]);
    }
    if level >= 3 {
        ladder.extend(["restart_wlan", "toggle_adapter"]);
    }
    if !vpn_up {
        return (ladder, Vec::new());
    }
    let skipped: Vec<&'static str> = ladder
        .iter()
        .copied()
        .filter(|a| skipped_by_vpn_guard(a))
        .collect();
    let kept: Vec<&'static str> = ladder
        .iter()
        .copied()
        .filter(|a| !skipped_by_vpn_guard(a))
        .collect();
    (kept, skipped)
}

/// VPN 护栏的说明文案（与 Python 一致）。
pub fn vpn_guard_note() -> &'static str {
    "检测到 VPN/隧道网卡在线，自动修复已跳过续租/重启 WLAN 等高危步骤"
}

/// 修复无效时的建议（**只建议，不自动执行**改配置类动作）。
pub fn suggestions() -> Vec<(&'static str, &'static str)> {
    vec![
        ("dns_auto", "多次自动修复无效，建议检查 DNS 配置"),
        ("ncsi_restore", "建议核对 NCSI 探测配置"),
    ]
}

/// 轻量连通性探测：任一 204 端点可达即算有网。
fn probe_quick() -> bool {
    crate::netcheck::probe_204(4.0).0
}

/// 监控线程的运行状态（供界面显示）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MonitorState {
    pub running: bool,
    pub state: String,
    pub detail: String,
    /// 最近一轮判定时间（Unix 秒）
    pub last_check_ts: i64,
    /// 已执行的自动修复次数
    pub repair_count: u64,
    /// 最近一次修复的动作 id 列表（逗号分隔）
    pub last_ladder: String,
    pub fail_streak: u32,
}

static RUNNING: AtomicBool = AtomicBool::new(false);
static FORCE: AtomicBool = AtomicBool::new(false);
static STOP: AtomicBool = AtomicBool::new(false);
/// 托盘状态灯用：set_status 切换状态时更新托盘图标/基础提示（spawn 时注入）
static APP_HANDLE: std::sync::OnceLock<tauri::AppHandle> = std::sync::OnceLock::new();
static REPAIR_COUNT: AtomicU64 = AtomicU64::new(0);
static LAST_CHECK: AtomicI64 = AtomicI64::new(0);
static FAIL_STREAK: AtomicU64 = AtomicU64::new(0);
static LAST_STATE: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
static LAST_DETAIL: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
static LAST_LADDER: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// 断网/恢复提示音判定（对齐 Python main_window._on_monitor_state）：
/// 仅**状态切换**发声；首态（prev 为空）不发声——启动时的初始状态不算事件。
fn should_alert(prev: &str, new: &str) -> bool {
    !prev.is_empty() && prev != new
}

fn set_status(state: NetState, detail: &str) {
    let prev = LAST_STATE.lock().expect("状态锁中毒").clone();
    *LAST_STATE.lock().expect("状态锁中毒") = state.as_str().to_string();
    *LAST_DETAIL.lock().expect("状态锁中毒") = detail.to_string();
    // 托盘状态灯接管：只在状态真正切换时换图标/基础悬浮提示（tick 每轮都会
    // 调 set_status，不能每轮都重绘图标）；实时速率提示由遥测每秒叠加（其
    // 基础段同样读这里的状态，两条路径不打架）。此前没人调 apply_state，
    // 托盘永远显示 offline 图标——这是接线缺口不是刻意行为。
    if prev != state.as_str() {
        if let Some(app) = APP_HANDLE.get() {
            crate::tray::apply_state(app, state.as_str());
        }
    }
    // 断网/恢复提示音：sound_alert_enabled 门控（默认开）；
    // 测试运行静默（cfg 门）——FFI 本身平凡，判定逻辑另有单测
    if should_alert(&prev, state.as_str()) && crate::settings::get_bool("sound_alert_enabled") {
        #[cfg(not(test))]
        crate::w32::play_alert_sound(state.as_str() == "ok");
        #[cfg(test)]
        let _ = state.as_str() == "ok";
    }
    LAST_CHECK.store(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
        Ordering::SeqCst,
    );
}

/// 当前监控状态。
pub fn state() -> MonitorState {
    MonitorState {
        running: RUNNING.load(Ordering::SeqCst),
        state: LAST_STATE.lock().expect("状态锁中毒").clone(),
        detail: LAST_DETAIL.lock().expect("状态锁中毒").clone(),
        last_check_ts: LAST_CHECK.load(Ordering::SeqCst),
        repair_count: REPAIR_COUNT.load(Ordering::SeqCst),
        last_ladder: LAST_LADDER.lock().expect("状态锁中毒").clone(),
        fail_streak: FAIL_STREAK.load(Ordering::SeqCst) as u32,
    }
}

/// 请求立即检查一次（手动刷新不必等满 poll_interval）。
pub fn force_check() {
    FORCE.store(true, Ordering::SeqCst);
}

/// 请求停止监控线程。
#[allow(dead_code)] // 停止语义对齐 Python（进程退出即线程消亡，优雅停止留作复用）
pub fn request_stop() {
    STOP.store(true, Ordering::SeqCst);
    FORCE.store(true, Ordering::SeqCst);
}

/// 起监控线程（Tauri setup 阶段调用一次）。
pub fn spawn(app: tauri::AppHandle) {
    let _ = APP_HANDLE.set(app);
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    STOP.store(false, Ordering::SeqCst);
    std::thread::Builder::new()
        .name("sk-monitor".into())
        .spawn(move || {
            // 用单调时钟：`repair_level` 拿它与 Instant::now() 相减，
            // 用墙上时钟会让差值为巨负、L3 冷却永久命中（自动修复静默降级到 L2）
            let mut last_repair: Option<Instant> = None;
            let mut last_l3: Option<Instant> = None;
            let mut fail_streak: u32 = 0;
            while !STOP.load(Ordering::SeqCst) {
                let cfg = MonitorConfig::from_settings();
                // 自愈：单拍 panic 记 ERROR 后继续（守护线程死 = 监控静默消失）
                crate::catch_tick("monitor", || {
                    if let Err(e) = tick(&cfg, &mut fail_streak, &mut last_repair, &mut last_l3) {
                        log::warn!("监控循环异常: {e}");
                    }
                });
                FAIL_STREAK.store(fail_streak as u64, Ordering::SeqCst);
                // 可中断等待：force 置位立即醒来
                let deadline = Instant::now() + Duration::from_secs(cfg.poll_interval_sec);
                while Instant::now() < deadline {
                    if STOP.load(Ordering::SeqCst) {
                        break;
                    }
                    if FORCE.swap(false, Ordering::SeqCst) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
            RUNNING.store(false, Ordering::SeqCst);
        })
        .expect("监控线程启动失败");
}

/// 一轮：探测 → （失败时）判定是否修复 → 执行梯度。
fn tick(
    cfg: &MonitorConfig,
    fail_streak: &mut u32,
    last_repair: &mut Option<Instant>,
    last_l3: &mut Option<Instant>,
) -> Result<(), String> {
    // 先探连通、后抓快照：快照是全量采集（秒级），只在探测失败时才有必要
    if probe_quick() {
        *fail_streak = 0;
        set_status(NetState::Ok, "");
        return Ok(());
    }

    let snap = crate::snapshot::take();
    let has_up = !snap.physical_up_adapters().is_empty();
    if !has_up {
        // 无活动物理网卡（纯虚拟网卡/网线未插）：不算断网，也不累计失败，
        // 否则会触发一轮毫无意义的自动修复
        *fail_streak = 0;
        set_status(NetState::Offline, "无活动网络连接");
        return Ok(());
    }

    *fail_streak += 1;
    set_status(
        NetState::NoNet,
        &format!("连通性探测失败 ×{}", *fail_streak),
    );

    let now = Instant::now();
    let since_repair = last_repair
        .map(|t| now.duration_since(t))
        .unwrap_or(Duration::MAX);
    if !should_repair(cfg, *fail_streak, since_repair) {
        return Ok(());
    }

    *fail_streak = 0;
    *last_repair = Some(now);

    let since_l3 = last_l3
        .map(|t| now.duration_since(t))
        .unwrap_or(Duration::MAX);
    let level = repair_level(cfg, since_l3);
    if level >= 3 {
        // L3 限频记在**梯度级别**：即使修复后立刻恢复，也计入冷却，
        // 避免反复全量 L3（重启 WlanSvc + 断合网卡）
        *last_l3 = Some(now);
    }

    let vpn_up = snapshot_has_tunnel(&snap);
    let (ladder, skipped) = build_ladder(level, vpn_up);
    if !skipped.is_empty() {
        log::warn!("检测到 VPN/隧道在线，自动修复跳过: {}", skipped.join(", "));
    }
    log::info!("自动修复触发, 级别 L{level}, 梯度 {:?}", ladder);
    *LAST_LADDER.lock().expect("梯度锁中毒") = ladder.join(",");

    for id in &ladder {
        if STOP.load(Ordering::SeqCst) {
            return Ok(());
        }
        let Some(action) = crate::repairs::get_action(id) else {
            continue;
        };
        let result = action.execute(&std::collections::BTreeMap::new());
        log::info!("自动修复 [{id}] -> {}: {}", result.success, result.message);
        REPAIR_COUNT.fetch_add(1, Ordering::SeqCst);
        if *id == "toggle_adapter" {
            // 与 Python 同理：必须用单调时钟
            *last_l3 = Some(Instant::now());
        }
        // 步间等待可被 stop 打断
        let wait_until = Instant::now() + Duration::from_secs(3);
        while Instant::now() < wait_until {
            if STOP.load(Ordering::SeqCst) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if probe_quick() {
            set_status(NetState::Ok, &format!("自动修复成功（{}）", action.title));
            return Ok(());
        }
    }
    set_status(NetState::NoNet, "自动修复梯度执行完毕仍未恢复");
    Ok(())
}

/// 快照里是否存在在用的隧道网卡。
fn snapshot_has_tunnel(snap: &crate::snapshot::Snapshot) -> bool {
    snap.adapters.iter().any(|a| {
        a.status.eq_ignore_ascii_case("up")
            && (crate::netcheck::is_tunnel(&a.desc) || crate::netcheck::is_tunnel(&a.name))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> MonitorConfig {
        MonitorConfig {
            poll_interval_sec: 30,
            fail_threshold: 2,
            max_level: 3,
            enabled: true,
            cooldown_sec: 180,
            level3_cooldown_sec: 300,
        }
    }

    #[test]
    fn should_repair_requires_all_three_gates() {
        let c = cfg();
        let past = Duration::from_secs(1000);
        // 三项齐备
        assert!(should_repair(&c, 2, past));
        // 未启用
        assert!(!should_repair(
            &MonitorConfig {
                enabled: false,
                ..c
            },
            99,
            past
        ));
        // 失败次数不足
        assert!(!should_repair(&c, 1, past));
        // 冷却未到
        assert!(!should_repair(&c, 99, Duration::from_secs(10)));
    }

    #[test]
    fn should_repair_boundary_is_inclusive() {
        let c = cfg();
        // 恰好等于冷却时间 → 允许（用 >= 而非 >）
        assert!(should_repair(&c, 2, Duration::from_secs(180)));
        assert!(!should_repair(&c, 2, Duration::from_secs(179)));
        // 恰好等阈值的失败次数 → 允许
        assert!(should_repair(&c, 2, Duration::from_secs(1000)));
        assert!(!should_repair(&c, 1, Duration::from_secs(1000)));
    }

    #[test]
    fn l3_is_throttled_within_cooldown() {
        let c = cfg();
        // L3 冷却 300s 内 → 压到 L2
        assert_eq!(repair_level(&c, Duration::from_secs(0)), 2);
        assert_eq!(repair_level(&c, Duration::from_secs(299)), 2);
        // 冷却过后 → 允许到配置上限
        assert_eq!(repair_level(&c, Duration::from_secs(300)), 3);
        assert_eq!(repair_level(&c, Duration::from_secs(9999)), 3);
    }

    #[test]
    fn l3_throttle_respects_configured_max_level() {
        // 用户把上限设成 2 时，即使 L3 冷却已过也只到 2
        let c = MonitorConfig {
            max_level: 2,
            ..cfg()
        };
        assert_eq!(repair_level(&c, Duration::from_secs(9999)), 2);
        assert_eq!(repair_level(&c, Duration::from_secs(0)), 2);
    }

    #[test]
    fn ladder_grows_with_level() {
        assert_eq!(build_ladder(1, false).0, vec!["flush_dns"]);
        assert_eq!(
            build_ladder(2, false).0,
            vec!["flush_dns", "start_nlasvc", "renew_dhcp"]
        );
        assert_eq!(
            build_ladder(3, false).0,
            vec![
                "flush_dns",
                "start_nlasvc",
                "renew_dhcp",
                "restart_wlan",
                "toggle_adapter"
            ]
        );
    }

    #[test]
    fn ladders_only_contain_whitelisted_actions() {
        // 自动修复**只能**走固定白名单；任何改配置类的动作闯进来都是严重问题
        for level in 1..=3 {
            for id in build_ladder(level, false).0 {
                assert!(
                    crate::repairs::get_action(id).is_some(),
                    "{id} 不在修复动作表里"
                );
                assert!(
                    [
                        "flush_dns",
                        "start_nlasvc",
                        "renew_dhcp",
                        "restart_wlan",
                        "toggle_adapter"
                    ]
                    .contains(&id),
                    "{id} 不该出现在自动修复梯度里"
                );
            }
        }
    }

    #[test]
    fn vpn_guard_skips_tunnel_breaking_actions() {
        let (kept, skipped) = build_ladder(3, true);
        assert_eq!(
            skipped,
            vec!["renew_dhcp", "restart_wlan", "toggle_adapter"]
        );
        // flush_dns 与 start_nlasvc 不断隧道，必须保留
        assert_eq!(kept, vec!["flush_dns", "start_nlasvc"]);
    }

    #[test]
    fn vpn_guard_keeps_l1_intact() {
        // L1 只有 flush_dns，VPN 在线时不该跳过任何东西
        let (kept, skipped) = build_ladder(1, true);
        assert_eq!(kept, vec!["flush_dns"]);
        assert!(skipped.is_empty());
    }

    #[test]
    fn vpn_guard_note_is_actionable() {
        let n = vpn_guard_note();
        assert!(n.contains("VPN") || n.contains("隧道"), "{n}");
        assert!(n.contains("跳过"), "要说明做了什么: {n}");
    }

    #[test]
    fn suggestions_are_suggestions_not_actions() {
        // 建议项绝不自动执行——它们都是改配置类动作
        for (id, reason) in suggestions() {
            assert!(crate::repairs::get_action(id).is_some());
            assert!(!reason.is_empty());
            assert!(
                ![
                    "flush_dns",
                    "start_nlasvc",
                    "renew_dhcp",
                    "restart_wlan",
                    "toggle_adapter"
                ]
                .contains(&id),
                "{id} 是自动执行项，不该同时作为'建议'"
            );
        }
    }

    #[test]
    fn config_clamps_dangerous_values() {
        let _g = crate::test_support::serial();
        crate::test_support::isolate();
        // 手工把设置文件改成危险值（设置层的范围校验可被绕过）
        let mut m = serde_json::Map::new();
        m.insert("poll_interval_sec".into(), serde_json::json!(0));
        m.insert("fail_threshold".into(), serde_json::json!(0));
        m.insert("auto_repair_max_level".into(), serde_json::json!(99));
        m.insert("cooldown_sec".into(), serde_json::json!(-5));
        crate::settings::save(&m).unwrap();

        let c = MonitorConfig::from_settings();
        assert_eq!(c.poll_interval_sec, 10, "0 会让线程忙等空转，必须夹到 10");
        assert_eq!(c.fail_threshold, 1);
        assert_eq!(c.max_level, 3, "上限不得超过 3（L3 是最高梯度）");
        assert_eq!(c.cooldown_sec, 0, "负数夹到 0");
    }

    #[test]
    fn disabled_by_default_is_respected() {
        let _g = crate::test_support::serial();
        crate::test_support::isolate();
        // 必须清掉上一个用例写下的设置文件，否则读到的是被改过的值
        crate::test_support::reset_persistent_files();
        let c = MonitorConfig::from_settings();
        // 默认值与 Python config.py::DEFAULTS 对齐
        assert!(c.enabled, "默认应为开启，与 config.py 对齐");
        assert_eq!(c.poll_interval_sec, 30);
        assert_eq!(c.fail_threshold, 2);
        assert_eq!(c.max_level, 3);
        assert_eq!(c.cooldown_sec, 180);
        assert_eq!(c.level3_cooldown_sec, 300);
    }

    #[test]
    fn should_alert_only_on_transition() {
        assert!(!should_alert("", "ok"), "首态不发声");
        assert!(should_alert("ok", "no_net"), "断网切换发声");
        assert!(should_alert("no_net", "ok"), "恢复发声");
        assert!(!should_alert("ok", "ok"), "同态不发声");
    }

    #[test]
    fn net_state_strings_match_python() {
        assert_eq!(NetState::Ok.as_str(), "ok");
        assert_eq!(NetState::NoNet.as_str(), "no_net");
        assert_eq!(NetState::NcsiFalse.as_str(), "ncsi_false");
        assert_eq!(NetState::Offline.as_str(), "offline");
    }

    #[test]
    fn state_serializes_with_expected_fields() {
        // 这个结构直接给前端消费，字段名变了界面就静默空白——所以锁住字段名。
        // （首版这里写的是 `assert!(s.repair_count >= 0)` 这种恒真断言，
        //   clippy 判为「比较无用」——确实什么也没测到。）
        let j = serde_json::to_string(&state()).unwrap();
        for k in [
            "running",
            "state",
            "detail",
            "last_check_ts",
            "repair_count",
            "last_ladder",
            "fail_streak",
        ] {
            assert!(j.contains(k), "序列化应含 {k}: {j}");
        }
    }

    #[test]
    fn state_before_start_does_not_claim_running() {
        // 未起线程时 RUNNING 必须为 false —— 谎报运行会让用户以为监控在跑
        assert!(
            !RUNNING.load(Ordering::SeqCst),
            "测试进程未调用 spawn()，不应报 running"
        );
        assert!(!state().running);
    }

    #[test]
    fn force_check_wakes_the_loop_immediately() {
        // 托盘「立即检测」走的就是这条路：置 FORCE 后轮询等待在 100ms 粒度内
        // 醒来（不必等满 poll_interval）。标志位丢了 = 菜单点了没反应。
        FORCE.store(false, Ordering::SeqCst);
        force_check();
        assert!(
            FORCE.swap(false, Ordering::SeqCst),
            "force_check 必须置起唤醒标志"
        );
        assert!(!FORCE.load(Ordering::SeqCst), "swap 后应被清掉（单次生效）");
    }

    #[test]
    fn tunnel_detection_covers_vpn_products() {
        // 护栏依赖它：漏检一个产品，用户挂着 VPN 就会被自动修复踢下线
        for desc in [
            "WireGuard Tunnel",
            "OpenVPN Wintun",
            "Clash TUN",
            "sing-box",
            "Sangfor EasyConnect",
            "Tailscale",
            "ZeroTier One",
            "Hamachi",
            "向日葵 Oray",
            "TAP-Windows Adapter V9",
        ] {
            assert!(crate::netcheck::is_tunnel(desc), "{desc} 应被识别为隧道");
        }
        // 普通网卡不得误判（否则自动修复会被无谓地削弱）
        for desc in [
            "Intel(R) Wi-Fi 6 AX201",
            "Realtek PCIe GbE Family Controller",
            "Microsoft Kernel Debug Network Adapter",
        ] {
            assert!(!crate::netcheck::is_tunnel(desc), "{desc} 不是隧道");
        }
    }

    #[test]
    fn tunnel_keywords_match_python_list() {
        // 与 Python vpn.TUNNEL_PAT 的产品清单对齐（漏一个就少一层保护）
        assert_eq!(crate::netcheck::TUNNEL_KEYWORDS.len(), 23);
        for k in [
            "wintun",
            "openvpn",
            "wireguard",
            "clash",
            "sangfor",
            "easyconnect",
            "tailscale",
            "zerotier",
            "hamachi",
            "oray",
            "nekoray",
            "hysteria",
            "sing-box",
        ] {
            assert!(crate::netcheck::TUNNEL_KEYWORDS.contains(&k), "缺少 {k}");
        }
    }
}
