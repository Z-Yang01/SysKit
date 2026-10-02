//! Win32 原语薄封装：所有 FFI 只在本模块出现，其余模块只依赖这里的类型化函数。
//!
//! 迁移期硬约束（见 app-rs/README.md 的 M1 清单）：
//! - x64 句柄一律按指针宽度处理（windows-rs 的类型体系天然安全，不手写 int 转换）；
//! - SendInput 事件带 magic `0x53594B54`，返回值必须断言等于期望注入数；
//! - 修饰键释放顺序固定 SHIFT→CONTROL→MENU→LWIN→RWIN，一次调用；
//! - 剪贴板一律 UTF-16/原始字节写入，禁止任何 ANSI 窄字符路径。

use windows::core::HSTRING;
use windows::Win32::Foundation::{GlobalFree, LocalFree, HANDLE, HGLOBAL, HLOCAL, HWND};
use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, SendInput, UnregisterHotKey, HOT_KEY_MODIFIERS, INPUT, INPUT_0, INPUT_KEYBOARD,
    KEYBDINPUT, KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
    VK_V,
};
use windows::Win32::UI::Shell::DROPFILES;
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, IsIconic, IsWindow,
    SetForegroundWindow, ShowWindow, SW_RESTORE,
};

/// 注入标记："SYKT" —— 便于识别 SysKit 注入的输入（硬约束）。
pub const PASTE_MAGIC: usize = 0x5359_4B54;

pub const MOD_ALT: u32 = 0x0001;
pub const MOD_CONTROL: u32 = 0x0002;
pub const MOD_SHIFT: u32 = 0x0004;
pub const MOD_WIN: u32 = 0x0008;
/// 按住不重复触发（Win7+）；恒置（规格书硬约束）。
pub const MOD_NOREPEAT: u32 = 0x4000;

pub const VK_SHIFT_C: u32 = 0x10;
pub const VK_CONTROL_C: u32 = 0x11;
pub const VK_MENU_C: u32 = 0x12;
pub const VK_LWIN_C: u32 = 0x5B;
pub const VK_RWIN_C: u32 = 0x5C;
pub const VK_V_C: u32 = 0x56;

/// 剪贴板无主窗口（`RegisterHotKey(NULL, …)` / `OpenClipboard(NULL)` 同款）。
fn null_hwnd() -> Option<HWND> {
    Some(HWND(std::ptr::null_mut()))
}

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

/// 全局内存块：写入 bytes 后交给系统（SetClipboardData 成功即归系统所有）。
fn global_from_bytes(bytes: &[u8]) -> Option<HGLOBAL> {
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)).ok()?;
        let p = GlobalLock(h);
        if p.is_null() {
            let _ = GlobalFree(Some(h));
            return None;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p as *mut u8, bytes.len());
        let _ = GlobalUnlock(h);
        Some(h)
    }
}

/// 以「自定义剪贴板格式名」写入一块内存（CF_HTML / 三种排除格式用）。
pub fn clipboard_set_named(format_name: &str, bytes: &[u8]) -> bool {
    unsafe {
        if OpenClipboard(null_hwnd()).is_err() {
            return false;
        }
        let mut ok = false;
        let fid = RegisterClipboardFormatW(&HSTRING::from(format_name));
        if fid != 0 {
            if let Some(h) = global_from_bytes(bytes) {
                if SetClipboardData(fid, Some(HANDLE(h.0))).is_ok() {
                    ok = true;
                } else {
                    let _ = GlobalFree(Some(h));
                }
            }
        }
        let _ = CloseClipboard();
        ok
    }
}

/// 以标准格式号写入一块内存（CF_UNICODETEXT / CF_DIB / CF_HDROP 用）。
///
/// `bytes` 已是该格式要求的原始内存布局（含结尾 NUL / BITMAPINFOHEADER 等）。
pub fn clipboard_set_std(format: u32, bytes: &[u8]) -> bool {
    unsafe {
        if OpenClipboard(null_hwnd()).is_err() {
            return false;
        }
        let mut ok = false;
        if let Some(h) = global_from_bytes(bytes) {
            if SetClipboardData(format, Some(HANDLE(h.0))).is_ok() {
                ok = true;
            } else {
                let _ = windows::Win32::Foundation::GlobalFree(Some(h));
            }
        }
        let _ = CloseClipboard();
        ok
    }
}

/// 清空剪贴板（随后必须立刻写入新内容，避免开窗口期）。
pub fn clipboard_empty() -> bool {
    unsafe {
        if OpenClipboard(null_hwnd()).is_err() {
            return false;
        }
        let ok = EmptyClipboard().is_ok();
        let _ = CloseClipboard();
        ok
    }
}

/// CF_HDROP：DROPFILES 头 + UTF-16LE 双 NUL 结尾的路径列表。
///
/// 这是「粘贴文件本体」的唯一正确载荷——`setText(路径文本)` 到资源管理器
/// 只会粘出一段文字，不会粘文件（硬约束）。
pub fn build_hdrop(paths: &[String]) -> Vec<u8> {
    let header = DROPFILES {
        pFiles: std::mem::size_of::<DROPFILES>() as u32,
        pt: Default::default(),
        fNC: false.into(),
        fWide: true.into(),
    };
    let mut out = Vec::new();
    unsafe {
        let raw = std::slice::from_raw_parts(
            (&header as *const DROPFILES) as *const u8,
            std::mem::size_of::<DROPFILES>(),
        );
        out.extend_from_slice(raw);
    }
    for p in paths {
        for unit in p.encode_utf16() {
            out.extend_from_slice(&unit.to_le_bytes());
        }
        out.extend_from_slice(&0u16.to_le_bytes());
    }
    out.extend_from_slice(&0u16.to_le_bytes()); // 列表结束
    out
}

/// CF_UNICODETEXT：UTF-16LE + 结尾 NUL。
pub fn build_unicode_text(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for unit in text.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// 给「当前」剪贴板内容附上三种排除格式：不进 Win+V 历史、不上传云剪贴板。
///
/// 必须在写入内容之后调用；尽力而为，失败静默（老系统无此格式名）。
pub fn exclude_os_history() -> bool {
    let zeros = 0u32.to_le_bytes();
    let mut any = false;
    for name in [
        "ExcludeClipboardContentFromMonitorProcessing",
        "CanIncludeInClipboardHistory",
        "CanUploadToCloudClipboard",
    ] {
        any |= clipboard_set_named(name, &zeros);
    }
    any
}

/// DPAPI 密封（绑定当前用户）。失败返回 None（UI 线程静默降级）。
pub fn dpapi_protect(data: &[u8]) -> Option<Vec<u8>> {
    unsafe {
        let input = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut out = CRYPT_INTEGER_BLOB::default();
        CryptProtectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
        .ok()?;
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        // CryptProtectData 用 LocalAlloc 分配输出，必须由调用方 LocalFree
        let _ = LocalFree(Some(HLOCAL(out.pbData as *mut std::ffi::c_void)));
        Some(v)
    }
}

/// DPAPI 解密。失败（换用户/截断）返回 None。
pub fn dpapi_unprotect(data: &[u8]) -> Option<Vec<u8>> {
    unsafe {
        let input = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut out = CRYPT_INTEGER_BLOB::default();
        CryptUnprotectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )
        .ok()?;
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(out.pbData as *mut std::ffi::c_void)));
        Some(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_v_event_shape_matches_python() {
        let e = ctrl_v_events();
        assert_eq!(e.len(), 4, "Python: down Ctrl, down V, up V, up Ctrl");
        let vks: Vec<u32> = e
            .iter()
            .map(|i| unsafe { u32::from(i.Anonymous.ki.wVk.0) })
            .collect();
        assert_eq!(vks, vec![VK_CONTROL_C, VK_V_C, VK_V_C, VK_CONTROL_C]);
        let ups: Vec<bool> = e
            .iter()
            .map(|i| unsafe { i.Anonymous.ki.dwFlags.contains(KEYEVENTF_KEYUP) })
            .collect();
        assert_eq!(ups, vec![false, false, true, true]);
        for ev in &e {
            assert_eq!(unsafe { ev.Anonymous.ki.dwExtraInfo }, PASTE_MAGIC);
            assert_eq!(ev.r#type, INPUT_KEYBOARD);
        }
    }

    #[test]
    fn release_modifier_order_is_fixed() {
        let e = release_modifier_events();
        assert_eq!(e.len(), 5, "Python: SHIFT→CONTROL→MENU→LWIN→RWIN 各一次");
        let vks: Vec<u32> = e
            .iter()
            .map(|i| unsafe { u32::from(i.Anonymous.ki.wVk.0) })
            .collect();
        assert_eq!(
            vks,
            vec![VK_SHIFT_C, VK_CONTROL_C, VK_MENU_C, VK_LWIN_C, VK_RWIN_C],
            "顺序是硬约束：错序会把目标窗口的组合键判定弄乱"
        );
        for ev in &e {
            assert!(unsafe { ev.Anonymous.ki.dwFlags.contains(KEYEVENTF_KEYUP) });
            assert_eq!(unsafe { ev.Anonymous.ki.dwExtraInfo }, PASTE_MAGIC);
        }
    }

    #[test]
    fn magic_is_sykt() {
        assert_eq!(PASTE_MAGIC, 0x53594B54);
        // 低 4 字节按大端读出来就是 "SYKT"
        let be32 = (PASTE_MAGIC as u32).to_be_bytes();
        assert_eq!(String::from_utf8_lossy(&be32), "SYKT");
    }

    #[test]
    fn hdrop_layout_is_dropfiles_then_wide_paths() {
        let bytes = build_hdrop(&["C:\\a.txt".to_string(), "D:\\b c.txt".to_string()]);
        let header_len = std::mem::size_of::<DROPFILES>();
        assert_eq!(header_len, 20, "DROPFILES 在 x64 下应打包为 20 字节");
        let p_files = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        assert_eq!(p_files as usize, header_len, "pFiles 必须等于结构体大小");
        let f_wide = u32::from_le_bytes(bytes[16..20].try_into().unwrap());
        assert_eq!(f_wide, 1, "fWide 必须为 TRUE（UTF-16 路径）");
        // 路径区解码后应得到两个路径，且以双 NUL 结束
        let utf16: Vec<u16> = bytes[header_len..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        assert_eq!(&utf16[utf16.len() - 2..], &[0, 0], "列表必须以双 NUL 结束");
        assert_eq!(String::from_utf16(&utf16[..9]).unwrap(), "C:\\a.txt\u{0}");
    }

    #[test]
    fn unicode_text_is_wide_and_nul_terminated() {
        let b = build_unicode_text("中文");
        assert_eq!(b.len(), 6, "2 个 BMP 字符 + 结尾 NUL，各 2 字节");
        assert_eq!(&b[4..6], &[0, 0]);
        assert_eq!(u16::from_le_bytes([b[0], b[1]]), '中' as u16);
    }

    #[test]
    fn hwnd_value_round_trip() {
        assert_eq!(hwnd_from_value(0), None);
        // 用真实存在的窗口句柄做往返（伪造的非规范地址在 Rust 严格来源模型下是 UB）
        let h = foreground_window();
        if let Some(h) = h {
            assert_eq!(hwnd_from_value(h.0 as isize), Some(h));
        }
        assert!(hwnd_from_value(-1).is_some(), "非 0 即应还原出句柄");
    }

    #[test]
    fn send_input_rejects_empty() {
        assert!(!send_input(&[]), "空序列必须失败而不是假成功");
    }
}
