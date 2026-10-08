//! 路径常量：与 Python 侧 config.py 的目录拓扑逐一对齐。
//!
//! 两版共用 `%APPDATA%\SysKit`（迁移期硬约束：零数据迁移）；便携模式下
//! （exe 旁存在 `data` 目录）全部数据落在安装目录，与 Python 版同规则。
//! 默认值表 DEFAULTS 是 settings.json 的类型收敛基准（见 settings.rs）。

use std::path::{Path, PathBuf};

pub const APP_NAME: &str = "SysKit";
pub const DISPLAY_NAME: &str = "SysKit 系统工具箱";
pub const VERSION: &str = "1.2.0";

/// 便携模式标记：exe 旁存在 `data` 目录时返回它（对应 config._portable_data_dir）。
fn portable_data_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let data = exe.parent()?.join("data");
    data.is_dir().then_some(data)
}

/// 进程级数据目录覆盖（仅测试注入；生产恒为 None）。
///
/// 存在的唯一理由：`History::save()` 等路径会把密封数据写进 `%APPDATA%\SysKit`，
/// 测试若落到真实用户目录就会污染用户数据并要求 DPAPI 成功——用覆盖把测试
/// 全部钉在临时目录里（对应 Python 侧 conftest 重定向 APPDATA 的做法）。
static APPDATA_OVERRIDE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// 设置数据目录覆盖；已设置时忽略（幂等，测试并发安全）。
#[cfg(test)]
pub fn set_appdata_override(dir: PathBuf) {
    let _ = APPDATA_OVERRIDE.set(dir);
}

fn appdata_override() -> Option<&'static PathBuf> {
    APPDATA_OVERRIDE.get()
}

fn env_dir(name: &str) -> PathBuf {
    match std::env::var_os(name) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => dirs_fallback(),
    }
}

/// APPDATA/LOCALAPPDATA 缺失时的兜底（与 Python 的 expanduser("~") 同义）。
fn dirs_fallback() -> PathBuf {
    match std::env::var_os("USERPROFILE") {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => PathBuf::from("."),
    }
}

/// `%APPDATA%\SysKit`（或便携 `<exe目录>\data\config`）。
pub fn appdata_dir() -> PathBuf {
    if let Some(d) = appdata_override() {
        return d.clone();
    }
    match portable_data_dir() {
        Some(p) => p.join("config"),
        None => env_dir("APPDATA").join(APP_NAME),
    }
}

/// `%LOCALAPPDATA%\SysKit`（或便携 `<exe目录>\data\local`）。
pub fn localappdata_dir() -> PathBuf {
    if let Some(d) = appdata_override() {
        return d.join("local");
    }
    match portable_data_dir() {
        Some(p) => p.join("local"),
        None => env_dir("LOCALAPPDATA").join(APP_NAME),
    }
}

pub fn settings_file() -> PathBuf {
    appdata_dir().join("settings.json")
}

pub fn clipboard_file() -> PathBuf {
    appdata_dir().join("clipboard.json")
}

pub fn clipboard_files_dir() -> PathBuf {
    appdata_dir().join("clipboard_files")
}

/// `traffic.json` —— 按天上下行累计（TrafficStore）。
pub fn traffic_file() -> PathBuf {
    appdata_dir().join("traffic.json")
}

/// `events.json` —— 断网/恢复事件时间线（EventStore）。
pub fn events_file() -> PathBuf {
    appdata_dir().join("events.json")
}

/// `netstats_buckets.json` —— 多粒度流量桶（RateStatsStore）。
pub fn netstats_file() -> PathBuf {
    appdata_dir().join("netstats_buckets.json")
}

/// 按应用流量桶（自有格式，非跨语言契约）。
pub fn netstats_apps_file() -> PathBuf {
    appdata_dir().join("netstats_apps.json")
}

pub fn log_dir() -> PathBuf {
    localappdata_dir().join("logs")
}

pub fn backup_dir() -> PathBuf {
    localappdata_dir().join("backup")
}

/// 修复备份新路径（v1.3）：`%APPDATA%\SysKit\repair-backups\`——便携模式
/// 自动落在 `<exe>\data\config\repair-backups\`，与「整体拷贝即迁移」对齐。
/// 旧路径 backup_dir()（%LOCALAPPDATA%）里的会话仍可列出（legacy 标记）。
pub fn repair_backups_dir() -> PathBuf {
    appdata_dir().join("repair-backups")
}

/// 图片载荷的绝对路径：`clipboard_files/<name>`；name 为空返回 None。
pub fn image_path(name: &str) -> Option<PathBuf> {
    if name.is_empty() {
        None
    } else {
        Some(clipboard_files_dir().join(name))
    }
}

/// 与 Python 的 `os.path.join` 等价的可读显示（日志/文案用）。
pub fn display(p: &Path) -> String {
    p.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topo_matches_python_config() {
        // 目录名与文件名的硬契约（config.py 同名同结构）。
        // 只做后缀断言：appdata_dir 可能被测试支撑重定向到临时目录。
        assert_eq!(APP_NAME, "SysKit");
        assert!(settings_file().ends_with("settings.json"));
        assert!(clipboard_file().ends_with("clipboard.json"));
        assert!(clipboard_files_dir().ends_with("clipboard_files"));
        assert!(log_dir().ends_with("logs"));
        assert!(backup_dir().ends_with("backup"));
    }

    #[test]
    fn image_path_empty_name_is_none() {
        assert!(image_path("").is_none());
        assert!(image_path("ab.png").unwrap().ends_with("ab.png"));
    }
}
