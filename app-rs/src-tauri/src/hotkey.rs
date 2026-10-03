//! 全局热键：热键字符串解析/显示（纯逻辑）+ RegisterHotKey 消息泵线程。
//!
//! Python 侧用 Qt 的 nativeEventFilter 接 WM_HOTKEY；Tauri 没有等价钩子，
//! 因此本模块起一条**专用线程**：在该线程上 RegisterHotKey(NULL,…) 并跑
//! GetMessageW/DispatchMessageW 消息泵——WM_HOTKEY 只会投递给注册线程的
//! 消息队列，同线程泵是唯一正确接法。命中后经 Tauri 事件送前端。
//!
//! 硬约束：恒置 MOD_NOREPEAT（长按不重复触发，只抑制自动重复、不抑制离散重按）；
//! 注册被占用一律静默降级（绝不钩子强拦），托盘菜单仍可用。

use std::sync::mpsc;
use std::thread;

use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, TranslateMessage, MSG, WM_HOTKEY,
};

/// 热键 id 分配（与 Python 侧一致）：1=命令面板，2=快速直贴，3=快捷粘贴面板。
pub const ID_PALETTE: i32 = 1;
pub const ID_QUICK_PASTE: i32 = 2;
pub const ID_PASTE_PANEL: i32 = 3;

/// 命名键 → 虚拟键码（逐项对齐 hotkey.py::_NAMED_KEYS）。
const NAMED_KEYS: &[(&str, u32)] = &[
    ("space", 0x20),
    ("esc", 0x1B),
    ("escape", 0x1B),
    ("tab", 0x09),
    ("enter", 0x0D),
    ("return", 0x0D),
    ("backspace", 0x08),
    ("del", 0x2E),
    ("delete", 0x2E),
    ("ins", 0x2D),
    ("insert", 0x2D),
    ("home", 0x24),
    ("end", 0x23),
    ("pgup", 0x21),
    ("pgdn", 0x22),
    ("left", 0x25),
    ("up", 0x26),
    ("right", 0x27),
    ("down", 0x28),
    ("oem_3", 0xC0),
    ("`", 0xC0),
    ("~", 0xC0),
    ("oem_plus", 0xBB),
    ("=", 0xBB),
    ("+", 0xBB),
    ("oem_minus", 0xBD),
    ("-", 0xBD),
    ("oem_comma", 0xBC),
    (",", 0xBC),
    ("oem_period", 0xBE),
    (".", 0xBE),
    ("oem_2", 0xBF),
    ("/", 0xBF),
    ("oem_1", 0xBA),
    (";", 0xBA),
    ("oem_4", 0xDB),
    ("[", 0xDB),
    ("oem_6", 0xDD),
    ("]", 0xDD),
    ("oem_7", 0xDE),
    ("'", 0xDE),
];

/// OEM 键的可读显示名（chr 会出 Latin-1 乱码，必须查表）。
const OEM_DISPLAY: &[(u32, &str)] = &[
    (0xBA, ";"),
    (0xBB, "="),
    (0xBC, ","),
    (0xBD, "-"),
    (0xBE, "."),
    (0xBF, "/"),
    (0xC0, "`"),
    (0xDB, "["),
    (0xDD, "]"),
    (0xDE, "'"),
];

/// 单字符 / F1-F24 / 命名键 → 虚拟键码；未知返回 None。
pub fn key_to_vk(key: &str) -> Option<u32> {
    let key = key.trim().to_lowercase();
    if key.is_empty() {
        return None;
    }
    // 单字符字母/数字（注意：必须判 ASCII，不能用 char 的 is_alphanumeric）
    let bytes = key.as_bytes();
    if bytes.len() == 1 {
        let ch = bytes[0].to_ascii_uppercase();
        if ch.is_ascii_uppercase() || ch.is_ascii_digit() {
            return Some(ch as u32);
        }
    }
    if let Some(rest) = key.strip_prefix('f') {
        if !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()) {
            if let Ok(n) = rest.parse::<u32>() {
                if (1..=24).contains(&n) {
                    return Some(0x70 + (n - 1));
                }
            }
        }
    }
    NAMED_KEYS
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, vk)| *vk)
}

/// `'Ctrl+Alt+K'` → `(mods, vk)`；无法解析返回 None（至少要有一个主键）。
///
/// 注意 mods 不含 MOD_NOREPEAT —— 该位由注册处恒置，不混进解析结果。
pub fn parse_hotkey(text: &str) -> Option<(u32, u32)> {
    let mut mods = 0u32;
    let mut key: Option<String> = None;
    for part in text.split('+') {
        let p = part.trim().to_lowercase();
        if p.is_empty() {
            continue;
        }
        match p.as_str() {
            "ctrl" | "control" => mods |= MOD_CONTROL.0,
            "alt" => mods |= MOD_ALT.0,
            "shift" => mods |= MOD_SHIFT.0,
            "win" => mods |= MOD_WIN.0,
            _ => key = Some(p),
        }
    }
    let vk = key.as_deref().and_then(key_to_vk)?;
    Some((mods, vk))
}

/// 规范化显示（`k` → `K`；非法返回空串）。
pub fn hotkey_display(text: &str) -> String {
    let (mods, vk) = match parse_hotkey(text) {
        Some(v) => v,
        None => return String::new(),
    };
    let mut names: Vec<String> = Vec::new();
    if mods & MOD_CONTROL.0 != 0 {
        names.push("Ctrl".into());
    }
    if mods & MOD_ALT.0 != 0 {
        names.push("Alt".into());
    }
    if mods & MOD_SHIFT.0 != 0 {
        names.push("Shift".into());
    }
    if mods & MOD_WIN.0 != 0 {
        names.push("Win".into());
    }
    // 按 Python 的遍历顺序取第一个「多字符且非 oem_」的命名键
    let named = NAMED_KEYS
        .iter()
        .find(|(name, code)| *code == vk && name.len() > 1 && !name.starts_with("oem"));
    if let Some((name, _)) = named {
        let mut chars = name.chars();
        let head = chars
            .next()
            .map(|c| c.to_ascii_uppercase().to_string())
            .unwrap_or_default();
        names.push(format!("{head}{}", chars.as_str()));
    } else if (0x70..=0x87).contains(&vk) {
        names.push(format!("F{}", vk - 0x70 + 1));
    } else if let Some((_, disp)) = OEM_DISPLAY.iter().find(|(code, _)| *code == vk) {
        names.push((*disp).to_string());
    } else if let Some(ch) = char::from_u32(vk) {
        names.push(ch.to_string());
    }
    names.join("+")
}

/// 一条待注册热键的意图（解析后）：id + 设置来源 + 触发时要发的事件名。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotkeyBinding {
    pub id: i32,
    pub mods: u32,
    pub vk: u32,
    /// 触发时经 Tauri 事件发给前端的动作名（如 "palette" / "quick_paste"）。
    pub action: &'static str,
}

/// 重挂链的一步：Register 尝试注册；Disable 是「禁用」终态（不注册），
/// value 为该步落盘的设置值（回退到空/不可解析旧值时原样保存，对齐 Python）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChainStep {
    Register {
        value: String,
        binding: HotkeyBinding,
    },
    Disable {
        value: String,
    },
}

/// 一把热键的重挂链：依次尝试，首个注册成功者生效。
/// 语义移植 Python `_rehook_hotkey`（palette/quick：新值失败回退旧值）与
/// `_rehook_paste_hotkey`（面板：新值 → Ctrl+Alt+V 预设 → 禁用）。
#[derive(Debug, Clone)]
pub struct HotkeyChain {
    pub key: &'static str,
    pub id: i32,
    pub action: &'static str,
    /// 保存前请求的值（用于判断「是否发生回退」）。
    pub requested: String,
    pub steps: Vec<ChainStep>,
}

/// 链执行结果：value_used 为实际生效的设置值（回退/禁用后 ≠ requested）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainResult {
    pub key: &'static str,
    pub id: i32,
    pub requested: String,
    pub value_used: String,
    pub ok: bool,
}

/// 由设置文本构造绑定；文本为空（用户显式关闭）或非法 → None。
pub fn binding_for(id: i32, text: &str, action: &'static str) -> Option<HotkeyBinding> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let (mods, vk) = parse_hotkey(text)?;
    Some(HotkeyBinding {
        id,
        mods,
        vk,
        action,
    })
}

/// 三个热键的重挂链构造（设置键 → id → 动作），逐项对齐 palette.py。
///
/// `get` 读当前设置（构造尝试序列）；`previous` 读保存前旧值（回退步来源，
/// 生产由 sk_settings_set 捕获快照注入；启动路径传 get 本身 → 旧值==当前值
/// 自动去重，无回退步——对齐 Python 启动注册无回退语义）。
pub fn build_chains<F, P>(get: F, previous: P) -> Vec<HotkeyChain>
where
    F: Fn(&str) -> String,
    P: Fn(&str) -> String,
{
    let mut out = Vec::new();
    for (key, id, action) in [
        ("palette_hotkey", ID_PALETTE, "palette"),
        ("clip_quick_hotkey", ID_QUICK_PASTE, "quick_paste"),
    ] {
        let cur = get(key).trim().to_string();
        let mut steps = Vec::new();
        // 空串 = 显式禁用：早退、不回退（对齐 Python `if not text or setup: return`）
        if !cur.is_empty() {
            if let Some(b) = binding_for(id, &cur, action) {
                steps.push(ChainStep::Register {
                    value: cur.clone(),
                    binding: b,
                });
            }
            let prev = previous(key).trim().to_string();
            if prev != cur {
                // 回退步：旧值可解析则注册之；不可解析/为空则落盘旧值（禁用态）
                match binding_for(id, &prev, action) {
                    Some(b) => steps.push(ChainStep::Register {
                        value: prev.clone(),
                        binding: b,
                    }),
                    None => steps.push(ChainStep::Disable { value: prev }),
                }
            }
        }
        out.push(HotkeyChain {
            key,
            id,
            action,
            requested: cur,
            steps,
        });
    }
    // 面板键走专用链（_rehook_paste_hotkey 语义）：
    // 当前值 → Ctrl+Alt+V 预设（仅当快速键未占用该组合）→ 禁用
    {
        let key = "clip_paste_hotkey";
        let id = ID_PASTE_PANEL;
        let action = "paste_panel";
        let cur = get(key).trim().to_string();
        let mut steps = Vec::new();
        if !cur.is_empty() {
            if let Some(b) = binding_for(id, &cur, action) {
                steps.push(ChainStep::Register {
                    value: cur.clone(),
                    binding: b,
                });
            }
            let preset = "Ctrl+Alt+V";
            let quick = get("clip_quick_hotkey").trim().to_lowercase();
            if quick != preset.to_lowercase() {
                if let Some(b) = binding_for(id, preset, action) {
                    steps.push(ChainStep::Register {
                        value: preset.to_string(),
                        binding: b,
                    });
                }
            }
            steps.push(ChainStep::Disable {
                value: String::new(),
            });
        }
        out.push(HotkeyChain {
            key,
            id,
            action,
            requested: cur,
            steps,
        });
    }
    out
}

/// 消息泵线程句柄：保留用于进程退出前的反注册。
pub struct HotkeyHost {
    thread_id: u32,
    join: Option<thread::JoinHandle<()>>,
}

/// 泵线程读到的控制消息（自定义，避免与 WM_HOTKEY 冲突）。
const WM_SK_STOP: u32 = 0x8000 + 0x5359;

impl HotkeyHost {
    /// 起线程逐链注册并跑消息泵；`on_hotkey` 在泵线程被调用。
    ///
    /// 每条链按序尝试：Register 步失败换下一步，Disable 步直接视为「禁用」
    /// 生效；全部 Register 失败且无 Disable 步 → ok=false（调用方记日志）。
    /// 注册必须在本线程做（WM_HOTKEY 派发给注册线程）——回退也因此在本线程走。
    pub fn start_chains<F>(chains: Vec<HotkeyChain>, on_hotkey: F) -> (Self, Vec<ChainResult>)
    where
        F: Fn(i32, &'static str) + Send + 'static,
    {
        let (ready_tx, ready_rx) = mpsc::channel::<(u32, Vec<ChainResult>)>();
        let join = thread::spawn(move || {
            let thread_id = unsafe { GetCurrentThreadId() };
            let mut registered = Vec::new(); // (id, ok) 收尾反注册用
            let mut results = Vec::new();
            let mut table: Vec<(i32, &'static str)> = Vec::new();
            for chain in &chains {
                let mut outcome = ChainResult {
                    key: chain.key,
                    id: chain.id,
                    requested: chain.requested.clone(),
                    value_used: chain.requested.clone(),
                    ok: false,
                };
                if chain.steps.is_empty() {
                    // 空链 = 显式禁用（空串早退）：视为成功，维持空值
                    outcome.ok = true;
                    results.push(outcome);
                    continue;
                }
                for step in &chain.steps {
                    match step {
                        ChainStep::Register { value, binding } => {
                            let ok =
                                crate::w32::register_hotkey(binding.mods, binding.vk, binding.id);
                            if ok {
                                outcome.ok = true;
                                outcome.value_used = value.clone();
                                registered.push((binding.id, true));
                                table.push((binding.id, chain.action));
                                break;
                            }
                        }
                        ChainStep::Disable { value } => {
                            outcome.ok = true;
                            outcome.value_used = value.clone();
                            break;
                        }
                    }
                }
                results.push(outcome);
            }
            let _ = ready_tx.send((thread_id, results.clone()));
            let mut msg = MSG::default();
            loop {
                // 阻塞取消息；WM_SK_STOP 由 stop() 投递以唤醒
                let r = unsafe { GetMessageW(&mut msg, None, 0, 0) };
                if r.0 <= 0 {
                    break; // 0 = WM_QUIT，-1 = 错误
                }
                if msg.message == WM_SK_STOP {
                    break;
                }
                if msg.message == WM_HOTKEY {
                    let id = msg.wParam.0 as i32;
                    if let Some((_, action)) = table.iter().find(|(i, _)| *i == id) {
                        on_hotkey(id, action);
                    }
                }
                unsafe {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            for (id, ok) in &registered {
                if *ok {
                    crate::w32::unregister_hotkey(*id);
                }
            }
        });
        let (thread_id, results) = ready_rx.recv().unwrap_or((0, Vec::new()));
        (
            Self {
                thread_id,
                join: Some(join),
            },
            results,
        )
    }

    /// 请求退出并等待泵线程收尾（反注册在收尾里做）。
    pub fn stop(&mut self) {
        if self.thread_id != 0 {
            unsafe {
                use windows::Win32::Foundation::{LPARAM, WPARAM};
                let _ = windows::Win32::UI::WindowsAndMessaging::PostThreadMessageW(
                    self.thread_id,
                    WM_SK_STOP,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        }
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_matches_python_examples() {
        assert_eq!(parse_hotkey("Ctrl+Alt+K"), Some((0x0003, 0x4B)));
        assert_eq!(parse_hotkey("ctrl+win+v"), Some((0x000A, 0x56)));
        assert_eq!(parse_hotkey("Ctrl+Alt+V"), Some((0x0003, 0x56)));
        assert_eq!(parse_hotkey("Ctrl+Alt+Shift+Win+F12"), Some((0x000F, 0x7B)));
        assert_eq!(parse_hotkey(""), None, "空串无法解析");
        assert_eq!(parse_hotkey("Ctrl+Alt"), None, "至少要有一个主键");
        assert_eq!(parse_hotkey("Ctrl+Alt+今"), None, "非 ASCII 键不认");
        assert_eq!(parse_hotkey("Ctrl+Alt+F25"), None, "F24 以上不存在");
    }

    #[test]
    fn parse_ignores_empty_parts() {
        // Python 用 continue 跳过空片段
        assert_eq!(parse_hotkey("Ctrl++Alt++K"), Some((0x0003, 0x4B)));
        assert_eq!(parse_hotkey("  K  "), Some((0, 0x4B)));
    }

    #[test]
    fn last_key_wins_like_python() {
        // Python 循环里 key 会被后续片段覆盖
        assert_eq!(parse_hotkey("Ctrl+A+B"), Some((0x0002, 0x42)));
    }

    #[test]
    fn mods_exclude_norepeat() {
        let (mods, _) = parse_hotkey("Ctrl+Alt+K").unwrap();
        assert_eq!(
            mods & crate::w32::MOD_NOREPEAT,
            0,
            "NOREPEAT 由注册处恒置，不混进解析"
        );
    }

    #[test]
    fn display_normalizes() {
        assert_eq!(hotkey_display("ctrl+alt+k"), "Ctrl+Alt+K");
        assert_eq!(hotkey_display("Ctrl+Win+V"), "Ctrl+Win+V");
        assert_eq!(hotkey_display("Ctrl+Alt+F12"), "Ctrl+Alt+F12");
        assert_eq!(hotkey_display("Ctrl+Alt+`"), "Ctrl+Alt+`");
        assert_eq!(hotkey_display("Ctrl+Alt+oem_3"), "Ctrl+Alt+`");
        assert_eq!(hotkey_display("k"), "K");
        assert_eq!(hotkey_display(""), "", "非法返回空串");
        assert_eq!(hotkey_display("nonsense"), "", "未知键返回空串");
    }

    #[test]
    fn display_named_keys_use_first_match() {
        // esc/escape 同码，显示取先出现的 "Esc"
        assert_eq!(hotkey_display("Ctrl+Esc"), "Ctrl+Esc");
        assert_eq!(hotkey_display("Ctrl+Del"), "Ctrl+Del");
    }

    #[test]
    fn binding_for_respects_disabled_and_invalid() {
        assert!(
            binding_for(ID_PASTE_PANEL, "", "paste_panel").is_none(),
            "空串=用户显式关闭"
        );
        assert!(binding_for(ID_PASTE_PANEL, "   ", "paste_panel").is_none());
        assert!(binding_for(ID_PASTE_PANEL, "Ctrl+今", "paste_panel").is_none());
        let b = binding_for(ID_PASTE_PANEL, " Ctrl+Win+V ", "paste_panel").unwrap();
        assert_eq!(b.id, ID_PASTE_PANEL);
        assert_eq!(b.mods, 0x000A);
        assert_eq!(b.vk, 0x56);
    }

    #[test]
    fn chains_follow_python_defaults() {
        let get = |k: &str| -> String {
            match k {
                "palette_hotkey" => "Ctrl+Alt+K".into(),
                "clip_paste_hotkey" => "Ctrl+Win+V".into(),
                "clip_quick_hotkey" => "Ctrl+Alt+V".into(),
                _ => String::new(),
            }
        };
        let chains = build_chains(get, get);
        assert_eq!(chains.len(), 3);
        // 每条链首步 = Python 默认值的注册步（id/动作对齐）
        let first = |c: &HotkeyChain| match &c.steps[0] {
            ChainStep::Register { binding, .. } => (binding.id, binding.action),
            other => panic!("首步应为 Register，实际 {other:?}"),
        };
        assert_eq!(first(&chains[0]), (ID_PALETTE, "palette"));
        assert_eq!(first(&chains[1]), (ID_QUICK_PASTE, "quick_paste"));
        assert_eq!(first(&chains[2]), (ID_PASTE_PANEL, "paste_panel"));
    }

    #[test]
    fn ids_match_python() {
        assert_eq!((ID_PALETTE, ID_QUICK_PASTE, ID_PASTE_PANEL), (1, 2, 3));
    }
}

#[cfg(test)]
mod chain_tests {
    use super::*;

    /// 假设置：当前值表 + 旧值表（sk_settings_set 的 previous 快照口径）。
    fn fixtures(
        cur: &[(&str, &str)],
        prev: &[(&str, &str)],
    ) -> (impl Fn(&str) -> String, impl Fn(&str) -> String) {
        let cur: Vec<(String, String)> = cur
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let prev: Vec<(String, String)> = prev
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let c = move |k: &str| -> String {
            cur.iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        let p = move |k: &str| -> String {
            prev.iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        (c, p)
    }

    fn chain_of<'a>(chains: &'a [HotkeyChain], key: &str) -> &'a HotkeyChain {
        chains.iter().find(|c| c.key == key).unwrap()
    }

    #[test]
    fn palette_chain_reverts_to_previous_on_fallback() {
        let (cur, prev) = fixtures(
            &[("palette_hotkey", "Ctrl+Alt+H")],
            &[("palette_hotkey", "Ctrl+Alt+K")],
        );
        let chains = build_chains(cur, prev);
        let c = chain_of(&chains, "palette_hotkey");
        assert_eq!(c.requested, "Ctrl+Alt+H");
        assert_eq!(c.steps.len(), 2, "新值 + 旧值两个 Register 步");
        match &c.steps[1] {
            ChainStep::Register { value, .. } => assert_eq!(value, "Ctrl+Alt+K"),
            other => panic!("第二步应为 Register(旧值)，实际 {other:?}"),
        }
    }

    #[test]
    fn palette_chain_no_fallback_when_previous_equals_current() {
        // 启动路径 previous 传 get 本身 → 旧值==当前值 → 无回退步
        let (cur, prev) = fixtures(
            &[("palette_hotkey", "Ctrl+Alt+K")],
            &[("palette_hotkey", "Ctrl+Alt+K")],
        );
        let chains = build_chains(cur, prev);
        let c = chain_of(&chains, "palette_hotkey");
        assert_eq!(c.steps.len(), 1);
    }

    #[test]
    fn palette_chain_empty_current_yields_no_register() {
        // 空串 = 显式禁用：早退、不回退（对齐 Python `if not text or setup: return`）
        let (cur, prev) = fixtures(
            &[("palette_hotkey", "")],
            &[("palette_hotkey", "Ctrl+Alt+K")],
        );
        let chains = build_chains(cur, prev);
        let c = chain_of(&chains, "palette_hotkey");
        assert!(c.steps.is_empty(), "空当前值不得产生任何注册/回退步");
    }

    #[test]
    fn panel_chain_skips_preset_when_quick_occupies_it() {
        let (cur, prev) = fixtures(
            &[
                ("clip_paste_hotkey", "Ctrl+Win+V"),
                ("clip_quick_hotkey", "Ctrl+Alt+V"),
            ],
            &[("clip_paste_hotkey", "Ctrl+Win+V")],
        );
        let chains = build_chains(cur, prev);
        let c = chain_of(&chains, "clip_paste_hotkey");
        assert_eq!(c.steps.len(), 2, "当前值 + 禁用终态（预设被快速键占用）");
        assert!(matches!(c.steps[1], ChainStep::Disable { .. }));
    }

    #[test]
    fn panel_chain_includes_preset_then_disable() {
        let (cur, prev) = fixtures(
            &[
                ("clip_paste_hotkey", "Ctrl+Win+V"),
                ("clip_quick_hotkey", "Ctrl+Shift+V"),
            ],
            &[("clip_paste_hotkey", "Ctrl+Win+V")],
        );
        let chains = build_chains(cur, prev);
        let c = chain_of(&chains, "clip_paste_hotkey");
        assert_eq!(c.steps.len(), 3);
        match &c.steps[1] {
            ChainStep::Register { value, .. } => assert_eq!(value, "Ctrl+Alt+V"),
            other => panic!("第二步应为 Ctrl+Alt+V 预设，实际 {other:?}"),
        }
        assert!(matches!(c.steps[2], ChainStep::Disable { .. }));
    }

    #[test]
    fn panel_chain_empty_current_goes_straight_to_disable() {
        let (cur, prev) = fixtures(
            &[
                ("clip_paste_hotkey", ""),
                ("clip_quick_hotkey", "Ctrl+Shift+V"),
            ],
            &[("clip_paste_hotkey", "")],
        );
        let chains = build_chains(cur, prev);
        let c = chain_of(&chains, "clip_paste_hotkey");
        assert!(c.steps.is_empty(), "空当前值早退，不尝试预设/禁用步");
    }

    #[test]
    fn rehook_succeeds_after_stop_releases_registration() {
        // 回归（真机日志实证 2026-10-03）：apply_hotkeys 曾先起新泵线程注册、
        // 后停旧宿主——RegisterHotKey 全系统互斥，旧宿主还持着同组合时新注册
        // 必失败，把自己的键误报成"被占"（真机上 palette+quick 成对"全链注册
        // 失败"，重启才恢复）。本测试锁住底层语义：同一组合，stop() 阻塞返回
        // 后必须能立刻被再次注册。
        //
        // 组合特意用 Ctrl+Alt+Shift+F24：开发机上用户实例的真实组合
        // （Ctrl+Alt+V / Ctrl+Alt+K 等）正被运行中的实例持有，不能用于本测试。
        let combo = "Ctrl+Alt+Shift+F24";
        let make_chains = || {
            vec![HotkeyChain {
                key: "clip_quick_hotkey",
                id: ID_QUICK_PASTE,
                action: "quick_paste",
                requested: combo.to_string(),
                steps: vec![ChainStep::Register {
                    value: combo.to_string(),
                    binding: HotkeyBinding {
                        id: ID_QUICK_PASTE,
                        // MOD_ALT|MOD_CONTROL|MOD_SHIFT，VK_F24=0x87（几乎无人占用的键）
                        mods: 0x01 | 0x02 | 0x04,
                        vk: 0x87,
                        action: "quick_paste",
                    },
                }],
            }]
        };
        let (mut host_a, res_a) = HotkeyHost::start_chains(make_chains(), |_, _| {});
        assert!(res_a[0].ok, "首次注册应成功: {:?}", res_a[0]);
        host_a.stop();
        let (mut host_b, res_b) = HotkeyHost::start_chains(make_chains(), |_, _| {});
        assert!(
            res_b[0].ok,
            "stop 释放后同组合重挂必须成功（回归：误报被占）: {:?}",
            res_b[0]
        );
        host_b.stop();
    }

    #[test]
    fn chains_cover_three_keys_with_ids() {
        let (cur, prev) = fixtures(&[], &[]);
        let chains = build_chains(cur, prev);
        assert_eq!(chains.len(), 3);
        assert_eq!(chains[0].id, ID_PALETTE);
        assert_eq!(chains[1].id, ID_QUICK_PASTE);
        assert_eq!(chains[2].id, ID_PASTE_PANEL);
    }
}
