//! 磁盘空间分析：目录树扫描 / 大文件 Top200 / 类型分布 / 回收站删除。
//!
//! 对齐 Python `diskusage.py` 的口径与语义：
//! - 扫描用 `fs::read_dir` 栈式后序遍历（每目录一个句柄，可随时取消），跳过
//!   reparse point/符号链接避免 junction 死循环与重复计数（如「Application Data」）
//! - 无权限/被占用条目跳过并计入 error_count（目录打开/stat 两处都计，父目录
//!   finalize 后总数才对得上）
//! - 删除走 shell32 `SHFileOperationW + FOF_ALLOWUNDO` 移入回收站（可在系统
//!   回收站还原）：相比 PowerShell 逐文件调用，一次进程内调用即可批量且无
//!   命令行长度限制；整批部分失败时逐个重试并剔除已消失路径（SHFileOperationW
//!   非原子：遇锁定文件中止，其前的文件往往已进回收站）

use serde::Serialize;
use std::fs;
use std::os::windows::fs::MetadataExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

pub const TOP_FILES_LIMIT: usize = 200;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

// ---------------- 数据结构 ----------------

#[derive(Debug, Clone, Serialize, Default)]
pub struct DirNode {
    pub name: String,
    pub path: String,
    /// 直接文件大小合计（不含子目录）
    pub size: u64,
    /// 递归总大小（含全部子目录）
    pub total: u64,
    pub file_count: u64,
    pub error_count: u64,
    pub children: Vec<DirNode>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TopFile {
    pub size: u64,
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanResult {
    pub root: DirNode,
    /// 扩展名（小写、无点）→ 总字节，按字节降序；无后缀记「无后缀」
    pub ext_stats: Vec<(String, u64)>,
    /// 按大小降序，最多 TOP_FILES_LIMIT 个
    pub top_files: Vec<TopFile>,
    pub file_count: u64,
    pub error_count: u64,
    pub duration_ms: u64,
    pub cancelled: bool,
}

// ---------------- 取消令牌 ----------------

#[derive(Default)]
pub struct ScanStop(pub AtomicBool);

impl ScanStop {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
    pub fn reset(&self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// 全局扫描取消令牌：同一时刻只跑一个目录扫描（sk_disk_scan_tree 复位，
/// sk_disk_scan_cancel 置位）。
pub static SCAN_STOP: ScanStop = ScanStop(std::sync::atomic::AtomicBool::new(false));

// ---------------- 内部遍历节点 ----------------

/// 扫描中的可变节点：children 逐步填入，total 等汇总字段在目录耗尽时回填。
struct WalkNode {
    name: String,
    path: String,
    size: u64,
    file_count: u64,
    error_count: u64,
    total: u64,
    children: Vec<WalkNode>,
}

impl WalkNode {
    fn new(name: String, path: String) -> Self {
        Self {
            name,
            path,
            size: 0,
            file_count: 0,
            error_count: 0,
            total: 0,
            children: Vec::new(),
        }
    }

    /// 子目录汇总回填：total = 自身 size + 子目录 total 之和
    /// （对齐 Python diskusage.py:147 `node.total = node.size + sum(...)`；
    /// 漏加自身 size 曾让所有目录的 total 恒为 0——轮次39实锤修复）
    fn finalize(&mut self) {
        self.total = self.size;
        for c in &self.children {
            self.total += c.total;
            self.file_count += c.file_count;
            self.error_count += c.error_count;
        }
    }

    fn into_dir_node(self) -> DirNode {
        DirNode {
            name: self.name,
            path: self.path,
            size: self.size,
            total: self.total,
            file_count: self.file_count,
            error_count: self.error_count,
            children: self
                .children
                .into_iter()
                .map(WalkNode::into_dir_node)
                .collect(),
        }
    }
}

// ---------------- 目录树扫描 ----------------

/// 扫描目录树。`stop` 置位后尽快返回已完成部分（cancelled=true）。
/// 无权限/被占用的条目跳过并计入 error_count。
pub fn scan_tree(root: &Path, stop: &ScanStop) -> ScanResult {
    let t0 = Instant::now();
    let mut ext_stats: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
    // 大文件 Top200：小顶堆保持最大的 N 个 (size, path)
    let mut heap: std::collections::BinaryHeap<std::cmp::Reverse<(u64, String)>> =
        std::collections::BinaryHeap::new();
    let mut file_count = 0u64;
    let mut error_count = 0u64;
    let mut cancelled = false;

    let root_name = root
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| root.to_string_lossy().trim_end_matches('\\').to_string());

    struct Frame {
        node: WalkNode,
        /// None = 无权限/打开失败的「真·空」占位。曾经这里拿 %TEMP% 的 ReadDir
        /// 顶包，把临时目录的内容整个算进无权限目录的 size/ext_stats/Top200
        /// （轮次39实锤修复）：占位必须表达「无内容」，不能是另一个目录的迭代器。
        it: Option<fs::ReadDir>,
    }

    let mut result_root: Option<WalkNode> = None;
    let mut stack: Vec<Frame> = match fs::read_dir(root) {
        Ok(it) => vec![Frame {
            node: WalkNode::new(root_name.clone(), root.to_string_lossy().to_string()),
            it: Some(it),
        }],
        Err(_) => {
            // 根目录都打不开：返回带 1 个错误的空树（不 cancelled，是真实失败）
            return ScanResult {
                root: DirNode {
                    name: root_name,
                    path: root.to_string_lossy().to_string(),
                    error_count: 1,
                    ..Default::default()
                },
                ext_stats: Vec::new(),
                top_files: Vec::new(),
                file_count: 0,
                error_count: 1,
                duration_ms: t0.elapsed().as_millis() as u64,
                cancelled: false,
            };
        }
    };

    while let Some(frame) = stack.last_mut() {
        if stop.is_cancelled() {
            cancelled = true;
            break;
        }
        // 取下一条目；None 占位（无权限目录）等同目录耗尽——无内容可读
        let next = match frame.it.as_mut() {
            None => None,
            Some(it) => it.next(),
        };
        match next {
            None => {
                // 目录耗尽：先汇总 children（后序遍历），再回退到父目录
                let mut done =
                    std::mem::replace(&mut frame.node, WalkNode::new(String::new(), String::new()));
                done.finalize();
                stack.pop();
                match stack.last_mut() {
                    Some(parent) => parent.node.children.push(done),
                    None => result_root = Some(done),
                }
            }
            Some(Err(_)) => {
                // readdir 本身出错：计错误、跳过该条目
                error_count += 1;
                frame.node.error_count += 1;
            }
            Some(Ok(entry)) => {
                let meta = match entry.metadata() {
                    Ok(m) => m,
                    Err(_) => {
                        error_count += 1;
                        frame.node.error_count += 1;
                        continue;
                    }
                };
                // 跳过 reparse point/符号链接：junction 死循环与重复计数
                if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 || meta.is_symlink() {
                    continue;
                }
                if meta.is_dir() {
                    let child = WalkNode::new(
                        entry.file_name().to_string_lossy().to_string(),
                        entry.path().to_string_lossy().to_string(),
                    );
                    match fs::read_dir(entry.path()) {
                        Ok(it) => stack.push(Frame {
                            node: child,
                            it: Some(it),
                        }),
                        // 无权限子目录：入树、计 1 个错误、it=None（真·空占位）。
                        // 对齐 Python child.error_count=1；不得拿别的目录的
                        // ReadDir 顶替（旧 empty_read_dir 实为 %TEMP% 迭代器）
                        Err(_) => stack.push(Frame {
                            node: {
                                let mut n = child;
                                n.error_count = 1;
                                n
                            },
                            it: None,
                        }),
                    }
                } else {
                    let size = meta.len();
                    let name = entry.file_name().to_string_lossy().to_string();
                    let path = entry.path().to_string_lossy().to_string();
                    frame.node.size += size;
                    frame.node.file_count += 1;
                    *ext_stats.entry(ext_key(&name)).or_insert(0) += size;
                    if heap.len() < TOP_FILES_LIMIT {
                        heap.push(std::cmp::Reverse((size, path.clone())));
                    } else if let Some(top) = heap.peek() {
                        if size > top.0 .0 {
                            heap.push(std::cmp::Reverse((size, path)));
                            heap.pop();
                        }
                    }
                    file_count += 1;
                }
            }
        }
    }

    // 取消/出错路径：栈中残留的目录自底向上补齐汇总并挂回父链
    while let Some(mut frame) = stack.pop() {
        frame.node.finalize();
        match stack.last_mut() {
            Some(parent) => parent.node.children.push(frame.node),
            None => {
                result_root = Some(frame.node);
            }
        }
    }

    ScanResult {
        root: result_root
            .unwrap_or_else(|| WalkNode::new(root_name, root.to_string_lossy().to_string()))
            .into_dir_node(),
        ext_stats: {
            // 契约（结构体注释与 Python 版一致）：按字节降序。HashMap 裸
            // into_iter 是乱序，必须显式排（并列按扩展名稳定排序，测试可复现）
            let mut v: Vec<(String, u64)> = ext_stats.into_iter().collect();
            v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            v
        },
        top_files: {
            // 契约（结构体注释与 Python 版一致）：按大小降序。BinaryHeap 的
            // into_iter 是堆序（非降序），必须显式排（并列按路径稳定排序）
            let mut v: Vec<TopFile> = heap
                .into_iter()
                .map(|std::cmp::Reverse((size, path))| TopFile { size, path })
                .collect();
            v.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.path.cmp(&b.path)));
            v
        },
        file_count,
        error_count,
        duration_ms: t0.elapsed().as_millis() as u64,
        cancelled,
    }
}

fn ext_key(name: &str) -> String {
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext.is_empty() {
        "无后缀".to_string()
    } else {
        ext
    }
}

// ---------------- 回收站删除 ----------------

/// 执行一次 SHFileOperationW 删除，pFrom 为双 null 结尾的宽字符路径列表。
#[cfg(windows)]
fn sh_delete(pfrom: &[u16]) -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::{SHFileOperationW, SHFILEOPSTRUCTW};
    unsafe {
        let mut op = SHFILEOPSTRUCTW {
            hwnd: Default::default(),
            wFunc: 3, // FO_DELETE
            pFrom: PCWSTR(pfrom.as_ptr()),
            pTo: PCWSTR::null(),
            // ALLOWUNDO|NOCONFIRMATION|SILENT|NOERRORUI|NO_CONNECTED_ELEMENTS
            fFlags: 0x40 | 0x10 | 0x4 | 0x400 | 0x2000,
            fAnyOperationsAborted: false.into(),
            hNameMappings: std::ptr::null_mut(),
            lpszProgressTitle: PCWSTR::null(),
        };
        SHFileOperationW(&mut op) == 0 && !op.fAnyOperationsAborted.as_bool()
    }
}

#[cfg(not(windows))]
fn sh_delete(_pfrom: &[u16]) -> bool {
    false
}

/// 把文件/文件夹移入回收站，返回 (成功数, 失败路径列表)。
///
/// 整批部分失败时逐个定位（SHFileOperationW 非原子：遇到锁定/无权限文件
/// 中止，其前面的文件往往已进回收站）——重试前剔除已消失的路径，
/// 否则已删文件被误报为「失败（可能无权限）」。
pub fn delete_to_recycle_bin(paths: &[String]) -> (u32, Vec<String>) {
    let paths: Vec<String> = paths
        .iter()
        .filter_map(|p| {
            fs::canonicalize(p)
                .ok()
                .map(|c| c.to_string_lossy().to_string())
        })
        .collect();
    if paths.is_empty() {
        return (0, Vec::new());
    }
    #[cfg(windows)]
    {
        // pFrom 需要双 null 结尾的宽字符路径列表
        let mut wide: Vec<u16> = Vec::new();
        for p in &paths {
            wide.extend(p.encode_utf16());
            wide.push(0);
        }
        wide.push(0);
        if sh_delete(&wide) {
            return (paths.len() as u32, Vec::new());
        }
    }
    // 整批失败：逐个重试，剔除已消失的路径
    let mut ok = 0u32;
    let mut errors = Vec::new();
    #[cfg(windows)]
    for p in &paths {
        if !Path::new(p).exists() {
            ok += 1; // 首轮已删掉
            continue;
        }
        let mut wide: Vec<u16> = p.encode_utf16().collect();
        wide.push(0);
        wide.push(0);
        if sh_delete(&wide) {
            ok += 1;
        } else {
            errors.push(p.clone());
        }
    }
    #[cfg(not(windows))]
    {
        errors = paths.clone();
    }
    (ok, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每用例独立临时树（local fixture 约定：夹具进自己模块，不共享状态）。
    fn temp_tree(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "syskit_rs_diskusage_{}_{}",
            std::process::id(),
            tag
        ));
        let _ = fs::remove_dir_all(&p);
        std::fs::create_dir_all(p.join("sub")).unwrap();
        std::fs::write(p.join("a.txt"), vec![0u8; 10]).unwrap();
        std::fs::write(p.join("noext"), vec![0u8; 7]).unwrap();
        std::fs::write(p.join("sub").join("b.bin"), vec![0u8; 100]).unwrap();
        std::fs::write(p.join("sub").join("c.txt"), vec![0u8; 20]).unwrap();
        p
    }

    fn scan(tag: &str) -> (std::path::PathBuf, ScanResult) {
        let root = temp_tree(tag);
        let stop = ScanStop(std::sync::atomic::AtomicBool::new(false));
        let r = scan_tree(&root, &stop);
        (root, r)
    }

    /// 回归锁（轮次39实锤①）：finalize 漏加自身 size 曾让所有目录的
    /// total 恒为 0（Python diskusage.py:147 契约 total = size + Σchildren.total）。
    #[test]
    fn total_is_size_plus_children_recursive() {
        let (root, r) = scan("total");
        let sub = r
            .root
            .children
            .iter()
            .find(|c| c.name == "sub")
            .expect("子目录 sub 必须入树");
        // 叶子目录：total == size（直接文件 b.bin 100 + c.txt 20）
        assert_eq!(sub.size, 120);
        assert_eq!(sub.total, 120, "叶子目录 total 不得丢自身 size");
        assert_eq!(sub.file_count, 2);
        // 根目录：size=直接文件(10+7)，total=递归(17+120)
        assert_eq!(r.root.size, 17);
        assert_eq!(r.root.total, 137, "根 total = 自身 size + 子目录 total");
        assert_eq!(r.root.file_count, 4); // 递归口径：2 直接 + 2 子目录
                                          // 顶层计数口径：全局不重复计数（本树共 4 个文件）
        assert_eq!(r.file_count, 4);
        assert_eq!(r.error_count, 0);
        assert!(!r.cancelled);
        let _ = fs::remove_dir_all(root);
    }

    /// 回归锁（轮次39实锤②）：top_files 契约「按大小降序」（Python
    /// sorted(heap, reverse=True)）；BinaryHeap::into_iter 是堆序非降序。
    #[test]
    fn top_files_sorted_desc_and_complete() {
        let (root, r) = scan("top");
        let sizes: Vec<u64> = r.top_files.iter().map(|f| f.size).collect();
        assert_eq!(sizes, vec![100, 20, 10, 7], "必须严格降序且含全部 4 个文件");
        assert_eq!(
            r.top_files[0].path,
            root.join("sub").join("b.bin").to_string_lossy()
        );
        let _ = fs::remove_dir_all(root);
    }

    /// 回归锁：Top200 小顶堆上限——205 个文件只留最大的 200 个，且仍降序。
    #[test]
    fn top_files_capped_at_limit() {
        let root =
            std::env::temp_dir().join(format!("syskit_rs_diskusage_{}_cap", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for i in 1..=205u32 {
            std::fs::write(root.join(format!("f{i:03}.dat")), vec![0u8; i as usize]).unwrap();
        }
        let stop = ScanStop(std::sync::atomic::AtomicBool::new(false));
        let r = scan_tree(&root, &stop);
        assert_eq!(r.top_files.len(), TOP_FILES_LIMIT);
        assert_eq!(r.top_files[0].size, 205);
        assert_eq!(r.top_files[TOP_FILES_LIMIT - 1].size, 6); // 1..=5 被 Top200 挤出
        for w in r.top_files.windows(2) {
            assert!(w[0].size >= w[1].size, "上限截断后仍须降序");
        }
        let _ = fs::remove_dir_all(&root);
    }

    /// 回归锁：ext_stats 按字节降序 + 无后缀归「无后缀」（结构体注释契约）。
    #[test]
    fn ext_stats_sorted_desc_with_noext_bucket() {
        let (root, r) = scan("ext");
        assert_eq!(
            r.ext_stats,
            vec![
                ("bin".to_string(), 100),
                ("txt".to_string(), 30),
                ("无后缀".to_string(), 7),
            ]
        );
        let _ = fs::remove_dir_all(root);
    }

    /// 回归锁：扫描取消——预置位的令牌立即生效（cancelled=true），结果仍成形。
    #[test]
    fn cancel_token_stops_scan_promptly() {
        let root = temp_tree("cancel");
        let stop = ScanStop(std::sync::atomic::AtomicBool::new(true));
        let r = scan_tree(&root, &stop);
        assert!(r.cancelled);
        assert_eq!(r.file_count, 0, "预取消不得再统计任何文件");
        let _ = fs::remove_dir_all(&root);
    }

    /// 回归锁（轮次39实锤③的结构面）：空子目录扫描后 total==0 且无幽灵内容；
    /// 无权限占位在类型上已是 Option<ReadDir> 的 None（真·空），把别的目录的
    /// 迭代器顶包进占位在此类型下无法表示（编译期锁）。
    #[test]
    fn empty_subdir_yields_empty_node_no_phantom_content() {
        let root =
            std::env::temp_dir().join(format!("syskit_rs_diskusage_{}_empty", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("void")).unwrap();
        std::fs::write(root.join("x.txt"), vec![0u8; 5]).unwrap();
        let stop = ScanStop(std::sync::atomic::AtomicBool::new(false));
        let r = scan_tree(&root, &stop);
        let void = r
            .root
            .children
            .iter()
            .find(|c| c.name == "void")
            .expect("空目录也要入树");
        assert_eq!(void.size, 0);
        assert_eq!(void.total, 0);
        assert!(void.children.is_empty(), "空目录不得出现幽灵子内容");
        assert_eq!(r.root.total, 5);
        let _ = fs::remove_dir_all(&root);
    }
}
