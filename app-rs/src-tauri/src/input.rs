//! 快捷粘贴闭环：焦点守卫 + SendInput 注键（对应 Python 侧 paste.py + clip_paste.py 的守卫链）。
//!
//! 守卫逻辑提为**纯函数**（`decide`），Win32 查询由调用方注入——这样焦点漂移、
//! 还焦复核、连击作废这些兜底分支都能被单元测试覆盖（兜底是一等公民，硬约束）。
//!
//! 时序硬约束：面板隐藏后 Windows 才把焦点还给原窗口，因此注键必须延后一拍
//! （Python 用 QTimer 120ms；此处用 Tauri 异步运行时，绝不阻塞 UI 线程）。

use serde::Serialize;

use crate::w32;

/// 还焦后复核前的等待（毫秒）：对应 Python 面板路径的 120ms。
pub const PASTE_DELAY_MS: u64 = 120;
/// 快速直贴路径的等待（毫秒）：对应 Python 快速链的 50ms。
pub const QUICK_DELAY_MS: u64 = 50;

/// 目标窗口、自身进程与第三方三种前台归属。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Foreground {
    /// 无前台窗口（切换桌面/锁屏过渡态）。
    None,
    /// 前台是 SysKit 自身（面板刚隐藏、主窗口持焦）。
    Own,
    /// 前台是第三方窗口（含目标窗口本身）。
    Other(isize),
}

/// 守卫决策（纯逻辑，可测）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// 序列已作废（连击/重呼出），静默放弃。
    Abandon,
    /// 未记录目标（托盘呼出）→ 提示手动粘贴。
    Manual,
    /// 前台已漂移到第三方 → 取消自动粘贴。
    Cancel,
    /// 先还焦目标窗口，再复核。
    FocusThenInject,
    /// 前台已是目标 → 直接注键。
    Inject,
}

/// 焦点守卫决策：绝不无条件抢焦点。
pub fn decide(target: isize, foreground: Foreground, seq_valid: bool) -> Decision {
    if !seq_valid {
        return Decision::Abandon;
    }
    if target == 0 {
        return Decision::Manual;
    }
    match foreground {
        // 前台是目标 → 直接注
        Foreground::Other(h) if h == target => Decision::Inject,
        // 前台是第三方 → 放弃（不抢焦点）
        Foreground::Other(_) => Decision::Cancel,
        // 前台为空或属自己 → 还焦后复核
        Foreground::None | Foreground::Own => Decision::FocusThenInject,
    }
}

/// 条目类型 → 降级文案前缀（图片/文件条目普通文本框接不住 Ctrl+V，必须点名）。
pub fn kind_word(kind: &str) -> &'static str {
    match kind {
        "image" => "图片",
        "file" => "文件",
        "html" => "富文本",
        _ => "",
    }
}

/// 降级提示文案（与 Python _KIND_WORD 拼接规则一致）。
pub fn failure_message(kind: &str) -> String {
    format!("{}已复制；自动粘贴未送达，请按 Ctrl+V", kind_word(kind))
}

pub fn manual_message(kind: &str) -> String {
    format!("{}已复制；切换窗口后按 Ctrl+V 粘贴", kind_word(kind))
}

pub const CANCEL_MESSAGE: &str = "目标窗口已切换，已取消自动粘贴（内容已在剪贴板）";
pub const SUCCESS_MESSAGE: &str = "已粘贴到原窗口";

/// 读当前前台归属（OpenProcess 失败/查不到进程名一律按第三方处理）。
pub fn current_foreground() -> Foreground {
    match w32::foreground_window() {
        None => Foreground::None,
        Some(h) => {
            let v = h.0 as isize;
            if is_own_foreground() {
                Foreground::Own
            } else {
                Foreground::Other(v)
            }
        }
    }
}

/// 前台窗口是否属于本进程。
pub fn is_own_foreground() -> bool {
    w32::foreground_process_id() == Some(std::process::id())
}

/// 注键结果（供前端/托盘提示）。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PasteOutcome {
    pub ok: bool,
    pub message: String,
}

/// 执行一次带守卫的粘贴（调用方负责延后一拍后调用）。
pub fn guarded_paste(target: isize, kind: &str, seq_valid: bool) -> PasteOutcome {
    match decide(target, current_foreground(), seq_valid) {
        Decision::Abandon => PasteOutcome {
            ok: false,
            message: String::new(),
        },
        Decision::Manual => PasteOutcome {
            ok: false,
            message: manual_message(kind),
        },
        Decision::Cancel => PasteOutcome {
            ok: false,
            message: CANCEL_MESSAGE.to_string(),
        },
        Decision::FocusThenInject => {
            if let Some(h) = w32::hwnd_from_value(target) {
                w32::focus_window(h);
            }
            // 还焦后复核：复核不过按第三方处理
            if w32::foreground_hwnd_value() == target {
                inject(kind)
            } else {
                PasteOutcome {
                    ok: false,
                    message: CANCEL_MESSAGE.to_string(),
                }
            }
        }
        Decision::Inject => inject(kind),
    }
}

/// 注键本体：先释放物理修饰键，再发 Ctrl+V，两步都断言计数。
fn inject(kind: &str) -> PasteOutcome {
    if w32::release_modifiers() && w32::send_ctrl_v() {
        PasteOutcome {
            ok: true,
            message: SUCCESS_MESSAGE.to_string(),
        }
    } else {
        PasteOutcome {
            ok: false,
            message: failure_message(kind),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seq_invalid_always_abandons() {
        for fg in [Foreground::None, Foreground::Own, Foreground::Other(7)] {
            assert_eq!(decide(7, fg, false), Decision::Abandon);
            assert_eq!(decide(0, fg, false), Decision::Abandon);
        }
    }

    #[test]
    fn no_target_asks_manual() {
        assert_eq!(decide(0, Foreground::None, true), Decision::Manual);
        assert_eq!(decide(0, Foreground::Own, true), Decision::Manual);
    }

    #[test]
    fn third_party_foreground_cancels() {
        // 前台是别的窗口（不是目标、也不是自己）→ 不抢焦点
        assert_eq!(decide(7, Foreground::Other(9), true), Decision::Cancel);
    }

    #[test]
    fn target_foreground_injects_directly() {
        assert_eq!(decide(7, Foreground::Other(7), true), Decision::Inject);
    }

    #[test]
    fn own_or_none_foreground_focuses_then_rechecks() {
        assert_eq!(decide(7, Foreground::Own, true), Decision::FocusThenInject);
        assert_eq!(decide(7, Foreground::None, true), Decision::FocusThenInject);
    }

    #[test]
    fn kind_words_match_python() {
        assert_eq!(kind_word("image"), "图片");
        assert_eq!(kind_word("file"), "文件");
        assert_eq!(kind_word("html"), "富文本");
        assert_eq!(kind_word("text"), "");
    }

    #[test]
    fn degradation_messages_name_the_kind() {
        assert_eq!(
            failure_message("image"),
            "图片已复制；自动粘贴未送达，请按 Ctrl+V"
        );
        assert_eq!(
            manual_message("file"),
            "文件已复制；切换窗口后按 Ctrl+V 粘贴"
        );
        assert_eq!(
            failure_message("text"),
            "已复制；自动粘贴未送达，请按 Ctrl+V"
        );
        assert!(CANCEL_MESSAGE.contains("已取消自动粘贴"));
    }

    #[test]
    fn delays_match_python_timers() {
        assert_eq!(PASTE_DELAY_MS, 120, "Python 面板路径 120ms");
        assert_eq!(QUICK_DELAY_MS, 50, "Python 快速链 50ms");
    }
}
