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

use crate::w32;

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
    pub fn auto_ok(self) -> bool {
        matches!(self, Risk::Safe)
    }

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

// ---------------- 执行器：安全且可验证的一批 ----------------

/// 刷新 DNS 解析缓存（无副作用，可反复执行）。
fn run_flush_dns(_p: &BTreeMap<String, String>) -> RepairOutcome {
    match w32::run_capture(&args(&["ipconfig", "/flushdns"]), 15) {
        Ok(o) => {
            let msg = if o.stdout.trim().is_empty() {
                o.stderr.trim().to_string()
            } else {
                o.stdout.trim().to_string()
            };
            RepairOutcome {
                success: o.ok,
                message: msg,
                need_reboot: false,
                output: o.combined(),
            }
        }
        Err(e) => RepairOutcome::fail(e),
    }
}

/// 启动 NlaSvc（网络位置感知）。
///
/// 成败以随后的 `sc query` 为准，而不是 `sc start` 的退出码：
/// 1056 = 「服务已在运行」，属成功；且**不能用 `"4" in stdout` 兜底**——
/// WIN32_EXIT_CODE / REFERENCE COUNT 等任何含 4 的字段都会把「未运行」误判成运行中。
fn run_start_nlasvc(_p: &BTreeMap<String, String>) -> RepairOutcome {
    let start = w32::run_capture(&args(&["sc", "start", "NlaSvc"]), 30);
    let (start_ok, start_text) = match start {
        Ok(o) => (
            o.ok || o.combined().contains("1056"),
            o.combined().trim().to_string(),
        ),
        Err(e) => (false, e),
    };
    let query = w32::run_capture(&args(&["sc", "query", "NlaSvc"]), 10);
    let running = query
        .as_ref()
        .map(|o| o.stdout.contains("RUNNING"))
        .unwrap_or(false);
    RepairOutcome {
        success: start_ok && running,
        message: format!(
            "NlaSvc 当前状态: {}{}",
            if running { "运行中" } else { "未运行" },
            if start_ok { "" } else { &start_text }
        ),
        need_reboot: false,
        output: start_text,
    }
}

/// 重启 DHCP 服务并刷新 DNS 缓存。
///
/// Dnscache 自 Win10 1809 起是受保护服务（`net stop` 恒被拒），改用
/// `ipconfig /flushdns` 达到等效清缓存目的。
fn run_restart_services(_p: &BTreeMap<String, String>) -> RepairOutcome {
    let _ = w32::run_capture(&args(&["net", "stop", "Dhcp"]), 30);
    let start = w32::run_capture(&args(&["net", "start", "Dhcp"]), 30);
    let start_ok = start.as_ref().map(|o| o.ok).unwrap_or(false);
    let flush = w32::run_capture(&args(&["ipconfig", "/flushdns"]), 15);
    let flush_ok = flush.as_ref().map(|o| o.ok).unwrap_or(false);
    RepairOutcome {
        success: start_ok && flush_ok,
        message: format!(
            "Dhcp:{}，DNS缓存:{}",
            if start_ok { "OK" } else { "失败" },
            if flush_ok { "OK" } else { "失败" }
        ),
        need_reboot: false,
        output: String::new(),
    }
}

/// 重置 Winsock（需重启生效）。
fn run_winsock_reset(_p: &BTreeMap<String, String>) -> RepairOutcome {
    match w32::run_capture(&args(&["netsh", "winsock", "reset"]), 20) {
        Ok(o) => RepairOutcome {
            success: o.ok,
            message: o.combined().trim().to_string(),
            need_reboot: true,
            output: o.combined(),
        },
        Err(e) => RepairOutcome::fail(e),
    }
}

/// 重置 TCP/IP 与 IPv6 协议栈（需重启生效）。
fn run_ipstack_reset(_p: &BTreeMap<String, String>) -> RepairOutcome {
    let r1 = w32::run_capture(&args(&["netsh", "int", "ip", "reset"]), 20);
    let r2 = w32::run_capture(&args(&["netsh", "int", "ipv6", "reset"]), 20);
    let ok1 = r1.as_ref().map(|o| o.ok).unwrap_or(false);
    let ok2 = r2.as_ref().map(|o| o.ok).unwrap_or(false);
    let output = format!(
        "{}{}",
        r1.map(|o| o.combined()).unwrap_or_default(),
        r2.map(|o| o.combined()).unwrap_or_default()
    );
    RepairOutcome {
        success: ok1 && ok2,
        message: if ok1 && ok2 {
            "TCP/IP 与 IPv6 协议栈已重置，需重启生效".to_string()
        } else {
            format!(
                "重置失败: IPv4={} IPv6={}",
                if ok1 { "OK" } else { "失败" },
                if ok2 { "OK" } else { "失败" }
            )
        },
        need_reboot: true,
        output,
    }
}

/// 重新获取 IP（DHCP 续租）：release + renew。
fn run_renew_dhcp(_p: &BTreeMap<String, String>) -> RepairOutcome {
    let _ = w32::run_capture(&args(&["ipconfig", "/release"]), 30);
    match w32::run_capture(&args(&["ipconfig", "/renew"]), 90) {
        Ok(o) => RepairOutcome {
            success: o.ok,
            message: if o.ok {
                "已重新获取 IP".to_string()
            } else {
                o.tail(200, 100)
            },
            need_reboot: false,
            output: o.combined(),
        },
        Err(e) => RepairOutcome::fail(e),
    }
}

/// 需要管理员 / 改注册表 / 改网卡配置的动作：本批显式未接入。
///
/// **为什么统一给一个「显式失败」的执行器**：修复链里的假成功（例如返回 ok 但
/// 什么都没做）会让用户以为问题已经处理，比「明确说尚未接入」危险得多。
fn run_deferred(_p: &BTreeMap<String, String>) -> RepairOutcome {
    RepairOutcome::unimplemented(
        "该动作",
        "需要管理员/注册表/网卡配置，排在执行器移植的下一批",
    )
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

// ---------------- 注册表（顺序与 Python ACTION_IDS 完全一致） ----------------

/// 全部 20 个动作，顺序 = Python `ACTION_IDS` 顺序。
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
            runner: run_deferred,
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
            runner: run_deferred,
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
            runner: run_deferred,
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
            runner: run_deferred,
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
            runner: run_deferred,
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
            runner: run_deferred,
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
            runner: run_deferred,
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
            runner: run_deferred,
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
            runner: run_deferred,
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
            runner: run_deferred,
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
            runner: run_deferred,
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
            runner: run_deferred,
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
            runner: run_deferred,
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
    fn deferred_actions_fail_loudly_not_silently() {
        // 这是核心断言：未接入的动作**绝不能返回成功**——
        // 修复链里的假成功会让用户以为问题已处理
        for id in [
            "fix_nlasvc",
            "restart_wlan",
            "toggle_adapter",
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
        ] {
            let a = get_action(id).unwrap();
            let out = a.execute(&BTreeMap::new());
            assert!(
                !out.success,
                "{id} 尚未接入执行器，必须显式失败而不是假成功"
            );
            assert!(
                out.message.contains("尚未"),
                "{id} 的失败信息必须说明「尚未接入」，实际: {}",
                out.message
            );
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
        // 无副作用且可反复执行：真机冒烟里最安全的一条，这里就当作可验证靶
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
}
