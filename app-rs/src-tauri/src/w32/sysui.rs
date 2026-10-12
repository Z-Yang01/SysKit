//! 原生文件对话框 / 系统关联打开 / 盘符类型 / 提醒音。
use super::*;

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
