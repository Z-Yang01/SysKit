//! Win32 原语薄封装：所有 FFI 只在本模块出现，其余模块只依赖这里的类型化函数。
//!
//! 迁移期硬约束（见 app-rs/README.md 的 M1 清单）：
//! - x64 句柄一律按指针宽度处理（windows-rs 的类型体系天然安全，不手写 int 转换）；
//! - SendInput 事件带 magic `0x53594B54`，返回值必须断言等于期望注入数；
//! - 修饰键释放顺序固定 SHIFT→CONTROL→MENU→LWIN→RWIN，一次调用；
//! - 剪贴板一律 UTF-16/原始字节写入，禁止任何 ANSI 窄字符路径。

use std::collections::BTreeMap;
use windows::core::HSTRING;
use windows::Win32::Foundation::{GlobalFree, LocalFree, HANDLE, HGLOBAL, HLOCAL, HWND};
use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};

use windows::Win32::System::Registry::REG_VALUE_TYPE;
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
        // CryptUnprotectData 同样用 LocalAlloc 分配输出，必须由调用方 LocalFree
        let _ = LocalFree(Some(HLOCAL(out.pbData as *mut std::ffi::c_void)));
        Some(v)
    }
}

// ---------------- 子进程执行（修复动作的底座） ----------------

/// 子进程执行结果（对应 Python `proc.run` 的返回）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcOutput {
    pub ok: bool,
    pub code: u32,
    pub stdout: String,
    pub stderr: String,
}

impl ProcOutput {
    /// `stdout + stderr` 合并（Python 侧多处这样拼接做关键字判定）。
    pub fn combined(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }

    /// 尾部截断（Python 有 `r.stdout[-200:] + r.stderr[-100:]` 这类写法）。
    pub fn tail(&self, out_n: usize, err_n: usize) -> String {
        format!(
            "{}{}",
            tail_chars(&self.stdout, out_n),
            tail_chars(&self.stderr, err_n)
        )
    }
}

fn tail_chars(s: &str, n: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= n {
        return s.to_string();
    }
    chars[chars.len() - n..].iter().collect()
}

/// 按 Windows 命令行规则给单个参数加引号（含空格/制表/引号时必须）。
///
/// 规则同 Python `subprocess.list2cmdline`：反斜杠只在引号前成对转义，
/// 参数末尾的反斜杠在加引号时要翻倍。
pub fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_string();
    }
    let mut out = String::with_capacity(arg.len() + 2);
    out.push('"');
    let mut backslashes = 0usize;
    for ch in arg.chars() {
        match ch {
            '\\' => {
                backslashes += 1;
                out.push(ch);
            }
            '"' => {
                out.push_str(&"\\".repeat(backslashes));
                out.push_str("\\\"");
                backslashes = 0;
            }
            _ => {
                backslashes = 0;
                out.push(ch);
            }
        }
    }
    out.push_str(&"\\".repeat(backslashes));
    out.push('"');
    out
}

/// 把参数表拼成命令行。
pub fn build_command_line(args: &[String]) -> String {
    args.iter()
        .map(|a| quote_arg(a))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 把命令名解析成绝对路径。
///
/// **为什么必须自己做**：一旦给 `CreateProcessW` 传了 `lpApplicationName`，
/// Windows 就**不再搜索 PATH**（只搜索当前目录）——`cmd`、`ipconfig`、`netsh`
/// 这些裸名字全都会报「系统找不到指定的文件」。Python 的 subprocess 会自己解析
/// PATH，所以这是移植时必须补上的一层。
///
/// 解析顺序：含路径分隔符 → 原样返回；否则按 PATHEXT 在 PATH 各目录里找。
pub fn resolve_executable(name: &str) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    if name.contains('\\') || name.contains('/') {
        let p = std::path::Path::new(name);
        return p.is_file().then(|| name.to_string());
    }
    // PATHEXT 默认值（Windows 未设 PATHEXT 时也要能用）
    let exts: Vec<String> = std::env::var("PATHEXT")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|s| {
            s.split(';')
                .map(|e| e.trim().to_ascii_lowercase())
                .filter(|e| !e.is_empty())
                .collect()
        })
        .unwrap_or_else(|| vec![".com".into(), ".exe".into(), ".bat".into(), ".cmd".into()]);
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        // 已经带扩展名的先直接试
        let direct = dir.join(name);
        if direct.is_file() {
            return Some(direct.to_string_lossy().to_string());
        }
        for ext in &exts {
            let cand = dir.join(format!("{name}{ext}"));
            if cand.is_file() {
                return Some(cand.to_string_lossy().to_string());
            }
        }
    }
    None
}

/// 执行一个可执行文件并捕获输出。
///
/// **为什么重定向到临时文件而不是管道**：管道要求父进程继承/读取句柄，在受限
/// 沙箱下会被拒（与 `spawn EPERM` 同源）；文件重定向只依赖 CreateFile，
/// 任何模式下都成立。代价是一次临时文件往返——修复动作都是秒级操作，可接受。
///
/// 读取注册表值（字符串 / DWORD / 多字符串 / 二进制）；键或值不存在返回 None。
///
/// 对应 Python `winregread.value(path, name, default)`。路径形如
/// `HKLM\SYSTEM\CurrentControlSet\Services\NlaSvc\Parameters\Internet`。
pub fn reg_value(path: &str, name: &str) -> Option<RegValue> {
    let hkey = reg_open(path)?;
    let out = reg_query(hkey, name);
    unsafe {
        let _ = windows::Win32::System::Registry::RegCloseKey(hkey);
    }
    out
}

/// 枚举一个键下的全部值（对应 Python `winregread.values(path)`）；失败返回空表。
/// 写 DWORD 值（对应 Python `winreg.SetValueEx(..., REG_DWORD, v)`）。
///
/// 需要 `KEY_SET_VALUE`：HKCU 通常无需提升，HKLM 需管理员——调用方负责判定，
/// 本层只返回成败，绝不静默吞掉失败。
pub fn reg_set_dword(path: &str, name: &str, value: u32) -> bool {
    let Some(hkey) = reg_open_write(path) else {
        return false;
    };
    let data = value.to_le_bytes();
    let rc = unsafe {
        windows::Win32::System::Registry::RegSetValueExW(
            hkey,
            &HSTRING::from(name),
            None,
            windows::Win32::System::Registry::REG_DWORD,
            Some(&data),
        )
    };
    unsafe {
        let _ = windows::Win32::System::Registry::RegCloseKey(hkey);
    }
    rc == windows::Win32::Foundation::WIN32_ERROR(0)
}

/// 写字符串值（REG_SZ）。
pub fn reg_set_string(path: &str, name: &str, value: &str) -> bool {
    let Some(hkey) = reg_open_write(path) else {
        return false;
    };
    let mut wide: Vec<u16> = value.encode_utf16().collect();
    wide.push(0);
    let data: Vec<u8> = wide.iter().flat_map(|u| u.to_le_bytes()).collect();
    let rc = unsafe {
        windows::Win32::System::Registry::RegSetValueExW(
            hkey,
            &HSTRING::from(name),
            None,
            windows::Win32::System::Registry::REG_SZ,
            Some(&data),
        )
    };
    unsafe {
        let _ = windows::Win32::System::Registry::RegCloseKey(hkey);
    }
    rc == windows::Win32::Foundation::WIN32_ERROR(0)
}

/// 写「以值名为规格」的注册表项（持久路由用：值名就是 `目标,掩码,网关,跃点`）。
///
/// 与 `reg_set_string` 的区别在于**先删同名旧值再写**：改路由时规格串本身是键名，
/// 不先删会留下旧路由（两条并存 → 依然劫持流量）。
pub fn reg_write_named_value(path: &str, name: &str, data: &str) -> bool {
    let _ = reg_delete_value(path, name);
    reg_set_string(path, name, data)
}

/// 通知 WinINET 代理设置已变更（对应 Python `wininet.InternetSetOptionW`）。
///
/// 39 = INTERNET_OPTION_SETTINGS_CHANGED，37 = INTERNET_OPTION_REFRESH。
/// 不刷新的话，已运行的应用仍用旧代理配置——用户会以为"改了没生效"。
pub fn notify_internet_settings_changed() -> bool {
    use windows::Win32::Networking::WinInet::InternetSetOptionW;
    unsafe {
        let a = InternetSetOptionW(None, 39, None, 0).is_ok();
        let b = InternetSetOptionW(None, 37, None, 0).is_ok();
        a && b
    }
}

/// 删除值。Ok(true) = 已删除，Ok(false) = 值本来就不存在，Err = 真失败。
///
/// **区分这三者很重要**：清 PAC 时「本来就没有」不是失败，而权限不足是失败。
pub fn reg_delete_value(path: &str, name: &str) -> Result<bool, String> {
    use windows::Win32::Foundation::WIN32_ERROR;
    const ERROR_FILE_NOT_FOUND: WIN32_ERROR = WIN32_ERROR(2);
    const ERROR_PATH_NOT_FOUND: WIN32_ERROR = WIN32_ERROR(3);
    let Some(hkey) = reg_open_write(path) else {
        return Err(format!("无法以写权限打开注册表键: {path}"));
    };
    let rc =
        unsafe { windows::Win32::System::Registry::RegDeleteValueW(hkey, &HSTRING::from(name)) };
    unsafe {
        let _ = windows::Win32::System::Registry::RegCloseKey(hkey);
    }
    if rc == WIN32_ERROR(0) {
        Ok(true)
    } else if rc == ERROR_FILE_NOT_FOUND || rc == ERROR_PATH_NOT_FOUND {
        Ok(false)
    } else {
        Err(format!(
            "删除注册表值失败 {path}\\{name}（错误码 {}）",
            rc.0
        ))
    }
}

/// 以写权限打开键（读用 `reg_open`）。
fn reg_open_write(path: &str) -> Option<windows::Win32::System::Registry::HKEY> {
    use windows::Win32::System::Registry::{
        RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_SET_VALUE, KEY_WOW64_64KEY,
    };
    let (root, sub) = split_reg_path(path)?;
    let hkey = match root {
        "HKLM" | "HKEY_LOCAL_MACHINE" => HKEY_LOCAL_MACHINE,
        "HKCU" | "HKEY_CURRENT_USER" => HKEY_CURRENT_USER,
        _ => return None,
    };
    let mut out = HKEY(std::ptr::null_mut());
    let rc = unsafe {
        RegOpenKeyExW(
            hkey,
            &HSTRING::from(sub),
            Some(0),
            KEY_SET_VALUE | KEY_WOW64_64KEY,
            &mut out,
        )
    };
    (rc == windows::Win32::Foundation::WIN32_ERROR(0)).then_some(out)
}

pub fn reg_values(path: &str) -> BTreeMap<String, RegValue> {
    use windows::Win32::System::Registry::{RegEnumValueW, RegQueryInfoKeyW};
    let mut out = BTreeMap::new();
    let Some(hkey) = reg_open(path) else {
        return out;
    };
    unsafe {
        let mut subkeys = 0u32;
        let mut values = 0u32;
        let mut max_name = 0u32;
        let mut max_data = 0u32;
        if RegQueryInfoKeyW(
            hkey,
            None,
            None,
            None,
            Some(&mut subkeys),
            None,
            None,
            Some(&mut values),
            Some(&mut max_name),
            Some(&mut max_data),
            None,
            None,
        ) != windows::Win32::Foundation::WIN32_ERROR(0)
        {
            let _ = windows::Win32::System::Registry::RegCloseKey(hkey);
            return out;
        }
        let name_cap = (max_name + 2) as usize;
        let data_cap = (max_data + 8) as usize;
        for i in 0..values {
            let mut name_buf = vec![0u16; name_cap];
            let mut name_len = name_cap as u32;
            let mut ty = REG_VALUE_TYPE(0);
            let mut data = vec![0u8; data_cap];
            let mut data_len = data_cap as u32;
            let rc = RegEnumValueW(
                hkey,
                i,
                Some(windows::core::PWSTR(name_buf.as_mut_ptr())),
                &mut name_len,
                None,
                Some(&mut ty as *mut REG_VALUE_TYPE as *mut u32),
                Some(data.as_mut_ptr()),
                Some(&mut data_len),
            );
            if rc != windows::Win32::Foundation::WIN32_ERROR(0) {
                continue;
            }
            let key = String::from_utf16_lossy(&name_buf[..name_len as usize]);
            if let Some(v) = decode_reg_value(ty, &data[..data_len as usize]) {
                out.insert(key, v);
            }
        }
        let _ = windows::Win32::System::Registry::RegCloseKey(hkey);
    }
    out
}

/// 注册表值（保留类型，便于与默认值做字符串比对——Python 侧也是 `str(got) != str(want)`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegValue {
    Dword(u32),
    Str(String),
    MultiStr(Vec<String>),
    Binary(Vec<u8>),
}

impl RegValue {
    /// 与 Python `str(value)` 等价的展示形态（比对默认值时用）。
    pub fn as_compare_str(&self) -> String {
        match self {
            RegValue::Dword(n) => n.to_string(),
            RegValue::Str(s) => s.clone(),
            RegValue::MultiStr(v) => v.join(","),
            RegValue::Binary(b) => b.iter().map(|x| format!("{x:02x}")).collect(),
        }
    }

    pub fn as_u32(&self) -> Option<u32> {
        match self {
            RegValue::Dword(n) => Some(*n),
            RegValue::Str(s) => s.trim().parse::<u32>().ok(),
            _ => None,
        }
    }
}

fn reg_open(path: &str) -> Option<windows::Win32::System::Registry::HKEY> {
    use windows::Win32::System::Registry::{
        RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY,
    };
    let (root, sub) = split_reg_path(path)?;
    let hkey = match root {
        "HKLM" | "HKEY_LOCAL_MACHINE" => HKEY_LOCAL_MACHINE,
        "HKCU" | "HKEY_CURRENT_USER" => HKEY_CURRENT_USER,
        _ => return None,
    };
    let mut out = HKEY(std::ptr::null_mut());
    // WOW64_64KEY：SysKit 是 64 位进程，显式读 64 位视图，避免被重定向到 Wow6432Node
    let rc = unsafe {
        RegOpenKeyExW(
            hkey,
            &HSTRING::from(sub),
            Some(0),
            KEY_READ | KEY_WOW64_64KEY,
            &mut out,
        )
    };
    (rc == windows::Win32::Foundation::WIN32_ERROR(0)).then_some(out)
}

fn split_reg_path(path: &str) -> Option<(&str, &str)> {
    let mut it = path.splitn(2, '\\');
    let root = it.next()?;
    let sub = it.next().unwrap_or("");
    (!root.is_empty()).then_some((root, sub))
}

fn reg_query(hkey: windows::Win32::System::Registry::HKEY, name: &str) -> Option<RegValue> {
    use windows::Win32::System::Registry::{RegQueryValueExW, REG_VALUE_TYPE};
    let mut ty = REG_VALUE_TYPE(0);
    let mut len = 0u32;
    let name_h = HSTRING::from(name);
    let rc = unsafe { RegQueryValueExW(hkey, &name_h, None, Some(&mut ty), None, Some(&mut len)) };
    if rc != windows::Win32::Foundation::WIN32_ERROR(0) || len == 0 {
        // 长度为 0 的合法值（空字符串）也要能取到：再走一次带缓冲的调用
        let mut buf = [0u8; 4];
        let mut l = buf.len() as u32;
        let rc2 = unsafe {
            RegQueryValueExW(
                hkey,
                &name_h,
                None,
                Some(&mut ty),
                Some(buf.as_mut_ptr()),
                Some(&mut l),
            )
        };
        if rc2 != windows::Win32::Foundation::WIN32_ERROR(0) {
            return None;
        }
        return decode_reg_value(ty, &buf[..l as usize]);
    }
    let mut buf = vec![0u8; len as usize + 2];
    let mut l = buf.len() as u32;
    let rc2 = unsafe {
        RegQueryValueExW(
            hkey,
            &name_h,
            None,
            Some(&mut ty),
            Some(buf.as_mut_ptr()),
            Some(&mut l),
        )
    };
    if rc2 != windows::Win32::Foundation::WIN32_ERROR(0) {
        return None;
    }
    decode_reg_value(ty, &buf[..l as usize])
}

fn decode_reg_value(
    ty: windows::Win32::System::Registry::REG_VALUE_TYPE,
    data: &[u8],
) -> Option<RegValue> {
    use windows::Win32::System::Registry::{REG_DWORD, REG_EXPAND_SZ, REG_MULTI_SZ, REG_SZ};
    if ty == REG_DWORD {
        if data.len() < 4 {
            return None;
        }
        return Some(RegValue::Dword(u32::from_le_bytes([
            data[0], data[1], data[2], data[3],
        ])));
    }
    if ty == REG_SZ || ty == REG_EXPAND_SZ {
        return Some(RegValue::Str(utf16_bytes_to_string(data)));
    }
    if ty == REG_MULTI_SZ {
        let units: Vec<u16> = data
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let mut items = Vec::new();
        let mut cur = Vec::new();
        for u in units {
            if u == 0 {
                if cur.is_empty() {
                    break;
                }
                items.push(String::from_utf16_lossy(&cur));
                cur.clear();
            } else {
                cur.push(u);
            }
        }
        return Some(RegValue::MultiStr(items));
    }
    Some(RegValue::Binary(data.to_vec()))
}

fn utf16_bytes_to_string(data: &[u8]) -> String {
    let units: Vec<u16> = data
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let end = units.iter().position(|c| *c == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

/// 当前进程是否以管理员（提升）身份运行。
///
/// 对应 Python `admin.is_admin()`：取令牌的 TokenElevation 判断。修复动作里
/// 「改配置类」在非管理员下必须跳过或明确失败，绝不能假装成功。
pub fn is_elevated() -> bool {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token = HANDLE(std::ptr::null_mut());
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut ret_len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut core::ffi::c_void),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut ret_len,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

/// 失败时返回人话原因（修复页要把它显示给用户，不能只报「失败了」）。
pub fn run_capture(args: &[String], timeout_secs: u64) -> Result<ProcOutput, String> {
    use std::process::{Command, Stdio};

    if args.is_empty() {
        return Err("空命令".to_string());
    }
    let resolved = resolve_executable(&args[0])
        .ok_or_else(|| format!("找不到可执行文件: {}（不在 PATH 中）", args[0]))?;

    let mut cmd = Command::new(&resolved);
    cmd.args(&args[1..]);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    // 不弹控制台黑窗（对应 Python 的 startupinfo 隐藏窗口）
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("无法启动 {}: {e}", args[0]))?;

    // 管道必须**边等边抽**（读线程）：子进程输出一旦超过管道缓冲就会阻塞在
    // write 上——「先等退出再读管道」的顺序会让它永远退不出，只能被超时 kill，
    // 修复动作被误报成失败。两个管道各挂一个读线程读到 EOF，等待循环只看
    // 进程状态；超时 kill 后管道关闭、线程自然收尾，无双死锁。
    // （std 的 wait_with_output 也能并发抽干，但它不带超时——这里需要超时 kill。）
    let mut stdout_pipe = child.stdout.take().expect("stdout 已声明 piped");
    let mut stderr_pipe = child.stderr.take().expect("stderr 已声明 piped");
    let stdout_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = std::io::Read::read_to_end(&mut stdout_pipe, &mut buf);
        buf
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = std::io::Read::read_to_end(&mut stderr_pipe, &mut buf);
        buf
    });

    // 超时：std 没有 wait_timeout，轮询 try_wait 最省事且不引额外依赖。
    // 到点先 kill 再收尸，绝不留下孤儿进程（修复动作里有 90 秒级的 ipconfig /renew）。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs.max(1));
    let mut timed_out = false;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(e) => return Err(format!("等待 {} 失败: {e}", args[0])),
        }
    }

    // 读线程在管道 EOF（正常退出或 kill 关句柄）后必然结束；join 只做收集
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    let status = child
        .wait()
        .map_err(|e| format!("读取 {} 状态失败: {e}", args[0]))?;

    let code = status.code().unwrap_or(-1);
    Ok(ProcOutput {
        ok: !timed_out && status.success(),
        code: if timed_out {
            u32::MAX
        } else {
            code.max(0) as u32
        },
        stdout: decode_console_bytes(&stdout),
        stderr: decode_console_bytes(&stderr),
    })
}

/// 控制台输出解码：先试 UTF-8，失败退化为逐字节（保 ASCII 可读，绝不 panic）。
///
/// 中文 Windows 控制台默认 GBK，完整码表需额外依赖；这里保证「修复杂志不出现
/// 替换字符乱码致无法判断成败」这一底线，同时 ASCII 部分（含错误码）始终可读。
fn decode_console_bytes(raw: &[u8]) -> String {
    if raw.is_empty() {
        return String::new();
    }
    match std::str::from_utf8(raw) {
        Ok(s) => s.to_string(),
        Err(_) => raw.iter().map(|b| *b as char).collect::<String>(),
    }
}

// ---------------- 原生文件选择器 ----------------

/// 弹出系统「打开文件」对话框，返回选中路径（取消返回空串）。
///
/// 用 `GetOpenFileNameW` 而不是 `tauri-plugin-dialog`：只为一个选择器引入一个插件
/// （前端包 + 权限配置 + IPC 命令）不划算，而这个 API 自 Win95 起就没变过。
///
/// `filter` 是 Win32 的双 NUL 结尾格式：`"名称\0*.exe\0所有文件\0*.*\0"`。
pub fn pick_file_dialog(title: &str, filter: &str) -> Result<String, String> {
    use windows::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };

    // 缓冲区要放得下长路径（MAX_PATH 在长路径启用的机器上不够用）
    let mut buf = [0u16; 1024];
    // filter 必须是「双 NUL 结尾」，即最后两个 u16 都是 0——调用方少写一个 NUL
    // 就会读到越界内存。这里统一补齐，不依赖调用方（已用测试锁住）。
    let mut filter_u16: Vec<u16> = filter.encode_utf16().collect();
    while filter_u16.len() < 2 || filter_u16[filter_u16.len() - 2..] != [0, 0] {
        filter_u16.push(0);
    }
    let title_u16: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let def_ext: Vec<u16> = "exe\0".encode_utf16().collect();

    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: HWND(std::ptr::null_mut()),
        lpstrFilter: windows::core::PCWSTR(filter_u16.as_ptr()),
        nFilterIndex: 1,
        lpstrFile: windows::core::PWSTR(buf.as_mut_ptr()),
        nMaxFile: buf.len() as u32,
        lpstrTitle: windows::core::PCWSTR(title_u16.as_ptr()),
        lpstrDefExt: windows::core::PCWSTR(def_ext.as_ptr()),
        // NOCHANGEDIR：对话框会改进程当前目录，而 `real_path` 依赖 cwd 解析相对路径
        Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };

    // SAFETY: ofn 的所有指针都指向本函数栈上、生命周期覆盖该调用的缓冲
    let ok = unsafe { GetOpenFileNameW(&mut ofn) };
    if !ok.as_bool() {
        // 用户取消（CommDlgExtendedError 为 0）与真失败都走这里；返回空串让前端保持安静
        return Ok(String::new());
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Ok(String::from_utf16_lossy(&buf[..len]))
}

// ---------------- 用系统关联打开文件/协议 ----------------

/// 弹出系统「另存为」对话框，返回目标路径（取消返回空串）。
///
/// 与 [`pick_file_dialog`] 同一套实现要点：双 NUL 结尾的 filter、`OFN_NOCHANGEDIR`
/// （对话框会改进程 cwd，而 `real_path` 依赖 cwd 解析相对路径）。
pub fn save_file_dialog(title: &str, default_name: &str, filter: &str) -> Result<String, String> {
    use windows::Win32::UI::Controls::Dialogs::{
        GetSaveFileNameW, OFN_NOCHANGEDIR, OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };

    let mut buf = [0u16; 1024];
    // 预填默认文件名（含扩展名），对话框会把它放进"文件名"框
    let default_u16: Vec<u16> = default_name.encode_utf16().collect();
    let n = default_u16.len().min(buf.len() - 1);
    buf[..n].copy_from_slice(&default_u16[..n]);

    let mut filter_u16: Vec<u16> = filter.encode_utf16().collect();
    while filter_u16.len() < 2 || filter_u16[filter_u16.len() - 2..] != [0, 0] {
        filter_u16.push(0);
    }
    let title_u16: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();

    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: HWND(std::ptr::null_mut()),
        lpstrFilter: windows::core::PCWSTR(filter_u16.as_ptr()),
        nFilterIndex: 1,
        lpstrFile: windows::core::PWSTR(buf.as_mut_ptr()),
        nMaxFile: buf.len() as u32,
        lpstrTitle: windows::core::PCWSTR(title_u16.as_ptr()),
        // OVERWRITEPROMPT：覆盖前问一次，避免用户误覆盖已有文件
        Flags: OFN_PATHMUSTEXIST | OFN_OVERWRITEPROMPT | OFN_NOCHANGEDIR,
        ..Default::default()
    };

    // SAFETY: ofn 的所有指针都指向本函数栈上、生命周期覆盖该调用的缓冲
    let ok = unsafe { GetSaveFileNameW(&mut ofn) };
    if !ok.as_bool() {
        return Ok(String::new()); // 用户取消
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Ok(String::from_utf16_lossy(&buf[..len]))
}
/// 用系统默认关联打开目标（`ShellExecuteW`）。
///
/// 用 `ShellExecuteW` 而不是 `std::process::Command`：它能处理 `ms-settings:` 这类
/// **协议**与文件夹，`Command` 只能起可执行文件。Python 版用 `os.startfile`，
/// 底层就是这个 API。
pub fn shell_open(target: &str) -> Result<(), String> {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let verb: Vec<u16> = "open\0".encode_utf16().collect();
    let file: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: 两个宽字符串都在本调用内有效且以 NUL 结尾
    let ret = unsafe {
        ShellExecuteW(
            None,
            windows::core::PCWSTR(verb.as_ptr()),
            windows::core::PCWSTR(file.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW 的返回值 >32 表示成功、<=32 是错误码——这是该 API 的历史约定，
    // 它**不是**可用句柄（拿它当句柄会让错误码被当成有效值）。
    let code = ret.0 as isize;
    if code > 32 {
        Ok(())
    } else {
        Err(format!("ShellExecute 失败（错误码 {code}）"))
    }
}

// ---------------- 随机数 / PBKDF2 / AES-GCM（BCrypt） ----------------

/// 生成 `n` 字节密码学随机数（`BCryptGenRandom`，系统 CSPRNG）。
///
/// 不用时间戳或自建 PRNG：数据密钥与 nonce 的随机性直接决定安全性。
pub fn random_bytes(n: usize) -> Vec<u8> {
    use windows::Win32::Security::Cryptography::{
        BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
    };
    let mut buf = vec![0u8; n];
    // SAFETY: buf 是本地 Vec，长度与传入的 n 一致
    let st = unsafe { BCryptGenRandom(None, &mut buf, BCRYPT_USE_SYSTEM_PREFERRED_RNG) };
    if st.is_err() {
        // 取不到随机数绝不能"退化"成可预测的值——那会让密钥可被猜测
        panic!("BCryptGenRandom 失败: 无法获取密码学随机数");
    }
    buf
}

/// PBKDF2-HMAC-SHA256 派生密钥。
pub fn pbkdf2_sha256(
    password: &[u8],
    salt: &[u8],
    iterations: u32,
    out_len: usize,
) -> Result<Vec<u8>, String> {
    use windows::Win32::Security::Cryptography::{
        BCryptCloseAlgorithmProvider, BCryptDeriveKeyPBKDF2, BCryptOpenAlgorithmProvider,
        BCRYPT_ALG_HANDLE, BCRYPT_ALG_HANDLE_HMAC_FLAG, BCRYPT_SHA256_ALGORITHM,
    };

    let mut alg = BCRYPT_ALG_HANDLE::default();
    // SAFETY: 句柄成对释放；输入切片在调用期间有效
    unsafe {
        // PBKDF2 要求以 HMAC 方式打开 PRF 算法
        let st = BCryptOpenAlgorithmProvider(
            &mut alg,
            BCRYPT_SHA256_ALGORITHM,
            None,
            BCRYPT_ALG_HANDLE_HMAC_FLAG,
        );
        if st.is_err() {
            return Err("BCryptOpenAlgorithmProvider(SHA256/HMAC) 失败".to_string());
        }
        let mut out = vec![0u8; out_len];
        // 迭代数是 u64（BCrypt 的 ABI 如此），出参要传 &mut [u8] 而不是 Option
        let st = BCryptDeriveKeyPBKDF2(
            alg,
            Some(password),
            Some(salt),
            iterations as u64,
            &mut out,
            0,
        );
        let _ = BCryptCloseAlgorithmProvider(alg, 0);
        if st.is_err() {
            return Err("BCryptDeriveKeyPBKDF2 失败".to_string());
        }
        Ok(out)
    }
}

/// AES-256-GCM 加密，返回 `ct || tag`（不含 nonce——外层拼装由调用方负责，
/// 与 Python `AESGCM.encrypt` 的返回一致）。
pub fn aes_gcm_encrypt(
    key: &[u8],
    nonce: &[u8],
    plaintext: &[u8],
    aad: &[u8],
    tag_len: usize,
) -> Result<Vec<u8>, String> {
    let mut tag = vec![0u8; tag_len];
    // GCM 的密文与明文等长
    let mut out = vec![0u8; plaintext.len()];
    let info = auth_info(nonce, aad, &mut tag);
    let done = bcrypt_gcm(key, &info, true, plaintext, &mut out)?;
    if done != plaintext.len() as u32 {
        return Err(format!("GCM 加密返回长度异常: {done}"));
    }
    out.extend_from_slice(&tag);
    Ok(out)
}

/// AES-256-GCM 解密，输入 `ct || tag`（不含 nonce）。失败即认证失败。
pub fn aes_gcm_decrypt(
    key: &[u8],
    nonce: &[u8],
    ct_and_tag: &[u8],
    aad: &[u8],
    tag_len: usize,
) -> Result<Vec<u8>, String> {
    if ct_and_tag.len() < tag_len {
        return Err("密文长度不足（缺少认证标签）".to_string());
    }
    let ct_len = ct_and_tag.len() - tag_len;
    let (ct, tag) = ct_and_tag.split_at(ct_len);
    let mut tag_buf = tag.to_vec();
    let info = auth_info(nonce, aad, &mut tag_buf);
    let mut out = vec![0u8; ct_len];
    let done = bcrypt_gcm(key, &info, false, ct, &mut out)?;
    if done != ct_len as u32 {
        return Err(format!("GCM 解密返回长度异常: {done}"));
    }
    Ok(out)
}

/// 构造 GCM 的认证模式信息（`cbSize` 与版本号必须正确填写，否则 BCrypt 拒绝）。
fn auth_info(
    nonce: &[u8],
    aad: &[u8],
    tag: &mut [u8],
) -> windows::Win32::Security::Cryptography::BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO {
    use windows::Win32::Security::Cryptography::{
        BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO, BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO_VERSION,
    };
    BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO {
        cbSize: std::mem::size_of::<BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO>() as u32,
        dwInfoVersion: BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO_VERSION,
        // 这些指针只在调用期间有效
        pbNonce: nonce.as_ptr() as *mut u8,
        cbNonce: nonce.len() as u32,
        pbAuthData: aad.as_ptr() as *mut u8,
        cbAuthData: aad.len() as u32,
        pbTag: tag.as_mut_ptr(),
        cbTag: tag.len() as u32,
        pbMacContext: std::ptr::null_mut(),
        cbMacContext: 0,
        cbAAD: 0,
        cbData: 0,
        dwFlags: 0,
    }
}

/// 打开 AES-GCM 算法 + 导入密钥，执行一次加密或解密，返回写入的字节数。
fn bcrypt_gcm(
    key: &[u8],
    info: &windows::Win32::Security::Cryptography::BCRYPT_AUTHENTICATED_CIPHER_MODE_INFO,
    encrypt: bool,
    input: &[u8],
    out: &mut [u8],
) -> Result<u32, String> {
    use windows::Win32::Security::Cryptography::{
        BCryptCloseAlgorithmProvider, BCryptDecrypt, BCryptDestroyKey, BCryptEncrypt,
        BCryptGenerateSymmetricKey, BCryptOpenAlgorithmProvider, BCryptSetProperty,
        BCRYPT_AES_ALGORITHM, BCRYPT_ALG_HANDLE, BCRYPT_CHAINING_MODE, BCRYPT_CHAIN_MODE_GCM,
        BCRYPT_FLAGS, BCRYPT_HANDLE, BCRYPT_KEY_HANDLE, BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS,
    };

    let mut alg = BCRYPT_ALG_HANDLE::default();
    let mut kh = BCRYPT_KEY_HANDLE::default();
    // SAFETY: 句柄成对释放；info/input/out 的生命周期覆盖调用
    unsafe {
        if BCryptOpenAlgorithmProvider(
            &mut alg,
            BCRYPT_AES_ALGORITHM,
            None,
            BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS(0),
        )
        .is_err()
        {
            return Err("BCryptOpenAlgorithmProvider(AES) 失败".to_string());
        }
        // **必须**显式设成 GCM：默认链模式是 CBC，不设就完全不是 GCM。
        // 这个属性的值是"宽字符串常量"而不是普通缓冲——**必须带结尾 NUL**，
        // BCrypt 会做 NUL 结尾的字符串比较（`ChainingModeGCM` 不带 NUL 会返回
        // STATUS_INVALID_PARAMETER，本机就是这样失败的）。
        let mode_str: Vec<u16> = BCRYPT_CHAIN_MODE_GCM
            .as_wide()
            .iter()
            .copied()
            .chain(std::iter::once(0))
            .collect();
        let mode_bytes: Vec<u8> = mode_str.iter().flat_map(|u| u.to_le_bytes()).collect();
        if BCryptSetProperty(BCRYPT_HANDLE(alg.0), BCRYPT_CHAINING_MODE, &mode_bytes, 0).is_err() {
            let _ = BCryptCloseAlgorithmProvider(alg, 0);
            return Err("设置 GCM 链模式失败".to_string());
        }
        if BCryptGenerateSymmetricKey(alg, &mut kh, None, key, 0).is_err() {
            let _ = BCryptCloseAlgorithmProvider(alg, 0);
            return Err("BCryptGenerateSymmetricKey 失败".to_string());
        }
        let mut done = 0u32;
        let iv = info as *const _ as *const core::ffi::c_void;
        let st = if encrypt {
            BCryptEncrypt(
                kh,
                Some(input),
                Some(iv),
                None,
                Some(out),
                &mut done,
                BCRYPT_FLAGS(0),
            )
        } else {
            BCryptDecrypt(
                kh,
                Some(input),
                Some(iv),
                None,
                Some(out),
                &mut done,
                BCRYPT_FLAGS(0),
            )
        };
        let _ = BCryptDestroyKey(kh);
        let _ = BCryptCloseAlgorithmProvider(alg, 0);
        if st.is_err() {
            return Err(if encrypt {
                "AES-GCM 加密失败".to_string()
            } else {
                "AES-GCM 解密失败（认证不通过：密钥、aad 或密文被改）".to_string()
            });
        }
        Ok(done)
    }
}

// ---------------- 剪贴板位图读取 ----------------

/// 读取剪贴板的原始 `CF_DIB` 字节（含 BITMAPINFOHEADER，无文件头）。
///
/// 优先 `CF_DIBV5`（带 alpha），退回 `CF_DIB`。**不做解码**——DIB 的位深/行序/掩码
/// 差异很大，解码集中在 `dib::parse_dib` 里逐条断言。
pub fn clipboard_get_dib() -> Option<Vec<u8>> {
    clipboard_get_dib_format(CF_DIBV5).or_else(|| clipboard_get_dib_format(CF_DIB))
}

fn clipboard_get_dib_format(format: u32) -> Option<Vec<u8>> {
    use windows::Win32::System::DataExchange::GetClipboardData;
    // SAFETY: 剪贴板在 open/close 成对之间访问；句柄由系统持有，不释放
    unsafe {
        if OpenClipboard(null_hwnd()).is_err() {
            return None;
        }
        let _guard = ClipboardGuard;
        let Ok(h) = GetClipboardData(format) else {
            return None;
        };
        if h.is_invalid() {
            return None;
        }
        let p = GlobalLock(HGLOBAL(h.0));
        if p.is_null() {
            return None;
        }
        // 剪贴板句柄**不携带可靠长度**：必须按 BITMAPINFOHEADER 里的字段精确算。
        // 前 40 字节一定存在（上面已确认是 CF_DIB），但若内容异常短则本函数
        // 会在读到非法值后返回 None——不能假设 p 指向 40 字节以上。
        let hdr = std::slice::from_raw_parts(p as *const u8, 40);
        let bi_size = u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as usize;
        let width = i32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]);
        let height = i32::from_le_bytes([hdr[8], hdr[9], hdr[10], hdr[11]]);
        let bit_count = u16::from_le_bytes([hdr[14], hdr[15]]) as usize;
        let compression = u32::from_le_bytes([hdr[16], hdr[17], hdr[18], hdr[19]]);
        let clr_used = u32::from_le_bytes([hdr[32], hdr[33], hdr[34], hdr[35]]) as usize;
        let size_image = u32::from_le_bytes([hdr[20], hdr[21], hdr[22], hdr[23]]) as usize;
        let _ = GlobalUnlock(HGLOBAL(h.0));

        if bi_size < 40 || width <= 0 || height == 0 || bit_count == 0 {
            return None;
        }
        let w = width as usize;
        let hgt = height.unsigned_abs() as usize;
        // 掩码 + 调色板长度
        let mut extra = match compression {
            3 => 12, // BI_BITFIELDS：3 个 DWORD 掩码
            6 => 16, // BI_ALPHABITFIELDS：4 个
            _ => 0,
        };
        let entries = if bit_count <= 8 {
            if clr_used > 0 {
                clr_used
            } else {
                1usize << bit_count
            }
        } else {
            clr_used
        };
        extra += entries * 4;
        let stride = (bit_count * w).div_ceil(32) * 4;
        let pixel_bytes = size_image.max(stride * hgt);
        let total = bi_size + extra + pixel_bytes;
        // 上限保护：错乱的头部可能声明巨大长度
        if total > 512 * 1024 * 1024 {
            return None;
        }
        Some(std::slice::from_raw_parts(p as *const u8, total).to_vec())
    }
}

// ---------------- 盘符类型 ----------------

/// 在资源管理器里打开父目录并**选中**该文件（`explorer /select,<path>`）。
pub fn open_folder_selecting(path: &str) -> Result<(), String> {
    // explorer 的 /select 需要完整路径且不能有额外引号包裹问题；
    // 用 run_capture 走 PATH 解析（explorer 在 System32 下）
    let out = run_capture(&["explorer".to_string(), format!("/select,{path}")], 15)
        .map_err(|e| format!("explorer 无法执行: {e}"))?;
    // explorer 对成功也常返回非 0（它把请求转给已有实例就退出），
    // 所以**不能**用退出码判定，只要进程起来了就算成功
    let _ = out;
    Ok(())
}

/// 把纯文本写入剪贴板（`CF_UNICODETEXT`）。
pub fn clipboard_set_text(text: &str) -> bool {
    clipboard_set_std(CF_UNICODETEXT, &build_unicode_text(text))
}
/// 查询盘符根路径的类型（`GetDriveTypeW`）。
///
/// 返回值：0 未知 / 1 无根目录 / 2 可移动 / 3 固定 / 4 远程 / 5 光驱 / 6 RAM 盘。
/// **用途是"该不该对路径做 `exists()` 探测"**：远程盘与未知类型上的 `exists()`
/// 在失效网络映射下会阻塞数秒到数十秒，而调用方常在 UI 线程里。
pub fn drive_type(root: &str) -> Option<u32> {
    use windows::Win32::Storage::FileSystem::GetDriveTypeW;
    if root.is_empty() {
        return None;
    }
    let w: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: w 以 NUL 结尾且在本调用内有效
    Some(unsafe { GetDriveTypeW(windows::core::PCWSTR(w.as_ptr())) })
}

// ---------------- 标准剪贴板格式号 ----------------

// windows crate 不导出这些 `CF_*` 常量（它们在 WinUser.h 里），所以按官方数值定义。
// 数值写错会静默读到别的格式或读不到——所以集中定义一处并加断言。
/// 位图（BITMAPINFOHEADER 起头）。
pub const CF_DIB: u32 = 8;
/// 文本（UTF-16LE + NUL 结尾）。
pub const CF_UNICODETEXT: u32 = 13;
/// 文件列表（DROPFILES 头 + 双 NUL 结尾路径）。
pub const CF_HDROP: u32 = 15;
/// 带 alpha 的位图（V5 头）。
pub const CF_DIBV5: u32 = 17;

// ---------------- 剪贴板读取（监听器用） ----------------

/// 剪贴板序号（`GetClipboardSequenceNumber`）：内容每次变化都会递增。
///
/// 轮询这个序号是**监听剪贴板最省事且不丢事件**的做法：它不需要窗口与消息循环
/// （`AddClipboardFormatListener` 要求一个 HWND 并处理 WM_CLIPBOARDUPDATE），
/// 也不像 `WM_DRAWCLIPBOARD` 那样需要维护监听链。序号不变就完全不用打开剪贴板，
/// 开销接近零。
pub fn clipboard_sequence() -> u32 {
    use windows::Win32::System::DataExchange::GetClipboardSequenceNumber;
    // SAFETY: 无参数、无副作用
    unsafe { GetClipboardSequenceNumber() }
}

/// 读取剪贴板文本（`CF_UNICODETEXT`）。无文本返回 None。
pub fn clipboard_get_text() -> Option<String> {
    use windows::Win32::System::DataExchange::GetClipboardData;
    // SAFETY: 剪贴板在 open/close 成对之间访问；句柄由系统持有，不释放
    unsafe {
        if OpenClipboard(null_hwnd()).is_err() {
            if std::env::var("SK_WATCH_TRACE").is_ok() {
                eprintln!("[cb-dbg] get_text: OpenClipboard 失败");
            }
            return None;
        }
        let _guard = ClipboardGuard;
        let Ok(h) = GetClipboardData(CF_UNICODETEXT) else {
            if std::env::var("SK_WATCH_TRACE").is_ok() {
                eprintln!("[cb-dbg] get_text: GetClipboardData Err");
            }
            return None;
        };
        if h.is_invalid() {
            if std::env::var("SK_WATCH_TRACE").is_ok() {
                eprintln!("[cb-dbg] get_text: 句柄 invalid");
            }
            return None;
        }
        let p = GlobalLock(HGLOBAL(h.0)) as *const u16;
        if p.is_null() {
            return None;
        }
        // 按 NUL 结尾扫描（CF_UNICODETEXT 保证以 NUL 结尾）
        let mut len = 0usize;
        while *p.add(len) != 0 {
            len += 1;
            // 防御：异常巨型内容不至于无限扫描（真实剪贴板远达不到）
            if len > 8_000_000 {
                break;
            }
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, len));
        let _ = GlobalUnlock(HGLOBAL(h.0));
        Some(s)
    }
}

/// 读取剪贴板文件列表（`CF_HDROP`）。无文件返回 None。
pub fn clipboard_get_files() -> Option<Vec<String>> {
    use windows::Win32::System::DataExchange::GetClipboardData;
    use windows::Win32::UI::Shell::{DragQueryFileW, DROPFILES, HDROP};
    // SAFETY: 同上；HDROP 由剪贴板持有
    unsafe {
        if OpenClipboard(null_hwnd()).is_err() {
            return None;
        }
        let _guard = ClipboardGuard;
        let Ok(h) = GetClipboardData(CF_HDROP) else {
            return None;
        };
        if h.is_invalid() {
            return None;
        }
        let p = GlobalLock(HGLOBAL(h.0)) as *const DROPFILES;
        if p.is_null() {
            return None;
        }
        let drop = HDROP(h.0);
        let count = DragQueryFileW(drop, 0xFFFF_FFFF, None);
        let mut out = Vec::new();
        for i in 0..count {
            let need = DragQueryFileW(drop, i, None) as usize;
            if need == 0 {
                continue;
            }
            let mut buf = vec![0u16; need + 1];
            let got = DragQueryFileW(drop, i, Some(&mut buf));
            if got > 0 {
                out.push(String::from_utf16_lossy(&buf[..got as usize]));
            }
        }
        let _ = GlobalUnlock(HGLOBAL(h.0));
        if out.is_empty() {
            None
        } else {
            Some(out)
        }
    }
}

/// 剪贴板里是否有位图（`CF_DIB` / `CF_DIBV5`）。
///
/// 只做"有没有"的探测，不搬运像素：真正的位图读取要处理 BITMAPINFOHEADER 的
/// 位深、压缩方式与行序，属于图片捕获单独一轮的工作。
pub fn clipboard_has_image() -> bool {
    use windows::Win32::System::DataExchange::IsClipboardFormatAvailable;
    // SAFETY: 无副作用查询
    let r = unsafe {
        IsClipboardFormatAvailable(CF_DIB).is_ok() || IsClipboardFormatAvailable(CF_DIBV5).is_ok()
    };
    if std::env::var("SK_WATCH_TRACE").is_ok() {
        eprintln!("[cb-dbg] has_image={r}");
    }
    r
}

/// 剪贴板里是否有 HTML（`HTML Format` 注册格式）。
pub fn clipboard_has_html() -> bool {
    clipboard_named_available("HTML Format")
}

/// 指定注册格式是否可用。
pub fn clipboard_named_available(name: &str) -> bool {
    use windows::Win32::System::DataExchange::{
        IsClipboardFormatAvailable, RegisterClipboardFormatW,
    };
    let w: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: w 以 NUL 结尾且在本调用内有效
    unsafe {
        let fmt = RegisterClipboardFormatW(windows::core::PCWSTR(w.as_ptr()));
        fmt != 0 && IsClipboardFormatAvailable(fmt).is_ok()
    }
}

/// 读完自动关闭剪贴板（提前 return 也必须关，否则会卡住其他程序）。
struct ClipboardGuard;
impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        // SAFETY: 与 OpenClipboard 成对
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

/// 断网/恢复提示音（winmm PlaySound，SND_ALIAS|SND_ASYNC——异步不阻塞调用线程，
/// 别名与 Python winsound 同表：ok=SystemAsterisk，其余=SystemExclamation）。
/// 仅限真实构建调用（测试运行静默，见 monitor.rs 调用点）。
#[cfg(all(windows, not(test)))]
pub fn play_alert_sound(ok: bool) {
    use windows::core::HSTRING;
    use windows::Win32::Media::Audio::{PlaySoundW, SND_ALIAS, SND_ASYNC};
    let name = HSTRING::from(if ok {
        "SystemAsterisk"
    } else {
        "SystemExclamation"
    });
    unsafe {
        // 失败静默：提示音是锦上添花，不为此上报错误
        let _ = PlaySoundW(
            windows::core::PCWSTR(name.as_ptr()),
            None,
            SND_ALIAS | SND_ASYNC,
        );
    }
}

/// 单实例互斥：创建命名互斥量并立即持有关闭句柄的能力由调用方决定——
/// 这里只创建；若 ERROR_ALREADY_EXISTS 则返回 false（已有实例）。
/// 句柄有意泄漏（与进程同寿），进程退出时系统自动回收。
pub fn acquire_single_instance_mutex(name: &str) -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;
    unsafe {
        let w: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let h = CreateMutexW(None, false, PCWSTR(w.as_ptr()));
        if h.is_err() {
            return false;
        }
        // 不 CloseHandle：互斥量必须与进程同寿（释放即失去单实例保护）
        std::mem::forget(h);
        GetLastError() != ERROR_ALREADY_EXISTS
    }
}

/// 是否有 SysKit 实例（syskit-rs.exe / SysKit.exe）在运行——
/// 真实剪贴板测试与运行中实例的 500ms 轮询竞争，有实例时跳过。
pub fn syskit_instance_running() -> bool {
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return false;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut found = false;
        if Process32FirstW(snap, &mut entry).is_ok() {
            loop {
                let name = String::from_utf16_lossy(
                    &entry.szExeFile[..entry.szExeFile.iter().position(|c| *c == 0).unwrap_or(0)],
                );
                if name.eq_ignore_ascii_case("syskit-rs.exe")
                    || name.eq_ignore_ascii_case("syskit.exe")
                {
                    found = true;
                    break;
                }
                if Process32NextW(snap, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = windows::Win32::Foundation::CloseHandle(snap);
        found
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

    // ---------------- 子进程执行 ----------------

    #[test]
    fn quote_arg_matches_windows_rules() {
        assert_eq!(quote_arg("ipconfig"), "ipconfig");
        assert_eq!(quote_arg("/flushdns"), "/flushdns");
        assert_eq!(quote_arg("a b"), "\"a b\"");
        assert_eq!(quote_arg(""), "\"\"");
        assert_eq!(quote_arg("say \"hi\""), "\"say \\\"hi\\\"\"");
        // 末尾反斜杠在加引号时必须翻倍，否则会吃掉收尾引号
        assert_eq!(
            quote_arg("C:\\dir with space\\"),
            "\"C:\\dir with space\\\\\""
        );
        // 不在引号前的反斜杠原样保留
        assert_eq!(quote_arg("C:\\a\\b"), "C:\\a\\b");
        assert_eq!(quote_arg("na me\\"), "\"na me\\\\\"");
    }

    #[test]
    fn build_command_line_joins_quoted_args() {
        let args = vec!["cmd".to_string(), "/c".to_string(), "echo hi".to_string()];
        assert_eq!(build_command_line(&args), "cmd /c \"echo hi\"");
        assert_eq!(build_command_line(&[]), "");
    }

    #[test]
    fn tail_truncates_by_chars_not_bytes() {
        let o = ProcOutput {
            ok: true,
            code: 0,
            stdout: "abcdefghij".into(),
            stderr: "XY".into(),
        };
        assert_eq!(o.tail(3, 1), "hijY", "stdout 取尾 3 + stderr 取尾 1");
        assert_eq!(o.tail(50, 50), "abcdefghijXY", "短于上限时原样返回");
        // 中文按字符截断，不产生半个字符
        let cn = ProcOutput {
            ok: true,
            code: 0,
            stdout: "中文输出测试".into(),
            stderr: String::new(),
        };
        assert_eq!(cn.tail(2, 0), "测试");
    }

    #[test]
    fn combined_concatenates_stdout_then_stderr() {
        let o = ProcOutput {
            ok: false,
            code: 1,
            stdout: "out".into(),
            stderr: "err".into(),
        };
        assert_eq!(o.combined(), "outerr");
    }

    #[test]
    fn resolve_executable_finds_system_tools() {
        // PATH 解析是移植时必须补的一层：CreateProcessW 带 lpApplicationName 时
        // **不搜 PATH**，裸名 cmd/ipconfig/netsh 会全报「系统找不到指定的文件」。
        for name in ["cmd", "ipconfig", "netsh", "sc", "reg"] {
            let p = resolve_executable(name).unwrap_or_else(|| panic!("应能解析 {name}"));
            assert!(
                std::path::Path::new(&p).is_file(),
                "{name} 解析结果必须是真实文件: {p}"
            );
            assert!(
                p.to_ascii_lowercase().ends_with(".exe"),
                "解析结果应带扩展名: {p}"
            );
        }
        assert!(resolve_executable("syskit_no_such_program_xyz").is_none());
        assert!(resolve_executable("").is_none());
        // 带路径分隔符的原样校验
        let cmd = resolve_executable("cmd").unwrap();
        assert!(resolve_executable(&cmd).is_some(), "绝对路径应可再次解析");
    }

    #[test]
    fn run_capture_executes_and_captures_stdout() {
        // 真实起一个子进程：cmd /c echo 是最小可验证目标
        let out = run_capture(&["cmd".into(), "/c".into(), "echo hello-syskit".into()], 20)
            .unwrap_or_else(|e| panic!("cmd 应能启动: {e}"));
        assert!(
            out.ok,
            "echo 应成功: code={} err={:?} out={:?}",
            out.code, out.stderr, out.stdout
        );
        assert_eq!(out.code, 0);
        assert!(
            out.stdout.contains("hello-syskit"),
            "必须捕获到 stdout: {:?}",
            out.stdout
        );
    }

    #[test]
    fn run_capture_drains_output_beyond_pipe_buffer() {
        // 回归：子进程输出远超管道缓冲（约 4KB）时，旧实现「先等退出再读管道」
        // 会让子进程阻塞在 write 上直到被超时 kill——修复动作被误报失败。
        // 4MB 输出必须在超时内正常退出且完整捕获（结尾标记证明没被截断丢弃）。
        let script = "$o = 'x' * 200; 1..20000 | ForEach-Object { $o }; 'END-MARKER-SYSKIT'";
        let out = run_capture(
            &[
                "powershell".into(),
                "-NoProfile".into(),
                "-Command".into(),
                script.into(),
            ],
            60,
        )
        .unwrap_or_else(|e| panic!("powershell 应能启动: {e}"));
        assert!(out.ok, "大输出子进程必须正常退出（code={})", out.code);
        assert_eq!(out.code, 0);
        assert!(
            out.stdout.contains("END-MARKER-SYSKIT"),
            "结尾标记必须被捕获（输出被抽干且未截断）"
        );
        assert!(
            out.stdout.len() >= 4_000_000,
            "输出量应远超管道缓冲（实际 {} 字节）",
            out.stdout.len()
        );
    }

    #[test]
    fn run_capture_reports_nonzero_exit() {
        let out = run_capture(&["cmd".into(), "/c".into(), "exit 3".into()], 20)
            .unwrap_or_else(|e| panic!("cmd 应能启动: {e}"));
        assert!(!out.ok, "非零退出必须判失败");
        assert_eq!(out.code, 3, "退出码要原样带出来（修复动作靠它判成败）");
    }

    #[test]
    fn run_capture_reports_stderr() {
        let out = run_capture(
            &["cmd".into(), "/c".into(), "echo oops 1>&2 & exit 1".into()],
            20,
        )
        .unwrap_or_else(|e| panic!("cmd 应能启动: {e}"));
        assert!(!out.ok);
        assert!(
            out.stderr.contains("oops"),
            "必须捕获到 stderr: {:?}",
            out.stderr
        );
    }

    #[test]
    fn run_capture_survives_unknown_program() {
        // 不存在的程序：必须返回 Err 或失败结果，绝不 panic
        match run_capture(&["syskit_no_such_program_xyz".into()], 5) {
            Err(_) => {}
            Ok(o) => assert!(!o.ok, "不存在的程序不得判成功"),
        }
    }

    #[test]
    fn run_capture_empty_args_is_err() {
        assert!(run_capture(&[], 5).is_err());
    }

    #[test]
    fn run_capture_handles_non_ascii_output() {
        // 中文输出不得 panic，且 ASCII 部分必须可读
        let out = run_capture(
            &["cmd".into(), "/c".into(), "echo 中文 & echo OK".into()],
            20,
        )
        .unwrap_or_else(|e| panic!("cmd 应能启动: {e}"));
        assert!(
            out.stdout.contains("OK"),
            "ASCII 部分必须可读: {:?}",
            out.stdout
        );
    }
}
