"""磁盘空间分析：目录树扫描 / 磁盘列表 / 回收站删除（无 Qt 依赖，GUI/CLI 共用）。

扫描用 os.scandir 惰性迭代（每个目录一个句柄，可随时取消），跳过 reparse point
避免 junction/符号链接死循环与重复计数（如「Application Data」）。
删除走 shell32 SHFileOperationW + FOF_ALLOWUNDO 移入回收站（可还原）：相比
PowerShell 逐文件调用，一次进程内调用即可批量删除且无命令行长度限制。
"""
import ctypes
import heapq
import os
import stat
import sys
import time
from dataclasses import dataclass, field

FILE_ATTRIBUTE_REPARSE_POINT = 0x400
TOP_FILES_LIMIT = 200

# ---------------- 回收站删除（SHFileOperationW） ----------------

FO_DELETE = 3
FOF_ALLOWUNDO = 0x40
FOF_NOCONFIRMATION = 0x10
FOF_SILENT = 0x4
FOF_NOERRORUI = 0x400
FOF_NO_CONNECTED_ELEMENTS = 0x2000
_FOF_FLAGS = (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT |
              FOF_NOERRORUI | FOF_NO_CONNECTED_ELEMENTS)


class _SHFILEOPSTRUCTW(ctypes.Structure):
    _fields_ = [("hwnd", ctypes.c_void_p),
                ("wFunc", ctypes.c_uint),
                ("pFrom", ctypes.c_wchar_p),
                ("pTo", ctypes.c_wchar_p),
                ("fFlags", ctypes.c_uint16),
                ("fAnyOperationsAborted", ctypes.c_int),
                ("hNameMappings", ctypes.c_void_p),
                ("lpszProgressTitle", ctypes.c_wchar_p)]


_shell32 = ctypes.windll.shell32
_shell32.SHFileOperationW.argtypes = [ctypes.POINTER(_SHFILEOPSTRUCTW)]
_shell32.SHFileOperationW.restype = ctypes.c_int


def _sh_delete(pfrom: str) -> bool:
    """执行一次 SHFileOperationW 删除，pFrom 为双 null 结尾的路径列表。"""
    op = _SHFILEOPSTRUCTW()
    op.wFunc = FO_DELETE
    op.pFrom = pfrom
    op.fFlags = _FOF_FLAGS
    return _shell32.SHFileOperationW(ctypes.byref(op)) == 0 and not op.fAnyOperationsAborted


def delete_to_recycle_bin(paths) -> tuple:
    """把文件/文件夹移入回收站，返回 (成功数, 失败路径列表)。"""
    paths = [os.path.abspath(p) for p in paths]
    if not paths:
        return 0, []
    if sys.platform != "win32":
        return 0, list(paths)
    if _sh_delete("\0".join(paths) + "\0"):
        return len(paths), []
    # 整批部分失败时逐个定位（SHFileOperationW 非原子：遇到锁定/无权限文件
    # 中止，其前面的文件往往已进回收站）——重试前剔除已消失的路径，
    # 否则已删文件被误报为"失败（可能无权限）"
    ok, errors = 0, []
    for p in paths:
        if not os.path.exists(p):
            ok += 1                      # 首轮已删掉
            continue
        if _sh_delete(p + "\0"):
            ok += 1
        else:
            errors.append(p)
    return ok, errors


# ---------------- 磁盘列表 ----------------

@dataclass
class DriveInfo:
    letter: str        # "C:"
    mountpoint: str    # "C:\\"
    total: int
    used: int
    free: int


def list_drives() -> list:
    import psutil
    drives = []
    for p in psutil.disk_partitions(all=False):
        if not p.fstype or "cdrom" in p.opts.lower():
            continue
        try:
            u = psutil.disk_usage(p.mountpoint)
        except OSError:
            continue
        letter = os.path.splitdrive(p.mountpoint)[0] or p.mountpoint
        drives.append(DriveInfo(letter, p.mountpoint, u.total, u.used, u.free))
    return drives


# ---------------- 目录树扫描 ----------------

@dataclass(slots=True)
class DirNode:
    name: str
    path: str
    size: int = 0          # 直接文件大小合计（不含子目录）
    total: int = 0         # 递归总大小
    file_count: int = 0
    error_count: int = 0
    children: list = field(default_factory=list)


@dataclass
class ScanResult:
    root: DirNode
    ext_stats: dict        # 扩展名（小写、无点）-> 总字节；无后缀文件记为 "无后缀"
    top_files: list        # [(size, path, mtime)] 按大小降序，最多 TOP_FILES_LIMIT
    file_count: int
    error_count: int
    duration: float
    cancelled: bool = False


def scan_tree(root: str, stop=None, progress=None) -> ScanResult:
    """扫描目录树。stop 为含 is_set() 的事件（如 threading.Event），置位后尽快
    返回已完成部分（cancelled=True）；progress(file_count, bytes, current_dir)
    每约 500 个文件回调一次。无权限/被占用的条目跳过并计入 error_count。
    """
    t0 = time.monotonic()
    ext_stats = {}
    heap = []              # (size, path, mtime) 小顶堆，保持 TOP_FILES_LIMIT 个最大
    file_count = 0
    error_count = 0
    bytes_seen = 0
    cancelled = False

    name = os.path.basename(root.rstrip("\\/")) or (os.path.splitdrive(root)[0] + "\\")
    root_node = DirNode(name, root)

    def finalize(node: DirNode):
        node.total = node.size + sum(c.total for c in node.children)
        node.file_count += sum(c.file_count for c in node.children)
        node.error_count += sum(c.error_count for c in node.children)

    # 栈式后序遍历：栈顶目录逐条目消费，耗尽时汇总并回退到父目录
    try:
        stack = [(root_node, os.scandir(root))]
    except OSError:
        stack = []
        error_count += 1
        root_node.error_count = 1

    while stack:
        if stop is not None and stop.is_set():
            cancelled = True
            break
        node, it = stack[-1]
        try:
            ent = next(it)
        except StopIteration:
            it.close()
            stack.pop()
            finalize(node)
            continue
        except OSError:
            it.close()
            stack.pop()
            error_count += 1
            node.error_count += 1
            finalize(node)
            continue
        try:
            st = ent.stat(follow_symlinks=False)
        except OSError:
            error_count += 1
            continue
        if getattr(st, "st_file_attributes", 0) & FILE_ATTRIBUTE_REPARSE_POINT:
            continue
        if stat.S_ISDIR(st.st_mode):
            child = DirNode(ent.name, ent.path)
            try:
                child_it = os.scandir(ent.path)
            except OSError:
                error_count += 1
                child.error_count = 1
                node.children.append(child)
                continue
            node.children.append(child)
            stack.append((child, child_it))
            continue
        size = st.st_size
        node.size += size
        node.file_count += 1
        file_count += 1
        bytes_seen += size
        ext = os.path.splitext(ent.name)[1].lower().lstrip(".")
        key = ext or "无后缀"
        ext_stats[key] = ext_stats.get(key, 0) + size
        if len(heap) < TOP_FILES_LIMIT:
            heapq.heappush(heap, (size, ent.path, st.st_mtime))
        elif size > heap[0][0]:
            heapq.heapreplace(heap, (size, ent.path, st.st_mtime))
        if progress and file_count % 500 == 0:
            progress(file_count, bytes_seen, node.path)

    # 取消时剩余栈中节点尚未汇总，自底向上补齐（栈内子目录在父目录之上）
    for node, it in reversed(stack):
        it.close()
        finalize(node)

    return ScanResult(root_node, ext_stats,
                      sorted(heap, reverse=True),
                      file_count, error_count,
                      time.monotonic() - t0, cancelled)
