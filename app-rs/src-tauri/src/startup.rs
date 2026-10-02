//! 启动项枚举与禁用/启用（对应 Python 侧 `ui/system_page.py::StartupTab`）。
//!
//! ## 禁用机制（两个版本必须一致）
//!
//! 注册表启动项**没有**"启用/禁用"标志位——Windows 的 `StartupApproved` 是另一套
//! （且位运算含义未文档化）。这里沿用 Python 版的机制：把值从 `Run` 键**搬**到
//! SysKit 自己的 `HKCU\Software\NetTool\DisabledRun`，存储格式 `<位置>|<命令>`。
//! 启动文件夹的项则**改名**加 `.disabled` 后缀。
//!
//! 因此有两处**跨版本共享标识**，改错就会让两版各藏一份自己的禁用列表：
//! - `DISABLED_KEY` 是 `Software\NetTool\DisabledRun`——**沿用旧产品名 `NetTool`**，
//!   不是 `SysKit`。这是 Python 版既成的键名，改名会让用户已禁用的项在 Rust 版里
//!   "凭空复活"（`Run` 键里没有、禁用键里也找不到 = 直接消失）。
//! - 后缀固定 `.disabled`，且启用时必须**只剥一层**。

use std::path::PathBuf;

use serde::Serialize;

/// `Run` 键相对路径（HKCU 与 HKLM 同名）。
pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// 已禁用项的存放键（HKCU）。**必须是 `NetTool`**——见模块文档。
pub const DISABLED_KEY: &str = r"Software\NetTool\DisabledRun";
/// 启动文件夹项禁用后的后缀。
pub const DISABLED_SUFFIX: &str = ".disabled";

/// 位置标签（决定重新启用时写回哪个 hive）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum StartupLoc {
    HkcuRun,
    HklmRun,
    Folder,
}

impl StartupLoc {
    pub fn label(self) -> &'static str {
        match self {
            StartupLoc::HkcuRun => "HKCU-Run",
            StartupLoc::HklmRun => "HKLM-Run",
            StartupLoc::Folder => "启动文件夹",
        }
    }

    /// 由标签解析（用于处理禁用键里的存量记录）。
    ///
    /// 认不出的标签返回 None——**不能默认成 HKCU**：把 HKLM 的项写回 HKCU 会
    /// 悄悄改变它的生效范围（所有用户 vs 当前用户）。
    pub fn from_label(label: &str) -> Option<Self> {
        match label {
            "HKCU-Run" => Some(StartupLoc::HkcuRun),
            "HKLM-Run" => Some(StartupLoc::HklmRun),
            "启动文件夹" => Some(StartupLoc::Folder),
            _ => None,
        }
    }
}

/// 一个启动项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StartupItem {
    pub location: String,
    pub name: String,
    pub command: String,
    pub enabled: bool,
}

/// 启动文件夹路径（`%APPDATA%\Microsoft\Windows\Start Menu\Programs\Startup`）。
pub fn user_startup_dir() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_default();
    PathBuf::from(base).join(r"Microsoft\Windows\Start Menu\Programs\Startup")
}

/// 禁用项在注册表里的存储值：`<位置>|<命令>`。
pub fn encode_disabled(loc: StartupLoc, command: &str) -> String {
    format!("{}|{}", loc.label(), command)
}

/// 解析禁用项的存储值 → (位置, 命令)。
///
/// **按第一个 `|` 切分**：命令里可能含 `|`（罕见但合法，比如 `cmd /c "a|b"`），
/// 用 `split` 取全部再拼会把命令截断。
pub fn decode_disabled(raw: &str) -> Option<(StartupLoc, String)> {
    let (label, command) = raw.split_once('|')?;
    let loc = StartupLoc::from_label(label.trim())?;
    if command.is_empty() {
        return None;
    }
    Some((loc, command.to_string()))
}

/// 判断启动文件夹里的文件名是否是"已禁用"形态。
pub fn is_disabled_filename(name: &str) -> bool {
    name.ends_with(DISABLED_SUFFIX)
}

/// 已禁用文件名 → 原文件名（只剥**一层**后缀）。
///
/// 只剥一层很重要：若用户手上有 `.disabled.disabled`（旧版连锁改名留下的），
/// 剥全部会变成原文件名的同时丢掉一层语义，再启用就会少一次改名。
pub fn strip_disabled_suffix(name: &str) -> Option<&str> {
    name.strip_suffix(DISABLED_SUFFIX)
}

/// 启动项是否需要管理员才能改（HKLM 与启动文件夹都在用户可写范围外）。
pub fn needs_admin(loc: StartupLoc) -> bool {
    // 启动文件夹在 %APPDATA% 下，用户可写 → 不需要管理员
    matches!(loc, StartupLoc::HklmRun)
}

/// 把一批"原始读数"整理成有序、去重的启动项列表。
///
/// 去重键是 `(位置, 名称)`：**不能只按名称**——同名项可以同时存在于 HKCU 与 HKLM
/// （各自生效），只按名称去重会让用户看到一条、实际系统里跑两条。
pub fn normalize_items(items: Vec<StartupItem>) -> Vec<StartupItem> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for it in items {
        if it.name.trim().is_empty() {
            continue;
        }
        if !seen.insert((it.location.clone(), it.name.clone())) {
            continue;
        }
        out.push(it);
    }
    out
}

/// 统计：启用/禁用条数。
pub fn startup_stats(items: &[StartupItem]) -> (usize, usize) {
    let on = items.iter().filter(|i| i.enabled).count();
    (on, items.len() - on)
}

/// 启动项的存取后端。
///
/// 抽出这层是为了让「禁用 → 枚举 → 启用」的往返能被**完整测试**而不碰真实注册表：
/// 用内存实现跑全流程，真机实现只做薄封装。禁用启动项是系统级改动，测试里绝不能
/// 真去动 `HKCU\...\Run`——那会把开发机的开机自启项搬走。
pub trait StartupBackend {
    /// 读某个 `Run` 键的全部 `名称 → 命令`。
    fn run_values(&self, loc: StartupLoc) -> Vec<(String, String)>;
    /// 向 `Run` 键写入一个值。
    fn set_run_value(&mut self, loc: StartupLoc, name: &str, command: &str) -> bool;
    /// 从 `Run` 键删除一个值。Ok(true)=删掉了，Ok(false)=本来就没有。
    fn delete_run_value(&mut self, loc: StartupLoc, name: &str) -> Result<bool, String>;
    /// 读禁用键的全部 `名称 → "<位置>|<命令>"`。
    fn disabled_values(&self) -> Vec<(String, String)>;
    /// 写禁用键。
    fn set_disabled(&mut self, name: &str, encoded: &str) -> bool;
    /// 删除禁用键中的一项。
    fn delete_disabled(&mut self, name: &str) -> Result<bool, String>;
    /// 列出启动文件夹里的文件名。
    fn folder_files(&self) -> Vec<String>;
    /// 重命名启动文件夹里的文件。
    fn rename_folder_file(&mut self, from: &str, to: &str) -> Result<(), String>;
}

/// 枚举全部启动项（注册表三处 + 启动文件夹）。
pub fn enumerate(be: &dyn StartupBackend) -> Vec<StartupItem> {
    let mut items = Vec::new();

    // HKCU / HKLM 的 Run 键
    for loc in [StartupLoc::HkcuRun, StartupLoc::HklmRun] {
        for (name, command) in be.run_values(loc) {
            items.push(StartupItem {
                location: loc.label().to_string(),
                name,
                command,
                enabled: true,
            });
        }
    }

    // 启动文件夹
    for f in be.folder_files() {
        if let Some(orig) = strip_disabled_suffix(&f) {
            // 已禁用项：以**原名** + 已禁用态列出。否则会显示成「已启用」且永远
            // 无法从 UI 恢复（启用按钮只在禁用态生效），用户再点一次禁用还会
            // 连锁改名成 .disabled.disabled。
            items.push(StartupItem {
                location: StartupLoc::Folder.label().to_string(),
                name: orig.to_string(),
                command: user_startup_dir().join(&f).to_string_lossy().to_string(),
                enabled: false,
            });
        } else {
            items.push(StartupItem {
                location: StartupLoc::Folder.label().to_string(),
                name: f.clone(),
                command: user_startup_dir().join(&f).to_string_lossy().to_string(),
                enabled: true,
            });
        }
    }

    // 禁用键里的存量记录
    for (name, raw) in be.disabled_values() {
        if let Some((loc, command)) = decode_disabled(&raw) {
            items.push(StartupItem {
                location: loc.label().to_string(),
                name,
                command,
                enabled: false,
            });
        }
    }

    normalize_items(items)
}

/// 禁用一个启动项。返回 Err 表示失败（调用方据此提示用户）。
pub fn disable_one(be: &mut dyn StartupBackend, loc: StartupLoc, name: &str) -> Result<(), String> {
    match loc {
        StartupLoc::Folder => {
            let from = name.to_string();
            let to = format!("{name}{DISABLED_SUFFIX}");
            be.rename_folder_file(&from, &to)
        }
        _ => {
            // 顺序很重要：**先搬进禁用键，再删原值**。反过来的话中途失败就会
            // 两头都没有——用户的启动项直接消失（既不自启也无法从界面恢复）。
            let command = be
                .run_values(loc)
                .into_iter()
                .find(|(n, _)| n == name)
                .map(|(_, c)| c)
                .ok_or_else(|| format!("{name} 不在 {} 里（可能已被外部删除）", loc.label()))?;
            if !be.set_disabled(name, &encode_disabled(loc, &command)) {
                return Err(format!(
                    "写入禁用记录失败（{name}）：需要管理员权限或注册表被策略锁定"
                ));
            }
            match be.delete_run_value(loc, name) {
                Ok(true) => Ok(()),
                Ok(false) => Err(format!("{name} 已不在 {} 里", loc.label())),
                Err(e) => {
                    // 原值删除失败：回滚禁用记录，避免出现"禁用键里有、Run 里也有"
                    // 的重复状态（枚举时会出现两条同名项，用户不知道该点哪条）
                    let _ = be.delete_disabled(name);
                    Err(format!("删除启动项失败（已回滚）：{e}"))
                }
            }
        }
    }
}

/// 启用一个启动项。
pub fn enable_one(be: &mut dyn StartupBackend, loc: StartupLoc, name: &str) -> Result<(), String> {
    match loc {
        StartupLoc::Folder => {
            let from = format!("{name}{DISABLED_SUFFIX}");
            let to = name.to_string();
            be.rename_folder_file(&from, &to)
        }
        _ => {
            let encoded = be
                .disabled_values()
                .into_iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v)
                .ok_or_else(|| format!("找不到 {name} 的禁用记录"))?;
            let (stored_loc, command) = decode_disabled(&encoded)
                .ok_or_else(|| format!("{name} 的禁用记录格式非法，无法恢复"))?;
            if stored_loc != loc {
                return Err(format!(
                    "{name} 的禁用记录属于 {} 而不是 {}，拒绝写回（会造成作用范围改变）",
                    stored_loc.label(),
                    loc.label()
                ));
            }
            if !be.set_run_value(loc, name, &command) {
                return Err(format!("写回启动项失败（{name}）：需要管理员权限"));
            }
            if let Err(e) = be.delete_disabled(name) {
                // 已经写回 Run 了；禁用记录留着只会在枚举时造成重复项
                return Err(format!("已恢复启动项，但清理禁用记录失败：{e}"));
            }
            Ok(())
        }
    }
}

/// 真实后端：HKCU/HKLM 注册表 + 启动文件夹。
pub struct WinBackend;

fn run_path(loc: StartupLoc) -> &'static str {
    match loc {
        StartupLoc::HklmRun => r"HKLM\Software\Microsoft\Windows\CurrentVersion\Run",
        _ => r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
    }
}

fn disabled_path() -> &'static str {
    r"HKCU\Software\NetTool\DisabledRun"
}

impl StartupBackend for WinBackend {
    fn run_values(&self, loc: StartupLoc) -> Vec<(String, String)> {
        crate::w32::reg_values(run_path(loc))
            .into_iter()
            .map(|(k, v)| (k, v.as_compare_str()))
            .collect()
    }

    fn set_run_value(&mut self, loc: StartupLoc, name: &str, command: &str) -> bool {
        crate::w32::reg_set_string(run_path(loc), name, command)
    }

    fn delete_run_value(&mut self, loc: StartupLoc, name: &str) -> Result<bool, String> {
        crate::w32::reg_delete_value(run_path(loc), name)
    }

    fn disabled_values(&self) -> Vec<(String, String)> {
        crate::w32::reg_values(disabled_path())
            .into_iter()
            .map(|(k, v)| (k, v.as_compare_str()))
            .collect()
    }

    fn set_disabled(&mut self, name: &str, encoded: &str) -> bool {
        crate::w32::reg_set_string(disabled_path(), name, encoded)
    }

    fn delete_disabled(&mut self, name: &str) -> Result<bool, String> {
        crate::w32::reg_delete_value(disabled_path(), name)
    }

    fn folder_files(&self) -> Vec<String> {
        let dir = user_startup_dir();
        let Ok(rd) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        rd.flatten()
            .filter(|e| e.path().is_file())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect()
    }

    fn rename_folder_file(&mut self, from: &str, to: &str) -> Result<(), String> {
        let dir = user_startup_dir();
        let src = dir.join(from);
        let dst = dir.join(to);
        if !src.exists() {
            return Err(format!("找不到启动文件夹中的 {from}"));
        }
        if dst.exists() {
            return Err(format!("目标 {to} 已存在，拒绝覆盖"));
        }
        std::fs::rename(&src, &dst).map_err(|e| format!("重命名失败（{from} → {to}）：{e}"))
    }
}

/// 枚举真实系统的启动项。
pub fn enumerate_real() -> Vec<StartupItem> {
    enumerate(&WinBackend)
}

/// 禁用/启用真实系统的启动项。
pub fn set_enabled_real(loc_label: &str, name: &str, enable: bool) -> Result<(), String> {
    let loc = StartupLoc::from_label(loc_label)
        .ok_or_else(|| format!("未知的启动项位置: {loc_label}"))?;
    // HKLM 与启动文件夹的写入需要管理员；HKCU 不需要
    if needs_admin(loc) && !crate::w32::is_elevated() {
        return Err(format!(
            "{name} 位于 {}，需要管理员权限：请以管理员身份重启 SysKit 后重试",
            loc.label()
        ));
    }
    let mut be = WinBackend;
    if enable {
        enable_one(&mut be, loc, name)
    } else {
        disable_one(&mut be, loc, name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(loc: &str, name: &str, cmd: &str, enabled: bool) -> StartupItem {
        StartupItem {
            location: loc.into(),
            name: name.into(),
            command: cmd.into(),
            enabled,
        }
    }

    #[test]
    fn disabled_key_must_keep_legacy_product_name() {
        // 这是跨版本共享标识：改成 SysKit 会让 Python 版已禁用的项在 Rust 版里消失
        assert_eq!(DISABLED_KEY, r"Software\NetTool\DisabledRun");
        assert!(
            DISABLED_KEY.contains("NetTool"),
            "必须沿用旧产品名，否则用户存量禁用项凭空复活"
        );
        assert_eq!(RUN_KEY, r"Software\Microsoft\Windows\CurrentVersion\Run");
        assert_eq!(DISABLED_SUFFIX, ".disabled");
    }

    #[test]
    fn loc_labels_match_python() {
        assert_eq!(StartupLoc::HkcuRun.label(), "HKCU-Run");
        assert_eq!(StartupLoc::HklmRun.label(), "HKLM-Run");
        assert_eq!(StartupLoc::Folder.label(), "启动文件夹");
        // 往返
        for l in [StartupLoc::HkcuRun, StartupLoc::HklmRun, StartupLoc::Folder] {
            assert_eq!(StartupLoc::from_label(l.label()), Some(l));
        }
    }

    #[test]
    fn unknown_label_is_rejected_not_defaulted() {
        // 默认成 HKCU 会悄悄把 HKLM 项的作用范围改成"仅当前用户"
        assert_eq!(StartupLoc::from_label("HKCU"), None);
        assert_eq!(StartupLoc::from_label(""), None);
        assert_eq!(StartupLoc::from_label("hklm-run"), None, "标签大小写敏感");
    }

    #[test]
    fn encode_decode_round_trip() {
        for (loc, cmd) in [
            (StartupLoc::HkcuRun, r"C:\a.exe"),
            (StartupLoc::HklmRun, r"C:\Program Files\B\b.exe --flag"),
            (StartupLoc::Folder, r"C:\Users\x\AppData\Roaming\...\a.lnk"),
        ] {
            let enc = encode_disabled(loc, cmd);
            let (l, c) = decode_disabled(&enc).unwrap();
            assert_eq!(l, loc);
            assert_eq!(c, cmd);
        }
    }

    #[test]
    fn decode_splits_on_first_pipe_only() {
        // 命令里含 | 时不能被截断
        let enc = r#"HKCU-Run|cmd /c "echo a|b""#;
        let (loc, cmd) = decode_disabled(enc).unwrap();
        assert_eq!(loc, StartupLoc::HkcuRun);
        assert_eq!(cmd, r#"cmd /c "echo a|b""#);
    }

    #[test]
    fn decode_rejects_malformed() {
        assert_eq!(decode_disabled(""), None, "无分隔符");
        assert_eq!(decode_disabled("HKCU-Run"), None);
        assert_eq!(decode_disabled("HKCU-Run|"), None, "命令为空");
        assert_eq!(decode_disabled("未知位置|c:\\a.exe"), None, "未知标签");
    }

    #[test]
    fn disabled_filename_detection_and_strip() {
        assert!(is_disabled_filename("a.lnk.disabled"));
        assert!(!is_disabled_filename("a.lnk"));
        assert_eq!(strip_disabled_suffix("a.lnk.disabled"), Some("a.lnk"));
        assert_eq!(strip_disabled_suffix("a.lnk"), None);
    }

    #[test]
    fn strip_removes_exactly_one_suffix() {
        // 旧版连锁改名留下的 .disabled.disabled：只剥一层，
        // 剥全部会让"启用"跳过一次改名而丢语义
        assert_eq!(
            strip_disabled_suffix("a.lnk.disabled.disabled"),
            Some("a.lnk.disabled")
        );
    }

    #[test]
    fn needs_admin_only_for_hklm() {
        assert!(needs_admin(StartupLoc::HklmRun));
        assert!(!needs_admin(StartupLoc::HkcuRun), "HKCU 用户可写");
        assert!(
            !needs_admin(StartupLoc::Folder),
            "启动文件夹在 %APPDATA% 下"
        );
    }

    #[test]
    fn normalize_dedupes_by_location_and_name() {
        let items = vec![
            item("HKCU-Run", "App", r"C:\a.exe", true),
            item("HKCU-Run", "App", r"C:\a.exe", true), // 完全重复
            item("HKLM-Run", "App", r"C:\b.exe", true), // 同名不同位置 → 必须保留
        ];
        let out = normalize_items(items);
        assert_eq!(out.len(), 2, "同名跨位置不能去重掉");
        assert_eq!(out[0].location, "HKCU-Run");
        assert_eq!(out[1].location, "HKLM-Run");
    }

    #[test]
    fn normalize_drops_empty_names() {
        let items = vec![
            item("HKCU-Run", "", r"C:\a.exe", true),
            item("HKCU-Run", "   ", r"C:\a.exe", true),
            item("HKCU-Run", "Real", r"C:\a.exe", true),
        ];
        let out = normalize_items(items);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "Real");
    }

    #[test]
    fn startup_stats_counts() {
        let items = vec![
            item("HKCU-Run", "a", "", true),
            item("HKCU-Run", "b", "", false),
            item("Folder", "c.lnk", "", false),
        ];
        assert_eq!(startup_stats(&items), (1, 2));
        assert_eq!(startup_stats(&[]), (0, 0));
    }

    #[test]
    fn user_startup_dir_is_under_appdata() {
        let d = user_startup_dir();
        let s = d.to_string_lossy().to_string();
        assert!(s.ends_with(r"Startup"), "{s}");
        assert!(s.contains("Start Menu"), "{s}");
    }

    #[test]
    fn serialize_shape() {
        let j = serde_json::to_string(&item("HKCU-Run", "App", "cmd", true)).unwrap();
        for k in ["location", "name", "command", "enabled"] {
            assert!(j.contains(k), "序列化必须含 {k}: {j}");
        }
    }

    // ---------------- 后端往返（内存实现，绝不碰真实注册表） ----------------

    /// 内存后端：模拟两个 Run 键 + 禁用键 + 启动文件夹。
    #[derive(Default)]
    struct FakeBackend {
        hkcu: Vec<(String, String)>,
        hklm: Vec<(String, String)>,
        disabled: Vec<(String, String)>,
        folder: Vec<String>,
        /// 设为 true 时所有写入失败（模拟非管理员 / 策略锁定）
        deny_writes: bool,
        /// 设为 true 时 delete_run_value 返回 Err（模拟权限拒绝）
        deny_delete: bool,
    }

    impl FakeBackend {
        fn with_defaults() -> Self {
            Self {
                hkcu: vec![("OneDrive".into(), r"C:\one.exe".into())],
                hklm: vec![("SecPol".into(), r"C:\sec.exe".into())],
                folder: vec!["shortcut.lnk".into()],
                ..Default::default()
            }
        }
        fn run(&self, loc: StartupLoc) -> &Vec<(String, String)> {
            match loc {
                StartupLoc::HklmRun => &self.hklm,
                _ => &self.hkcu,
            }
        }
        fn run_mut(&mut self, loc: StartupLoc) -> &mut Vec<(String, String)> {
            match loc {
                StartupLoc::HklmRun => &mut self.hklm,
                _ => &mut self.hkcu,
            }
        }
    }

    impl StartupBackend for FakeBackend {
        fn run_values(&self, loc: StartupLoc) -> Vec<(String, String)> {
            self.run(loc).clone()
        }
        fn set_run_value(&mut self, _loc: StartupLoc, _n: &str, _c: &str) -> bool {
            !self.deny_writes
        }
        fn delete_run_value(&mut self, _loc: StartupLoc, _n: &str) -> Result<bool, String> {
            if self.deny_delete {
                return Err("拒绝访问".into());
            }
            Ok(true)
        }
        fn disabled_values(&self) -> Vec<(String, String)> {
            self.disabled.clone()
        }
        fn set_disabled(&mut self, _n: &str, _e: &str) -> bool {
            !self.deny_writes
        }
        fn delete_disabled(&mut self, _n: &str) -> Result<bool, String> {
            Ok(true)
        }
        fn folder_files(&self) -> Vec<String> {
            self.folder.clone()
        }
        fn rename_folder_file(&mut self, _f: &str, _t: &str) -> Result<(), String> {
            if self.deny_writes {
                return Err("拒绝访问".into());
            }
            Ok(())
        }
    }

    /// 带真实可变状态的后端（用于跑通"禁用后枚举不到、启用后回来"的完整语义）。
    struct StatefulBackend {
        hkcu: Vec<(String, String)>,
        hklm: Vec<(String, String)>,
        disabled: Vec<(String, String)>,
        folder: Vec<String>,
    }

    impl StatefulBackend {
        fn new() -> Self {
            Self {
                hkcu: vec![("OneDrive".into(), r"C:\one.exe".into())],
                hklm: Vec::new(),
                disabled: Vec::new(),
                folder: vec!["sc.lnk".into()],
            }
        }
    }

    impl StartupBackend for StatefulBackend {
        fn run_values(&self, loc: StartupLoc) -> Vec<(String, String)> {
            match loc {
                StartupLoc::HklmRun => self.hklm.clone(),
                _ => self.hkcu.clone(),
            }
        }
        fn set_run_value(&mut self, loc: StartupLoc, name: &str, command: &str) -> bool {
            let v = match loc {
                StartupLoc::HklmRun => &mut self.hklm,
                _ => &mut self.hkcu,
            };
            v.retain(|(n, _)| n != name);
            v.push((name.to_string(), command.to_string()));
            true
        }
        fn delete_run_value(&mut self, loc: StartupLoc, name: &str) -> Result<bool, String> {
            let v = match loc {
                StartupLoc::HklmRun => &mut self.hklm,
                _ => &mut self.hkcu,
            };
            let before = v.len();
            v.retain(|(n, _)| n != name);
            Ok(v.len() != before)
        }
        fn disabled_values(&self) -> Vec<(String, String)> {
            self.disabled.clone()
        }
        fn set_disabled(&mut self, name: &str, encoded: &str) -> bool {
            self.disabled.retain(|(n, _)| n != name);
            self.disabled.push((name.to_string(), encoded.to_string()));
            true
        }
        fn delete_disabled(&mut self, name: &str) -> Result<bool, String> {
            let before = self.disabled.len();
            self.disabled.retain(|(n, _)| n != name);
            Ok(self.disabled.len() != before)
        }
        fn folder_files(&self) -> Vec<String> {
            self.folder.clone()
        }
        fn rename_folder_file(&mut self, from: &str, to: &str) -> Result<(), String> {
            let i = self
                .folder
                .iter()
                .position(|f| f == from)
                .ok_or_else(|| format!("找不到 {from}"))?;
            self.folder[i] = to.to_string();
            Ok(())
        }
    }

    #[test]
    fn enumerate_reads_all_three_sources() {
        let be = FakeBackend::with_defaults();
        let items = enumerate(&be);
        assert_eq!(items.len(), 3, "HKCU + HKLM + 启动文件夹");
        assert!(items
            .iter()
            .any(|i| i.location == "HKCU-Run" && i.name == "OneDrive"));
        assert!(items
            .iter()
            .any(|i| i.location == "HKLM-Run" && i.name == "SecPol"));
        assert!(items
            .iter()
            .any(|i| i.location == "启动文件夹" && i.name == "shortcut.lnk"));
        assert!(items.iter().all(|i| i.enabled), "初始都应是启用态");
    }

    #[test]
    fn enumerate_lists_disabled_folder_item_under_original_name() {
        // 关键：必须显示**原名** + 禁用态，否则启用按钮永远点不到（它只在禁用态生效）
        let be = FakeBackend {
            folder: vec!["sc.lnk.disabled".into()],
            ..Default::default()
        };
        let items = enumerate(&be);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "sc.lnk", "应剥掉 .disabled 后缀显示原名");
        assert!(!items[0].enabled);
    }

    #[test]
    fn enumerate_includes_stored_disabled_entries() {
        let be = FakeBackend {
            disabled: vec![("Removed".into(), r"HKCU-Run|C:\r.exe".into())],
            ..Default::default()
        };
        let items = enumerate(&be);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "Removed");
        assert_eq!(items[0].location, "HKCU-Run");
        assert_eq!(items[0].command, r"C:\r.exe");
        assert!(!items[0].enabled);
    }

    #[test]
    fn enumerate_skips_malformed_disabled_records() {
        // 坏记录不能被当成有效项（否则用户点启用会写回一个垃圾命令）
        let be = FakeBackend {
            disabled: vec![
                ("bad".into(), "没有分隔符".into()),
                ("worse".into(), "未知位置|c:\\x.exe".into()),
                ("ok".into(), r"HKCU-Run|C:\ok.exe".into()),
            ],
            ..Default::default()
        };
        let items = enumerate(&be);
        assert_eq!(items.len(), 1, "只有合法记录出现");
        assert_eq!(items[0].name, "ok");
    }

    #[test]
    fn disable_then_enable_round_trip_preserves_command() {
        // 完整往返：禁用后 Run 里没有、禁用键里有；启用后完全还原
        let mut be = StatefulBackend::new();
        let before = enumerate(&be);
        assert_eq!(before.len(), 2, "OneDrive + sc.lnk");

        disable_one(&mut be, StartupLoc::HkcuRun, "OneDrive").unwrap();
        assert!(be.hkcu.is_empty(), "Run 里的值必须被搬走");
        assert_eq!(be.disabled.len(), 1);
        assert_eq!(be.disabled[0].1, r"HKCU-Run|C:\one.exe");

        let after_disable = enumerate(&be);
        let od = after_disable.iter().find(|i| i.name == "OneDrive").unwrap();
        assert!(!od.enabled, "禁用后应显示为已禁用");
        assert_eq!(od.command, r"C:\one.exe", "命令必须原样保留");

        enable_one(&mut be, StartupLoc::HkcuRun, "OneDrive").unwrap();
        assert_eq!(be.hkcu.len(), 1, "值必须回到 Run");
        assert_eq!(be.hkcu[0].1, r"C:\one.exe", "命令逐字节还原");
        assert!(
            be.disabled.is_empty(),
            "禁用记录必须清理掉（否则枚举出两条同名项）"
        );

        let restored = enumerate(&be);
        assert_eq!(restored.len(), 2, "不能多出重复项");
        assert!(restored.iter().all(|i| i.enabled));
    }

    #[test]
    fn disable_folder_item_renames_with_suffix() {
        let mut be = StatefulBackend::new();
        disable_one(&mut be, StartupLoc::Folder, "sc.lnk").unwrap();
        assert_eq!(be.folder, vec!["sc.lnk.disabled"]);
        let items = enumerate(&be);
        // 按名字取而不是按下标：列表顺序不是本用例要断言的东西
        let sc = items
            .iter()
            .find(|i| i.name == "sc.lnk")
            .expect("应有 sc.lnk");
        assert!(!sc.enabled, "禁用后应显示为已禁用");

        enable_one(&mut be, StartupLoc::Folder, "sc.lnk").unwrap();
        assert_eq!(be.folder, vec!["sc.lnk"], "只剥一层后缀");
        let after = enumerate(&be);
        assert!(
            after
                .iter()
                .find(|i| i.name == "sc.lnk")
                .expect("应有 sc.lnk")
                .enabled,
            "启用后应恢复为启用态"
        );
    }

    #[test]
    fn disable_missing_entry_fails_without_touching_disabled_key() {
        let mut be = StatefulBackend::new();
        let err = disable_one(&mut be, StartupLoc::HkcuRun, "不存在").unwrap_err();
        assert!(err.contains("不在"), "{err}");
        assert!(be.disabled.is_empty(), "失败时不得留下禁用记录");
        assert_eq!(be.hkcu.len(), 1, "原 Run 值不得被动");
    }

    #[test]
    fn disable_reports_write_failure_without_deleting() {
        // 写禁用记录失败时必须**保留原启动项**：删了就没法恢复了
        let mut be = FakeBackend {
            deny_writes: true,
            ..FakeBackend::with_defaults()
        };
        let err = disable_one(&mut be, StartupLoc::HkcuRun, "OneDrive").unwrap_err();
        assert!(err.contains("写入禁用记录失败"), "{err}");
    }

    #[test]
    fn disable_rolls_back_when_delete_fails() {
        // 删除原值失败 → 必须回滚禁用记录，否则会出现"禁用键有 + Run 也有"的重复状态
        struct RollbackBackend {
            inner: StatefulBackend,
            rolled_back: std::cell::Cell<bool>,
        }
        impl StartupBackend for RollbackBackend {
            fn run_values(&self, l: StartupLoc) -> Vec<(String, String)> {
                self.inner.run_values(l)
            }
            fn set_run_value(&mut self, l: StartupLoc, n: &str, c: &str) -> bool {
                self.inner.set_run_value(l, n, c)
            }
            fn delete_run_value(&mut self, _l: StartupLoc, _n: &str) -> Result<bool, String> {
                Err("拒绝访问".into())
            }
            fn disabled_values(&self) -> Vec<(String, String)> {
                self.inner.disabled_values()
            }
            fn set_disabled(&mut self, n: &str, e: &str) -> bool {
                self.inner.set_disabled(n, e)
            }
            fn delete_disabled(&mut self, n: &str) -> Result<bool, String> {
                self.rolled_back.set(true);
                self.inner.delete_disabled(n)
            }
            fn folder_files(&self) -> Vec<String> {
                self.inner.folder_files()
            }
            fn rename_folder_file(&mut self, f: &str, t: &str) -> Result<(), String> {
                self.inner.rename_folder_file(f, t)
            }
        }
        let mut be = RollbackBackend {
            inner: StatefulBackend::new(),
            rolled_back: std::cell::Cell::new(false),
        };
        let err = disable_one(&mut be, StartupLoc::HkcuRun, "OneDrive").unwrap_err();
        assert!(err.contains("回滚"), "{err}");
        assert!(be.rolled_back.get(), "必须调用回滚");
        assert!(be.inner.disabled.is_empty(), "禁用记录必须被清掉");
    }

    #[test]
    fn enable_without_disabled_record_fails() {
        let mut be = StatefulBackend::new();
        let err = enable_one(&mut be, StartupLoc::HkcuRun, "没有记录").unwrap_err();
        assert!(err.contains("找不到"), "{err}");
    }

    #[test]
    fn enable_rejects_location_mismatch() {
        // 禁用记录属于 HKLM，却按 HKCU 启用 → 必须拒绝（会改变作用范围）
        let mut be = StatefulBackend::new();
        be.disabled.clear(); // 只留这一条，避免同名项干扰定位
        be.disabled.push(("X".into(), r"HKLM-Run|C:\x.exe".into()));
        // 先确认这一层的解析是对的（否则下面的断言在测错东西）
        assert_eq!(
            decode_disabled(r"HKLM-Run|C:\x.exe"),
            Some((StartupLoc::HklmRun, r"C:\x.exe".to_string())),
            "存储值本身应能解出 HKLM-Run"
        );
        let err = enable_one(&mut be, StartupLoc::HkcuRun, "X").unwrap_err();
        assert!(err.contains("拒绝写回"), "{err}");
        // 断言"X 没被写进去"，而不是"Run 键为空"——后者会被 new() 里预置的
        // OneDrive 干扰，测的就不是本用例要测的东西了
        assert!(
            !be.hkcu.iter().any(|(n, _)| n == "X"),
            "被拒绝的项不得写进 Run: {:?}",
            be.hkcu
        );
        assert_eq!(be.disabled.len(), 1, "禁用记录不得被清理");
    }

    #[test]
    fn enable_write_failure_keeps_disabled_record() {
        let mut be = FakeBackend {
            disabled: vec![("X".into(), r"HKCU-Run|C:\x.exe".into())],
            deny_writes: true,
            ..Default::default()
        };
        let err = enable_one(&mut be, StartupLoc::HkcuRun, "X").unwrap_err();
        assert!(err.contains("写回启动项失败"), "{err}");
    }

    #[test]
    fn enable_rejects_malformed_disabled_record() {
        let mut be = FakeBackend {
            disabled: vec![("X".into(), "垃圾数据".into())],
            ..Default::default()
        };
        let err = enable_one(&mut be, StartupLoc::HkcuRun, "X").unwrap_err();
        assert!(err.contains("格式非法"), "{err}");
    }
}
