//! HTTPS 下载测速：WinInet（系统 TLS 栈 + 系统根证书 + 跟随系统代理），
//! 与 Python 版 `urllib.request`（同样走系统栈）完全同构——零新依赖。
//!
//! 语义对齐 Python `ToolboxSpeed._DownloadWorker.run`：逐 URL 独立开窗 `seconds`
//! 秒（避免跨 URL 累计字节导致速率高估），64KB 分块读，记录总字节与峰值 1 秒
//! 瞬时速率；一个 URL 失败跳过下一个（不拖垮整轮）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use windows::core::HSTRING;
use windows::Win32::Networking::WinInet::{
    InternetCloseHandle, InternetOpenUrlW, InternetOpenW, InternetReadFile, InternetSetOptionW,
    INTERNET_FLAG_RELOAD, INTERNET_OPEN_TYPE_PRECONFIG, INTERNET_OPTION_CONNECT_TIMEOUT,
    INTERNET_OPTION_RECEIVE_TIMEOUT, INTERNET_OPTION_SEND_TIMEOUT,
};

/// 与 Python `TEST_URLS` 逐字一致（大文件直链，下载即断不落盘）。
pub const TEST_URLS: [&str; 3] = [
    "https://dldir1.qq.com/weixin/Windows/WeChatSetup.exe",
    "https://dldir1.qq.com/qqfile/qq/QQNT/Windows/QQ_9.9.9_250avis_x64.exe",
    "https://mirrors.aliyun.com/debian-cd/current/amd64/iso-cd/debian-12.9.0-amd64-netinst.iso",
];

/// 单请求超时（毫秒）：连接/发送/接收统一 10s，对齐 Python
/// `urllib.request.urlopen(req, timeout=10)`。
///
/// **为什么必须有**：`InternetReadFile` 是阻塞读，CANCEL 标志只在两次读之间
/// 检查——黑洞连接（TCP 建立后服务器不发数据，中间设备也会造成）会让读永不
/// 返回，测速页永远转圈且「停止」按钮无效。Python 版靠 socket 超时把这种
/// 连接变成异常跳过下一个 URL；Rust 版不设超时就丢了这个保护。
pub const REQUEST_TIMEOUT_MS: u32 = 10_000;

#[derive(Debug, Clone, serde::Serialize)]
pub struct UrlResult {
    pub url: String,
    pub ok: bool,
    pub err: String,
    /// 开窗期内实际下载的字节数。
    pub bytes: u64,
    /// 平均速率（字节/秒）= bytes / 实际耗时。
    pub avg_bps: f64,
    /// 峰值 1 秒窗口瞬时速率（字节/秒）。
    pub peak_bps: f64,
}

static CANCEL: AtomicBool = AtomicBool::new(false);

/// 请求中止进行中的测速（前端「停止」按钮用）。
#[allow(dead_code)] // 测速取消能力已实现，UI 接线待做（超时护栏已由并行会话补齐）
pub fn cancel() {
    CANCEL.store(true, Ordering::SeqCst);
}

pub fn reset_cancel() {
    CANCEL.store(false, Ordering::SeqCst);
}

/// 单 URL 开窗下载。`seconds` 为窗口时长；返回该 URL 的计量结果。
fn probe_url(url: &str, seconds: u64) -> UrlResult {
    let mut out = UrlResult {
        url: url.to_string(),
        ok: false,
        err: String::new(),
        bytes: 0,
        avg_bps: 0.0,
        peak_bps: 0.0,
    };
    unsafe {
        // PRECONFIG = 跟随系统代理（与 Python urllib 同构）；
        // UA 走 InternetOpen 的 agent（Python 的 Request UA 同义）
        let hinternet = InternetOpenW(
            &HSTRING::from("SysKit/1.0"),
            INTERNET_OPEN_TYPE_PRECONFIG.0,
            None,
            None,
            0,
        );
        if hinternet.is_null() {
            out.err = "InternetOpen 失败".into();
            return out;
        }
        // 超时必须设在 session 句柄上（InternetOpenUrl 之前才对后续请求生效）：
        // 连接/发送/接收三处都按 Python timeout=10s 收口，防黑洞连接永久挂住
        for opt in [
            INTERNET_OPTION_CONNECT_TIMEOUT,
            INTERNET_OPTION_SEND_TIMEOUT,
            INTERNET_OPTION_RECEIVE_TIMEOUT,
        ] {
            let v = REQUEST_TIMEOUT_MS;
            let _ = InternetSetOptionW(
                Some(hinternet as *const core::ffi::c_void),
                opt,
                Some(&v as *const u32 as *const core::ffi::c_void),
                std::mem::size_of::<u32>() as u32,
            );
        }
        // RELOAD：禁缓存，保证测的是真网络
        let hurl = InternetOpenUrlW(
            hinternet,
            &HSTRING::from(url),
            None,
            INTERNET_FLAG_RELOAD,
            None,
        );
        if hurl.is_null() {
            out.err = "打开 URL 失败（网络不可达/证书问题）".into();
            let _ = InternetCloseHandle(hinternet);
            return out;
        }

        let start = Instant::now();
        let deadline = start + std::time::Duration::from_secs(seconds);
        let mut buf = vec![0u8; 65536];
        let mut last_t = start;
        let mut last_b = 0u64;
        loop {
            if CANCEL.load(Ordering::SeqCst) || Instant::now() >= deadline {
                break;
            }
            let mut got = 0u32;
            if InternetReadFile(hurl, buf.as_mut_ptr() as _, buf.len() as u32, &mut got).is_err()
                || got == 0
            {
                break; // EOF 或读错误：本 URL 计量结束
            }
            out.bytes += got as u64;
            let now = Instant::now();
            if (now - last_t).as_secs_f64() >= 1.0 {
                let inst = (out.bytes - last_b) as f64 / (now - last_t).as_secs_f64();
                out.peak_bps = out.peak_bps.max(inst);
                last_t = now;
                last_b = out.bytes;
            }
        }
        let elapsed = start.elapsed().as_secs_f64().max(0.001);
        out.avg_bps = out.bytes as f64 / elapsed;
        out.ok = out.bytes > 0;
        out.peak_bps = out.peak_bps.max(out.avg_bps);
        let _ = InternetCloseHandle(hurl);
        let _ = InternetCloseHandle(hinternet);
    }
    out
}

/// 整轮测速：逐 URL 独立开窗。`cancel()` 可中止（中止前的已计量 URL 照常返回）。
pub fn run_probe_all(seconds: u64) -> Vec<UrlResult> {
    reset_cancel();
    TEST_URLS.iter().map(|u| probe_url(u, seconds)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_timeout_matches_python_urlopen() {
        // Python: urllib.request.urlopen(req, timeout=10)——10 秒覆盖连接/读全程。
        // 回归：初版未设任何 WinInet 超时，黑洞连接（TCP 建立后无数据）会让
        // InternetReadFile 永久阻塞，CANCEL 只在两次读之间检查、救不了已阻塞的读。
        assert_eq!(REQUEST_TIMEOUT_MS, 10_000);
    }

    #[test]
    fn cancel_flag_round_trip() {
        reset_cancel();
        assert!(!CANCEL.load(Ordering::SeqCst));
        cancel();
        assert!(CANCEL.load(Ordering::SeqCst));
        reset_cancel();
        assert!(!CANCEL.load(Ordering::SeqCst));
    }
}
