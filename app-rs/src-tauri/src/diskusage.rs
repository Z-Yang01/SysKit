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
