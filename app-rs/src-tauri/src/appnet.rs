//! 应用联网控制（对应 Python 侧 appnet.py）：用 Windows 防火墙规则按程序阻断出口流量。
//!
//! ## 跨版本身份契约（本模块最重要的约束）
//!
//! 规则名 = `SysKit-AppNet-<sha1(norm_path(program))[:10]>`。
//! **Python 版与 Rust 版必须算出完全相同的规则名**，否则同一个 exe 会被两个版本
//! 各建一条规则，用户在 `wf.msc` 里看到重复项，而"关闭阻断"只关掉其中一条——
//! 程序仍然联不上网，用户却已被告知"已放行"。
//!
//! 因此这里有**两个不同的路径形态**，混用就是 bug：
//! - [`norm_path`]：**身份键**，只用于比对与哈希。统一小写 + 反斜杠，
//!   等于 Python 的 `os.path.normcase(os.path.abspath(p))`。
//! - [`real_path`]：**下发给 netsh 的值**，保留用户原始大小写。防火墙本身对大小写
//!   不敏感，但把用户程序名改成小写会让规则在 `wf.msc` 里显得像伪造的。
//!
//! 已用真实 Python 输出校准：`C:\Program Files\A\a.exe` → `SysKit-AppNet-5bd6cecf63`。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// 规则名前缀（同时用于辨别"哪些规则是我们建的"）。
pub const RULE_PREFIX: &str = "SysKit-AppNet";
/// 台账文件名（与 Python `STORE_NAME` 一致，共享 `%APPDATA%\SysKit`）。
pub const STORE_NAME: &str = "appnet.json";
/// 需要管理员时的统一提示（与 Python `ADMIN_MSG` 同义）。
pub const ADMIN_MSG: &str = "修改防火墙规则需要管理员权限。请以管理员身份运行 SysKit 后重试。";
/// PowerShell 可用性哨兵：只有它出现才说明 `Get-NetFirewallRule` 跑得动。
pub const PS_PROBE: &str = "SysKit-AppNet-PS-Available";

/// 一条受控程序台账。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppNetEntry {
    /// 原始大小写的绝对路径（展示与下发都用它）
    pub program: String,
    /// 规则名；旧版/手工编辑的台账可能没有 → 由 [`rule_name`] 补算
    #[serde(default)]
    pub rule: String,
    #[serde(default)]
    pub blocked: bool,
    #[serde(default)]
    pub added: String,
}

/// 规则实际状态（三态，`drift` 不可省略——见 [`parse_status_output`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleState {
    /// 启用且动作为阻止：真的在拦
    On,
    /// 规则在但已停用：放行
    Off,
    /// 动作不再是 Block（多半是用户在 wf.msc 里改成了"允许"）：无论启不启用都不拦
    Drift,
    /// 查不到该规则
    Missing,
}

impl RuleState {
    /// 是否真的在阻断。
    pub fn is_blocking(self) -> bool {
        self == RuleState::On
    }

    pub fn as_str(self) -> &'static str {
        match self {
            RuleState::On => "on",
            RuleState::Off => "off",
            RuleState::Drift => "drift",
            RuleState::Missing => "missing",
        }
    }
}

/// 去引号并转绝对路径，**保留原始大小写**（下发给 netsh 的值与展示用）。
///
/// 对应 Python `real_path`。注意 Rust 的 `std::path::absolute` 不做 `..` 归一化，
/// 而 Python `abspath` 会——这里补上纯词法的 `..`/`.` 折叠，保证两边一致。
pub fn real_path(program: &str) -> String {
    let p = program.trim().trim_matches('"').trim();
    if p.is_empty() {
        return String::new();
    }
    let abs = if is_absolute(p) {
        p.to_string()
    } else {
        // 相对路径：拼当前工作目录（abspath 语义）
        match std::env::current_dir() {
            Ok(cwd) => {
                let mut s = cwd.to_string_lossy().to_string();
                if !s.ends_with('\\') && !s.ends_with('/') {
                    s.push('\\');
                }
                s.push_str(p);
                s
            }
            Err(_) => p.to_string(),
        }
    };
    lexical_normalize(&abs)
}

/// 是否绝对路径（`C:\...`、`\\?\C:\...`、`\\server\share`、`/x`）。
fn is_absolute(p: &str) -> bool {
    let b = p.as_bytes();
    if b.len() >= 2 && b[1] == b':' {
        return true;
    }
    p.starts_with('\\') || p.starts_with('/')
}

/// 纯词法归一化：折叠 `\\`、`/` → `\\`，处理 `.` 与 `..`（不访问文件系统）。
///
/// 不解析 `..` 会不会"猜错"符号链接？会——但 Python `abspath` 同样只做词法处理，
/// 两边一致才是本函数的目的。
pub fn lexical_normalize(path: &str) -> String {
    let unified = path.replace('/', "\\");
    // 保留 UNC/长路径前缀
    let (prefix, rest) = if let Some(r) = unified.strip_prefix("\\\\?\\") {
        ("\\\\?\\", r)
    } else if let Some(r) = unified.strip_prefix("\\\\") {
        ("\\\\", r)
    } else {
        ("", unified.as_str())
    };
    let mut parts: Vec<&str> = Vec::new();
    for seg in rest.split('\\') {
        match seg {
            "" | "." => continue,
            ".." => {
                // 盘符根（`C:`）不可被 `..` 弹掉
                if parts.len() > 1 || (parts.len() == 1 && !parts[0].ends_with(':')) {
                    parts.pop();
                }
            }
            s => parts.push(s),
        }
    }
    let body = parts.join("\\");
    if prefix.is_empty() {
        body
    } else {
        format!("{prefix}{body}")
    }
}

/// **身份键**：统一小写 + 反斜杠，仅用于比对与哈希。
///
/// 等价 Python `os.path.normcase(os.path.abspath(p))`。同一个 exe 从文件选择器
/// （`C:\Program Files\A\a.exe`）与手输（`c:/program files/a/a.exe`）必须落到同一键，
/// 否则用户会看到两条重复规则却谁也没生效。
pub fn norm_path(program: &str) -> String {
    let p = real_path(program);
    p.to_lowercase().replace('/', "\\")
}

/// 规则名：`SysKit-AppNet-<sha1(身份键) 前 10 位十六进制>`。
pub fn rule_name(program: &str) -> String {
    use sha1::{Digest, Sha1};
    let key = norm_path(program);
    let digest = Sha1::digest(key.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("{RULE_PREFIX}-{}", &hex[..10])
}

/// 规则描述（人类可读信息只写这里——`netsh add rule` 没有 group/displayName 参数）。
pub fn rule_description(program: &str) -> String {
    format!("SysKit 应用联网控制 · {}", real_path(program))
}

/// netsh：新建阻断规则。
///
/// **只用 netsh 文档参数**（name/program/dir/action/enable/description）。
/// 真机实锤：`netsh advfirewall add rule` **没有** `group`/`displayName` 参数
/// （那是 PowerShell `New-NetFirewallRule` 的概念），带上就报"不是这个指令的有效参数"
/// ——初版写路径因此在任何机器上都从未成功过，还被桩测试掩盖。
pub fn cmd_add(program: &str, enable: bool) -> Vec<String> {
    vec![
        "netsh".into(),
        "advfirewall".into(),
        "firewall".into(),
        "add".into(),
        "rule".into(),
        format!("name={}", rule_name(program)),
        format!("program={}", real_path(program)),
        "dir=out".into(),
        "action=block".into(),
        format!("enable={}", if enable { "yes" } else { "no" }),
        format!("description={}", rule_description(program)),
    ]
}

/// netsh：启用/停用已有规则（`new` 是 set rule 的必需子参数）。
pub fn cmd_set_enable(program: &str, enable: bool) -> Vec<String> {
    vec![
        "netsh".into(),
        "advfirewall".into(),
        "firewall".into(),
        "set".into(),
        "rule".into(),
        format!("name={}", rule_name(program)),
        "new".into(),
        format!("enable={}", if enable { "yes" } else { "no" }),
    ]
}

/// netsh：删除规则。
pub fn cmd_delete(program: &str) -> Vec<String> {
    vec![
        "netsh".into(),
        "advfirewall".into(),
        "firewall".into(),
        "delete".into(),
        "rule".into(),
        format!("name={}", rule_name(program)),
    ]
}

/// netsh：查询规则。
pub fn cmd_show(program: &str) -> Vec<String> {
    vec![
        "netsh".into(),
        "advfirewall".into(),
        "firewall".into(),
        "show".into(),
        "rule".into(),
        format!("name={}", rule_name(program)),
    ]
}

/// 批量查规则状态的 PowerShell 脚本。
///
/// 三处都是真机踩出来的：
/// 1. `-Name` 里有一个名字不存在时，`powershell.exe` **即使输出正常也返回退出码 1**
///    ——所以脚本末尾必须 `exit 0`，否则"查 3 条、1 条被外部删了"会被当成查询失败；
/// 2. 光看退出码/空输出分不清"NetSecurity 模块不可用"和"规则都不在"，
///    故显式探测 cmdlet 是否存在并输出哨兵行（没有哨兵 = 真查不动）；
/// 3. **只读 Enabled 不够**：用户把"操作"从阻止改成允许后 Enabled 仍是 True，
///    只看 Enabled 会把已经放行的程序报成"已禁止联网"——对安全控制属实质性错报，
///    所以连 Action 一起读。
pub fn ps_status_script(programs: &[String]) -> String {
    let names = programs
        .iter()
        .map(|p| {
            // 规则名本应只含 ASCII 十六进制，仍做 PS 单引号转义——
            // 不依赖"调用方一定传安全值"这个假设
            format!("'{}'", rule_name(p).replace('\'', "''"))
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "$ErrorActionPreference='SilentlyContinue';$ok=[bool](Get-Command Get-NetFirewallRule -ErrorAction SilentlyContinue);if($ok){{ '{PS_PROBE}'; Get-NetFirewallRule -Name {names} | ForEach-Object {{ \"$($_.Name)=$($_.Enabled)=$($_.Action)\" }} }}; exit 0"
    )
}

/// 解析 PS 输出 → `{规则名: 状态}`。不认识的行忽略（哨兵与 PS 告警）。
///
/// 规则名不含 `=`，所以 `split('=')` 出三段才是规则行；哨兵行不含 `=`。
pub fn parse_status_output(text: &str) -> BTreeMap<String, RuleState> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.trim().split('=').map(|s| s.trim()).collect();
        if parts.len() != 3 {
            continue;
        }
        let (name, enabled, action) = (parts[0], parts[1], parts[2]);
        if !name.starts_with(RULE_PREFIX) {
            continue;
        }
        let state = if !action.eq_ignore_ascii_case("block") {
            // 动作被改成"允许"：无论启不启用都不拦，绝不能报"已禁止联网"
            RuleState::Drift
        } else if enabled.eq_ignore_ascii_case("true") {
            RuleState::On
        } else {
            RuleState::Off
        };
        out.insert(name.to_string(), state);
    }
    out
}

/// 解析 `netsh advfirewall firewall show rule` 输出 → (是否启用, 动作)。
///
/// netsh 输出随系统语言变化，中英双语都要认（与 Python 的正则同口径）。
pub fn parse_netsh_state(text: &str) -> Option<(bool, String)> {
    let mut enabled: Option<bool> = None;
    let mut action: Option<String> = None;
    for line in text.lines() {
        let t = line.trim();
        if !(t.contains(':') || t.contains('：')) {
            continue;
        }
        let (key, val) = match t.split_once('：').or_else(|| t.split_once(':')) {
            Some(kv) => kv,
            None => continue,
        };
        let (key, val) = (key.trim(), val.trim());
        if key == "已启用" || key.eq_ignore_ascii_case("Enabled") {
            enabled = Some(
                val.eq_ignore_ascii_case("是")
                    || val.eq_ignore_ascii_case("yes")
                    || val.eq_ignore_ascii_case("true"),
            );
        } else if key == "操作" || key.eq_ignore_ascii_case("Action") {
            action = Some(val.to_string());
        }
    }
    match (enabled, action) {
        (Some(e), Some(a)) => Some((e, a)),
        _ => None,
    }
}

/// netsh 的"操作"文本是否为阻止（中英双语）。
pub fn action_is_block(action: &str) -> bool {
    let a = action.to_lowercase();
    a.contains("阻止") || a.contains("block")
}

/// 台账去重（按身份键，保留第一条）。
///
/// JSON 可手工编辑、也可能来自大小写不同的旧版本；留着重复行会让表格出现两行同名
/// 程序——`find_entry` 恒命中第一条，用户选中第二行点"移除"会弹"已删除规则"却把
/// 另一条 pop 掉，提示骗人。
pub fn dedupe_entries(entries: &[AppNetEntry]) -> Vec<AppNetEntry> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for e in entries {
        let program = real_path(&e.program);
        if program.is_empty() {
            continue;
        }
        let key = norm_path(&program);
        if !seen.insert(key) {
            continue;
        }
        out.push(AppNetEntry {
            rule: if e.rule.is_empty() {
                rule_name(&program)
            } else {
                e.rule.clone()
            },
            program,
            blocked: e.blocked,
            added: e.added.clone(),
        });
    }
    out
}

/// 校验待添加的程序路径：返回 Ok(绝对路径) 或 Err(可读原因)。
pub fn validate_program(program: &str, exists: bool) -> Result<String, String> {
    let p = real_path(program);
    if p.is_empty() {
        return Err("请选择或输入要控制的程序路径".to_string());
    }
    if !exists {
        return Err(format!("文件不存在: {p}"));
    }
    Err(String::new()).or(Ok(p))
}

/// 台账文件路径（与 Python 共享 `%APPDATA%\SysKit\appnet.json`）。
pub fn store_file() -> std::path::PathBuf {
    crate::paths::appdata_dir().join(STORE_NAME)
}

/// 读取台账：`{"entries":[...]}`，按身份键去重。
///
/// 与 Python 同格式——**两个版本共享这个文件**，字段名必须一致，
/// 否则迁移期间台账会互相"看不见"对方的条目。
pub fn load_entries() -> Vec<AppNetEntry> {
    let path = store_file();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    #[derive(serde::Deserialize)]
    struct Root {
        #[serde(default)]
        entries: Vec<AppNetEntry>,
    }
    match serde_json::from_str::<Root>(&text) {
        Ok(r) => dedupe_entries(&r.entries),
        Err(_) => Vec::new(), // 损坏的台账按空处理，不炸调用方
    }
}

/// 写回台账（原子替换）。返回是否成功。
pub fn save_entries(entries: &[AppNetEntry]) -> bool {
    let path = store_file();
    if let Some(dir) = path.parent() {
        if std::fs::create_dir_all(dir).is_err() {
            return false;
        }
    }
    #[derive(serde::Serialize)]
    struct Root<'a> {
        entries: &'a [AppNetEntry],
    }
    let Ok(text) = serde_json::to_string_pretty(&Root { entries }) else {
        return false;
    };
    let tmp = path.with_extension("json.syskit.tmp");
    if std::fs::write(&tmp, text.as_bytes()).is_err() {
        return false;
    }
    if std::fs::rename(&tmp, &path).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    true
}

/// 按身份键查找台账下标。
pub fn find_entry(entries: &[AppNetEntry], program: &str) -> Option<usize> {
    let key = norm_path(program);
    entries.iter().position(|e| norm_path(&e.program) == key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_name_matches_python_reference_values() {
        // 这两组值来自真实 Python 输出（python -c 'appnet.rule_name(...)'），
        // 是跨版本身份契约的锚点：算错就会产生重复规则。
        assert_eq!(
            rule_name(r"C:\Program Files\A\a.exe"),
            "SysKit-AppNet-5bd6cecf63"
        );
        assert_eq!(
            rule_name(r"C:\Windows\notepad.exe"),
            "SysKit-AppNet-84a531d86f"
        );
    }

    #[test]
    fn rule_name_is_stable_across_path_spellings() {
        // 同一个 exe 的四种写法必须落到同一个规则名，否则 wf.msc 里出现重复规则
        let want = "SysKit-AppNet-5bd6cecf63";
        for p in [
            r"C:\Program Files\A\a.exe",
            "c:/program files/a/a.exe",
            r#""C:\Program Files\A\a.exe""#,
            r"C:\Program Files\A\.\a.exe",
        ] {
            assert_eq!(rule_name(p), want, "{p} 应归一到同一规则名");
        }
    }

    #[test]
    fn rule_name_ignores_surrounding_whitespace_and_quotes() {
        assert_eq!(
            rule_name("  \"C:\\Windows\\notepad.exe\"  "),
            rule_name(r"C:\Windows\notepad.exe")
        );
    }

    #[test]
    fn norm_path_lowercases_and_backslashes() {
        assert_eq!(
            norm_path(r"C:\Program Files\A\a.exe"),
            r"c:\program files\a\a.exe"
        );
        assert_eq!(
            norm_path("C:/Program Files/A/a.exe"),
            r"c:\program files\a\a.exe"
        );
        assert_eq!(norm_path(""), "");
        assert_eq!(norm_path("   "), "");
    }

    #[test]
    fn real_path_keeps_original_case() {
        // 下发给 netsh 的值必须保留大小写：改成小写会让规则在 wf.msc 里像伪造的
        assert_eq!(
            real_path(r"C:\Program Files\A\a.exe"),
            r"C:\Program Files\A\a.exe"
        );
        assert_eq!(
            real_path("C:/Windows/Notepad.EXE"),
            r"C:\Windows\Notepad.EXE"
        );
    }

    #[test]
    fn lexical_normalize_handles_dotdot_and_dots() {
        assert_eq!(lexical_normalize(r"C:\a\b\..\c"), r"C:\a\c");
        assert_eq!(lexical_normalize(r"C:\a\.\b"), r"C:\a\b");
        assert_eq!(lexical_normalize(r"C:\a\\b"), r"C:\a\b");
        // 盘符根不可被 .. 弹掉
        assert_eq!(lexical_normalize(r"C:\..\a"), r"C:\a");
        // UNC 前缀保留
        assert_eq!(lexical_normalize(r"\\server\share\x"), r"\\server\share\x");
        assert_eq!(lexical_normalize(r"\\?\C:\x\..\y"), r"\\?\C:\y");
    }

    #[test]
    fn is_absolute_variants() {
        assert!(is_absolute(r"C:\x"));
        assert!(is_absolute(r"\\server\share"));
        assert!(is_absolute("/x"));
        assert!(!is_absolute(r"rel\x"));
        assert!(!is_absolute("a.exe"));
    }

    #[test]
    fn cmd_add_uses_only_netsh_documented_params() {
        let c = cmd_add(r"C:\Program Files\A\a.exe", true);
        assert_eq!(
            &c[0..5],
            &["netsh", "advfirewall", "firewall", "add", "rule"]
        );
        let joined = c.join(" ");
        assert!(joined.contains("name=SysKit-AppNet-5bd6cecf63"));
        assert!(joined.contains(r"program=C:\Program Files\A\a.exe"));
        assert!(joined.contains("dir=out"));
        assert!(joined.contains("action=block"));
        assert!(joined.contains("enable=yes"));
        assert!(joined.contains("description=SysKit 应用联网控制 · C:\\Program Files\\A\\a.exe"));
        // 真机实锤：netsh 的 add rule 没有 group/displayName 参数，带上必然失败
        assert!(!joined.contains("group="), "不得带 group=");
        assert!(!joined.contains("displayName="), "不得带 displayName=");
    }

    #[test]
    fn cmd_add_disable_variant() {
        let c = cmd_add(r"C:\a.exe", false);
        assert!(c.join(" ").contains("enable=no"));
    }

    #[test]
    fn cmd_set_enable_requires_new_subparam() {
        let c = cmd_set_enable(r"C:\a.exe", false);
        // 规则名同样取自真实 Python 输出（不是猜的）
        assert_eq!(
            &c[0..6],
            &[
                "netsh",
                "advfirewall",
                "firewall",
                "set",
                "rule",
                "name=SysKit-AppNet-c0bae2df43"
            ]
        );
        assert_eq!(c[6], "new", "set rule 必须带 new 子参数");
        assert_eq!(c[7], "enable=no");
    }

    #[test]
    fn cmd_delete_and_show_shapes() {
        let d = cmd_delete(r"C:\a.exe");
        assert_eq!(
            &d[0..5],
            &["netsh", "advfirewall", "firewall", "delete", "rule"]
        );
        let s = cmd_show(r"C:\a.exe");
        assert_eq!(
            &s[0..5],
            &["netsh", "advfirewall", "firewall", "show", "rule"]
        );
    }

    #[test]
    fn ps_script_has_probe_and_exit_zero() {
        let script = ps_status_script(&[r"C:\a.exe".to_string()]);
        assert!(
            script.contains("Get-Command Get-NetFirewallRule"),
            "必须探测 cmdlet"
        );
        assert!(script.contains(PS_PROBE), "必须输出哨兵行");
        assert!(
            script.ends_with("exit 0"),
            "末尾必须 exit 0：规则缺失时 PS 也返回 1"
        );
        assert!(
            script.contains("$($_.Enabled)=$($_.Action)"),
            "必须同时读 Enabled 与 Action"
        );
    }

    #[test]
    fn ps_script_escapes_quotes() {
        // 规则名由 rule_name 生成、实际不含单引号；仍验证转义不依赖该假设
        let script = ps_status_script(&["x".to_string()]);
        assert!(!script.contains("''''"), "不应产生非法转义: {script}");
        assert!(script.contains("'SysKit-AppNet-"));
    }

    #[test]
    fn parse_status_output_three_states() {
        let text = format!(
            "{PS_PROBE}\n\
             SysKit-AppNet-aaaaaaaaaa=True=Block\n\
             SysKit-AppNet-bbbbbbbbbb=False=Block\n\
             SysKit-AppNet-cccccccccc=True=Allow\n"
        );
        let m = parse_status_output(&text);
        assert_eq!(m.len(), 3, "哨兵行不含 = 应被忽略");
        assert_eq!(m["SysKit-AppNet-aaaaaaaaaa"], RuleState::On);
        assert_eq!(m["SysKit-AppNet-bbbbbbbbbb"], RuleState::Off);
        assert_eq!(
            m["SysKit-AppNet-cccccccccc"],
            RuleState::Drift,
            "动作被改成 Allow 的必须报 drift，不能报『已禁止联网』"
        );
    }

    #[test]
    fn parse_status_output_ignores_foreign_and_malformed_lines() {
        let text = "Get-NetFirewallRule : 找不到...\n\
                    Some-Other-Rule=True=Block\n\
                    只有两段=True\n\
                    四=段=的=行\n\
                    =====\n";
        assert!(parse_status_output(text).is_empty());
    }

    #[test]
    fn drift_takes_priority_over_enabled() {
        // 启用但动作是 Allow：drift（不拦），而不是 on
        let m = parse_status_output("SysKit-AppNet-x=True=Allow\n");
        assert_eq!(m["SysKit-AppNet-x"], RuleState::Drift);
        assert!(!m["SysKit-AppNet-x"].is_blocking());
    }

    #[test]
    fn rule_state_helpers() {
        assert!(RuleState::On.is_blocking());
        assert!(!RuleState::Off.is_blocking());
        assert!(!RuleState::Drift.is_blocking());
        assert!(!RuleState::Missing.is_blocking());
        assert_eq!(RuleState::On.as_str(), "on");
        assert_eq!(RuleState::Missing.as_str(), "missing");
    }

    #[test]
    fn parse_netsh_state_bilingual() {
        let zh = "规则名称:  SysKit-AppNet-x\n已启用:  是\n操作:  阻止\n";
        assert_eq!(parse_netsh_state(zh), Some((true, "阻止".to_string())));
        let en = "Rule Name:  SysKit-AppNet-x\nEnabled:  No\nAction:  Block\n";
        assert_eq!(parse_netsh_state(en), Some((false, "Block".to_string())));
        // 半角冒号也算
        assert_eq!(
            parse_netsh_state("已启用: 否\n操作: 允许\n"),
            Some((false, "允许".to_string()))
        );
    }

    #[test]
    fn parse_netsh_state_returns_none_when_incomplete() {
        assert_eq!(parse_netsh_state(""), None);
        assert_eq!(parse_netsh_state("已启用:  是\n"), None, "缺操作不算可解析");
        assert_eq!(parse_netsh_state("没有规则匹配\n"), None);
    }

    #[test]
    fn action_is_block_bilingual() {
        assert!(action_is_block("阻止"));
        assert!(action_is_block("Block"));
        assert!(action_is_block("BLOCK"));
        assert!(!action_is_block("允许"));
        assert!(!action_is_block("Allow"));
    }

    #[test]
    fn dedupe_keeps_first_and_rekeys_different_case() {
        let entries = vec![
            AppNetEntry {
                program: r"C:\A\a.exe".into(),
                rule: String::new(),
                blocked: true,
                added: "t1".into(),
            },
            AppNetEntry {
                program: r"c:\a\A.EXE".into(), // 同一文件、不同大小写
                rule: String::new(),
                blocked: false,
                added: "t2".into(),
            },
            AppNetEntry {
                program: r"C:\B\b.exe".into(),
                rule: String::new(),
                blocked: true,
                added: "t3".into(),
            },
        ];
        let out = dedupe_entries(&entries);
        assert_eq!(out.len(), 2, "同身份键只保留第一条");
        assert_eq!(out[0].program, r"C:\A\a.exe");
        assert_eq!(out[0].added, "t1", "保留的是第一条");
        assert_eq!(out[1].program, r"C:\B\b.exe");
        // rule 为空时补算
        assert_eq!(out[0].rule, rule_name(r"C:\A\a.exe"));
    }

    #[test]
    fn dedupe_skips_empty_and_fills_rule() {
        let entries = vec![
            AppNetEntry {
                program: "   ".into(),
                rule: "keep".into(),
                blocked: true,
                added: String::new(),
            },
            AppNetEntry {
                program: r"C:\a.exe".into(),
                rule: "custom-name".into(),
                blocked: true,
                added: String::new(),
            },
        ];
        let out = dedupe_entries(&entries);
        assert_eq!(out.len(), 1, "空路径条目应被丢弃");
        assert_eq!(out[0].rule, "custom-name", "已有规则名应保留");
    }

    #[test]
    fn validate_program_rejects_empty_and_missing() {
        assert!(validate_program("", true).is_err());
        assert!(validate_program("   ", true).is_err());
        let e = validate_program(r"C:\nope.exe", false).unwrap_err();
        assert!(e.contains("文件不存在"), "{e}");
        let e2 = validate_program("", true).unwrap_err();
        assert!(e2.contains("请选择"), "{e2}");
    }

    #[test]
    fn validate_program_accepts_existing() {
        assert_eq!(
            validate_program(r"C:\Windows\Notepad.exe", true).unwrap(),
            r"C:\Windows\Notepad.exe"
        );
    }

    #[test]
    fn store_name_matches_python() {
        assert_eq!(STORE_NAME, "appnet.json");
        assert_eq!(RULE_PREFIX, "SysKit-AppNet");
        assert_eq!(PS_PROBE, "SysKit-AppNet-PS-Available");
        assert!(ADMIN_MSG.contains("管理员"));
    }

    #[test]
    fn ledger_round_trips_python_format() {
        let _g = crate::test_support::serial();
        crate::test_support::isolate();
        let _ = std::fs::remove_file(store_file());

        assert!(load_entries().is_empty(), "文件不存在时应为空表");

        let entries = vec![
            AppNetEntry {
                program: r"C:\A\a.exe".into(),
                rule: String::new(),
                blocked: true,
                added: "2026-01-01 10:00:00".into(),
            },
            AppNetEntry {
                program: r"C:\B\b.exe".into(),
                rule: String::new(),
                blocked: false,
                added: "2026-01-02 10:00:00".into(),
            },
        ];
        assert!(save_entries(&entries), "写入应成功");

        let back = load_entries();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].program, r"C:\A\a.exe");
        assert_eq!(back[0].rule, rule_name(r"C:\A\a.exe"), "rule 应被补算");
        assert!(back[0].blocked);
        assert!(!back[1].blocked);
        assert_eq!(back[1].added, "2026-01-02 10:00:00");
    }

    #[test]
    fn ledger_reads_python_written_json() {
        // 关键：Python 版写的文件必须能被读进来（迁移期间两版共享该文件）
        let _g = crate::test_support::serial();
        crate::test_support::isolate();
        let python_json = r#"{"entries": [
            {"program": "C:\\Program Files\\A\\a.exe",
             "rule": "SysKit-AppNet-5bd6cecf63",
             "blocked": true,
             "added": "2025-12-31 23:59:59"}
        ]}"#;
        if let Some(dir) = store_file().parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(store_file(), python_json).unwrap();
        let back = load_entries();
        assert_eq!(back.len(), 1, "Python 写的台账必须能读");
        assert_eq!(back[0].program, r"C:\Program Files\A\a.exe");
        assert_eq!(back[0].rule, "SysKit-AppNet-5bd6cecf63");
        assert!(back[0].blocked);
    }

    #[test]
    fn ledger_survives_corrupt_file() {
        let _g = crate::test_support::serial();
        crate::test_support::isolate();
        std::fs::write(store_file(), "{ 这不是 json").unwrap();
        assert!(load_entries().is_empty(), "损坏台账按空处理，不 panic");
    }

    #[test]
    fn ledger_tolerates_missing_optional_fields() {
        let _g = crate::test_support::serial();
        crate::test_support::isolate();
        // 缺 rule/added（旧版或手工编辑）
        std::fs::write(
            store_file(),
            r#"{"entries":[{"program":"C:\\a.exe","blocked":true}]}"#,
        )
        .unwrap();
        let back = load_entries();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].rule, rule_name(r"C:\a.exe"));
        assert_eq!(back[0].added, "");
    }

    #[test]
    fn find_entry_matches_identity_key() {
        let entries = vec![AppNetEntry {
            program: r"C:\Program Files\A\a.exe".into(),
            rule: String::new(),
            blocked: true,
            added: String::new(),
        }];
        assert_eq!(find_entry(&entries, r"C:\Program Files\A\a.exe"), Some(0));
        // 大小写与斜杠不同也要命中（否则"移除"会找不到刚加进去的条目）
        assert_eq!(find_entry(&entries, "c:/program files/a/a.exe"), Some(0));
        assert_eq!(find_entry(&entries, r"C:\other.exe"), None);
    }
}
