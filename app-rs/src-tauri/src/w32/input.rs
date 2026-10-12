//! 键盘注入 / 窗口焦点 / 热键注册（SendInput 与 foreground 家族）。
use super::*;

/// 键盘注入事件（dwExtraInfo 恒为 magic）。
fn key_event(vk: VIRTUAL_KEY, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up {
                    KEYEVENTF_KEYUP
                } else {
                    Default::default()
                },
                time: 0,
                dwExtraInfo: PASTE_MAGIC,
            },
        },
    }
}

/// 一次 Ctrl+V 的注入序列（纯函数，便于断言事件内容）。
pub fn ctrl_v_events() -> Vec<INPUT> {
    vec![
        key_event(VK_CONTROL, false),
        key_event(VK_V, false),
        key_event(VK_V, true),
        key_event(VK_CONTROL, true),
    ]
}

/// 释放物理按住的修饰键序列（顺序固定，纯函数）。
pub fn release_modifier_events() -> Vec<INPUT> {
    vec![
        key_event(VK_SHIFT, true),
        key_event(VK_CONTROL, true),
        key_event(VK_MENU, true),
        key_event(VK_LWIN, true),
        key_event(VK_RWIN, true),
    ]
}

/// 注入事件并断言全部送达（不足即失败，交调用方降级）。
pub fn send_input(events: &[INPUT]) -> bool {
    if events.is_empty() {
        return false;
    }
    let sent = unsafe { SendInput(events, std::mem::size_of::<INPUT>() as i32) };
    sent == events.len() as u32
}

/// 向当前焦点窗口注入一次 Ctrl+V。
pub fn send_ctrl_v() -> bool {
    send_input(&ctrl_v_events())
}

/// 释放物理按住的修饰键（热键触发时用户仍按着组合键）。
pub fn release_modifiers() -> bool {
    send_input(&release_modifier_events())
}

/// 当前前台窗口；无前台返回 None。
pub fn foreground_window() -> Option<HWND> {
    let h = unsafe { GetForegroundWindow() };
    (!h.0.is_null()).then_some(h)
}

/// 前台窗口的原始句柄值（跨 IPC 传递用；0 表示无）。
pub fn foreground_hwnd_value() -> isize {
    foreground_window().map_or(0, |h| h.0 as isize)
}

/// 前台窗口所属进程 id；无前台/查询失败返回 None。
pub fn foreground_process_id() -> Option<u32> {
    let h = foreground_window()?;
    let mut pid = 0u32;
    unsafe {
        let tid = GetWindowThreadProcessId(h, Some(&mut pid));
        if tid == 0 || pid == 0 {
            return None;
        }
    }
    Some(pid)
}

/// 前台窗口进程名（小写、去 .exe 后缀）；失败返回空串。
pub fn foreground_process_name() -> String {
    let Some(pid) = foreground_process_id() else {
        return String::new();
    };
    process_name(pid).unwrap_or_default()
}

/// 查询进程名的可执行文件部分（小写、去 .exe）。
pub fn process_name(pid: u32) -> Option<String> {
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let r = QueryFullProcessImageNameW(
            h,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = windows::Win32::Foundation::CloseHandle(h);
        r.ok()?;
        let full = String::from_utf16_lossy(&buf[..len as usize]);
        let base = full.rsplit(['\\', '/']).next().unwrap_or(&full);
        Some(base.trim_end_matches(".exe").to_ascii_lowercase())
    }
}

/// 由原始句柄值还原 HWND（IPC 回传）。
pub fn hwnd_from_value(v: isize) -> Option<HWND> {
    if v == 0 {
        None
    } else {
        Some(HWND(v as *mut std::ffi::c_void))
    }
}

/// 把焦点还给 hwnd（最小化先还原），返回其是否确实成为前台窗口。
/// 光标所在显示器的可用区域（工作区，扣任务栏）——多显示器下弹出面板
/// 应居中在**鼠标所在屏**（Python popup() 同语义；副屏呼出时面板钉主屏
/// 会"消失"，真机实锤）。
pub fn cursor_monitor_workarea() -> Option<(i32, i32, i32, i32)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut pt = POINT { x: 0, y: 0 };
    unsafe {
        if GetCursorPos(&mut pt).is_err() {
            return None;
        }
        let monitor = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        if monitor.is_invalid() {
            return None;
        }
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return None;
        }
        let r = info.rcWork;
        Some((r.left, r.top, r.right - r.left, r.bottom - r.top))
    }
}

pub fn focus_window(hwnd: HWND) -> bool {
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() {
            return false;
        }
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        if GetForegroundWindow() == hwnd {
            return true;
        }
        // 前台窗口可能属于另一线程：附着输入队列解除 SetForegroundWindow 限制
        let cur = GetCurrentThreadId();
        let tgt = GetWindowThreadProcessId(hwnd, None);
        let attached = cur != tgt && AttachThreadInput(cur, tgt, true).as_bool();
        let _ = BringWindowToTop(hwnd);
        let _ = SetForegroundWindow(hwnd);
        if attached {
            let _ = AttachThreadInput(cur, tgt, false);
        }
        GetForegroundWindow() == hwnd
    }
}

/// 注册全局热键（须在同一线程的消息泵上调用）；失败（被占用等）返回 false。
pub fn register_hotkey(mods: u32, vk: u32, id: i32) -> bool {
    unsafe { RegisterHotKey(null_hwnd(), id, HOT_KEY_MODIFIERS(mods | MOD_NOREPEAT), vk).is_ok() }
}

pub fn unregister_hotkey(id: i32) -> bool {
    unsafe { UnregisterHotKey(null_hwnd(), id).is_ok() }
}
