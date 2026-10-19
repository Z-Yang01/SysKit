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

#[allow(dead_code)] // RegisterHotKey 修饰键对齐面（与 windows crate 常量语义重复，保留作文档）
pub const MOD_ALT: u32 = 0x0001;
#[allow(dead_code)] // RegisterHotKey 修饰键对齐面（与 windows crate 常量语义重复，保留作文档）
pub const MOD_CONTROL: u32 = 0x0002;
#[allow(dead_code)] // RegisterHotKey 修饰键对齐面（与 windows crate 常量语义重复，保留作文档）
pub const MOD_SHIFT: u32 = 0x0004;
#[allow(dead_code)] // RegisterHotKey 修饰键对齐面（与 windows crate 常量语义重复，保留作文档）
pub const MOD_WIN: u32 = 0x0008;
/// 按住不重复触发（Win7+）；恒置（规格书硬约束）。
pub const MOD_NOREPEAT: u32 = 0x4000;

#[cfg(test)]
pub const VK_SHIFT_C: u32 = 0x10;
#[cfg(test)]
pub const VK_CONTROL_C: u32 = 0x11;
#[cfg(test)]
pub const VK_MENU_C: u32 = 0x12;
#[cfg(test)]
pub const VK_LWIN_C: u32 = 0x5B;
#[cfg(test)]
pub const VK_RWIN_C: u32 = 0x5C;
#[cfg(test)]
pub const VK_V_C: u32 = 0x56;

/// 剪贴板无主窗口（`RegisterHotKey(NULL, …)` / `OpenClipboard(NULL)` 同款）。
fn null_hwnd() -> Option<HWND> {
    Some(HWND(std::ptr::null_mut()))
}

mod clipboard;
mod crypto;
mod input;
mod process;
mod sysui;

// 扁平重导出：调用方继续用 crate::w32::X，与拆分前完全一致
pub use clipboard::*;
pub use crypto::*;
pub use input::*;
pub use process::*;
pub use sysui::*;

/// 是否有 SysKit 实例（syskit-rs.exe / SysKit.exe）在运行——
/// 真实剪贴板测试与运行中实例的 500ms 轮询竞争，有实例时跳过。
#[cfg(test)]
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

    /// 原生进程快照必须能看到**自己**且工作集非 0——这条同时验证了
    /// Toolhelp 结构体布局（dwSize）与 `PROCESS_MEMORY_COUNTERS.cb` 都对，
    /// 写错任一处会静默返回 0 条/0 字节。
    #[test]
    fn native_snapshot_sees_self_with_memory() {
        let t0 = std::time::Instant::now();
        let v = snapshot_process_memory().expect("Toolhelp 快照应能创建");
        let ms = t0.elapsed().as_millis();
        assert!(!v.is_empty(), "至少应枚举到进程（含自己）");
        let me = std::process::id();
        let mine = v.iter().find(|p| p.pid == me).expect("快照里应有当前进程");
        assert!(mine.working_set > 0, "工作集不应为 0");
        assert!(!mine.name.is_empty());
        // 回归守卫：这条路径必须是全内存操作。若有人改回 spawn 子进程
        // （实测 tasklist 单次 1.4s），这里会直接红——性能页每 3 秒调一次，
        // 那就等于常驻半个核。
        eprintln!("原快照：{} 个进程，耗时 {ms}ms", v.len());
        assert!(ms < 500, "原生快照不该接近子进程量级（实测 {ms}ms）");
    }

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

    /// 40 字节 DIB 头构造器：按偏移填字段，便于逐例表达。
    fn dib_hdr(
        bi_size: u32,
        w: i32,
        height: i32,
        bpp: u16,
        comp: u32,
        image: u32,
        clr: u32,
    ) -> [u8; 40] {
        let mut h = [0u8; 40];
        h[0..4].copy_from_slice(&bi_size.to_le_bytes());
        h[4..8].copy_from_slice(&w.to_le_bytes());
        h[8..12].copy_from_slice(&height.to_le_bytes());
        h[14..16].copy_from_slice(&bpp.to_le_bytes());
        h[16..20].copy_from_slice(&comp.to_le_bytes());
        h[20..24].copy_from_slice(&image.to_le_bytes());
        h[32..36].copy_from_slice(&clr.to_le_bytes());
        h
    }

    #[test]
    fn dib_layout_matches_manual_math() {
        // 32bpp 4×3 BI_RGB：头 40 + 像素 4*3*4=48（sizeImage 谎报更大取更大者）
        let (bi, extra, px) =
            dib_layout_from_header(&dib_hdr(40, 4, 3, 32, 0, 48, 0)).expect("合法头");
        assert_eq!((bi, extra, px), (40, 0, 48));
        // BI_BITFIELDS(3) 加 12 字节掩码；8bpp 全调色板 256*4=1024
        let (_, extra2, _) =
            dib_layout_from_header(&dib_hdr(40, 2, 2, 8, 3, 0, 0)).expect("合法头");
        assert_eq!(extra2, 12 + 1024);
        // 底朝上负高：像素字节按 |height| 算
        let (_, _, px3) =
            dib_layout_from_header(&dib_hdr(40, 4, -3, 32, 0, 0, 0)).expect("负高合法");
        assert_eq!(px3, 48);
        // 非法头：biSize<40 / 宽 0 / bpp 0
        assert!(dib_layout_from_header(&dib_hdr(12, 4, 3, 32, 0, 0, 0)).is_none());
        assert!(dib_layout_from_header(&dib_hdr(40, 0, 3, 32, 0, 0, 0)).is_none());
        assert!(dib_layout_from_header(&dib_hdr(40, 4, 3, 0, 0, 0, 0)).is_none());
    }

    #[test]
    fn dib_layout_no_usize_overflow_on_absurd_head() {
        // 变异验证目标：极端头（宽高 i32 近满 + sizeImage 满值）像素长度在 u64
        // 下计算，绝不回绕成小值绕过 512MB 上限检查。
        let hdr = dib_hdr(40, i32::MAX, i32::MAX, 32, 0, u32::MAX, u32::MAX);
        let (_, extra, px) = dib_layout_from_header(&hdr).expect("尺寸字段本身合法");
        let total = 40u64
            .checked_add(extra as u64)
            .and_then(|t| t.checked_add(px))
            .unwrap_or(u64::MAX);
        assert!(total > 512 * 1024 * 1024, "荒谬头必须撞上限被拒（px={px}）");
    }

    #[test]
    fn dib_read_len_clamps_to_allocation() {
        // 回归（越界读）：读取长度必须被 GlobalSize(avail) 截断——
        // 初版直接按（可谎报的）total 拷贝，畸形 DIB 让监听线程越界读崩进程。
        assert_eq!(dib_read_len(88, 88, 40), Some(88), "正常：total==avail");
        assert_eq!(
            dib_read_len(1 << 30, 4096, 40),
            Some(4096),
            "谎报头被截到分配大小"
        );
        assert_eq!(dib_read_len(88, 30, 40), None, "连头都不完整 → 拒绝");
        assert_eq!(
            dib_read_len(88, 50, 50),
            Some(50),
            "头+掩码齐、像素不足 → 截断交解析层判"
        );
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
    fn run_capture_drains_output_beyond_pipe_buffer_400k() {
        // 回归：子进程输出远超管道缓冲（约 4KB）时，旧实现「先等退出再读管道」
        // 会让子进程阻塞在 write 上直到被超时 kill——修复动作被误报失败。
        // 400KB 档在门禁里活跃锁定 drain 语义（4MB 压力档见下，#[ignore] 手工跑）。
        let script = "$o = 'x' * 200; 1..2000 | ForEach-Object { $o }; 'END-MARKER-SYSKIT'";
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
            out.stdout.len() >= 400_000,
            "输出量应远超管道缓冲（实际 {} 字节）",
            out.stdout.len()
        );
    }

    #[test]
    #[ignore = "4MB 压力档：本机管道读速异常时挂满 60s 超时（2026-10-09）；drain 正确性由上方 400KB 活跃档锁定，本档手工单独跑（cargo test -- --ignored）验收"]
    fn run_capture_drains_output_beyond_pipe_buffer_4mb() {
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
