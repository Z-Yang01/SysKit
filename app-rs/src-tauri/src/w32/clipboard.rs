//! 剪贴板写入与读取（UTF-16/CF_DIB/CF_HDROP/命名格式）。
use super::*;

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

// ---------------- 剪贴板位图读取 ----------------

/// 读取剪贴板的原始 `CF_DIB` 字节（含 BITMAPINFOHEADER，无文件头）。
///
/// 优先 `CF_DIBV5`（带 alpha），退回 `CF_DIB`。**不做解码**——DIB 的位深/行序/掩码
/// 差异很大，解码集中在 `dib::parse_dib` 里逐条断言。
pub fn clipboard_get_dib() -> Option<Vec<u8>> {
    clipboard_get_dib_format(CF_DIBV5).or_else(|| clipboard_get_dib_format(CF_DIB))
}

/// 由 DIB 头 40 字节推算布局（纯函数，越界审计核心）。
///
/// 返回 `(bi_size, 掩码+调色板字节数, 像素字节数 u64)`；头字段非法返回 None。
/// 像素长度全程 u64（i32 极值下 `stride*height` 可达 2^64 边界，usize 溢出即
/// 回绕成小值——读越界的同时还绕过 512MB 上限检查，双倍危险）。
pub fn dib_layout_from_header(hdr: &[u8; 40]) -> Option<(usize, usize, u64)> {
    let bi_size = u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as usize;
    let width = i32::from_le_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]);
    let height = i32::from_le_bytes([hdr[8], hdr[9], hdr[10], hdr[11]]);
    let bit_count = u16::from_le_bytes([hdr[14], hdr[15]]) as usize;
    let compression = u32::from_le_bytes([hdr[16], hdr[17], hdr[18], hdr[19]]);
    let clr_used = u32::from_le_bytes([hdr[32], hdr[33], hdr[34], hdr[35]]) as u64;
    let size_image = u32::from_le_bytes([hdr[20], hdr[21], hdr[22], hdr[23]]) as u64;
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
    let entries: u64 = if bit_count <= 8 {
        if clr_used > 0 {
            clr_used
        } else {
            1u64 << bit_count
        }
    } else {
        clr_used
    };
    extra += (entries * 4) as usize;
    let stride: u64 = (bit_count as u64 * w as u64).div_ceil(32) * 4;
    let pixel_bytes = size_image.max(stride.saturating_mul(hgt as u64));
    Some((bi_size, extra, pixel_bytes))
}

/// 理论总长按实际分配截断后的安全读取长度（纯函数）。
///
/// `avail` 来自 `GlobalSize`——剪贴板句柄携带的唯一可靠读边界。截断后连
/// 「头+掩码/调色板」都不完整的载荷没有解析价值，返回 None（调用方按无图处理）。
pub fn dib_read_len(total: usize, avail: usize, header_bytes: usize) -> Option<usize> {
    let read = total.min(avail);
    (read >= header_bytes).then_some(read)
}

fn clipboard_get_dib_format(format: u32) -> Option<Vec<u8>> {
    use windows::Win32::System::DataExchange::GetClipboardData;
    use windows::Win32::System::Memory::GlobalSize;
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
        // GlobalSize = 这块全局内存**实际分配**的大小，是不可信头之外唯一
        // 可靠的读边界。回归（新鲜眼审计 2026-10-04）：初版完全不看它——
        // ① 头部无条件读 40 字节；② 按（可能谎报的）头字段推算 total 后直接
        // `from_raw_parts(p, total)` 整块拷贝。畸形/截断 DIB（biSizeImage 远大于
        // 分配）会让监听线程越界读，最坏直接 access violation 崩掉整个进程。
        let avail = GlobalSize(HGLOBAL(h.0));
        if avail < 40 {
            let _ = GlobalUnlock(HGLOBAL(h.0));
            return None;
        }
        let hdr: [u8; 40] = std::slice::from_raw_parts(p as *const u8, 40)
            .try_into()
            .expect("长度已判 40");
        let layout = dib_layout_from_header(&hdr);
        let _ = GlobalUnlock(HGLOBAL(h.0));
        let (bi_size, extra, pixel_bytes) = layout?;
        // 上限保护：错乱的头部可能声明巨大长度。u64 判定 + 溢出即视为超限
        //（usize 回绕会让荒谬头伪装成小 total 绕过检查）
        let total64 = (bi_size as u64)
            .checked_add(extra as u64)
            .and_then(|t| t.checked_add(pixel_bytes))
            .unwrap_or(u64::MAX);
        if total64 > 512 * 1024 * 1024 {
            return None;
        }
        let read_len = dib_read_len(total64 as usize, avail, bi_size + extra)?;
        Some(std::slice::from_raw_parts(p as *const u8, read_len).to_vec())
    }
}

/// 把纯文本写入剪贴板（`CF_UNICODETEXT`）。
pub fn clipboard_set_text(text: &str) -> bool {
    clipboard_set_std(CF_UNICODETEXT, &build_unicode_text(text))
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
    use windows::Win32::System::Memory::GlobalSize;
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
        // 按 NUL 结尾扫描（CF_UNICODETEXT 保证以 NUL 结尾）；上限取
        // GlobalSize/2 与 8M 字符中较小者：分配边界内必安全，防异常内容
        // （无 NUL）时越界扫到邻近内存。
        let avail = GlobalSize(HGLOBAL(h.0));
        let max_chars = (avail / 2).min(8_000_000);
        let mut len = 0usize;
        while len < max_chars && *p.add(len) != 0 {
            len += 1;
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
