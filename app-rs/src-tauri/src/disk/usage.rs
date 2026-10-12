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

/// 单层展开的一行子项（目录或文件）。
/// 目录：size=直接文件合计、total=递归总大小（轮次39 契约 total=自身+子树）；
/// 文件：size=total=自身大小、file_count=1。
#[derive(Debug, Clone, Serialize, Default)]
pub struct ChildEntry {
    pub name: String,
    pub path: String,
    /// 目录=直接文件合计；文件=自身大小
    pub size: u64,
    /// 目录=递归总大小；文件=自身大小
    pub total: u64,
    /// 目录=子树文件数（递归口径，与 scan_tree 一致）；文件=1
    pub file_count: u64,
    pub error_count: u64,
    pub is_dir: bool,
}

/// `scan_children` 的返回：某目录直接子项的单层摘要。
/// 子项内部不再嵌套内容（孙辈要前端再次调 sk_disk_scan_children 下钻），
/// 否则一次展开就会把整棵子树塞进响应。
#[derive(Debug, Clone, Serialize, Default)]
pub struct ChildrenScan {
    pub path: String,
    pub name: String,
    /// 本目录直接文件合计
    pub size: u64,
    /// 本目录递归总大小
    pub total: u64,
    pub file_count: u64,
    pub error_count: u64,
    /// 直接子项，按 total 降序（并列按名称稳定排序），最多 MAX_CHILD_ROWS 个
    pub children: Vec<ChildEntry>,
    /// children 被截断到 MAX_CHILD_ROWS 时为 true
    pub truncated: bool,
    /// 子树类型分布（复用 scan_tree 口径：字节降序、无后缀归「无后缀」）
    pub ext_stats: Vec<(String, u64)>,
    /// 子树大文件 Top200（复用 scan_tree 口径）
    pub top_files: Vec<TopFile>,
    pub duration_ms: u64,
    pub cancelled: bool,
}

/// 单层子项行数上限：目录里可能堆几万个文件，整包序列化既慢又会卡渲染。
/// 按大小降序截断后「占空间的都在」，前端用 truncated 提示「仅显示前 N 项」。
pub const MAX_CHILD_ROWS: usize = 500;

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

// ---------------- 单层展开（目录逐级下钻） ----------------

/// 扫描单个目录的**直接子项**（磁盘分析页逐级下钻用）。
///
/// 实现上复用 `scan_tree` 跑完整子树——子项的 total（递归大小）/ext_stats/
/// top_files 都要从整棵子树算出来才有意义；随后只投影出第一层子项返回，
/// 孙辈内容由前端再次调用本函数按需下钻。取消令牌与 sk_disk_scan_tree
/// 同一枚 SCAN_STOP（同一时刻只跑一个扫描，预取消连直接子项也不枚举）。
pub fn scan_children(dir: &Path, stop: &ScanStop) -> ChildrenScan {
    let t0 = Instant::now();
    let r = scan_tree(dir, stop);
    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| dir.to_string_lossy().trim_end_matches('\\').to_string());
    let mut result = ChildrenScan {
        path: dir.to_string_lossy().to_string(),
        name,
        size: r.root.size,
        total: r.root.total,
        file_count: r.file_count,
        error_count: r.error_count,
        duration_ms: t0.elapsed().as_millis() as u64,
        cancelled: r.cancelled,
        ext_stats: r.ext_stats,
        top_files: r.top_files,
        ..Default::default()
    };
    // 取消即时生效：预取消/中途取消不再枚举直接子项（与 scan_tree 的
    // 「预取消不得再统计任何文件」同口径），cancelled 标记结果不完整
    if r.cancelled {
        return result;
    }

    // 子目录的递归口径按名称对齐到 scan_tree 结果（name 在同一目录内唯一）
    let mut subdirs: std::collections::HashMap<String, &DirNode> = std::collections::HashMap::new();
    for c in &r.root.children {
        subdirs.insert(c.name.clone(), c);
    }

    if let Ok(it) = fs::read_dir(dir) {
        for entry in it.flatten() {
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => {
                    // 与 scan_tree 同口径：readdir 条目失败计 1 个错误
                    result.error_count += 1;
                    continue;
                }
            };
            // 跳过 reparse point/符号链接：与 scan_tree 同规则（junction 死循环）
            if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 || meta.is_symlink() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            let path = entry.path().to_string_lossy().to_string();
            if meta.is_dir() {
                // 对齐不到 scan_tree 结果（两次遍历之间目录树变动的竞态）：
                // 给 0 值占位，行本身仍要列出——不列会让用户以为子目录丢了
                let (size, total, fc, ec) = match subdirs.get(&name) {
                    Some(n) => (n.size, n.total, n.file_count, n.error_count),
                    None => (0, 0, 0, 0),
                };
                result.children.push(ChildEntry {
                    name,
                    path,
                    size,
                    total,
                    file_count: fc,
                    error_count: ec,
                    is_dir: true,
                });
            } else {
                let size = meta.len();
                result.children.push(ChildEntry {
                    name,
                    path,
                    size,
                    total: size,
                    file_count: 1,
                    error_count: 0,
                    is_dir: false,
                });
            }
        }
    } else {
        // scan_tree 能打开、这里打不开：目录在两次遍历间被删（竞态）
        result.error_count += 1;
    }

    // 契约：按 total 降序（目录=递归大小、文件=自身），并列按名称稳定排序
    result
        .children
        .sort_by(|a, b| b.total.cmp(&a.total).then_with(|| a.name.cmp(&b.name)));
    if result.children.len() > MAX_CHILD_ROWS {
        result.truncated = true;
        result.children.truncate(MAX_CHILD_ROWS);
    }
    result.duration_ms = t0.elapsed().as_millis() as u64;
    result
}

/// `sk_disk_scan_children` / `sk_disk_scan_tree` 共用的路径守卫（单一事实源）：
/// 绝对路径 / 存在且为目录 / 拒绝设备命名空间（`\\.\`、`\\?\`）与
/// DOS 保留设备名（CON/NUL/COM1…）。tree 一侧的校验是轮次114 补齐的历史欠账，
/// 新命令不得再复制无守卫的口子。
pub fn validate_scan_dir(raw: &str) -> Result<std::path::PathBuf, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("目录路径不能为空".to_string());
    }
    // 设备命名空间（\\.\PhysicalDrive0 / \\?\C:\…）：绕过文件系统语义直接
    // 摸设备，扫描语义在这里没有定义，一律拒绝
    let lowered = raw.to_ascii_lowercase().replace('/', "\\");
    if lowered.starts_with("\\\\.\\") || lowered.starts_with("\\\\?\\") {
        return Err(format!("不支持设备命名空间路径: {raw}"));
    }
    let path = Path::new(raw);
    if !path.is_absolute() {
        return Err(format!("必须是绝对路径（如 C:\\Users）: {raw}"));
    }
    // DOS 保留设备名：即使拼成 C:\CON\ 子路径，Win32 也可能把它解析到
    // 控制台设备而不是文件系统——逐级组件排查（含「CON.txt」式带扩展名变体）
    const DEVICE_NAMES: [&str; 22] = [
        "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
        "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ];
    for comp in path.components() {
        if let std::path::Component::Normal(os) = comp {
            let name = os.to_string_lossy().to_ascii_lowercase();
            let stem = name.split('.').next().unwrap_or("");
            if DEVICE_NAMES.contains(&stem) {
                return Err(format!(
                    "路径包含 Windows 保留设备名（{stem}），已拒绝: {raw}"
                ));
            }
        }
    }
    if !path.exists() {
        return Err(format!("目录不存在: {raw}"));
    }
    if !path.is_dir() {
        return Err(format!("不是目录: {raw}"));
    }
    Ok(path.to_path_buf())
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

    // ---------------- 单层展开 scan_children ----------------

    /// 下钻夹具：根下 1 个直接文件 + 2 个子目录（其一再有嵌套），可断言递归口径。
    fn children_tree(tag: &str) -> std::path::PathBuf {
        let p =
            std::env::temp_dir().join(format!("syskit_rs_children_{}_{}", std::process::id(), tag));
        let _ = fs::remove_dir_all(&p);
        std::fs::create_dir_all(p.join("big").join("deep")).unwrap();
        std::fs::create_dir_all(p.join("small")).unwrap();
        std::fs::write(p.join("root.txt"), vec![0u8; 3]).unwrap();
        std::fs::write(p.join("big").join("a.bin"), vec![0u8; 300]).unwrap();
        std::fs::write(p.join("big").join("deep").join("b.bin"), vec![0u8; 30]).unwrap();
        std::fs::write(p.join("small").join("c.txt"), vec![0u8; 4]).unwrap();
        p
    }

    fn scan_children_of(tag: &str) -> (std::path::PathBuf, ChildrenScan) {
        let root = children_tree(tag);
        let stop = ScanStop(std::sync::atomic::AtomicBool::new(false));
        let r = scan_children(&root, &stop);
        (root, r)
    }

    /// 回归锁：单层展开的 size 口径延续轮次39 契约——目录 total=自身+子树
    /// （递归），size=直接文件；文件 size=total=自身；根摘要与 scan_tree 一致。
    #[test]
    fn children_size_contract_recursive_total() {
        let (root, r) = scan_children_of("size");
        assert!(!r.cancelled);
        assert_eq!(r.name, root.file_name().unwrap().to_string_lossy());
        assert_eq!(r.path, root.to_string_lossy());
        let big = r
            .children
            .iter()
            .find(|c| c.name == "big")
            .expect("子目录 big 必须在列");
        // big：直接文件 a.bin=300，子树另有 deep/b.bin=30
        assert_eq!(big.size, 300, "目录 size=直接文件合计");
        assert_eq!(big.total, 330, "目录 total=自身 size+子树（轮次39契约）");
        assert_eq!(big.file_count, 2, "file_count 递归口径");
        assert!(big.is_dir);
        // deep 不出现在第一层（只返回一层，孙辈要再次下钻）
        assert!(
            !r.children.iter().any(|c| c.name == "deep"),
            "单层展开不得夹带孙辈内容"
        );
        // 文件行：size=total=自身
        let f = r.children.iter().find(|c| c.name == "root.txt").unwrap();
        assert!(!f.is_dir);
        assert_eq!(f.size, 3);
        assert_eq!(f.total, 3);
        assert_eq!(f.file_count, 1);
        // 根摘要与 scan_tree 同口径
        assert_eq!(r.size, 3);
        assert_eq!(r.total, 337);
        assert_eq!(r.file_count, 4);
        assert_eq!(r.error_count, 0);
        let _ = fs::remove_dir_all(root);
    }

    /// 回归锁：子项按 total 降序（目录与文件混排同列），并列按名称稳定排序。
    #[test]
    fn children_sorted_by_total_desc() {
        let (root, r) = scan_children_of("order");
        let totals: Vec<u64> = r.children.iter().map(|c| c.total).collect();
        assert_eq!(totals, vec![330, 4, 3], "big 目录 > small 目录 > root.txt");
        for w in r.children.windows(2) {
            assert!(
                w[0].total > w[1].total || (w[0].total == w[1].total && w[0].name <= w[1].name),
                "必须降序且并列按名称稳定"
            );
        }
        let _ = fs::remove_dir_all(root);
    }

    /// 回归锁：空目录展开——零子项、零大小、truncated=false、无幽灵行。
    #[test]
    fn children_empty_dir_yields_no_rows() {
        let root =
            std::env::temp_dir().join(format!("syskit_rs_children_{}_void", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let stop = ScanStop(std::sync::atomic::AtomicBool::new(false));
        let r = scan_children(&root, &stop);
        assert!(!r.cancelled);
        assert!(r.children.is_empty());
        assert!(!r.truncated);
        assert_eq!(r.total, 0);
        assert_eq!(r.size, 0);
        let _ = fs::remove_dir_all(root);
    }

    /// 回归锁：取消即时生效——预置位令牌下 cancelled=true 且不再枚举任何子项。
    #[test]
    fn children_cancel_token_takes_effect_immediately() {
        let root = children_tree("cancel");
        let stop = ScanStop(std::sync::atomic::AtomicBool::new(true));
        let r = scan_children(&root, &stop);
        assert!(r.cancelled);
        assert!(r.children.is_empty(), "预取消不得再枚举任何子项");
        assert_eq!(r.total, 0);
        let _ = fs::remove_dir_all(&root);
    }

    /// 回归锁：超过 MAX_CHILD_ROWS 截断 + truncated 标记，留下的必是最大的前 N。
    #[test]
    fn children_truncated_at_limit_keeps_largest() {
        let root =
            std::env::temp_dir().join(format!("syskit_rs_children_{}_cap", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let n = MAX_CHILD_ROWS + 5;
        for i in 1..=n as u64 {
            std::fs::write(root.join(format!("f{i:04}.dat")), vec![0u8; i as usize]).unwrap();
        }
        let stop = ScanStop(std::sync::atomic::AtomicBool::new(false));
        let r = scan_children(&root, &stop);
        assert_eq!(r.children.len(), MAX_CHILD_ROWS);
        assert!(r.truncated);
        // 降序切齐：首行 = 最大的文件
        assert_eq!(r.children[0].total, n as u64);
        assert_eq!(
            r.children[MAX_CHILD_ROWS - 1].total,
            (n - MAX_CHILD_ROWS + 1) as u64
        );
        let _ = fs::remove_dir_all(root);
    }

    /// 回归锁：路径守卫拒绝用例（空/相对/设备命名空间/保留设备名/不存在/文件），
    /// 并断言错误文案命中对应拒绝原因（防止「都返回 Err」的假绿）。
    #[test]
    fn children_guard_rejects_suspicious_paths() {
        let rejects = |p: &str| validate_scan_dir(p).expect_err("必须被拒绝");
        // 空与纯空白
        assert!(rejects("").contains("不能为空"));
        assert!(rejects("   ").contains("不能为空"));
        // 相对路径
        assert!(rejects("relative\\dir").contains("绝对路径"));
        assert!(rejects(".").contains("绝对路径"));
        // 设备命名空间（正反斜杠两种写法）
        assert!(rejects(r"\\.\PhysicalDrive0").contains("设备命名空间"));
        assert!(rejects(r"\\?\C:\Windows").contains("设备命名空间"));
        assert!(rejects("//./PhysicalDrive0").contains("设备命名空间"));
        // DOS 保留设备名（含带扩展名变体与子路径形式）
        assert!(rejects(r"C:\CON").contains("保留设备名"));
        assert!(rejects(r"C:\con.txt").contains("保留设备名"));
        assert!(rejects(r"C:\Users\NUL\sub").contains("保留设备名"));
        assert!(rejects(r"C:\lpt9").contains("保留设备名"));
        // 不存在
        assert!(rejects(r"C:\__syskit_no_such_dir_9x__").contains("不存在"));
        // 存在但不是目录（测试二进制本身是真实的绝对文件路径）
        let exe = std::env::current_exe().expect("取测试二进制路径");
        assert!(rejects(exe.to_string_lossy().as_ref()).contains("不是目录"));
    }

    /// 回归锁：合法目录路径放行（守卫不得误伤正常使用）。
    #[test]
    fn children_guard_accepts_real_dir() {
        let root = children_tree("guard_ok");
        let p = validate_scan_dir(root.to_string_lossy().as_ref()).expect("真实目录必须放行");
        assert_eq!(p, root);
        let _ = fs::remove_dir_all(root);
    }
}
