//! 安全执行器批次（无需管理员：DNS 缓存/服务/Winsock/IP 栈/DHCP）。
use super::*;

// ---------------- 执行器：安全且可验证的一批 ----------------

/// 刷新 DNS 解析缓存（无副作用，可反复执行）。
pub fn run_flush_dns(_p: &BTreeMap<String, String>) -> RepairOutcome {
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
pub fn run_start_nlasvc(_p: &BTreeMap<String, String>) -> RepairOutcome {
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
pub fn run_restart_services(_p: &BTreeMap<String, String>) -> RepairOutcome {
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
pub fn run_winsock_reset(_p: &BTreeMap<String, String>) -> RepairOutcome {
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
pub fn run_ipstack_reset(_p: &BTreeMap<String, String>) -> RepairOutcome {
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
pub fn run_renew_dhcp(_p: &BTreeMap<String, String>) -> RepairOutcome {
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

/// 需要管理员 / 改注册表 / 改网卡配置的动作：尚未接入的那批。
///
/// **为什么统一给一个「显式失败」的执行器**：修复链里的假成功（例如返回 ok 但
/// 什么都没做）会让用户以为问题已经处理，比「明确说尚未接入」危险得多。
#[allow(dead_code)] // 延迟执行器机制——显式失败执行器，新增暂缓动作时接回
pub fn run_deferred(_p: &BTreeMap<String, String>) -> RepairOutcome {
    RepairOutcome::unimplemented(
        "该动作",
        "需要管理员/注册表/网卡配置，排在执行器移植的下一批",
    )
}

/// 仍未接入执行器的动作 id（**声明式清单**）。
///
/// 用途是让测试在不执行动作的前提下断言「接入进度」——早期版本靠
/// `execute()` 遍历所有动作来判断，结果在单元测试里**真的执行了**
/// `renew_dhcp`（ipconfig release/renew）与 `winsock_reset` 等系统级动作，
/// 既慢（实测 101 秒）又危险。判断实现状态不该有副作用。
///
/// **已清空**：19 个动作的执行器全部接入（批次 0 + A + B）。
/// 新增动作若暂不实现执行器，把 id 加回这里并让测试断言它显式失败。
#[allow(dead_code)] // 延迟执行器机制——完成度断言消费
pub const DEFERRED_ACTION_IDS: [&str; 0] = [];

/// 该动作是否尚未接入执行器。
#[allow(dead_code)] // 延迟执行器机制——判定入口
pub fn is_deferred(id: &str) -> bool {
    DEFERRED_ACTION_IDS.contains(&id)
}
