//! 磁盘空间与目录大小（对应 Python 侧 `DiskTab` / `diskusage.py`）。
//!
//! 纯计算部分（字节换算、用量分档、目录累加）与 Win32 取数分开：
//! 取数靠 `GetLogicalDrives` + `GetDiskFreeSpaceExW`，换算与判定全在纯函数里，
//! 这样"可用/已用/百分比"的口径能被逐条断言，而不必依赖真机磁盘状态。

use serde::Serialize;

/// 一个卷的容量信息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DriveInfo {
    /// 盘符，如 `C:`
    pub letter: String,
    /// 卷标（可能为空）
    pub label: String,
    /// 文件系统，如 `NTFS`
    pub fs: String,
    pub total: u64,
    /// 对**当前用户**可用的字节数（受磁盘配额影响，与 total-free 可能不同）
    pub free_to_caller: u64,
    /// 卷上总的空闲字节数（不含配额限制）
    pub free_total: u64,
    /// 是否可移动介质（U 盘/光驱等）
    pub removable: bool,
}

impl DriveInfo {
    /// 已用字节（按卷总空闲算，即"这个盘还剩多少"）。
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.free_total)
    }

    /// 已用百分比（0~100）。总容量为 0 时返回 0 而不是除零。
    pub fn used_percent(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        (self.used() as f64 / self.total as f64) * 100.0
    }

    /// 可移动介质不该用来做"磁盘空间不足"告警（U 盘经常是满的）。
    #[allow(dead_code)] // 目录/驱动器统计工具箱预留面（dir_size 套件）
    pub fn is_fixed(&self) -> bool {
        !self.removable
    }
}

/// 已用比例 → 分档（与其它页面的三档一致）。
pub fn usage_token(percent: f64) -> &'static str {
    if percent >= 90.0 {
        "err"
    } else if percent >= 75.0 {
        "warn"
    } else {
        "ok"
    }
}

/// 人类可读容量。
///
/// 与 Python 的 `human_bytes` 同口径（1024 进制、KB/MB/GB/TB）。
/// `bytes` 而非 `B/s`：磁盘容量不是速率，不能带 `/s`。
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} {}", bytes, UNITS[i])
    } else if v >= 100.0 {
        format!("{v:.0} {}", UNITS[i])
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

/// 目录大小扫描结果的一行。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[allow(dead_code)] // 目录/驱动器统计工具箱预留面（dir_size 套件）
pub struct DirSize {
    pub path: String,
    /// 字节数
    pub bytes: u64,
    /// 文件数
    pub files: u64,
    /// 被跳过的条目数（正在使用/权限不足）
    pub skipped: u64,
}

impl DirSize {
    /// 是否完全没扫到（可能路径不存在）。
    #[allow(dead_code)] // 同上
    pub fn is_empty(&self) -> bool {
        self.bytes == 0 && self.files == 0
    }
}

/// 累加目录扫描结果（纯函数，便于断言跳过与累加口径）。
///
/// `skipped` 必须单独计数：**只报"清理了多少"而不报"跳过了多少"会让用户以为
/// 清干净了**，实际还在用着的文件一个没动。
#[allow(dead_code)] // 同上
pub fn accumulate(results: &[DirSize]) -> DirSize {
    DirSize {
        path: String::new(),
        bytes: results.iter().map(|r| r.bytes).sum(),
        files: results.iter().map(|r| r.files).sum(),
        skipped: results.iter().map(|r| r.skipped).sum(),
    }
}

/// 递归统计目录大小。跳过符号链接/重解析点以免重复计数与死循环。
#[allow(dead_code)] // 同上
pub fn dir_size(path: &std::path::Path) -> DirSize {
    let mut out = DirSize {
        path: path.to_string_lossy().to_string(),
        bytes: 0,
        files: 0,
        skipped: 0,
    };
    let Ok(rd) = std::fs::read_dir(path) else {
        return out;
    };
    for entry in rd.flatten() {
        let Ok(meta) = entry.metadata() else {
            // 权限不足或文件正在被独占：跳过并计数，不中断整轮扫描
            out.skipped += 1;
            continue;
        };
        if meta.is_dir() {
            // 重解析点（junction/symlink）不跟随：跟了会把同一份数据算两次，
            // 而且目录联接指回祖先时会无限递归
            if is_reparse_point(&entry.path()) {
                out.skipped += 1;
                continue;
            }
            let sub = dir_size(&entry.path());
            out.bytes += sub.bytes;
            out.files += sub.files;
            out.skipped += sub.skipped;
        } else {
            out.bytes += meta.len();
            out.files += 1;
        }
    }
    out
}

/// 该路径是否是重解析点（符号链接 / 目录联接）。
pub fn is_reparse_point(path: &std::path::Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

// ---------------- FFI ----------------

#[cfg(windows)]
mod ffi {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{
        GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
    };

    use super::DriveInfo;

    /// 可移动/固定盘的 `GetDriveTypeW` 返回值。
    const DRIVE_REMOVABLE: u32 = 2;
    const DRIVE_FIXED: u32 = 3;
    const DRIVE_REMOTE: u32 = 4;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// 枚举所有本地卷（固定盘 + 可移动盘；网络盘与光驱单独标注）。
    pub fn list_drives() -> Vec<DriveInfo> {
        let mut out = Vec::new();
        let mask = unsafe { GetLogicalDrives() };
        for i in 0..26u32 {
            if mask & (1 << i) == 0 {
                continue;
            }
            let letter = format!("{}:", (b'A' + i as u8) as char);
            let root = format!("{letter}\\");
            let root_w = wide(&root);

            // SAFETY: root_w 以 NUL 结尾且在本轮内有效
            let dtype = unsafe { GetDriveTypeW(PCWSTR(root_w.as_ptr())) };
            if dtype != DRIVE_FIXED && dtype != DRIVE_REMOVABLE && dtype != DRIVE_REMOTE {
                continue; // 光驱(5)/RAM 盘(6)/未知(0) 不列
            }

            let mut free_to_caller = 0u64;
            let mut total = 0u64;
            let mut free_total = 0u64;
            // SAFETY: 三个出参都是本函数栈上的有效 u64
            let ok = unsafe {
                GetDiskFreeSpaceExW(
                    PCWSTR(root_w.as_ptr()),
                    Some(&mut free_to_caller),
                    Some(&mut total),
                    Some(&mut free_total),
                )
            };
            if ok.is_err() {
                continue; // 空光驱/未就绪的读卡器会失败，跳过而不是报错
            }

            // 卷标与文件系统（失败不影响容量展示）
            let mut label_buf = [0u16; 261];
            let mut fs_buf = [0u16; 261];
            let mut serial = 0u32;
            let mut max_comp = 0u32;
            let mut flags = 0u32;
            // SAFETY: 两个缓冲都是本函数栈上定长数组
            let vinfo = unsafe {
                GetVolumeInformationW(
                    PCWSTR(root_w.as_ptr()),
                    Some(&mut label_buf),
                    Some(&mut serial),
                    Some(&mut max_comp),
                    Some(&mut flags),
                    Some(&mut fs_buf),
                )
            };
            let (label, fs) = if vinfo.is_ok() {
                (from_wide(&label_buf), from_wide(&fs_buf))
            } else {
                (String::new(), String::new())
            };

            out.push(DriveInfo {
                letter,
                label,
                fs,
                total,
                free_to_caller,
                free_total,
                removable: dtype == DRIVE_REMOVABLE || dtype == DRIVE_REMOTE,
            });
        }
        out
    }

    fn from_wide(buf: &[u16]) -> String {
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..len])
    }
}

#[cfg(not(windows))]
mod ffi {
    use super::DriveInfo;
    pub fn list_drives() -> Vec<DriveInfo> {
        Vec::new()
    }
}

/// 枚举本地卷（非 Windows 返回空表）。
pub fn list_drives() -> Vec<DriveInfo> {
    ffi::list_drives()
}

/// 按剩余空间升序（最紧张的排前面），可移动盘排在固定盘之后。
pub fn sort_by_pressure(mut drives: Vec<DriveInfo>) -> Vec<DriveInfo> {
    drives.sort_by(|a, b| {
        a.removable
            .cmp(&b.removable)
            .then(a.free_total.cmp(&b.free_total))
    });
    drives
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive(total: u64, free: u64, removable: bool) -> DriveInfo {
        DriveInfo {
            letter: "C:".into(),
            label: "System".into(),
            fs: "NTFS".into(),
            total,
            free_to_caller: free,
            free_total: free,
            removable,
        }
    }

    #[test]
    fn used_and_percent() {
        let d = drive(1000, 250, false);
        assert_eq!(d.used(), 750);
        assert!((d.used_percent() - 75.0).abs() < 1e-9);
    }

    #[test]
    fn zero_total_does_not_divide_by_zero() {
        let d = drive(0, 0, false);
        assert_eq!(d.used(), 0, "已用不得下溢");
        assert_eq!(d.used_percent(), 0.0);
    }

    #[test]
    fn free_greater_than_total_saturates() {
        // 配额/统计异常时 free 可能大于 total；saturating_sub 保证不下溢成天文数字
        let d = drive(100, 200, false);
        assert_eq!(d.used(), 0, "不得下溢成 u64::MAX");
    }

    #[test]
    fn free_to_caller_differs_from_free_total_under_quota() {
        // 两个值语义不同：配额场景下 caller 可用的更少。
        // 已用比例必须按 free_total 算（"这个盘还剩多少"），不能按 caller 的。
        let d = DriveInfo {
            letter: "D:".into(),
            label: String::new(),
            fs: "NTFS".into(),
            total: 1000,
            free_to_caller: 10, // 配额几乎用满
            free_total: 500,    // 卷上其实还很空
            removable: false,
        };
        assert_eq!(d.used(), 500, "已用按卷总空闲算");
        assert!((d.used_percent() - 50.0).abs() < 1e-9);
    }

    #[test]
    fn usage_token_three_tiers() {
        assert_eq!(usage_token(0.0), "ok");
        assert_eq!(usage_token(74.9), "ok");
        assert_eq!(usage_token(75.0), "warn");
        assert_eq!(usage_token(89.9), "warn");
        assert_eq!(usage_token(90.0), "err");
        assert_eq!(usage_token(100.0), "err");
    }

    #[test]
    fn human_size_matches_python_units() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1024), "1.0 KB");
        assert_eq!(human_size(1024 * 1024), "1.0 MB");
        assert_eq!(human_size(1024 * 1024 * 1024), "1.0 GB");
        assert_eq!(human_size(1024u64.pow(4)), "1.0 TB");
    }

    #[test]
    fn human_size_drops_decimals_when_large() {
        assert_eq!(human_size(500 * 1024 * 1024), "500 MB");
    }

    #[test]
    fn human_size_has_no_per_second_suffix() {
        // 容量不是速率：带 /s 会让用户以为这是读写速度
        let s = human_size(1024 * 1024);
        assert!(!s.contains("/s"), "{s}");
    }

    #[test]
    fn accumulate_sums_bytes_files_and_skipped() {
        let rs = vec![
            DirSize {
                path: "a".into(),
                bytes: 100,
                files: 2,
                skipped: 1,
            },
            DirSize {
                path: "b".into(),
                bytes: 250,
                files: 3,
                skipped: 4,
            },
        ];
        let total = accumulate(&rs);
        assert_eq!(total.bytes, 350);
        assert_eq!(total.files, 5);
        assert_eq!(total.skipped, 5, "跳过数必须单独累计并汇报");
    }

    #[test]
    fn accumulate_empty() {
        let t = accumulate(&[]);
        assert!(t.is_empty());
        assert_eq!(t.skipped, 0);
    }

    #[test]
    fn sort_by_pressure_puts_tightest_first_and_removable_last() {
        let drives = vec![
            drive(1000, 10, true),   // U 盘几乎满了
            drive(1000, 500, false), // C 盘还行
            drive(1000, 20, false),  // D 盘很紧张
        ];
        let sorted = sort_by_pressure(drives);
        assert_eq!(sorted[0].free_total, 20, "固定盘里最紧张的排前面");
        assert_eq!(sorted[1].free_total, 500);
        assert!(sorted[2].removable, "可移动盘排最后（不该参与空间告警）");
    }

    #[test]
    fn is_fixed_excludes_removable() {
        assert!(drive(100, 50, false).is_fixed());
        assert!(!drive(100, 50, true).is_fixed());
    }

    #[test]
    fn dir_size_of_missing_path_is_empty_not_error() {
        let d = dir_size(std::path::Path::new(r"Z:\SysKit不存在目录"));
        assert!(d.is_empty());
        assert_eq!(d.skipped, 0);
    }

    #[test]
    fn dir_size_counts_real_files() {
        // 用测试临时目录，不碰用户数据
        let base = std::env::temp_dir().join("sk_dirsize_test");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("sub")).unwrap();
        std::fs::write(base.join("a.bin"), vec![0u8; 100]).unwrap();
        std::fs::write(base.join("sub/b.bin"), vec![0u8; 250]).unwrap();

        let d = dir_size(&base);
        assert_eq!(d.files, 2, "子目录里的文件也要算");
        assert_eq!(d.bytes, 350);
        assert_eq!(d.skipped, 0);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn live_drive_enumeration_is_plausible() {
        let drives = list_drives();
        assert!(!drives.is_empty(), "真机至少应有一个本地卷");
        let sys = drives.iter().find(|d| d.letter == "C:");
        assert!(sys.is_some(), "应有 C 盘");
        let c = sys.unwrap();
        assert!(c.total > 0, "C 盘总容量应大于 0");
        assert!(c.free_total <= c.total, "空闲不得大于总容量");
        assert!(
            !c.fs.is_empty(),
            "文件系统类型应可读（真实断言而非只看不 panic）"
        );
        assert!(c.used_percent() >= 0.0 && c.used_percent() <= 100.0);
    }

    #[test]
    fn live_drive_letters_are_well_formed() {
        for d in list_drives() {
            assert_eq!(d.letter.len(), 2, "盘符应是 `X:` 形态，实际 {}", d.letter);
            assert!(d.letter.ends_with(':'));
        }
    }
}
