//! 安装器协作：让运行中的实例优雅退出，安装完成后由安装器重新拉起。
//!
//! 为什么必须这么做（用户提问：「安装时能不能让软件暂停一下再装」）：
//! - **暂停进程救不了**：被挂起的进程仍然锁着自己的 exe，安装器照样换不掉文件；
//! - 窗口的 X 是"收进托盘"，WM_CLOSE 不会退出，所以也不能靠 taskkill 的软关闭；
//! - 唯一干净的路是「请它自己退出 → 覆盖安装 → 重新启动」，而且**不能留下
//!   用户手动点关闭的步骤**。
//!
//! 通道：命名事件 `Local\SysKit.QuitForUpdate`。
//! - 运行实例在 setup 里起一个线程等这个事件（`spawn_quit_listener`）；
//! - 安装器执行 `SysKit.exe --quit-for-update`，本模块在**拿到单实例互斥量
//!   之前**处理该参数（拿不到互斥量恰恰说明有实例在跑），所以不会启动 UI、
//!   也不会弹「已在运行」的框。
//!
//! 退出码约定（安装器据此决定要不要重启 / 强杀）：
//! - `0` = 本来就没在运行；
//! - `1` = 已优雅退出，装完应重新启动；
//! - `2` = 等了 6 秒还没退（老版本不认这个参数，或卡在某个阻塞操作）——
//!   安装器此时走 taskkill 兜底。

use std::time::Duration;

/// 事件名（`Local\` = 当前登录会话，跨用户不可见，正好是安装器的作用域）。
pub const QUIT_EVENT: &str = r"Local\SysKit.QuitForUpdate";
/// 与 `run()` 里的单实例互斥量同名：旧实例退出后我们才能拿到它。
const SINGLE_INSTANCE: &str = "SysKit_SingleInstance_Mutex";
/// 等旧实例退出的上限：退出流程包含落盘，6 秒足够；超时说明它没在听事件。
const WAIT_TOTAL: Duration = Duration::from_secs(6);
const STEP: Duration = Duration::from_millis(100);

/// `SysKit.exe --quit-for-update` 的实现：请求退出 + 等它真正结束。
pub fn quit_running_instance() -> i32 {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenEventW, SetEvent, EVENT_MODIFY_STATE};

    let name = windows::core::HSTRING::from(QUIT_EVENT);
    let opened = unsafe { OpenEventW(EVENT_MODIFY_STATE, false, &name) };
    let Ok(ev) = opened else {
        // 事件不存在：没有运行实例，或者是还不支持该通道的老版本
        return 0;
    };
    unsafe {
        let _ = SetEvent(ev);
        let _ = CloseHandle(ev);
    }

    // 事件只负责"通知"；判据是**进程是否真的不在了**（进程枚举，排除自身）。
    // 不用"抢互斥量"判定：那依赖旧进程确实持有同名 mutex，一旦判定口径
    // 对不上就会白等满整个超时（实测正好卡 6 秒）。
    let deadline = std::time::Instant::now() + WAIT_TOTAL;
    while std::time::Instant::now() < deadline {
        if !app_process_alive() {
            return 1;
        }
        std::thread::sleep(STEP);
    }
    2
}

/// 是否还有 SysKit 主进程活着（排除本进程）。
fn app_process_alive() -> bool {
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    let me = std::process::id();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return true; // 快照拿不到就保守认为还活着，交给超时兜底
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut alive = false;
        if Process32FirstW(snap, &mut entry).is_ok() {
            loop {
                let name = String::from_utf16_lossy(
                    &entry.szExeFile[..entry
                        .szExeFile
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(entry.szExeFile.len())],
                );
                if entry.th32ProcessID != me && name.eq_ignore_ascii_case("SysKit.exe") {
                    alive = true;
                    break;
                }
                if Process32NextW(snap, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = windows::Win32::Foundation::CloseHandle(snap);
        alive
    }
}

/// 运行实例侧：起一个线程等退出事件，收到就走正常退出流程（保留清理逻辑）。
pub fn spawn_quit_listener(app: tauri::AppHandle) {
    std::thread::spawn(move || unsafe {
        use windows::Win32::Foundation::WAIT_OBJECT_0;
        use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject, INFINITE};

        let name = windows::core::HSTRING::from(QUIT_EVENT);
        let Ok(ev) = CreateEventW(None, false, false, &name) else {
            return;
        };
        if WaitForSingleObject(ev, INFINITE) == WAIT_OBJECT_0 {
            // 兜底硬退出：`app.exit(0)` 走 Tauri 的正常链路（会等窗口关闭），
            // 而本项目的关窗被 minimize_to_tray 拦截，实测能拖 6 秒以上——
            // 安装器那边只有 5 秒预算，等不起。所以给 1.2 秒让正常清理跑，
            // 之后直接 process::exit：状态都是实时落盘的，丢掉的最多是
            // 最后一两条剪贴板记录。
            std::thread::spawn(|| {
                std::thread::sleep(Duration::from_millis(1200));
                // **必须 TerminateProcess，不能用 std::process::exit**：
                // exit 会依次跑 atexit 与各 DLL 的 DllMain(DETACH)，WebView2 /
                // 托盘 / 日志线程的清理能把它拖到 6 秒以上（实测），安装器等不起。
                unsafe {
                    use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
                    let _ = TerminateProcess(GetCurrentProcess(), 0);
                }
            });
            // 用 app.exit 而不是 process::exit：走 Tauri 的正常退出链路，
            // 托盘图标、剪贴板监听、日志句柄都能按序收尾
            let _ = app.exit(0);
        }
    });
}
