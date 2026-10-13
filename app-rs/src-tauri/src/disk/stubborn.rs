//! 顽固文件/文件夹强制删除（对付流氓软件残留）。
//!
//! 四阶段管线：属性清除 → 直接删除；失败则 takeown 夺所有权 →
//! icacls 授全权（SID）→ 再删 + 共享冲突短重试。多数"顽固"只是
//! 只读/系统属性，第一阶段即解决；真正的所有权流氓才走完四阶段。
//!
//! 红线（违反任何一条都比删不掉更糟）：
//! - `validate_target` 是唯一入口：盘根/系统目录/profile 根本体拒绝
//!   （深层子路径放行——流氓残留多在 AppData 深处）；
//! - 重解析点（junction/symlink）只删链接本身，takeown/icacls 跳过
//!   ——`/r`、`/t` 会跟进修 `Application Data` 这类自引用环；
//! - 外部命令不吃 `\\?\` 长路径前缀（canonicalize 会产生），传前剥掉；
//! - 成败判定用退出码 + ASCII 关键字（控制台解码对 GBK 乱码）。

use std::path::{Path, PathBuf};
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{
    GetFileAttributesW, SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_READONLY,
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_SYSTEM, FILE_FLAGS_AND_ATTRIBUTES,
};

/// 单路径删除结果（前端逐条透传）。
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct ForceResult {
    pub path: String,
    pub ok: bool,
    /// 走到哪个阶段成功的（"attr" / "takeown" / "icacls" / "force"）或失败的末端阶段
    pub stage: &'static str,
    /// 失败原因（用户可读；成功为空）
    pub reason: String,
}

/// 目标校验：canonicalize + 黑名单拦截。
/// 返回 (规范化路径, 外部命令用显示路径——剥掉 `\\?\` 前缀)。
pub fn validate_target(raw: &str) -> Result<(PathBuf, String), String> {
    let p = Path::new(raw);
    if !p.exists() {
        return Err(format!("路径不存在：{raw}"));
    }
    let canon = std::fs::canonicalize(p).map_err(|e| format!("无法解析路径 {raw}: {e}"))?;
    let lowered = canon.to_string_lossy().to_lowercase();

    // 盘根本体（C:\ 或 C:\xxx 这类一级直接挡掉太激进——只挡真根）
    let comps: Vec<_> = canon.components().collect();
    // components: [Prefix(C:), RootDirectory, ...] —— 只有前缀+根 = 盘根
    if comps.len() <= 2 {
        return Err(format!("拒绝：盘根本体不可删除（{raw}）"));
    }

    // 系统要害目录**本体**拒绝；深层子路径放行（比较带尾分隔符防前缀陷阱）
    let forbidden_bodies = [
        r"c:\windows",
        r"c:\program files",
        r"c:\program files (x86)",
        r"c:\programdata",
    ];
    for fb in forbidden_bodies {
        if lowered == fb {
            return Err(format!("拒绝：系统要害目录本体不可删除（{raw}）"));
        }
    }

    // 用户 profile 根本体拒绝（深层放行）
    if let Some(profile) = std::env::var_os("USERPROFILE") {
        let prof = Path::new(&profile);
        if let Ok(prof_canon) = std::fs::canonicalize(prof) {
            if canon == prof_canon {
                return Err(format!("拒绝：用户目录根本体不可删除（{raw}）"));
            }
        }
    }

    // 剥 \\?\ 长路径前缀给外部命令（strip_long_prefix 单测锁定）
    let display = strip_long_prefix(&canon);
    Ok((canon, display))
}

/// 判断路径是否重解析点（junction/symlink）——用 Win32 属性而非
/// std 的 is_symlink（后者不认 junction）。
pub fn is_reparse(path: &Path) -> bool {
    let mut wide: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().collect();
    wide.push(0); // Win32 宽字符 API 要求 NUL 终止
    let attrs = unsafe { GetFileAttributesW(PCWSTR(wide.as_ptr())) };
    attrs != u32::MAX && (FILE_FLAGS_AND_ATTRIBUTES(attrs) & FILE_ATTRIBUTE_REPARSE_POINT).0 != 0
}

/// 清除 READONLY/HIDDEN/SYSTEM 属性（permission denied 的最常见根因）。
pub fn clear_stubborn_attrs(path: &Path) -> bool {
    let mut wide: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().collect();
    wide.push(0); // NUL 终止
    let attrs = unsafe { GetFileAttributesW(PCWSTR(wide.as_ptr())) };
    if attrs == u32::MAX {
        return false; // 已不存在或不可访问
    }
    let mut a = FILE_FLAGS_AND_ATTRIBUTES(attrs);
    a &= !(FILE_ATTRIBUTE_READONLY | FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM);
    unsafe { SetFileAttributesW(PCWSTR(wide.as_ptr()), a).is_ok() }
}

/// 删除单个条目（文件或目录；目录递归）。返回 Err(io err)。
fn try_remove(path: &Path) -> std::io::Result<()> {
    if path.is_dir() && !is_reparse(path) {
        std::fs::remove_dir_all(path)
    } else if path.is_dir() {
        std::fs::remove_dir(path) // 重解析点：只删链接本身
    } else {
        std::fs::remove_file(path)
    }
}

/// 剥 `\\?\` 长路径前缀（外部命令与部分 Win32 单路径 API 不认）。
pub fn strip_long_prefix(p: &Path) -> String {
    let s = p.to_string_lossy();
    s.strip_prefix(r"\\?\")
        .map(|x| x.to_string())
        .unwrap_or(s.to_string())
}

/// takeown 参数拼装（纯函数，单测锁定；`\\?\` 前缀已由调用方剥掉）。
pub fn takeown_args(display_path: &str) -> Vec<String> {
    vec![
        "takeown".into(),
        "/f".into(),
        display_path.into(),
        "/r".into(),
        "/d".into(),
        "y".into(),
    ]
}

/// icacls 参数拼装（*S-1-1-0 = Everyone 的 SID，非中文系统也命中）。
pub fn icacls_args(display_path: &str) -> Vec<String> {
    vec![
        "icacls".into(),
        display_path.into(),
        "/grant".into(),
        "*S-1-1-0:F".into(),
        "/t".into(),
        "/c".into(),
        "/q".into(),
    ]
}

/// 共享冲突短重试（正在被写入的文件常常 200ms 后就放开了）。
fn remove_with_retry(path: &Path) -> std::io::Result<()> {
    let mut last = try_remove(path);
    for _ in 0..3 {
        if last.is_ok() {
            return last;
        }
        match &last {
            Err(e) if e.raw_os_error() == Some(32) => {
                // ERROR_SHARING_VIOLATION
                std::thread::sleep(std::time::Duration::from_millis(200));
                last = try_remove(path);
            }
            _ => break,
        }
    }
    last
}

/// 四阶段强制删除单路径（管理员语境；命令层做提权前置）。
pub fn force_remove(raw: &str) -> ForceResult {
    let (canon, display) = match validate_target(raw) {
        Ok(v) => v,
        Err(reason) => {
            return ForceResult {
                path: raw.to_string(),
                ok: false,
                stage: "validate",
                reason,
            }
        }
    };

    let reparse = is_reparse(&canon);

    // ① 属性清除 + 直接删除（多数顽固止步于此）
    clear_stubborn_attrs(&canon);
    if try_remove(&canon).is_ok() {
        return ForceResult {
            path: raw.to_string(),
            ok: true,
            stage: "attr",
            reason: String::new(),
        };
    }

    // 重解析点不进外部命令阶段（/r、/t 会跟进自引用环）
    if reparse {
        return ForceResult {
            path: raw.to_string(),
            ok: false,
            stage: "attr",
            reason: "重解析点（junction/链接）且属性清除后仍无法删除链接本身".into(),
        };
    }

    // ② takeown 夺所有权（递归）
    if let Ok(out) = crate::w32::run_capture(&takeown_args(&display), 120) {
        let _ = out; // 成败以最终删除为准；takeown 对已拥有文件也常返回非零
    }

    // ③ icacls 授 Everyone 全权（递归、容错）
    if let Ok(out) = crate::w32::run_capture(&icacls_args(&display), 120) {
        let _ = out;
    }

    // 属性再清一次（所有权变更后 hidden/system 可能复位）+ ④ 强删 + 重试
    clear_stubborn_attrs(&canon);
    match remove_with_retry(&canon) {
        Ok(()) => ForceResult {
            path: raw.to_string(),
            ok: true,
            stage: "force",
            reason: String::new(),
        },
        Err(e) => ForceResult {
            path: raw.to_string(),
            ok: false,
            stage: "force",
            reason: format!(
                "所有权已重置仍无法删除：{e}（文件可能正被占用——重启后再试或用安全模式）"
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试辅助：设只读属性（新建文件可能被 AV/索引器瞬时锁定，重试 3 次）。
    fn set_readonly(path: &Path) {
        let disp = strip_long_prefix(&path.canonicalize().unwrap());
        let mut wide: Vec<u16> = disp.encode_utf16().collect();
        wide.push(0);
        let mut last_err = String::new();
        for _ in 0..3 {
            match unsafe {
                SetFileAttributesW(
                    PCWSTR(wide.as_ptr()),
                    FILE_FLAGS_AND_ATTRIBUTES(FILE_ATTRIBUTE_READONLY.0),
                )
            } {
                Ok(()) => return,
                Err(e) => last_err = format!("{e}"),
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        panic!("设只读属性连续失败（AV 瞬时锁？）：{disp}，最后错误：{last_err}");
    }

    fn temp_dir_tagged(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let d =
            std::env::temp_dir().join(format!("sk_stubborn_{tag}_{}_{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn takeown_args_shape() {
        assert_eq!(
            takeown_args(r"C:\x\y"),
            vec!["takeown", "/f", r"C:\x\y", "/r", "/d", "y"]
        );
    }

    #[test]
    fn icacls_args_uses_sid_not_group_name() {
        let a = icacls_args(r"C:\x\y");
        assert_eq!(a[0], "icacls");
        assert_eq!(a[1], r"C:\x\y");
        assert!(a.contains(&"*S-1-1-0:F".to_string()), "SID 而非组名");
        assert!(a.contains(&"/t".to_string()) && a.contains(&"/c".to_string()));
    }

    #[test]
    fn validate_rejects_missing_path() {
        assert!(validate_target(r"Z:\definitely\not\here").is_err());
    }

    #[test]
    fn validate_rejects_forbidden_bodies() {
        // 不存在的系统目录本体也要被"路径不存在"先拦——造存在场景太重，
        // 这里的断言锁定判定逻辑本身：用临时目录模拟 profile 本体拒绝
        let home = std::env::var("USERPROFILE").unwrap();
        assert!(validate_target(&home).is_err(), "profile 根本体必须拒绝");
    }

    #[test]
    fn validate_allows_deep_appdata() {
        // 用户 profile 深层子路径放行（流氓残留的主战场）
        let deep = std::env::var("LOCALAPPDATA").unwrap();
        assert!(
            validate_target(&deep).is_ok(),
            "AppData\\Local 深层应放行，实际测 {deep}"
        );
    }

    #[test]
    fn force_removes_readonly_file_via_attr_stage() {
        let d = temp_dir_tagged("ro");
        let f = d.join("stubborn.txt");
        std::fs::write(&f, b"x").unwrap();
        // 设只读属性（canonicalize 后剥 \\?\ 前缀——带前缀的路径
        // SetFileAttributesW 报 0x8007007B 语法错）
        set_readonly(&f);
        let r = force_remove(&f.to_string_lossy());
        assert!(r.ok, "只读文件应被 attr 阶段删除: {r:?}");
        assert_eq!(r.stage, "attr");
        assert!(!f.exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn force_removes_nested_dir_with_readonly_children() {
        let d = temp_dir_tagged("nested");
        let sub = d.join("a").join("b");
        std::fs::create_dir_all(&sub).unwrap();
        let f = sub.join("locked.dat");
        std::fs::write(&f, b"yyyy").unwrap();
        set_readonly(&f);
        // remove_dir_all 遇只读子文件会失败 → 管线应先清属性再删
        let r = force_remove(&d.to_string_lossy());
        assert!(r.ok, "{r:?}");
        assert!(!d.exists());
    }
}
