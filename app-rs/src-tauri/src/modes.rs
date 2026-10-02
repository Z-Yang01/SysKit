//! 场景模式：办公（勿扰/静音/开常用软件/备份配置）与游戏（关通知/高性能/暂停更新/清后台）。
//! 对应 Python 侧 `modes.py`。
//!
//! 三条原则（与 Python 相同，都是踩过坑总结出来的）：
//! 1. **每一步返回 `(名称, 是否成功, 说明)`，单步失败不中断后续**——用户要的是
//!    "做了哪些、哪些没成"的完整清单，不是第一个错误就停；
//! 2. 改系统配置尽量走注册表/`powercfg`，便于测试与审计；
//! 3. **清后台只处理白名单应用**，绝不碰 explorer/系统关键进程。
//!
//! 两个诚实性细节：
//! - `NoAutoUpdate` 策略是**无限期**的，不是"暂停 N 天"。文案不得暗示到期自动恢复
//!   （Python 注释里专门写了这条）；
//! - 取消静音要**恢复进模式前的音量**，而不是想当然置成最大——旧行为会把用户 30%
//!   的音量变成 100%。

use serde::Serialize;

/// 通知（全局 Toast）开关的注册表位置——`Focus Assist` 的轻量等价物。
pub const NOTIFY_SETTINGS: &str =
    r"HKCU\Software\Microsoft\Windows\CurrentVersion\Notifications\Settings";
pub const NOTIFY_GLOBAL_VALUE: &str = "NOC_GLOBAL_SETTING_TOASTS_ENABLED";

/// Windows Update 暂停策略（**需管理员**）。
pub const WU_POLICY: &str = r"HKLM\SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate\AU";

/// 电源方案别名（`powercfg` 内置）。
pub const POWER_HIGH_PERF: &str = "SCHEME_MIN";
pub const POWER_BALANCED: &str = "SCHEME_BALANCED";

/// 常用软件（办公模式一键打开）。
pub const COMMON_APPS: [(&str, &str); 7] = [
    ("记事本", "notepad.exe"),
    ("计算器", "calc.exe"),
    ("资源管理器", "explorer.exe"),
    ("画图", "mspaint.exe"),
    ("任务管理器", "taskmgr.exe"),
    ("系统设置", "ms-settings:"),
    ("终端", "powershell.exe"),
];

/// 办公模式默认打开的四个。
pub const OFFICE_DEFAULT_APPS: [&str; 4] = ["记事本", "计算器", "资源管理器", "画图"];

/// 可安全结束后台（游戏模式）——**白名单，不含系统/浏览器主进程**。
pub const BACKGROUND_CLOSE_WHITELIST: [&str; 16] = [
    "OneDrive.exe",
    "Spotify.exe",
    "Discord.exe",
    "Slack.exe",
    "Teams.exe",
    "ms-teams.exe",
    "Steam.exe",
    "EpicGamesLauncher.exe",
    "Battle.net.exe",
    "RiotClientServices.exe",
    "CloudMusic.exe",
    "kugou.exe",
    "KwMusic.exe",
    "iCloudServices.exe",
    "AdobeDesktopService.exe",
    "AdobeUpdateService.exe",
];

/// 模式执行的单步结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModeStep {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

impl ModeStep {
    pub fn new(name: &str, ok: bool, detail: &str) -> Self {
        Self {
            name: name.to_string(),
            ok,
            detail: detail.to_string(),
        }
    }

    pub fn ok(name: &str, detail: &str) -> Self {
        Self::new(name, true, detail)
    }

    pub fn fail(name: &str, detail: &str) -> Self {
        Self::new(name, false, detail)
    }
}

/// 一批步骤的汇总。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModeOutcome {
    pub mode: String,
    pub steps: Vec<ModeStep>,
    pub ok_count: usize,
    pub fail_count: usize,
}

impl ModeOutcome {
    pub fn new(mode: &str, steps: Vec<ModeStep>) -> Self {
        let ok_count = steps.iter().filter(|s| s.ok).count();
        let fail_count = steps.len() - ok_count;
        Self {
            mode: mode.to_string(),
            steps,
            ok_count,
            fail_count,
        }
    }

    /// 汇总文案（**必须如实报出失败步数**，否则用户以为全部成功）。
    pub fn summary(&self) -> String {
        if self.fail_count == 0 {
            format!("{}：{} 步全部完成", self.mode, self.steps.len())
        } else {
            let failed: Vec<&str> = self
                .steps
                .iter()
                .filter(|s| !s.ok)
                .map(|s| s.name.as_str())
                .collect();
            format!(
                "{}：{}/{} 步成功；失败：{}",
                self.mode,
                self.ok_count,
                self.steps.len(),
                failed.join("、")
            )
        }
    }
}

/// 该进程是否在白名单里（大小写不敏感）。
pub fn in_close_whitelist(name: &str) -> bool {
    let n = name.to_lowercase();
    BACKGROUND_CLOSE_WHITELIST
        .iter()
        .any(|w| w.to_lowercase() == n)
}

/// 解析"目标音量"的设置值。
///
/// 必须同时约束**类型与范围**（Python 注释里列了三种脏数据）：
/// - `0`（本来就是静音态）——恢复成 0 等于没取消静音；
/// - `bool`（JSON 手改成 `true`）——`true` 在多数语言里是 `1`，会被当成音量 1；
/// - 非整数/越界——旧版或手改写坏的记录。
///
/// 任一不满足都视为"没有可用记录"，返回 `None`，由调用方按"设为最大并如实说明"处理。
pub fn usable_prev_volume(prev: Option<i64>) -> Option<u32> {
    match prev {
        Some(v) if v > 0 && v <= 0xFFFF_FFFF => Some(v as u32),
        _ => None,
    }
}

/// 取消静音时要写入的音量 + 说明文案（一并返回，避免两处判断不一致）。
pub fn restore_volume_plan(prev: Option<i64>) -> (u32, &'static str) {
    match usable_prev_volume(prev) {
        Some(v) => (v, "已恢复进模式前的音量"),
        // 没有可用记录时设为最大——但必须**说明**，不能让用户以为恢复成了原音量
        None => (
            0xFFFF_FFFF,
            "已取消静音（未找到进模式前的音量，已设为最大）",
        ),
    }
}

/// 暂停更新的说明文案。
///
/// `NoAutoUpdate` 是**无限期策略**，不是"暂停 N 天"——文案绝不能暗示到期自动恢复，
/// 否则用户会以为 7 天后自动恢复更新而实际上一直关着。
pub fn update_pause_detail(paused: bool) -> &'static str {
    if paused {
        "已关闭自动更新（策略持久生效，需退出游戏模式或手动恢复）"
    } else {
        "已恢复自动更新"
    }
}

// ---------------- 实际动作 ----------------

/// 开关全局通知（HKCU，无需管理员）。
pub fn set_notifications(enabled: bool) -> ModeStep {
    if crate::w32::reg_set_dword(NOTIFY_SETTINGS, NOTIFY_GLOBAL_VALUE, u32::from(enabled)) {
        ModeStep::ok(
            "通知/勿扰",
            if enabled {
                "已开启"
            } else {
                "已关闭（勿扰）"
            },
        )
    } else {
        ModeStep::fail(
            "通知/勿扰",
            "写注册表失败（HKCU 通常无需管理员，可能是策略锁定）",
        )
    }
}

/// 切换电源方案。
pub fn set_power_plan(high_perf: bool) -> ModeStep {
    let alias = if high_perf {
        POWER_HIGH_PERF
    } else {
        POWER_BALANCED
    };
    let r = crate::w32::run_capture(&["powercfg".into(), "/setactive".into(), alias.into()], 10);
    match r {
        Ok(o) if o.ok => ModeStep::ok("电源计划", if high_perf { "高性能" } else { "平衡" }),
        Ok(o) => ModeStep::fail("电源计划", o.tail(0, 120).trim()),
        Err(e) => ModeStep::fail("电源计划", &format!("powercfg 无法执行: {e}")),
    }
}

/// 暂停/恢复 Windows 自动更新（需管理员）。
pub fn pause_windows_update(pause: bool) -> ModeStep {
    if !crate::w32::is_elevated() {
        return ModeStep::fail("暂停 Windows 更新", "需要管理员权限");
    }
    if crate::w32::reg_set_dword(WU_POLICY, "NoAutoUpdate", u32::from(pause)) {
        ModeStep::ok("暂停 Windows 更新", update_pause_detail(pause))
    } else {
        ModeStep::fail("暂停 Windows 更新", "写 HKLM 策略失败")
    }
}

/// 打开一个常用软件（`ShellExecute` 走系统关联，支持 `ms-settings:` 这类协议）。
pub fn open_app(name: &str) -> ModeStep {
    let Some(target) = COMMON_APPS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, t)| *t)
    else {
        return ModeStep::fail(&format!("打开 {name}"), "未知应用");
    };
    match crate::w32::shell_open(target) {
        Ok(()) => ModeStep::ok(&format!("打开 {name}"), target),
        Err(e) => ModeStep::fail(&format!("打开 {name}"), &e),
    }
}

/// 备份配置到 `backup/config_<时间戳>/`。
///
/// **无文件也要落一份默认设置**，保证备份非空——一个空目录对用户毫无意义，
/// 而且之后"备份成功"的提示会让人以为有东西可恢复。
pub fn backup_config() -> ModeStep {
    let stamp = crate::timefmt::now_fmt("%Y%m%d_%H%M%S");
    let dest = crate::paths::backup_dir().join(format!("config_{stamp}"));
    if let Err(e) = std::fs::create_dir_all(&dest) {
        return ModeStep::fail("备份配置", &format!("创建备份目录失败: {e}"));
    }
    let mut copied = 0usize;
    for name in [
        "settings.json",
        "traffic.json",
        "events.json",
        "clipboard.json",
        "netstats_buckets.json",
        "appnet.json",
    ] {
        let src = crate::paths::appdata_dir().join(name);
        if src.is_file() {
            let dst = dest.join(name);
            if std::fs::copy(&src, &dst).is_ok() {
                copied += 1;
            }
        }
    }
    if copied == 0 {
        // 空备份没意义：写一份当前设置快照
        let snapshot = crate::settings::snapshot_json();
        let dst = dest.join("settings.json");
        let text = serde_json::to_string_pretty(&snapshot).unwrap_or_else(|_| "{}".to_string());
        if std::fs::write(&dst, text.as_bytes()).is_err() {
            return ModeStep::fail("备份配置", "写入默认设置失败");
        }
        copied = 1;
    }
    ModeStep::ok(
        "备份配置",
        &format!("{copied} 个文件 → {}", dest.to_string_lossy()),
    )
}

/// 列出当前在跑、且在白名单里的进程。
pub fn background_candidates() -> Vec<(u32, String)> {
    let out = crate::w32::run_capture(
        &["tasklist".into(), "/fo".into(), "csv".into(), "/nh".into()],
        30,
    );
    let Ok(out) = out else {
        return Vec::new();
    };
    crate::portscan::parse_tasklist_mem(&out.stdout)
        .into_iter()
        .filter(|p| in_close_whitelist(&p.name))
        .map(|p| (p.pid, p.name))
        .collect()
}

/// 结束白名单进程。
///
/// **击杀前复验进程名**（PID 复用防护）：从"列出候选"到"结束它"之间进程可能已退出、
/// 其 PID 被系统分配给另一个无关进程；不复验就可能误杀。Python 版专门写了这条。
pub fn close_background() -> ModeStep {
    let targets = background_candidates();
    if targets.is_empty() {
        return ModeStep::ok("清理后台", "无需清理（白名单内无运行中的应用）");
    }
    let mut killed = Vec::new();
    let mut failed = Vec::new();
    for (pid, name) in targets {
        // 复验：当前该 PID 的进程名是否仍是白名单里的那个
        let current = current_process_name(pid);
        match current {
            Some(cur) if in_close_whitelist(&cur) && cur.eq_ignore_ascii_case(&name) => {
                let r = crate::w32::run_capture(
                    &[
                        "taskkill".into(),
                        "/PID".into(),
                        pid.to_string(),
                        "/F".into(),
                    ],
                    15,
                );
                if r.map(|o| o.ok).unwrap_or(false) {
                    killed.push(name);
                } else {
                    failed.push(name);
                }
            }
            Some(cur) => {
                // PID 已被复用成别的进程 → 绝不误杀
                failed.push(format!("{name}(PID {pid} 已复用为 {cur}，跳过)"));
            }
            None => failed.push(format!("{name}(PID {pid} 已退出)")),
        }
    }
    let mut detail = format!("已结束 {} 个", killed.len());
    if !killed.is_empty() {
        detail.push('：');
        detail.push_str(
            &killed
                .iter()
                .take(8)
                .cloned()
                .collect::<Vec<_>>()
                .join("、"),
        );
    }
    if !failed.is_empty() {
        detail.push_str(&format!("；失败/跳过 {}", failed.len()));
    }
    ModeStep::new("清理后台", !killed.is_empty(), &detail)
}

/// 查某个 PID 当前的进程名（不存在返回 None）。
pub fn current_process_name(pid: u32) -> Option<String> {
    let out = crate::w32::run_capture(
        &[
            "tasklist".into(),
            "/FI".into(),
            format!("PID eq {pid}"),
            "/FO".into(),
            "CSV".into(),
            "/NH".into(),
        ],
        15,
    )
    .ok()?;
    crate::portscan::parse_tasklist_mem(&out.stdout)
        .into_iter()
        .find(|p| p.pid == pid)
        .map(|p| p.name)
}

// ---------------- 组合入口 ----------------

/// 办公模式：勿扰 → 静音 → （可选）开常用软件 → 备份配置。
pub fn run_office_mode(open_apps: bool) -> ModeOutcome {
    let mut steps = vec![set_notifications(false), set_mute(true)];
    if open_apps {
        for name in OFFICE_DEFAULT_APPS {
            steps.push(open_app(name));
        }
    }
    steps.push(backup_config());
    log_steps("办公模式", &steps);
    ModeOutcome::new("办公模式", steps)
}

/// 游戏模式：关通知 → 高性能 → 暂停更新 → （可选）清后台。
pub fn run_game_mode(clear_bg: bool) -> ModeOutcome {
    let mut steps = vec![
        set_notifications(false),
        set_power_plan(true),
        pause_windows_update(true),
    ];
    if clear_bg {
        steps.push(close_background());
    }
    log_steps("游戏模式", &steps);
    ModeOutcome::new("游戏模式", steps)
}

/// 退出办公：恢复通知 + 取消静音（不自动关软件、不回滚备份）。
pub fn restore_office_defaults() -> ModeOutcome {
    let steps = vec![set_notifications(true), set_mute(false)];
    ModeOutcome::new("退出办公模式", steps)
}

/// 退出游戏：平衡电源 + 恢复更新 + 开通知。
pub fn restore_game_defaults() -> ModeOutcome {
    let steps = vec![
        set_notifications(true),
        set_power_plan(false),
        pause_windows_update(false),
    ];
    ModeOutcome::new("退出游戏模式", steps)
}

fn log_steps(mode: &str, steps: &[ModeStep]) {
    for s in steps {
        if s.ok {
            log::info!("{mode} [{}] {}", s.name, s.detail);
        } else {
            log::warn!("{mode} [{}] 失败: {}", s.name, s.detail);
        }
    }
}

// ---------------- 静音（Core Audio 走 shell 后门避免额外依赖） ----------------

/// 静音前的音量登记键（存进 settings，与 Python 的 `modes_prev_volume` 同名）。
pub const PREV_VOLUME_KEY: &str = "modes_prev_volume";

fn ps_audio(script_tail: &str) -> Result<String, String> {
    // 与 Python 用同一套 winmm 成员定义（Add-Type -MemberDefinition 要的是"成员定义"
    // 而非完整类定义；给完整类会生成嵌套类、方法不可达）
    let preamble = "$sig = @'\n[DllImport(\"winmm.dll\")]\npublic static extern int waveOutSetVolume(IntPtr h, uint v);\n[DllImport(\"winmm.dll\")]\npublic static extern int waveOutGetVolume(IntPtr h, out uint v);\n'@\nAdd-Type -MemberDefinition $sig -Name Wv -Namespace SysKitAudio -PassThru | Out-Null\n";
    let script = format!("{preamble}{script_tail}");
    let out = crate::w32::run_capture(
        &[
            "powershell".into(),
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
            script,
        ],
        15,
    )
    .map_err(|e| format!("PowerShell 无法执行: {e}"))?;
    if out.ok {
        Ok(out.stdout.trim().to_string())
    } else {
        Err(out.tail(120, 0))
    }
}

/// 静音/取消静音默认输出设备。
pub fn set_mute(mute: bool) -> ModeStep {
    if mute {
        // 先读当前音量存档，再静音
        let prev = ps_audio("$v = [uint32]0; [SysKitAudio.Wv]::waveOutGetVolume([IntPtr]::Zero, [ref]$v) | Out-Null; $v\n")
            .ok()
            .and_then(|s| s.split_whitespace().last().and_then(|t| t.parse::<i64>().ok()));
        // prev==0 说明已是静音态（可能是本工具此前所为）：不覆盖更早的原音量记录
        let mut save_note = String::new();
        if let Some(v) = usable_prev_volume(prev) {
            // 存档失败必须说出来：静音本身是成功的，但退出时恢复不了原音量——
            // 用户需要在静音**之前**就知道，而不是退出时才发现声音回不来了
            if let Err(e) = crate::settings::set_i64(PREV_VOLUME_KEY, v as i64) {
                save_note = format!("（警告：原音量 {v} 未能存档，退出模式时将无法恢复：{e}）");
            }
        }
        match ps_audio("[SysKitAudio.Wv]::waveOutSetVolume([IntPtr]::Zero, [uint32]0)\n") {
            Ok(_) => ModeStep::ok("系统静音", &format!("已静音{save_note}")),
            Err(e) => ModeStep::fail("系统静音", &e),
        }
    } else {
        let prev = crate::settings::get_i64(PREV_VOLUME_KEY);
        let (vol, note) = restore_volume_plan(prev);
        match ps_audio(&format!(
            "[SysKitAudio.Wv]::waveOutSetVolume([IntPtr]::Zero, [uint32]{vol})\n"
        )) {
            Ok(_) => ModeStep::ok("系统静音", note),
            Err(e) => ModeStep::fail("系统静音", &e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_match_python() {
        assert_eq!(
            NOTIFY_SETTINGS,
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Notifications\Settings"
        );
        assert_eq!(NOTIFY_GLOBAL_VALUE, "NOC_GLOBAL_SETTING_TOASTS_ENABLED");
        assert_eq!(
            WU_POLICY,
            r"HKLM\SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate\AU"
        );
        assert_eq!(POWER_HIGH_PERF, "SCHEME_MIN");
        assert_eq!(POWER_BALANCED, "SCHEME_BALANCED");
    }

    #[test]
    fn common_apps_match_python() {
        assert_eq!(COMMON_APPS.len(), 7);
        let names: Vec<&str> = COMMON_APPS.iter().map(|(n, _)| *n).collect();
        assert!(names.contains(&"记事本"));
        assert!(names.contains(&"系统设置"));
        assert!(names.contains(&"终端"));
        assert_eq!(COMMON_APPS[0], ("记事本", "notepad.exe"));
        // 办公模式默认打开的四个
        assert_eq!(
            OFFICE_DEFAULT_APPS,
            ["记事本", "计算器", "资源管理器", "画图"]
        );
        for n in OFFICE_DEFAULT_APPS {
            assert!(
                COMMON_APPS.iter().any(|(k, _)| *k == n),
                "{n} 应在 COMMON_APPS 里"
            );
        }
    }

    #[test]
    fn whitelist_matches_python_and_excludes_system_processes() {
        assert_eq!(BACKGROUND_CLOSE_WHITELIST.len(), 16);
        assert!(in_close_whitelist("OneDrive.exe"));
        assert!(in_close_whitelist("onedrive.exe"), "大小写不敏感");
        assert!(in_close_whitelist("Spotify.exe"));
        // 关键：绝不包含系统/浏览器主进程
        for dangerous in [
            "explorer.exe",
            "svchost.exe",
            "System",
            "csrss.exe",
            "lsass.exe",
            "winlogon.exe",
            "chrome.exe",
            "msedge.exe",
            "firefox.exe",
            "dwm.exe",
            "wininit.exe",
            "services.exe",
        ] {
            assert!(
                !in_close_whitelist(dangerous),
                "{dangerous} 绝不能在清后台白名单里"
            );
        }
        assert!(!in_close_whitelist("notepad.exe"));
        assert!(!in_close_whitelist(""));
    }

    #[test]
    fn usable_prev_volume_rejects_dirty_data() {
        // 正常值
        assert_eq!(usable_prev_volume(Some(0x8000_8000)), Some(0x8000_8000));
        assert_eq!(usable_prev_volume(Some(1)), Some(1));
        assert_eq!(usable_prev_volume(Some(0xFFFF_FFFF)), Some(0xFFFF_FFFF));
        // 0：本来就是静音态，恢复成 0 等于没取消静音
        assert_eq!(usable_prev_volume(Some(0)), None);
        // 负数 / 越界：脏数据
        assert_eq!(usable_prev_volume(Some(-1)), None);
        assert_eq!(usable_prev_volume(Some(0x1_0000_0000)), None);
        // 无记录
        assert_eq!(usable_prev_volume(None), None);
    }

    #[test]
    fn restore_plan_is_honest_when_no_record() {
        let (vol, note) = restore_volume_plan(Some(0x8000_8000));
        assert_eq!(vol, 0x8000_8000);
        assert!(note.contains("恢复"), "{note}");

        let (vol2, note2) = restore_volume_plan(None);
        assert_eq!(vol2, 0xFFFF_FFFF, "没记录时才设为最大");
        assert!(
            note2.contains("未找到"),
            "必须如实说明没找到原音量: {note2}"
        );
        assert!(!note2.contains("恢复原音量"), "不能谎报已恢复: {note2}");

        // 0 与脏数据走同一分支（Python 注释里的三种脏数据）
        for dirty in [Some(0), Some(-5)] {
            let (v, n) = restore_volume_plan(dirty);
            assert_eq!(v, 0xFFFF_FFFF, "{dirty:?} 应视为无记录");
            assert!(n.contains("未找到"));
        }
    }

    #[test]
    fn update_pause_text_does_not_imply_expiry() {
        let paused = update_pause_detail(true);
        // NoAutoUpdate 是无限期策略，文案不得暗示"到期自动恢复"
        assert!(!paused.contains("7 天"), "{paused}");
        assert!(!paused.contains("自动恢复"), "{paused}");
        assert!(paused.contains("持久"), "要说明它是持久的: {paused}");
        assert!(paused.contains("恢复"), "要告诉用户怎么恢复: {paused}");
        assert_eq!(update_pause_detail(false), "已恢复自动更新");
    }

    #[test]
    fn mode_outcome_counts_and_summary() {
        let steps = vec![
            ModeStep::ok("a", "1"),
            ModeStep::fail("b", "2"),
            ModeStep::ok("c", "3"),
        ];
        let o = ModeOutcome::new("测试模式", steps);
        assert_eq!(o.ok_count, 2);
        assert_eq!(o.fail_count, 1);
        assert!(o.summary().contains("2/3"), "{}", o.summary());
        assert!(
            o.summary().contains('b'),
            "要点名失败的步骤: {}",
            o.summary()
        );
    }

    #[test]
    fn mode_outcome_all_ok_summary() {
        let o = ModeOutcome::new("测试", vec![ModeStep::ok("a", ""), ModeStep::ok("b", "")]);
        assert_eq!(o.fail_count, 0);
        assert!(o.summary().contains("全部完成"), "{}", o.summary());
    }

    #[test]
    fn unknown_app_fails_cleanly() {
        let s = open_app("不存在的应用");
        assert!(!s.ok);
        assert!(s.detail.contains("未知应用"), "{}", s.detail);
    }

    #[test]
    fn single_step_failure_does_not_abort_the_batch() {
        // 核心契约：某一步失败，后续步骤仍要执行并出现在结果里。
        // 用一个必然失败的步骤夹在可成功的步骤之间验证结构。
        let steps = vec![
            ModeStep::fail("会失败的第一步", "模拟"),
            ModeStep::ok("第二步", "仍执行"),
            ModeStep::ok("第三步", "仍执行"),
        ];
        let o = ModeOutcome::new("结构验证", steps);
        assert_eq!(o.steps.len(), 3, "失败步骤不得吞掉后续步骤");
        assert!(o.steps[1].ok && o.steps[2].ok);
    }

    #[test]
    fn notifications_write_is_reversible() {
        // HKCU 写入无需管理员：开启 → 关闭 → 复原。读回确认生效。
        let key = NOTIFY_SETTINGS;
        let before = crate::w32::reg_value(key, NOTIFY_GLOBAL_VALUE).and_then(|v| v.as_u32());
        let on = set_notifications(true);
        assert!(on.ok, "写通知开关失败: {}", on.detail);
        assert_eq!(
            crate::w32::reg_value(key, NOTIFY_GLOBAL_VALUE).and_then(|v| v.as_u32()),
            Some(1),
            "读回必须是 1"
        );
        let off = set_notifications(false);
        assert!(off.ok);
        assert_eq!(
            crate::w32::reg_value(key, NOTIFY_GLOBAL_VALUE).and_then(|v| v.as_u32()),
            Some(0),
            "读回必须是 0"
        );
        // 复原原值，避免污染开发机
        match before {
            Some(v) => {
                crate::w32::reg_set_dword(key, NOTIFY_GLOBAL_VALUE, v);
            }
            None => {
                let _ = crate::w32::reg_delete_value(key, NOTIFY_GLOBAL_VALUE);
            }
        }
    }

    #[test]
    fn power_plan_switch_reaches_both_aliases() {
        // 断言用**方案 GUID** 而不是本地化名称：`powercfg /getactivescheme` 的输出随
        // 系统语言变化（本机是「（高性能）」），按名字断言在不同语言机器上会假失败。
        fn active_guid() -> Option<String> {
            let o = crate::w32::run_capture(&["powercfg".into(), "/getactivescheme".into()], 10)
                .ok()?;
            // 取第一个 8-4-4-4-12 形态的十六进制串
            o.stdout.split_whitespace().find_map(|tok| {
                let t = tok.trim_matches(|c: char| !c.is_ascii_hexdigit() && c != '-');
                let parts: Vec<&str> = t.split('-').collect();
                if parts.len() == 5
                    && [8, 4, 4, 4, 12]
                        .iter()
                        .zip(&parts)
                        .all(|(n, p)| p.len() == *n && p.chars().all(|c| c.is_ascii_hexdigit()))
                {
                    Some(t.to_string())
                } else {
                    None
                }
            })
        }

        let before = active_guid().expect("应能读出当前方案 GUID");

        // 先归到「平衡」再测切换：如果机器本来就停在「高性能」，直接切高性能是
        // 空操作、GUID 不变，断言会假失败（本机初始正是高性能，第一版就栽在这）。
        let seed = set_power_plan(false);
        assert!(seed.ok, "预置平衡失败: {}", seed.detail);
        let balanced = active_guid().expect("应能读出 GUID");
        assert_ne!(
            balanced, before,
            "预置平衡后应已离开原方案（除非原本就是平衡）"
        );

        let hi = set_power_plan(true);
        assert!(hi.ok, "切高性能失败: {}", hi.detail);
        assert_eq!(hi.detail, "高性能", "文案应与请求一致");
        let after_hi = active_guid().expect("切完应能读出 GUID");
        assert_ne!(
            after_hi, balanced,
            "切到高性能后 GUID 必须变化（读回确认，不自证）"
        );

        let bal = set_power_plan(false);
        assert!(bal.ok, "切回平衡失败: {}", bal.detail);
        assert_eq!(bal.detail, "平衡");
        let after_bal = active_guid().expect("切回后应能读出 GUID");
        assert_eq!(after_bal, balanced, "切回平衡应回到同一个方案 GUID");

        // 复原成测试开始前的方案，避免污染开发机
        let _ = crate::w32::run_capture(
            &["powercfg".into(), "/setactive".into(), before.clone()],
            10,
        );
        assert_eq!(
            active_guid().as_deref(),
            Some(before.as_str()),
            "应已复原原方案"
        );
    }

    #[test]
    fn needs_admin_for_update_policy() {
        let s = pause_windows_update(true);
        if crate::w32::is_elevated() {
            // 管理员下会真写 HKLM；读完立刻恢复
            assert!(s.ok, "{}", s.detail);
            let restore = pause_windows_update(false);
            assert!(restore.ok, "{}", restore.detail);
        } else {
            assert!(!s.ok);
            assert!(s.detail.contains("管理员"), "{}", s.detail);
        }
    }

    #[test]
    fn background_candidates_are_all_whitelisted() {
        // 真机：候选列表里的每一项都必须在白名单内（这是"绝不误杀"的前提）
        for (_pid, name) in background_candidates() {
            assert!(
                in_close_whitelist(&name),
                "候选 {name} 不在白名单里——筛选逻辑有漏洞"
            );
        }
    }

    #[test]
    fn current_process_name_reads_self() {
        // 用自身 PID 验证复验逻辑可用（查不到就说明 tasklist 过滤有问题）
        let me = std::process::id();
        let name = current_process_name(me);
        assert!(name.is_some(), "应能查到自身进程名");
        assert!(name.unwrap().to_lowercase().ends_with(".exe"));
    }

    #[test]
    fn backup_config_creates_non_empty_backup() {
        // 备份**必须非空**：空目录对用户没意义，还会让"备份成功"变成误导
        let _g = crate::test_support::serial();
        crate::test_support::isolate();
        let s = backup_config();
        assert!(s.ok, "{}", s.detail);
        // 从 detail 里取路径（格式：N 个文件 → 路径）
        let path = s.detail.split("→ ").nth(1).expect("detail 应含路径");
        let dir = std::path::Path::new(path.trim());
        assert!(dir.is_dir(), "备份目录应存在: {path}");
        let entries: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().collect();
        assert!(!entries.is_empty(), "备份目录不得为空");
    }

    #[test]
    fn mode_outcome_serializes() {
        let j =
            serde_json::to_string(&ModeOutcome::new("x", vec![ModeStep::ok("s", "d")])).unwrap();
        for k in [
            "mode",
            "steps",
            "ok_count",
            "fail_count",
            "name",
            "ok",
            "detail",
        ] {
            assert!(j.contains(k), "序列化应含 {k}: {j}");
        }
    }
}
