//! 白名单式垃圾清理（对应 Python 侧 `system_page.CleanupTab`）。
//!
//! ## 安全模型（三条，缺一不可）
//!
//! 1. **白名单**：只清理写死的位置（用户临时目录、Windows 临时目录、缩略图
//!    缓存、回收站、Windows 更新缓存、崩溃转储、错误报告队列、着色器缓存），
//!    不接受调用方传入任意路径。
//! 2. **只删文件，不删目录**：目录（含空目录）一律保留。删目录会连带删掉正在使用
//!    的文件所属结构，且一旦误删空目录，某些程序的缓存目录就不再自动重建。
//! 3. **正在使用的文件跳过并计数**：被独占的文件删不掉是**正常情况**，
//!    必须汇报"跳过了多少"，否则用户以为清干净了、实际一个都没动。
//!
//! 这里不做"智能判断"（比如按内容筛选）：清理工具最大的风险是把用户文件当垃圾。
//! 白名单 + 只删文件 + 名字过滤（缩略图/转储）是最保守且够用的组合。

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::disk::overview::human_size;

/// 清理目标（v1.3 从 4 类扩到 8 类；前 4 个 key 与 Python 一致）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanTarget {
    UserTemp,
    WinTemp,
    ThumbCache,
    Recycle,
    WinUpdateCache,
    CrashDumps,
    WerReports,
    ShaderCache,
}

/// 全部清理目标及其显示名（前 4 个顺序与 Python 一致；v1.3 新增 4 类排后）。
pub const CLEAN_TARGETS: [(CleanTarget, &str); 8] = [
    (CleanTarget::UserTemp, "用户临时文件"),
    (CleanTarget::WinTemp, "Windows 临时文件"),
    (CleanTarget::ThumbCache, "缩略图缓存"),
    (CleanTarget::Recycle, "回收站"),
    (CleanTarget::WinUpdateCache, "Windows 更新缓存"),
    (CleanTarget::CrashDumps, "崩溃转储文件"),
    (CleanTarget::WerReports, "错误报告队列"),
    (CleanTarget::ShaderCache, "着色器缓存"),
];

impl CleanTarget {
    pub fn key(self) -> &'static str {
        match self {
            CleanTarget::UserTemp => "user_temp",
            CleanTarget::WinTemp => "win_temp",
            CleanTarget::ThumbCache => "thumbcache",
            CleanTarget::Recycle => "recycle",
            CleanTarget::WinUpdateCache => "win_update_cache",
            CleanTarget::CrashDumps => "crash_dumps",
            CleanTarget::WerReports => "wer_reports",
            CleanTarget::ShaderCache => "shader_cache",
        }
    }

    pub fn title(self) -> &'static str {
        CLEAN_TARGETS
            .iter()
            .find(|(t, _)| *t == self)
            .map(|(_, title)| *title)
            .unwrap_or("未知")
    }

    pub fn from_key(key: &str) -> Option<Self> {
        CLEAN_TARGETS
            .iter()
            .find(|(t, _)| t.key() == key)
            .map(|(t, _)| *t)
    }

    /// 该目标的候选根目录（可多个：崩溃转储在用户与系统两处；回收站走 Shell
    /// COM 清空，不是普通目录遍历）。全部不存在时该目标计 0。
    pub fn roots(self) -> Vec<PathBuf> {
        let env_dir = |var: &str, sub: &str| {
            std::env::var(var)
                .ok()
                .filter(|s| !s.is_empty())
                .map(|s| PathBuf::from(s).join(sub))
        };
        match self {
            CleanTarget::UserTemp => std::env::var("TEMP")
                .ok()
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
                .into_iter()
                .collect(),
            CleanTarget::WinTemp => vec![PathBuf::from(r"C:\Windows\Temp")],
            CleanTarget::ThumbCache => env_dir("LOCALAPPDATA", r"Microsoft\Windows\Explorer")
                .into_iter()
                .collect(),
            CleanTarget::Recycle => Vec::new(),
            CleanTarget::WinUpdateCache => {
                vec![PathBuf::from(r"C:\Windows\SoftwareDistribution\Download")]
            }
            CleanTarget::CrashDumps => {
                let mut v = env_dir("LOCALAPPDATA", "CrashDumps")
                    .map(|p| vec![p])
                    .unwrap_or_default();
                v.push(PathBuf::from(r"C:\Windows\Minidump"));
                v
            }
            CleanTarget::WerReports => {
                vec![PathBuf::from(r"C:\ProgramData\Microsoft\Windows\WER")]
            }
            CleanTarget::ShaderCache => env_dir("LOCALAPPDATA", r"Microsoft\DirectX Shader Cache")
                .into_iter()
                .collect(),
        }
    }

    /// 该目标的文件名过滤（None = 全部文件都算；详见 [`should_clean`]）。
    pub fn name_filter(self) -> Option<NameFilter> {
        match self {
            CleanTarget::ThumbCache => Some(NameFilter::Prefix("thumbcache")),
            // 崩溃转储目录里偶尔混有非转储文件（如进程存活时的临时写入）
            CleanTarget::CrashDumps => Some(NameFilter::Ext(".dmp")),
            _ => None,
        }
    }
}

/// 文件名过滤器：前缀（缩略图）或扩展名（转储）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameFilter {
    Prefix(&'static str),
    Ext(&'static str),
}

/// 一次清理的结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CleanOutcome {
    /// 成功删除的文件数
    pub deleted: u64,
    /// 释放的字节数
    pub freed: u64,
    /// 删除失败（多数是"正在使用"）
    pub failed: u64,
    /// 失败文件名的样本（最多 10 个，便于提示）
    pub errors: Vec<String>,
    /// 是否跳过了整个目标（根目录不存在等）
    pub skipped_roots: Vec<String>,
}

impl CleanOutcome {
    pub fn empty() -> Self {
        Self {
            deleted: 0,
            freed: 0,
            failed: 0,
            errors: Vec::new(),
            skipped_roots: Vec::new(),
        }
    }

    /// 面向用户的摘要文案。
    pub fn summary(&self) -> String {
        let mut s = format!(
            "已删除 {} 个文件，释放 {}",
            self.deleted,
            human_size(self.freed)
        );
        if self.failed > 0 {
            // 明确说明是"正在使用"，否则用户会以为清理坏了
            s.push_str(&format!(
                "；{} 个文件被占用或无权限已跳过（正在使用的文件属正常）",
                self.failed
            ));
        }
        if !self.skipped_roots.is_empty() {
            s.push_str(&format!(
                "；{} 个位置不存在已跳过",
                self.skipped_roots.len()
            ));
        }
        s
    }

    /// 合并两次清理结果。
    pub fn merge(mut self, other: CleanOutcome) -> Self {
        self.deleted += other.deleted;
        self.freed += other.freed;
        self.failed += other.failed;
        self.errors.extend(other.errors);
        self.errors.truncate(10);
        self.skipped_roots.extend(other.skipped_roots);
        self
    }
}

/// 该文件是否应被清理（名字过滤：缩略图前缀 / 转储扩展名）。
pub fn should_clean(target: CleanTarget, file_name: &str) -> bool {
    match target.name_filter() {
        Some(NameFilter::Prefix(p)) => file_name.to_lowercase().starts_with(p),
        Some(NameFilter::Ext(ext)) => file_name.to_lowercase().ends_with(ext),
        None => true,
    }
}

/// 路径是否位于给定根目录之内（**防越界删除的最后一道闸**）。
///
/// 用词法前缀比较而不是 `canonicalize`：后者要求路径存在，而我们要在删除前判断，
/// 且目标可能刚好是坏链接。比较时按小写（Windows 不敏感）并确保根目录以分隔符结尾，
/// 否则 `C:\Windows\TempBackup` 会被误判为在 `C:\Windows\Temp` 之内。
pub fn is_within(root: &Path, candidate: &Path) -> bool {
    let r = root.to_string_lossy().to_lowercase().replace('/', "\\");
    let c = candidate
        .to_string_lossy()
        .to_lowercase()
        .replace('/', "\\");
    let r = if r.ends_with('\\') {
        r
    } else {
        format!("{r}\\")
    };
    c.starts_with(&r)
}

/// 递归收集根目录下的所有**文件**（不含目录）。
pub fn collect_files(root: &Path, target: CleanTarget) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            // 不跟随重解析点：junction 指回祖先会无限递归，且同一份数据会被删两次
            if crate::disk::overview::is_reparse_point(&path) {
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            if !meta.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if should_clean(target, &name) {
                out.push(path);
            }
        }
    }
    out
}

/// 扫描目标大小（不删除；多根目录求和）。
pub fn scan_target(target: CleanTarget) -> u64 {
    target
        .roots()
        .iter()
        .map(|root| scan_dir(target, root))
        .sum()
}

/// 扫描指定根目录的大小（与 [`scan_target`] 同一口径，可注入路径以便测试）。
pub fn scan_dir(target: CleanTarget, root: &Path) -> u64 {
    if !root.is_dir() {
        return 0;
    }
    collect_files(root, target)
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .sum()
}

/// 清理一个目标的全部根目录（多根求和合并；回收站无根返回空结果）。
pub fn clean_target(target: CleanTarget) -> CleanOutcome {
    let mut out = CleanOutcome::empty();
    for root in target.roots() {
        out = out.merge(clean_dir(target, &root));
    }
    out
}

/// 清理一个目标目录下的文件（只删文件）。
///
/// `collect_files` 已经跳过了重解析点；删除前**再判一次**是否在根目录内——
/// 收集与删除之间目录内容可能变化，这是防越界的最后一道闸。
pub fn clean_dir(target: CleanTarget, root: &Path) -> CleanOutcome {
    let mut out = CleanOutcome::empty();
    if !root.is_dir() {
        out.skipped_roots.push(root.to_string_lossy().to_string());
        return out;
    }
    for path in collect_files(root, target) {
        if !is_within(root, &path) {
            // 不该发生；真发生说明收集逻辑被绕过，宁可漏删也不越界删
            out.failed += 1;
            if out.errors.len() < 10 {
                out.errors
                    .push(format!("越界已拒绝: {}", path.to_string_lossy()));
            }
            continue;
        }
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        match std::fs::remove_file(&path) {
            Ok(()) => {
                out.deleted += 1;
                out.freed += size;
            }
            Err(_) => {
                // 正在使用/只读/权限不足：跳过并计数，不中断整轮
                out.failed += 1;
                if out.errors.len() < 10 {
                    out.errors.push(
                        path.file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default(),
                    );
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sk_clean_{tag}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn target_keys_match_python() {
        let keys: Vec<&str> = CLEAN_TARGETS.iter().map(|(t, _)| t.key()).collect();
        // 前 4 个与 Python 顺序一致；v1.3 新增 4 类排后
        assert_eq!(
            keys,
            vec![
                "user_temp",
                "win_temp",
                "thumbcache",
                "recycle",
                "win_update_cache",
                "crash_dumps",
                "wer_reports",
                "shader_cache",
            ]
        );
        for (t, title) in CLEAN_TARGETS {
            assert_eq!(CleanTarget::from_key(t.key()), Some(t));
            assert_eq!(t.title(), title, "显示名应与常量一致");
        }
        assert_eq!(CleanTarget::from_key("nope"), None);
    }

    #[test]
    fn only_thumbcache_has_name_filter() {
        assert_eq!(
            CleanTarget::ThumbCache.name_filter(),
            Some(NameFilter::Prefix("thumbcache"))
        );
        assert_eq!(CleanTarget::UserTemp.name_filter(), None);
        assert_eq!(CleanTarget::WinTemp.name_filter(), None);
        assert_eq!(CleanTarget::Recycle.name_filter(), None);
        assert_eq!(
            CleanTarget::CrashDumps.name_filter(),
            Some(NameFilter::Ext(".dmp"))
        );
    }

    #[test]
    fn should_clean_applies_thumbcache_filter() {
        // Explorer 目录里还有用户的其他配置，无差别清空会一起删掉
        assert!(should_clean(CleanTarget::ThumbCache, "thumbcache_96.db"));
        assert!(
            should_clean(CleanTarget::ThumbCache, "THUMBCACHE_256.DB"),
            "大小写不敏感"
        );
        assert!(!should_clean(CleanTarget::ThumbCache, "iconcache_96.db"));
        assert!(!should_clean(CleanTarget::ThumbCache, "settings.ini"));
        // 其它目标不过滤
        assert!(should_clean(CleanTarget::UserTemp, "anything.tmp"));
    }

    #[test]
    fn is_within_rejects_sibling_with_shared_prefix() {
        // 这是前缀比较最容易错的地方：TempBackup 不在 Temp 之内
        let root = Path::new(r"C:\Windows\Temp");
        assert!(is_within(root, Path::new(r"C:\Windows\Temp\a\b.tmp")));
        assert!(is_within(root, Path::new(r"C:\Windows\Temp\a.tmp")));
        assert!(
            !is_within(root, Path::new(r"C:\Windows\TempBackup\x.tmp")),
            "共享前缀的兄弟目录必须被拒绝"
        );
        assert!(!is_within(root, Path::new(r"C:\Windows\Other\x.tmp")));
        assert!(!is_within(root, Path::new(r"C:\Windows")));
    }

    #[test]
    fn is_within_is_case_and_separator_insensitive() {
        let root = Path::new(r"C:\Windows\Temp");
        assert!(is_within(root, Path::new(r"c:\windows\temp\x.tmp")));
        assert!(is_within(root, Path::new("C:/Windows/Temp/sub/x.tmp")));
    }

    #[test]
    fn is_within_handles_trailing_separator_on_root() {
        assert!(is_within(Path::new(r"C:\Temp\"), Path::new(r"C:\Temp\a")));
    }

    #[test]
    fn is_within_rejects_the_root_itself() {
        // 根目录本身不是"其内的文件"，不该被当作可删项
        assert!(!is_within(Path::new(r"C:\Temp"), Path::new(r"C:\Temp")));
    }

    #[test]
    fn collect_files_finds_nested_files_only() {
        let root = temp_root("collect");
        std::fs::create_dir_all(root.join("sub/deep")).unwrap();
        std::fs::write(root.join("a.tmp"), b"12345").unwrap();
        std::fs::write(root.join("sub/b.tmp"), b"1234567890").unwrap();
        std::fs::write(root.join("sub/deep/c.tmp"), b"x").unwrap();

        let files = collect_files(&root, CleanTarget::UserTemp);
        assert_eq!(files.len(), 3, "递归到的所有文件都要收集: {files:?}");
        // 目录本身不得出现在结果里（只删文件）
        assert!(files.iter().all(|p| p.is_file()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn collect_files_respects_name_filter() {
        let root = temp_root("filter");
        std::fs::write(root.join("thumbcache_96.db"), b"aaaa").unwrap();
        std::fs::write(root.join("iconcache_96.db"), b"bbbb").unwrap();
        let files = collect_files(&root, CleanTarget::ThumbCache);
        assert_eq!(files.len(), 1, "只收集 thumbcache*: {files:?}");
        assert!(files[0].to_string_lossy().contains("thumbcache"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn clean_deletes_files_and_reports_freed_bytes() {
        let root = temp_root("clean");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("a.tmp"), vec![0u8; 100]).unwrap();
        std::fs::write(root.join("sub/b.tmp"), vec![0u8; 250]).unwrap();

        let out = clean_dir(CleanTarget::UserTemp, &root);
        assert_eq!(out.deleted, 2);
        assert_eq!(out.freed, 350);
        assert_eq!(out.failed, 0);
        // 目录必须保留（只删文件）
        assert!(root.is_dir(), "根目录不得被删");
        assert!(root.join("sub").is_dir(), "子目录不得被删");
        assert!(!root.join("a.tmp").exists());
        assert!(!root.join("sub/b.tmp").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn clean_scans_and_deletes_are_consistent() {
        let root = temp_root("scan");
        std::fs::write(root.join("x.tmp"), vec![0u8; 1024]).unwrap();
        std::fs::write(root.join("y.tmp"), vec![0u8; 2048]).unwrap();
        // 用显式根目录断言扫描口径：collect → 元数据求和 = 3072
        let files = collect_files(&root, CleanTarget::UserTemp);
        let sum: u64 = files
            .iter()
            .filter_map(|p| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .sum();
        assert_eq!(sum, 3072);
        let out = clean_dir(CleanTarget::UserTemp, &root);
        assert_eq!(out.freed, 3072, "释放字节应与扫描一致");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_dir_matches_collect_then_sum() {
        // 扫描与删除必须同口径：扫出来多少，清完就该释放多少
        let root = temp_root("scandir");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("a.tmp"), vec![0u8; 1000]).unwrap();
        std::fs::write(root.join("sub/b.tmp"), vec![0u8; 2000]).unwrap();
        assert_eq!(scan_dir(CleanTarget::UserTemp, &root), 3000);
        let out = clean_dir(CleanTarget::UserTemp, &root);
        assert_eq!(out.freed, 3000, "释放字节应与扫描一致");
        assert_eq!(scan_dir(CleanTarget::UserTemp, &root), 0, "清完应为 0");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_dir_missing_path_is_zero() {
        assert_eq!(scan_dir(CleanTarget::UserTemp, Path::new(r"Z:\不存在")), 0);
    }

    #[test]
    fn scan_dir_applies_name_filter() {
        let root = temp_root("scanfilter");
        std::fs::write(root.join("thumbcache_96.db"), vec![0u8; 500]).unwrap();
        std::fs::write(root.join("iconcache_96.db"), vec![0u8; 7000]).unwrap();
        assert_eq!(
            scan_dir(CleanTarget::ThumbCache, &root),
            500,
            "只统计会被清理的那部分，否则用户看到的可清理大小是虚高的"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_target_returns_zero_for_recycle() {
        // 回收站没有普通目录，scan_target 不应去遍历任何东西
        assert_eq!(scan_target(CleanTarget::Recycle), 0);
    }

    #[test]
    fn clean_missing_root_is_skipped_not_failed() {
        let out = clean_dir(CleanTarget::UserTemp, Path::new(r"Z:\不存在"));
        assert_eq!(out.deleted, 0);
        assert_eq!(out.failed, 0, "目录不存在不是失败");
        assert_eq!(out.skipped_roots.len(), 1);
        assert!(out.summary().contains("不存在"));
    }

    #[test]
    fn clean_is_idempotent_second_run_frees_nothing() {
        let root = temp_root("idem");
        std::fs::write(root.join("a.tmp"), vec![0u8; 10]).unwrap();
        let first = clean_dir(CleanTarget::UserTemp, &root);
        assert_eq!(first.deleted, 1);
        let second = clean_dir(CleanTarget::UserTemp, &root);
        assert_eq!(second.deleted, 0, "第二次没有可删的");
        assert_eq!(second.freed, 0);
        assert_eq!(second.failed, 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn outcome_summary_mentions_skipped_files() {
        let out = CleanOutcome {
            deleted: 3,
            freed: 2048,
            failed: 5,
            errors: vec!["locked.log".into()],
            skipped_roots: Vec::new(),
        };
        let s = out.summary();
        assert!(s.contains("3 个"), "{s}");
        assert!(s.contains("2.0 KB"), "{s}");
        assert!(s.contains("5 个"), "必须汇报跳过数: {s}");
        assert!(
            s.contains("占用") || s.contains("正常"),
            "要说明跳过是正常的: {s}"
        );
    }

    #[test]
    fn outcome_summary_clean_run_has_no_skip_noise() {
        let out = CleanOutcome {
            deleted: 1,
            freed: 1024,
            failed: 0,
            errors: Vec::new(),
            skipped_roots: Vec::new(),
        };
        let s = out.summary();
        assert!(!s.contains("跳过"), "没有跳过就不该提: {s}");
    }

    #[test]
    fn outcome_merge_accumulates_and_caps_errors() {
        let a = CleanOutcome {
            deleted: 1,
            freed: 100,
            failed: 1,
            errors: (0..8).map(|i| format!("e{i}")).collect(),
            skipped_roots: vec!["r1".into()],
        };
        let b = CleanOutcome {
            deleted: 2,
            freed: 200,
            failed: 3,
            errors: (0..8).map(|i| format!("f{i}")).collect(),
            skipped_roots: vec!["r2".into()],
        };
        let m = a.merge(b);
        assert_eq!(m.deleted, 3);
        assert_eq!(m.freed, 300);
        assert_eq!(m.failed, 4);
        assert_eq!(m.errors.len(), 10, "错误样本要截断到 10 个，别把界面撑爆");
        assert_eq!(m.skipped_roots.len(), 2);
    }

    #[test]
    fn recycle_has_no_plain_root() {
        // 回收站走 Shell COM；把它当普通目录遍历会删到别的东西
        assert!(
            CleanTarget::Recycle.roots().is_empty(),
            "回收站走 Shell COM，无根目录"
        );
    }

    #[test]
    fn roots_are_plausible_on_real_machine() {
        // 真机：临时目录与缩略图目录应能解析出来（不实际清理）
        let ut = CleanTarget::UserTemp.roots().into_iter().next();
        assert!(ut.is_some(), "TEMP 环境变量应可解析");
        let wt = CleanTarget::WinTemp.roots().into_iter().next().unwrap();
        assert_eq!(wt.to_string_lossy(), r"C:\Windows\Temp");
    }
}
