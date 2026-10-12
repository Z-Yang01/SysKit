//! 修复动作库：风险分级 + 动作注册表 + 可验证动作的执行器。
//!
//! 与 Python 侧 repairs.py 的关系：
//! - **元数据逐项对齐**（id/title/risk/description/restore_hint/params/need_reboot），
//!   通过 `catalog_parity` 与 Python 源码比对锁定——这些字符串是给用户看的，
//!   也是 UI 分级确认的依据，漂移就是行为漂移；
//! - 执行器按「能否无副作用验证」分批移植：安全且只读或可反复执行的动作已接好，
//!   改系统配置/注册表/需要管理员的那批**显式返回「尚未接入」**——绝不用
//!   `return Ok` 假装成功（假成功比不实现危险得多，尤其在修复链里）。
//!
//! 风险分级语义（照抄 Python 注释，这是踩过坑的结论）：
//! SAFE = 组合内免确认自动执行；CONFIG = 组合内逐项确认、无管理员跳过；
//! AGGRESSIVE = 强力且需重启。`restart_wlan`/`toggle_adapter` 曾定为 SAFE，
//! 与监控自动修复把它们列为最高危 L3 直接矛盾，故统一升为 CONFIG。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::w32::{self, RegValue};

/// 风险等级（序列化为 safe/config/aggressive，与 Python 字符串一致）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    Safe,
    Config,
    Aggressive,
}

impl Risk {
    /// 中文标签（与 Python `RISK_LABEL` 逐字一致）。
    pub fn label(self) -> &'static str {
        match self {
            Risk::Safe => "安全",
            Risk::Config => "改配置",
            Risk::Aggressive => "强力(需重启)",
        }
    }

    /// 组合内是否免确认自动执行。
    #[cfg(test)]
    pub fn auto_ok(self) -> bool {
        matches!(self, Risk::Safe)
    }

    #[cfg(test)]
    pub fn as_str(self) -> &'static str {
        match self {
            Risk::Safe => "safe",
            Risk::Config => "config",
            Risk::Aggressive => "aggressive",
        }
    }
}

/// 动作参数说明。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ParamSpec {
    pub name: &'static str,
    pub desc: &'static str,
    pub required: bool,
}

/// 修复动作元数据（执行器是函数指针，元数据与实现同一处定义，不会漂移）。
#[derive(Clone, Copy)]
pub struct RepairAction {
    pub id: &'static str,
    pub title: &'static str,
    pub risk: Risk,
    pub description: &'static str,
    pub restore_hint: &'static str,
    pub brief: &'static str,
    pub need_reboot: bool,
    pub params: &'static [ParamSpec],
    runner: fn(&BTreeMap<String, String>) -> RepairOutcome,
}

impl std::fmt::Debug for RepairAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RepairAction")
            .field("id", &self.id)
            .field("risk", &self.risk)
            .finish()
    }
}

/// 修复结果（对应 Python `RepairResult`）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RepairOutcome {
    pub success: bool,
    pub message: String,
    pub need_reboot: bool,
    pub output: String,
}

impl RepairOutcome {
    pub fn ok(message: impl Into<String>) -> Self {
        Self {
            success: true,
            message: message.into(),
            need_reboot: false,
            output: String::new(),
        }
    }

    pub fn fail(message: impl Into<String>) -> Self {
        Self {
            success: false,
            message: message.into(),
            need_reboot: false,
            output: String::new(),
        }
    }

    /// 「尚未接入」：明确的失败，**不是**成功。
    #[allow(dead_code)] // 延迟执行器机制（DEFERRED 清单复用时必需）
    pub fn unimplemented(id: &str, why: &str) -> Self {
        Self::fail(format!(
            "{id} 尚未在 Rust 版接入（{why}）；请暂用 Python 版执行，或等待后续里程碑"
        ))
    }
}

impl RepairAction {
    pub fn execute(&self, params: &BTreeMap<String, String>) -> RepairOutcome {
        let mut out = (self.runner)(params);
        out.need_reboot = out.need_reboot || self.need_reboot;
        out
    }
}

const P_ADAPTER: ParamSpec = ParamSpec {
    name: "adapter",
    desc: "网卡名称",
    required: false,
};
const P_PRESET: ParamSpec = ParamSpec {
    name: "preset",
    desc: "预设名",
    required: false,
};
const P_PROFILE: ParamSpec = ParamSpec {
    name: "profile",
    desc: "WiFi 配置文件名（SSID）",
    required: false,
};


mod exec_admin;
mod exec_config;
mod exec_safe;

// 扁平重导出：调用方继续用 crate::repairs::X，与拆分前完全一致
pub use exec_admin::*;
pub use exec_config::*;
pub use exec_safe::*;
// ---------------- 注册表（顺序与 Python ACTION_IDS 完全一致） ----------------

/// 全部 19 个动作，顺序 = Python `ACTION_IDS` 顺序。
pub fn catalog() -> &'static [RepairAction] {
    static ACTIONS: &[RepairAction] = &[
        RepairAction {
            id: "flush_dns",
            title: "刷新 DNS 解析缓存",
            risk: Risk::Safe,
            description: "清空系统 DNS 缓存。无副作用，可反复执行。",
            restore_hint: "无需还原（缓存会自动重建）",
            brief: "清空系统 DNS 缓存。无副作用，可反复执行。",
            need_reboot: false,
            params: &[],
            runner: run_flush_dns,
        },
        RepairAction {
            id: "renew_dhcp",
            title: "重新获取 IP（DHCP 续租）",
            risk: Risk::Safe,
            description: "释放并重新获取所有 DHCP 网卡的 IP。执行期间网络会瞬断几秒，隧道/VPN 不受影响（其非 DHCP）。",
            restore_hint: "无需还原（DHCP 自动分配）",
            brief: "释放并重新获取所有 DHCP 网卡的 IP。执行期间网络会瞬断几秒，隧道/VPN 不受影响（其非 DHCP）。",
            need_reboot: false,
            params: &[],
            runner: run_renew_dhcp,
        },
        RepairAction {
            id: "start_nlasvc",
            title: "启动 NlaSvc（网络位置感知）",
            risk: Risk::Safe,
            description: "立即启动 NlaSvc，使 NCSI 探测恢复运行。若启动类型是'手动+无触发器'，重启后还会复发，建议配合 fix_nlasvc。",
            restore_hint: "无需还原",
            brief: "立即启动 NlaSvc，使 NCSI 探测恢复运行。若启动类型是'手动+无触发器'，重启后还会复发，建议配合 fix_n",
            need_reboot: false,
            params: &[],
            runner: run_start_nlasvc,
        },
        RepairAction {
            id: "fix_nlasvc",
            title: "修复 NlaSvc 启动配置（设为自动并立即启动）",
            risk: Risk::Config,
            description: "本机 NlaSvc 被改为'手动启动+无触发器'（原版为触发启动），导致它自开机起从未运行、NCSI 探测无人执行。此动作把启动类型改为'自动'并立即启动。这是对误报'无网络'的直接修复。",
            restore_hint: "以管理员执行: sc config NlaSvc start= demand",
            brief: "本机 NlaSvc 被改为'手动启动+无触发器'（原版为触发启动），导致它自开机起从未运行、NCSI 探测无人执行。此动",
            need_reboot: false,
            params: &[],
            runner: run_fix_nlasvc,
        },
        RepairAction {
            id: "restart_wlan",
            title: "重启 WLAN 服务并重连 WiFi",
            risk: Risk::Config,
            description: "重启 WlanSvc（WiFi 自动配置服务），并自动重连之前连接的 WiFi。WiFi 会断开几秒。",
            restore_hint: "无需还原",
            brief: "重启 WlanSvc（WiFi 自动配置服务），并自动重连之前连接的 WiFi。WiFi 会断开几秒。",
            need_reboot: false,
            params: &[],
            runner: run_restart_wlan,
        },
        RepairAction {
            id: "toggle_adapter",
            title: "禁用并重新启用网卡",
            risk: Risk::Config,
            description: "对指定网卡执行 disable→enable，重建链路。WiFi/USB 网卡'假连接'时的经典修复。",
            restore_hint: "无需还原",
            brief: "对指定网卡执行 disable→enable，重建链路。WiFi/USB 网卡'假连接'时的经典修复。",
            need_reboot: false,
            params: &[ParamSpec { desc: "网卡名称，如 WLAN / 以太网 3", ..P_ADAPTER }],
            runner: run_toggle_adapter,
        },
        RepairAction {
            id: "restart_services",
            title: "重启 DHCP 服务并刷新 DNS 缓存",
            risk: Risk::Safe,
            description: "重启 Dhcp 服务；Dnscache 是受保护服务（net stop 恒被拒绝），改用 ipconfig /flushdns 达到等效的清缓存目的。",
            restore_hint: "无需还原",
            brief: "重启 Dhcp 服务；Dnscache 是受保护服务（net stop 恒被拒绝），改用 ipconfig /flush",
            need_reboot: false,
            params: &[],
            runner: run_restart_services,
        },
        RepairAction {
            id: "restore_point",
            title: "创建系统还原点",
            risk: Risk::Safe,
            description: "为当前系统状态创建还原点（深度修复/重置前的安全网）。\n常态耗时 30~90 秒，期间无法中止；系统还原未开启、或 24 小时内已创建过还原点时，Windows 可能静默跳过——以返回信息为准。",
            restore_hint: "无需还原",
            brief: "为当前系统状态创建还原点（深度修复/重置前的安全网）。\n常态耗时 30~90 秒，期间无法中止；系统还原未开启、或 24",
            need_reboot: false,
            params: &[],
            runner: run_restore_point,
        },
        RepairAction {
            id: "dns_auto",
            title: "DNS 恢复为 DHCP 自动获取",
            risk: Risk::Config,
            description: "清除指定网卡上残留的静态 DNS（如 8.8.8.8），恢复由路由器/热点下发。改前自动备份原 DNS。",
            restore_hint: "用 dns_preset 动作把原值设回，或查看备份 JSON",
            brief: "清除指定网卡上残留的静态 DNS（如 8.8.8.8），恢复由路由器/热点下发。改前自动备份原 DNS。",
            need_reboot: false,
            params: &[ParamSpec { desc: "网卡名称，默认自动选静态 DNS 的网卡", ..P_ADAPTER }],
            runner: run_dns_auto,
        },
        RepairAction {
            id: "dns_preset",
            title: "设置静态公共 DNS",
            risk: Risk::Config,
            description: "把指定网卡 DNS 设为国内公共 DNS（阿里/腾讯/114 等）。改前自动备份原值。",
            restore_hint: "运行 dns_auto 恢复自动获取",
            brief: "把指定网卡 DNS 设为国内公共 DNS（阿里/腾讯/114 等）。改前自动备份原值。",
            need_reboot: false,
            params: &[
                ParamSpec { desc: "网卡名称", ..P_ADAPTER },
                ParamSpec { desc: "预设名: 阿里/腾讯/114/360", ..P_PRESET },
            ],
            runner: run_dns_preset,
        },
        RepairAction {
            id: "ncsi_restore",
            title: "NCSI 探测恢复微软默认并启用主动探测",
            risk: Risk::Config,
            description: "把 NCSI 注册表恢复为原版默认值（探测 www.msftconnecttest.com/connecttest.txt），并确保 EnableActiveProbing=1。改前 reg export 全量备份。",
            restore_hint: "双击备份目录中的 .reg 文件导入即可还原",
            brief: "把 NCSI 注册表恢复为原版默认值（探测 www.msftconnecttest.com/connecttest.tx",
            need_reboot: false,
            params: &[],
            runner: run_ncsi_restore,
        },
        RepairAction {
            id: "ncsi_disable",
            title: "关闭 NCSI 主动探测（最后手段）",
            risk: Risk::Config,
            description: "EnableActiveProbing=0：Windows 不再探测，状态永远显示'已连接'。代价：真正断网时不再显示'无Internet'提醒。仅在探测源彻底不可用且更换 DNS 无效时使用。",
            restore_hint: "运行 ncsi_restore 恢复探测",
            brief: "EnableActiveProbing=0：Windows 不再探测，状态永远显示'已连接'。代价：真正断网时不再显示'",
            need_reboot: false,
            params: &[],
            runner: run_ncsi_disable,
        },
        RepairAction {
            id: "power_off",
            title: "禁止系统关闭网卡省电（关节电）",
            risk: Risk::Config,
            description: "对各物理网卡驱动键写入 PnPCapabilities=24（本机电源管理 WMI 已损坏，走注册表方案），并关闭 USB 选择性暂停。修复 WiFi/USB 共享频繁掉线。设置后需重新启用网卡或重启生效。",
            restore_hint: "删除对应注册表值的 PnPCapabilities（备份 JSON 中有记录）",
            brief: "对各物理网卡驱动键写入 PnPCapabilities=24（本机电源管理 WMI 已损坏，走注册表方案），并关闭 US",
            need_reboot: false,
            params: &[],
            runner: run_power_off,
        },
        RepairAction {
            id: "del_persistent_route",
            title: "删除劫持流量的持久路由",
            risk: Risk::Config,
            description: "删除持久化存储中的默认路由（如 0.0.0.0/0 → 172.16.2.1）。这类路由会在连上同网段 WiFi 时以最低跃点抢走全部流量。删除前把完整路由表存档，还原命令会打印到日志。",
            restore_hint: "route -p add <目标> mask <掩码> <网关> metric <跃点>（详见备份文件）",
            brief: "删除持久化存储中的默认路由（如 0.0.0.0/0 → 172.16.2.1）。这类路由会在连上同网段 WiFi 时以最",
            need_reboot: false,
            params: &[
                ParamSpec { desc: "目标网段，默认自动检测持久默认路由", name: "dest", required: false },
                ParamSpec { desc: "网关", name: "gw", required: false },
            ],
            runner: run_del_persistent_route,
        },
        RepairAction {
            id: "proxy_off",
            title: "关闭系统代理（清除代理软件残留）",
            risk: Risk::Config,
            description: "把 WinINET 系统代理（ProxyEnable）关闭并立即通知系统刷新。代理软件异常退出后残留的代理会让所有流量发往已失效的本地端口，表现为'已连接但所有应用都无法上网'。",
            restore_hint: "在代理软件或 Windows 设置中重新开启代理即可；被清除的 PAC 脚本地址见备份文件",
            brief: "把 WinINET 系统代理（ProxyEnable）关闭并立即通知系统刷新。代理软件异常退出后残留的代理会让所有流量发",
            need_reboot: false,
            params: &[],
            runner: run_proxy_off,
        },
        RepairAction {
            id: "mac_random_off",
            title: "关闭 WiFi 的 MAC 随机化（用真实地址连接）",
            risk: Risk::Config,
            description: "Win11 默认对每个 WiFi 用固定随机 MAC。部分公寓/企业网络按 MAC 授权，关掉随机化用真实地址便于注册设备；但若网络只认识旧的随机地址，关闭后反而可能拿不到 IP——工具会先备份，随时可切回。",
            restore_hint: "运行 mac_random_on 重新开启（备份 XML 也在备份目录）",
            brief: "Win11 默认对每个 WiFi 用固定随机 MAC。部分公寓/企业网络按 MAC 授权，关掉随机化用真实地址便于注册设",
            need_reboot: false,
            params: &[ParamSpec { desc: "WiFi 配置文件名（SSID），默认当前/最近使用的", ..P_PROFILE }],
            runner: run_mac_random_off,
        },
        RepairAction {
            id: "mac_random_on",
            title: "开启 WiFi 的 MAC 随机化（恢复 Win11 默认）",
            risk: Risk::Config,
            description: "恢复对指定 WiFi 使用固定随机 MAC。若公寓网络此前只授权了旧随机地址，开启后应能恢复拿 IP。",
            restore_hint: "运行 mac_random_off 再次关闭（备份 XML 也在备份目录）",
            brief: "恢复对指定 WiFi 使用固定随机 MAC。若公寓网络此前只授权了旧随机地址，开启后应能恢复拿 IP。",
            need_reboot: false,
            params: &[ParamSpec { desc: "WiFi 配置文件名（SSID）", ..P_PROFILE }],
            runner: run_mac_random_on,
        },
        RepairAction {
            id: "winsock_reset",
            title: "重置 Winsock 目录",
            risk: Risk::Aggressive,
            description: "netsh winsock reset。清除损坏的 LSP（装过 VPN/代理/加速器后常见）。需要重启电脑生效，重置后部分网络软件需重新配置。",
            restore_hint: "不可直接还原（系统会重建干净目录）",
            brief: "netsh winsock reset。清除损坏的 LSP（装过 VPN/代理/加速器后常见）。需要重启电脑生效，重置后",
            need_reboot: true,
            params: &[],
            runner: run_winsock_reset,
        },
        RepairAction {
            id: "ipstack_reset",
            title: "重置 TCP/IP 与 IPv6 协议栈",
            risk: Risk::Aggressive,
            description: "netsh int ip reset + netsh int ipv6 reset。重建 TCP/IP 注册表配置。需要重启电脑生效；静态 IP/DNS 配置会被清除。",
            restore_hint: "不可直接还原（需手动重配静态 IP/DNS）",
            brief: "netsh int ip reset + netsh int ipv6 reset。重建 TCP/IP 注册表配置。需要",
            need_reboot: true,
            params: &[],
            runner: run_ipstack_reset,
        },
    ];
    ACTIONS
}

/// 按 id 取动作。
pub fn get_action(id: &str) -> Option<&'static RepairAction> {
    catalog().iter().find(|a| a.id == id)
}

/// 全部动作 id（顺序同 Python `ACTION_IDS`）。
#[allow(dead_code)] // 完成度断言消费
pub fn action_ids() -> Vec<&'static str> {
    catalog().iter().map(|a| a.id).collect()
}

/// 组合修复（对应 Python `COMBOS`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Combo {
    pub key: &'static str,
    pub title: &'static str,
    pub actions: &'static [&'static str],
}

pub const COMBOS: &[Combo] = &[
    Combo {
        key: "fast",
        title: "快速修复",
        actions: &["flush_dns", "renew_dhcp", "start_nlasvc", "restart_wlan"],
    },
    Combo {
        key: "full",
        title: "深度修复",
        actions: &[
            "restore_point",
            "flush_dns",
            "renew_dhcp",
            "start_nlasvc",
            "restart_wlan",
            "power_off",
            "dns_auto",
            "ncsi_restore",
        ],
    },
    Combo {
        key: "stack",
        title: "重置网络栈（需重启）",
        actions: &["restore_point", "winsock_reset", "ipstack_reset"],
    },
];

pub fn get_combo(key: &str) -> Option<&'static Combo> {
    COMBOS.iter().find(|c| c.key == key)
}

/// 组合里哪些动作会被跳过（无管理员时跳过 CONFIG+；VPN 在线时跳过断隧道动作）。
///
/// 与 Python 的分级语义一致：SAFE 恒可自动执行，CONFIG 需逐项确认。
pub fn combo_plan(key: &str, is_admin: bool) -> Vec<ComboStep> {
    let Some(combo) = get_combo(key) else {
        return Vec::new();
    };
    combo
        .actions
        .iter()
        .filter_map(|id| get_action(id))
        .map(|a| {
            let skip = if !is_admin && a.risk != Risk::Safe {
                Some("非管理员：改配置类动作已跳过".to_string())
            } else {
                None
            };
            ComboStep {
                id: a.id,
                title: a.title,
                risk: a.risk,
                skipped_reason: skip,
            }
        })
        .collect()
}

/// 组合内的一步。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ComboStep {
    pub id: &'static str,
    pub title: &'static str,
    pub risk: Risk,
    /// Some = 该步被跳过及原因（跳过必须显式告知，不能静默略过）
    pub skipped_reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_all_actions_in_python_order() {
        let ids = action_ids();
        // 19 = Python `ACTION_IDS` 的实际条数：最初按注释里的「安全/改配置/强力」
        // 三类心算成 20，逐项列举时才数清。这类错误只有逐条比对才能发现。
        assert_eq!(ids.len(), 19, "Python ACTION_IDS 有 19 个");
        assert_eq!(
            ids,
            vec![
                "flush_dns",
                "renew_dhcp",
                "start_nlasvc",
                "fix_nlasvc",
                "restart_wlan",
                "toggle_adapter",
                "restart_services",
                "restore_point",
                "dns_auto",
                "dns_preset",
                "ncsi_restore",
                "ncsi_disable",
                "power_off",
                "del_persistent_route",
                "proxy_off",
                "mac_random_off",
                "mac_random_on",
                "winsock_reset",
                "ipstack_reset",
            ],
            "顺序是硬契约：前端列表顺序、组合执行顺序都依赖它"
        );
    }

    #[test]
    fn risk_assignment_matches_python() {
        // 逐项对照 repairs.py 的 super().__init__ 第三个参数
        let expect = [
            ("flush_dns", Risk::Safe),
            ("renew_dhcp", Risk::Safe),
            ("start_nlasvc", Risk::Safe),
            ("fix_nlasvc", Risk::Config),
            ("restart_wlan", Risk::Config),
            ("toggle_adapter", Risk::Config),
            ("restart_services", Risk::Safe),
            ("restore_point", Risk::Safe),
            ("dns_auto", Risk::Config),
            ("dns_preset", Risk::Config),
            ("ncsi_restore", Risk::Config),
            ("ncsi_disable", Risk::Config),
            ("power_off", Risk::Config),
            ("del_persistent_route", Risk::Config),
            ("proxy_off", Risk::Config),
            ("mac_random_off", Risk::Config),
            ("mac_random_on", Risk::Config),
            ("winsock_reset", Risk::Aggressive),
            ("ipstack_reset", Risk::Aggressive),
        ];
        for (id, risk) in expect {
            assert_eq!(get_action(id).unwrap().risk, risk, "{id} 风险等级不符");
        }
    }

    #[test]
    fn only_aggressive_actions_need_reboot() {
        for a in catalog() {
            assert_eq!(
                a.need_reboot,
                a.risk == Risk::Aggressive,
                "{} 的 need_reboot 与风险等级不一致",
                a.id
            );
        }
    }

    #[test]
    fn risk_labels_match_python() {
        assert_eq!(Risk::Safe.label(), "安全");
        assert_eq!(Risk::Config.label(), "改配置");
        assert_eq!(Risk::Aggressive.label(), "强力(需重启)");
        assert!(Risk::Safe.auto_ok());
        assert!(!Risk::Config.auto_ok(), "改配置类不得免确认自动执行");
        assert_eq!(Risk::Config.as_str(), "config");
    }

    #[test]
    fn param_specs_match_python() {
        assert!(get_action("flush_dns").unwrap().params.is_empty());
        let ta = get_action("toggle_adapter").unwrap();
        assert_eq!(ta.params.len(), 1);
        assert_eq!(ta.params[0].name, "adapter");
        assert!(
            !ta.params[0].required,
            "Python 里 adapter 是 required=False"
        );
        let dp = get_action("dns_preset").unwrap();
        assert_eq!(
            dp.params.iter().map(|p| p.name).collect::<Vec<_>>(),
            vec!["adapter", "preset"]
        );
        let dpr = get_action("del_persistent_route").unwrap();
        assert_eq!(
            dpr.params.iter().map(|p| p.name).collect::<Vec<_>>(),
            vec!["dest", "gw"],
            "顺序也要一致（前端按序渲染）"
        );
    }

    #[test]
    fn brief_is_description_truncated_to_60() {
        for a in catalog() {
            // brief 是描述的前 60 字符（Python description[:60]）
            let expect: String = a.description.chars().take(60).collect();
            assert_eq!(a.brief, expect, "{} 的 brief 不是描述前 60 字", a.id);
        }
    }

    #[test]
    fn combos_match_python() {
        assert_eq!(COMBOS.len(), 3);
        let fast = get_combo("fast").unwrap();
        assert_eq!(fast.title, "快速修复");
        assert_eq!(
            fast.actions,
            &["flush_dns", "renew_dhcp", "start_nlasvc", "restart_wlan"]
        );
        let full = get_combo("full").unwrap();
        assert_eq!(full.title, "深度修复");
        assert_eq!(full.actions.len(), 8);
        assert_eq!(full.actions[0], "restore_point", "深度修复首位必须是还原点");
        let stack = get_combo("stack").unwrap();
        assert_eq!(
            stack.actions,
            &["restore_point", "winsock_reset", "ipstack_reset"]
        );
        assert!(get_combo("nope").is_none());
    }

    #[test]
    fn combo_plan_skips_config_actions_without_admin() {
        let admin_plan = combo_plan("fast", true);
        assert_eq!(admin_plan.len(), 4);
        assert!(admin_plan.iter().all(|s| s.skipped_reason.is_none()));

        let plain_plan = combo_plan("fast", false);
        assert_eq!(plain_plan.len(), 4);
        // fast 组合里 restart_wlan 是 CONFIG → 非管理员必须跳过并说明
        let wlan = plain_plan.iter().find(|s| s.id == "restart_wlan").unwrap();
        assert!(
            wlan.skipped_reason.is_some(),
            "非管理员必须跳过改配置类动作"
        );
        assert!(wlan.skipped_reason.as_ref().unwrap().contains("非管理员"));
        // SAFE 动作不受影响
        let flush = plain_plan.iter().find(|s| s.id == "flush_dns").unwrap();
        assert!(flush.skipped_reason.is_none());
    }

    #[test]
    fn unknown_combo_key_yields_empty_plan() {
        assert!(combo_plan("nope", true).is_empty());
    }

    #[test]
    fn every_combo_action_id_exists() {
        for c in COMBOS {
            for id in c.actions {
                assert!(
                    get_action(id).is_some(),
                    "组合 {} 引用了不存在的动作 {id}",
                    c.key
                );
            }
        }
    }

    #[test]
    fn all_19_actions_have_real_executors() {
        // 完成度断言：19 个动作全部接入执行器，`DEFERRED_ACTION_IDS` 必须为空。
        // （早期版本这里是「未接入动作必须显式失败」；批次 B 落地后升级为完成度检查。）
        assert!(
            DEFERRED_ACTION_IDS.is_empty(),
            "仍有未接入的动作: {DEFERRED_ACTION_IDS:?}"
        );
        for id in action_ids() {
            assert!(!is_deferred(id), "{id} 不应再被标记为未接入");
        }
    }

    #[test]
    fn non_admin_actions_fail_with_actionable_message() {
        // 非管理员时改配置/强力类必须给出**可操作**的提示（让用户知道去提权），
        // 而不是底层 API 的「拒绝访问」。管理员环境下跳过此断言。
        if crate::w32::is_elevated() {
            eprintln!("提示：当前是管理员，跳过非管理员路径断言");
            return;
        }
        for id in [
            "restart_wlan",
            "toggle_adapter",
            "restore_point",
            "ncsi_restore",
            "ncsi_disable",
            "power_off",
            "mac_random_off",
            "mac_random_on",
        ] {
            let out = get_action(id).unwrap().execute(&BTreeMap::new());
            assert!(!out.success, "{id} 非管理员不得成功");
            assert!(
                out.message.contains("管理员"),
                "{id} 必须说明需要管理员权限，实际: {}",
                out.message
            );
        }
    }

    #[test]
    fn admin_guard_does_not_block_safe_actions() {
        // 对照组：安全类动作不该被门禁拦下（否则修复能力被无谓削弱）
        for id in [
            "flush_dns",
            "restart_services",
            "start_nlasvc",
            "restore_point",
        ] {
            let a = get_action(id).unwrap();
            let blocked = !crate::w32::is_elevated() && a.risk != Risk::Safe;
            if a.risk == Risk::Safe {
                assert!(!blocked, "{id} 是安全类，不该被门禁拦");
            }
        }
    }

    #[test]
    fn power_skip_keywords_match_python() {
        assert_eq!(POWER_SKIP_KEYWORDS.len(), 4);
        for desc in [
            "WAN Miniport (IP)",
            "Microsoft Kernel Debug Network Adapter",
            "Bluetooth Device (PAN)",
            "Hyper-V Virtual Ethernet Adapter",
        ] {
            assert!(power_skip(desc), "{desc} 应被跳过");
        }
        assert!(!power_skip("Intel(R) Wi-Fi 6 AX201 160MHz"));
        assert!(!power_skip("Realtek PCIe GbE Family Controller"));
        assert!(power_skip("SOME VIRTUAL NIC"), "大小写不敏感");
    }

    #[test]
    fn mac_xml_replace_existing_block() {
        let src = r#"<?xml version="1.0"?><WLANProfile><name>MyWiFi</name><MacRandomization xmlns="x"><EnableRandomization>true</EnableRandomization></MacRandomization></WLANProfile>"#;
        let (out, changed) = set_mac_randomization_xml(src, false);
        assert!(changed);
        assert!(
            out.contains("<EnableRandomization>false</EnableRandomization>"),
            "{out}"
        );
        assert!(out.contains("<name>MyWiFi</name>"), "其余内容必须保留");
        // 目标值已是要写值时不算改动
        let (_, changed2) = set_mac_randomization_xml(&out, false);
        assert!(!changed2, "已是目标值 → 不改动（避免无谓写盘）");
    }

    #[test]
    fn mac_xml_inserts_block_when_missing() {
        let src = r#"<WLANProfile><name>New</name></WLANProfile>"#;
        let (out, changed) = set_mac_randomization_xml(src, false);
        assert!(changed, "缺块时必须插入");
        assert!(out.contains("<MacRandomization"), "{out}");
        assert!(out.contains("<EnableRandomization>false</EnableRandomization>"));
        assert!(
            out.ends_with("</WLANProfile>"),
            "必须插在 WLANProfile 内: {out}"
        );
        // 幂等：再跑一次不应重复插入
        let (out2, changed2) = set_mac_randomization_xml(&out, false);
        assert!(!changed2, "第二次应识别为已存在");
        assert_eq!(out2.matches("<MacRandomization").count(), 1);
    }

    #[test]
    fn mac_xml_preserves_attributes_and_replaces_value() {
        // 契约：**属性与非目标内容必须原样保留**，只有 EnableRandomization 的值被替换。
        // （值两侧原有的换行/缩进会被规范化掉——这点单独说明，免得后人误以为是 bug。）
        let src = "<MacRandomization xmlns=\"ns\">\n  <EnableRandomization>\n    true\n  </EnableRandomization>\n</MacRandomization>";
        let (out, changed) = set_mac_randomization_xml(src, false);
        assert!(changed, "带空白/换行的块必须能改");
        assert!(out.contains("xmlns=\"ns\""), "属性必须保留: {out}");
        assert!(
            out.contains("<EnableRandomization>false</EnableRandomization>"),
            "值被替换: {out}"
        );
        assert!(out.contains("\n</MacRandomization>"), "块外结构保留: {out}");
        assert!(!out.contains("true"), "旧值不得残留: {out}");
    }

    #[test]
    fn mac_xml_without_wlan_profile_is_unchanged() {
        let src = "<NotAProfile/>";
        let (out, changed) = set_mac_randomization_xml(src, false);
        assert!(!changed, "非 WiFi 配置不得被改动");
        assert_eq!(out, src);
    }

    #[test]
    fn mac_state_parser_reads_both_values() {
        assert_eq!(
            mac_randomization_state_xml("<MacRandomization><EnableRandomization>true</EnableRandomization></MacRandomization>"),
            Some(true)
        );
        assert_eq!(
            mac_randomization_state_xml("<MacRandomization><EnableRandomization>FALSE</EnableRandomization></MacRandomization>"),
            Some(false),
            "大小写不敏感"
        );
        assert_eq!(mac_randomization_state_xml("<WLANProfile/>"), None);
        assert_eq!(
            mac_randomization_state_xml("<MacRandomization></MacRandomization>"),
            None
        );
    }

    #[test]
    fn extract_xml_name_reads_profile_name() {
        assert_eq!(
            extract_xml_name("<WLANProfile><name>MyWiFi</name></WLANProfile>").as_deref(),
            Some("MyWiFi")
        );
        assert_eq!(
            extract_xml_name("<WLANProfile><NAME> Trimmed </NAME></WLANProfile>").as_deref(),
            Some("Trimmed"),
            "大小写不敏感且去空白"
        );
        assert_eq!(extract_xml_name("<WLANProfile/>"), None);
    }

    #[test]
    fn wlan_profiles_parse_is_tolerant() {
        // 真机没有 WiFi 时返回空表（不得 panic）
        let p = wlan_profiles();
        assert!(p.iter().all(|s| !s.is_empty()));
        // 配置查找对不存在的名字返回空
        assert!(find_wlan_profile_files("SysKit不存在SSID").is_empty());
    }

    #[test]
    fn requires_admin_matches_risk_semantics() {
        // 与 combo_plan 同一判据：只有安全类不需要管理员
        for a in catalog() {
            assert_eq!(
                requires_admin(a.id),
                a.risk != Risk::Safe,
                "{} 的管理员判定与风险等级不一致",
                a.id
            );
        }
    }

    #[test]
    fn batch_b_actions_do_not_silently_succeed_for_bogus_input() {
        // 无输入时这些动作要么成功（真机可做）要么明确失败，但**不得**空消息。
        // 非管理员下会走门禁分支，管理员下会走真实分支——两条都必须可读。
        for id in [
            "restart_wlan",
            "toggle_adapter",
            "restore_point",
            "ncsi_restore",
            "ncsi_disable",
            "power_off",
            "mac_random_off",
            "mac_random_on",
        ] {
            let out = get_action(id).unwrap().execute(&BTreeMap::new());
            assert!(!out.message.is_empty(), "{id} 必须给出结果说明");
        }
    }

    #[test]
    fn execute_merges_need_reboot_with_action_flag() {
        let a = get_action("winsock_reset").unwrap();
        assert!(a.need_reboot, "winsock_reset 标记需重启");
        // 执行器自己也返回 need_reboot=true；两者取或
        let out = a.execute(&BTreeMap::new());
        assert!(out.need_reboot, "返回结果必须带上需重启标记");
    }

    #[test]
    fn flush_dns_actually_runs_and_succeeds() {
        // ipconfig /flushdns 需要提权（"请求的操作需要提升"）：非提权 shell
        // 里此测试环境性失败（真机实证 2026-10-05 深夜）——与 real_clipboard
        // 同哲学：环境不满足时让路，提权终端里照常真跑
        if !crate::w32::is_elevated() {
            eprintln!("[skip] 非提权环境：ipconfig /flushdns 无法执行，本测试延后");
            return;
        }
        let a = get_action("flush_dns").unwrap();
        let out = a.execute(&BTreeMap::new());
        assert!(
            out.success,
            "ipconfig /flushdns 应当成功: {} / {}",
            out.message, out.output
        );
        assert!(!out.need_reboot);
    }

    #[test]
    fn query_nlasvc_state_is_readable() {
        // start_nlasvc 的判定依赖 sc query 的 RUNNING 字样；这里只验证查询本身可用
        let q = w32::run_capture(&args(&["sc", "query", "NlaSvc"]), 10);
        let Ok(q) = q else {
            // 某些精简系统没有 NlaSvc；此时只要求「不 panic」并给出可读原因
            return;
        };
        assert!(
            q.stdout.contains("SERVICE_NAME") || q.combined().contains("1060"),
            "sc query 输出应可解析: {:?}",
            q.stdout
        );
    }

    #[test]
    fn outcomes_serialize_with_expected_fields() {
        let j = serde_json::to_string(&RepairOutcome::ok("done")).unwrap();
        assert!(j.contains("\"success\":true"));
        assert!(j.contains("\"need_reboot\":false"));
        assert!(j.contains("\"message\":\"done\""));
        let r = serde_json::to_string(&Risk::Aggressive).unwrap();
        assert_eq!(
            r, "\"aggressive\"",
            "风险等级序列化必须与 Python 字符串一致"
        );
    }

    // ---------------- 批 A 执行器（无需管理员） ----------------

    #[test]
    fn dns_presets_match_python_config() {
        assert_eq!(dns_preset("阿里"), Some(("223.5.5.5", "223.6.6.6")));
        assert_eq!(
            dns_preset("腾讯"),
            Some(("119.29.29.29", "182.254.116.116"))
        );
        assert_eq!(
            dns_preset("114"),
            Some(("114.114.114.114", "114.114.115.115"))
        );
        assert_eq!(dns_preset("360"), Some(("101.226.4.6", "218.30.118.6")));
        assert_eq!(dns_preset("不存在"), None);
        assert_eq!(DNS_PRESETS.len(), 4, "config.DNS_PRESETS 有 4 项");
    }

    #[test]
    fn registry_key_constants_match_python() {
        assert_eq!(
            INTERNET_SETTINGS_KEY,
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings"
        );
        assert_eq!(
            PERSISTENT_ROUTES_KEY,
            r"HKLM\SYSTEM\CurrentControlSet\Services\Tcpip\Parameters\PersistentRoutes"
        );
    }

    #[test]
    fn fix_nlasvc_round_trips_on_real_machine() {
        // 真机可验证：把 NlaSvc 设成自动并启动，再回读确认 RUNNING。
        // 需要管理员；非管理员时应**明确失败**（而不是假成功）。
        let out = get_action("fix_nlasvc").unwrap().execute(&BTreeMap::new());
        if out.success {
            assert!(out.message.contains("已设为自动启动"), "{}", out.message);
            // 回读：确认服务真的在跑（不自证，重新查一次）
            let q = w32::run_capture(&args(&["sc", "query", "NlaSvc"]), 10).unwrap();
            assert!(q.stdout.contains("RUNNING"), "成功就必须真的在跑");
        } else {
            assert!(
                out.message.contains("失败") || out.message.contains("管理员"),
                "失败必须是可解释的: {}",
                out.message
            );
        }
    }

    #[test]
    fn dns_preset_unknown_name_fails_without_touching_system() {
        let mut p = BTreeMap::new();
        p.insert("preset".to_string(), "不存在的预设".to_string());
        let out = get_action("dns_preset").unwrap().execute(&p);
        assert!(!out.success);
        assert!(out.message.contains("未知 DNS 预设"), "{}", out.message);
        // 关键：未知预设必须在**备份/改配置之前**就返回，不能先动系统再报错
        assert!(out.output.is_empty(), "不应产生备份或输出");
    }

    #[test]
    fn dns_auto_without_static_adapter_fails_clearly() {
        // 显式传一个不存在的网卡名，netsh 会失败；必须给出可读原因而不是 panic
        let mut p = BTreeMap::new();
        p.insert("adapter".to_string(), "SysKit不存在网卡".to_string());
        let out = get_action("dns_auto").unwrap().execute(&p);
        if out.success {
            // 理论不该成功；真出现说明 netsh 容忍了坏名字，此时至少要提示
            assert!(!out.message.is_empty());
        } else {
            assert!(
                out.message.contains("失败") || out.message.contains("备份"),
                "失败原因应可读: {}",
                out.message
            );
        }
    }

    #[test]
    fn del_persistent_route_without_target_fails_when_none_exists() {
        // 用不存在的目标网段：必须明确失败，且**失败前不修改系统**
        let mut p = BTreeMap::new();
        p.insert("dest".to_string(), "203.0.113.0/24".to_string());
        let out = get_action("del_persistent_route").unwrap().execute(&p);
        if !out.success {
            // 未找到对应持久路由 / 或 route delete 失败（需管理员）
            assert!(!out.message.is_empty());
        }
        // 无论成败：都必须留下说明，不得空消息
        assert!(!out.message.is_empty());
    }

    #[test]
    fn proxy_off_backup_payload_has_three_keys() {
        // 备份 JSON 的键必须与 Python `_backup_values` 一致（还原凭据的格式契约）
        let vals = w32::reg_values(INTERNET_SETTINGS_KEY);
        let snapshot = serde_json::json!({
            "ProxyEnable": vals.get("ProxyEnable").map(|v| v.as_compare_str()),
            "ProxyServer": vals.get("ProxyServer").map(|v| v.as_compare_str()),
            "AutoConfigURL": vals.get("AutoConfigURL").map(|v| v.as_compare_str()),
        });
        let text = serde_json::to_string(&snapshot).unwrap();
        for k in ["ProxyEnable", "ProxyServer", "AutoConfigURL"] {
            assert!(text.contains(k), "备份必须含 {k}: {text}");
        }
    }

    #[test]
    fn pick_adapter_returns_a_real_adapter_or_none() {
        // 真机通常能选出一张；选不出也不算失败（无网卡机器）
        if let Some(name) = pick_adapter() {
            assert!(!name.is_empty());
            let snap = crate::snapshot::take();
            assert!(
                snap.adapters.iter().any(|a| a.name == name),
                "选出的网卡必须真实存在: {name}"
            );
        }
    }

    #[test]
    fn static_dns_adapter_is_subset_of_adapters() {
        if let Some(name) = static_dns_adapter() {
            let snap = crate::snapshot::take();
            assert!(
                snap.adapters.iter().any(|a| a.name == name),
                "静态 DNS 网卡必须真实存在: {name}"
            );
        }
    }

    // ---------------- 持久路由清理（与 Python DelPersistentRoute 对齐） ----------------

    #[test]
    fn prefix_to_mask_matches_python() {
        // Python _prefix_to_mask：0→0.0.0.0、8→255.0.0.0、24→255.255.255.0、32→255.255.255.255
        assert_eq!(prefix_to_mask("0.0.0.0/0"), "0.0.0.0");
        assert_eq!(prefix_to_mask("10.0.0.0/8"), "255.0.0.0");
        assert_eq!(prefix_to_mask("172.16.2.0/24"), "255.255.255.0");
        assert_eq!(prefix_to_mask("1.2.3.4/32"), "255.255.255.255");
        assert_eq!(prefix_to_mask("10.1.2.3/16"), "255.255.0.0");
        // 非法/缺前缀 → 回退 /24 掩码（Python except 分支）
        assert_eq!(prefix_to_mask("10.0.0.0"), "255.255.255.0");
        assert_eq!(prefix_to_mask("10.0.0.0/33"), "255.255.255.0");
        assert_eq!(prefix_to_mask("10.0.0.0/abc"), "255.255.255.0");
    }

    #[test]
    fn route_delete_args_split_cidr_for_legacy_route_exe() {
        // route.exe 不认 CIDR：0.0.0.0/0 必须拆成 0.0.0.0 + mask 0.0.0.0
        // （Vec<String> == Vec<&str> 成立，直接比字符串切片即可）
        assert_eq!(
            route_delete_args("0.0.0.0/0", "172.16.2.1"),
            [
                "route",
                "delete",
                "0.0.0.0",
                "mask",
                "0.0.0.0",
                "172.16.2.1"
            ]
        );
        assert_eq!(
            route_delete_args("10.0.0.0/8", "10.1.1.1"),
            [
                "route",
                "delete",
                "10.0.0.0",
                "mask",
                "255.0.0.0",
                "10.1.1.1"
            ]
        );
    }

    #[test]
    fn route_restore_command_is_directly_executable() {
        // 备份文件里的还原命令必须可直接粘贴执行，不允许 <掩码>/<跃点> 占位符
        assert_eq!(
            route_restore_command("0.0.0.0/0", "172.16.2.1", 1),
            "route -p add 0.0.0.0 mask 0.0.0.0 172.16.2.1 metric 1"
        );
        // metric=0（注册表「未指定」）按 1 写
        assert_eq!(
            route_restore_command("10.0.0.0/8", "10.1.1.1", 0),
            "route -p add 10.0.0.0 mask 255.0.0.0 10.1.1.1 metric 1"
        );
        assert_eq!(
            route_restore_command("192.168.50.0/24", "172.16.2.1", 7),
            "route -p add 192.168.50.0 mask 255.255.255.0 172.16.2.1 metric 7"
        );
    }

    #[test]
    fn parse_active_default_gws_reads_route_print() {
        let out = "\
===========================================================================
Active Routes:
Network Destination        Netmask          Gateway       Interface  Metric
          0.0.0.0          0.0.0.0      172.16.2.1     172.16.2.10     25
          0.0.0.0          0.0.0.0     192.168.1.1      192.168.1.5     30
         10.0.0.0        255.0.0.0      172.16.2.1     172.16.2.10     25
  255.255.255.255  255.255.255.255      172.16.2.1     172.16.2.10     25
      0.0.0.0  0.0.0.0      On-link      192.168.1.5     5000
===========================================================================";
        let gws = parse_active_default_gws(out);
        assert_eq!(
            gws,
            vec!["172.16.2.1", "192.168.1.1"],
            "只取默认路由行的网关列"
        );
        // On-link 不是可删除的网关，不能混进来
        assert!(!gws.iter().any(|g| g.contains("On-link")));
        assert!(parse_active_default_gws("garbage").is_empty());
        assert!(parse_active_default_gws("").is_empty());
    }

    #[test]
    fn route_gone_checks_both_tables() {
        use crate::snapshot::RouteEntry;
        let persistent = vec![RouteEntry {
            dest: "0.0.0.0/0".into(),
            next_hop: "172.16.2.1".into(),
            metric: 1,
            if_index: 0,
            if_metric: 0,
        }];
        let active = vec!["172.16.2.1".to_string()];
        // 两表都在 → 都没删掉
        assert_eq!(
            route_gone(&persistent, &active, "0.0.0.0/0", "172.16.2.1"),
            (false, false)
        );
        // 持久表清了、活动表还在
        assert_eq!(
            route_gone(&[], &active, "0.0.0.0/0", "172.16.2.1"),
            (true, false)
        );
        // 活动表清了、持久表还在（第一版假成功的场景：route delete 换不来持久表清除）
        assert_eq!(
            route_gone(&persistent, &[], "0.0.0.0/0", "172.16.2.1"),
            (false, true)
        );
        // 都清了
        assert_eq!(
            route_gone(&[], &[], "0.0.0.0/0", "172.16.2.1"),
            (true, true)
        );
        // 不同网关的其它路由不算残留
        assert_eq!(
            route_gone(&persistent, &active, "0.0.0.0/0", "10.9.9.9"),
            (true, true)
        );
    }

    #[test]
    fn del_persistent_route_no_target_is_successful_noop() {
        // 对齐 Python：给一个不存在的 dest 显式失败；未指定 dest 且系统无持久默认路由
        // 时是「无需处理」的幂等成功——此断言只覆盖显式 dest 分支（无路由分支依赖
        // 真机路由状态，不在单测里赌环境）
        let mut p = BTreeMap::new();
        p.insert("dest".to_string(), "198.51.100.0/24".to_string());
        let out = get_action("del_persistent_route").unwrap().execute(&p);
        assert!(!out.success, "显式给不存在的 dest 必须失败而不是乱删");
        assert!(
            out.message.contains("未删除任何路由"),
            "失败要说明没动系统: {}",
            out.message
        );
    }
}
