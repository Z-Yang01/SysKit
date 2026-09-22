"""剪贴板历史：多格式条目（文本/图片/文件/HTML）、置顶、搜索、敏感内容防护。

条目 schema：{"ts", "text", "kind": "text|image|file|html", "pinned": bool, "data"}
- image 条目：text 为可搜索的占位描述（如 "[图片 128×96]"），data 为 PNG 文件名，
  实际字节存 %APPDATA%\\SysKit\\clipboard_files\\（md5 命名去重）。
- html 条目：text 为纯文本预览，data 为原始 HTML。
- 落盘经 secure_store DPAPI 密封；兼容旧明文 JSON（无 kind/pinned 字段，下次保存升级）。
"""
import hashlib
import os
import sys
import threading
import time
import weakref

from . import config
from .logger import LOGGER

# RLock：add/remove 在持锁时调用 save，必须可重入
_lock = threading.RLock()

# 存活实例弱引用表：托盘/命令面板读磁盘态前先 flush_pending()，
# 消除 10s 防抖窗口内"最新复制内容看不到"的滞后（无强引用，不阻碍 GC）
_instances = weakref.WeakSet()

# 敏感内容摘要：[(digest, until_ts)]，过期自动清理。密码库等来源标记后监听跳过。
_sensitive = []

# 回填抑制摘要：快捷粘贴自动写回剪贴板时，监听器会把「自己写入」的内容
# 当成一次新复制——历史顺序失真（被粘贴的旧条目顶到最新）。短 TTL 窗口内
# 按内容摘要跳过记录；只用于文本类回填（图片回填语义不同，不抑制）。
_suppressed = []

DEFAULT_BLACKLIST = "keepass,bitwarden,1password,enpass"


def files_dir() -> str:
    return os.path.join(config.APPDATA_DIR, "clipboard_files")


def image_path(name: str) -> str:
    return os.path.join(files_dir(), name) if name else ""


def save_image_bytes(png: bytes) -> str:
    """PNG 字节存入 clipboard_files（md5 命名去重），返回文件名；失败返回 ""。"""
    if not png:
        return ""
    name = hashlib.md5(png).hexdigest() + ".png"
    try:
        os.makedirs(files_dir(), exist_ok=True)
        path = image_path(name)
        if not os.path.exists(path):
            with open(path, "wb") as f:
                f.write(png)
    except OSError:
        return ""
    return name


def _digest(text) -> str:
    return hashlib.sha256(str(text).encode("utf-8", "ignore")).hexdigest()


def _prune(window: list) -> None:
    """丢弃已过期摘要（调用方须持有 _lock）。"""
    window[:] = [s for s in window if s["until"] > time.time()]


def _mark_text(window: list, text, until: float) -> None:
    if text is None:
        return
    with _lock:
        window.append({"digest": _digest(text), "until": until})
        _prune(window)


def _text_in(window: list, digest: str) -> bool:
    """按摘要查窗口（调用方已算好 digest：add 热路径两道闸共享一次哈希）。"""
    with _lock:
        _prune(window)
        return any(s["digest"] == digest for s in window)


def mark_sensitive(text, ttl: int = 25) -> None:
    """标记文本为敏感：ttl 秒内监听到相同内容不记录（密码库复制用）。"""
    _mark_text(_sensitive, text, time.time() + max(1, int(ttl)))


def _is_sensitive(text) -> bool:
    return _text_in(_sensitive, _digest(text))


def suppress_next(text, ttl: float = 2.0) -> None:
    """短期抑制某文本被记录（快捷粘贴自动回填时用，见 _suppressed 注释）。"""
    _mark_text(_suppressed, text, time.time() + max(0.5, float(ttl)))


def _is_suppressed(text) -> bool:
    return _text_in(_suppressed, _digest(text))


def exclude_os_history() -> None:
    """给「当前」剪贴板内容附加 Windows 排除格式：不进 Win+V 历史记录、
    不上传云剪贴板（Win10+）。mark_sensitive 只防得住本应用自己的历史，
    系统侧历史/跨设备同步需要这三个 RegisterClipboardFormat 标记（KeePass 同款）。

    必须在 setText **之后**调用（对当前剪贴板补设格式）；尽力而为——
    非 Windows / 打不开剪贴板 / 老系统无此格式名时静默跳过。
    """
    if sys.platform != "win32":
        return
    try:
        import ctypes
        from ctypes import wintypes

        u32 = ctypes.windll.user32
        k32 = ctypes.windll.kernel32
        # 64 位下必须显式签名：ctypes 默认 restype=c_int 会把 GlobalLock 的
        # 指针/SetClipboardData 的句柄截断成 32 位 → 错误地址/误判失败
        u32.OpenClipboard.restype = wintypes.BOOL
        u32.OpenClipboard.argtypes = [wintypes.HWND]
        u32.CloseClipboard.restype = wintypes.BOOL
        u32.CloseClipboard.argtypes = []
        u32.RegisterClipboardFormatW.restype = wintypes.UINT
        u32.RegisterClipboardFormatW.argtypes = [wintypes.LPCWSTR]
        u32.SetClipboardData.restype = wintypes.HANDLE
        u32.SetClipboardData.argtypes = [wintypes.UINT, wintypes.HANDLE]
        k32.GlobalAlloc.restype = wintypes.HGLOBAL
        k32.GlobalAlloc.argtypes = [wintypes.UINT, ctypes.c_size_t]
        k32.GlobalLock.restype = wintypes.LPVOID
        k32.GlobalLock.argtypes = [wintypes.HGLOBAL]
        k32.GlobalUnlock.restype = wintypes.BOOL
        k32.GlobalUnlock.argtypes = [wintypes.HGLOBAL]
        k32.GlobalFree.restype = wintypes.HGLOBAL
        k32.GlobalFree.argtypes = [wintypes.HGLOBAL]

        if not u32.OpenClipboard(None):
            return
        try:
            GMEM_MOVEABLE = 0x0002

            def _set_zero(fmt_name: str):
                fid = u32.RegisterClipboardFormatW(fmt_name)
                if not fid:
                    return
                # 注意是 ctypes.sizeof(...)：DWORD.sizeof 属性不存在，曾致整个
                # 函数 AttributeError 被吞、排除格式从未真正写入
                size = ctypes.sizeof(wintypes.DWORD)
                h = k32.GlobalAlloc(GMEM_MOVEABLE, size)
                if not h:
                    return
                p = k32.GlobalLock(h)
                if not p:
                    k32.GlobalFree(h)
                    return
                ctypes.memmove(p, ctypes.byref(wintypes.DWORD(0)), size)
                k32.GlobalUnlock(h)
                # 成功后句柄归系统所有，不得释放；失败系统未接管 → 归还避免泄漏
                if not u32.SetClipboardData(fid, h):
                    k32.GlobalFree(h)

            _set_zero("ExcludeClipboardContentFromMonitorProcessing")
            _set_zero("CanIncludeInClipboardHistory")
            _set_zero("CanUploadToCloudClipboard")
        finally:
            u32.CloseClipboard()
    except Exception:
        pass


def foreground_process_name() -> str:
    """前台窗口进程名（小写）；非 Windows / 失败返回 ""。"""
    if sys.platform != "win32":
        return ""
    try:
        import ctypes
        import psutil
        hwnd = ctypes.windll.user32.GetForegroundWindow()
        if not hwnd:
            return ""
        pid = ctypes.c_ulong()
        ctypes.windll.user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
        if not pid.value:
            return ""
        return (psutil.Process(pid.value).name() or "").lower().removesuffix(".exe")
    except Exception:
        return ""


def should_record_now(blacklist: str = None) -> bool:
    """当前时刻是否应记录剪贴板：前台进程不在黑名单中。

    blacklist 为逗号分隔进程名；None 时从设置读取。
    """
    if blacklist is None:
        from . import settings
        blacklist = settings.load().get("clip_process_blacklist", DEFAULT_BLACKLIST)
    names = {p.strip().lower().removesuffix(".exe")
             for p in str(blacklist).split(",") if p.strip()}
    if not names:
        return True
    fg = foreground_process_name().lower().removesuffix(".exe")
    return fg not in names


class ClipboardHistory:
    """按时间倒序保存条目；连续重复不入库；置顶条目不被淘汰。"""

    def __init__(self, max_items: int = 50, max_len: int = 5000):
        self.max_items = max(1, int(max_items))
        self.max_len = max(64, int(max_len))
        self._items = []          # 最新在前
        self._dirty = False
        self._path = os.path.join(config.APPDATA_DIR, "clipboard.json")
        self._load()
        _instances.add(self)

    def _load(self):
        """读取历史；DPAPI 密封容器为本格式，兼容旧明文文件（下次保存自动密封）。"""
        from .secure_store import load_sealed_json
        data, _sealed = load_sealed_json(self._path, [])
        items = data if isinstance(data, list) else []
        parsed = []
        for it in items:
            if isinstance(it, dict) and isinstance(it.get("text"), str):
                parsed.append({
                    "ts": int(it.get("ts", 0)),
                    "text": it["text"],
                    "kind": it.get("kind") or "text",
                    "pinned": bool(it.get("pinned", False)),
                    "data": it.get("data", "") or "",
                })
        self._items = parsed

    def mark_dirty(self):
        """防抖落盘标记：复制瞬间只改内存；由 ClipboardTab 的定时器与退出时机刷盘。

        权衡（§15）：程序崩溃最多丢最近一个刷盘周期内的几条历史——
        换取高频复制场景不在 UI 线程做 O(N) 序列化+DPAPI 密封+写盘。
        """
        self._dirty = True

    def flush_if_dirty(self):
        with _lock:
            if not getattr(self, "_dirty", False):
                return False
            self._dirty = False
            self.save()
            return True

    def save(self):
        with _lock:
            try:
                from .secure_store import save_sealed_json
                # DPAPI 密封（绑定当前用户）；返回 False=密封/写盘失败，
                # 历史将不再持久化——必须留痕，否则用户无痕丢历史无从排查
                if not save_sealed_json(self._path, self._items):
                    LOGGER.warning("剪贴板历史密封写盘失败（DPAPI/磁盘），本次变更未持久化: {}",
                                   self._path)
            except Exception:
                LOGGER.warning("剪贴板历史写盘异常", exc_info=True)

    def add(self, text, kind: str = "text", data: str = "") -> bool:
        """追加一条。空白/敏感内容/与最新条相同/与置顶条目相同则不入库。"""
        if not text or not str(text).strip():
            return False
        text = str(text)
        # 摘要只算一次：敏感/抑制两道闸共享（add 是复制热路径，旧版各算一遍）
        digest = _digest(text)
        if _text_in(_sensitive, digest):
            return False
        if _text_in(_suppressed, digest):
            return False    # 快捷粘贴回填的内容不算一次「用户复制」
        if kind == "text" and len(text) > self.max_len:
            text = text[: self.max_len]
        with _lock:
            first = self._items[0] if self._items else None
            if (first and first["text"] == text and first.get("kind", "text") == kind
                    and first.get("data", "") == data):
                return False
            if data and any(it.get("data", "") == data
                            and it.get("kind", "text") == kind and it["text"] == text
                            for it in self._items):
                return False    # 同一图片/HTML 载荷已入库，不再重复
            if any(it.get("pinned") and it["text"] == text and it.get("data", "") == data
                   for it in self._items):
                return False
            self._items.insert(0, {"ts": int(time.time()), "text": text,
                                   "kind": kind, "pinned": False, "data": data})
            self._resort()
            self._evict()
            self.mark_dirty()          # 防抖：连发复制只动内存，由定时器/退出刷盘
        return True

    def purge_older_than(self, days: int) -> int:
        """删除超过 days 天的条目（含其图片载荷；置顶条目同样过期——按天是时间语义）。

        days<=0 表示永不按天清理。返回删除条数。由每日定时器与启动时调用。
        """
        if days <= 0:
            return 0
        cutoff = time.time() - days * 86400
        with _lock:
            keep, expired = [], []
            for it in self._items:
                ts = int(it.get("ts", 0) or 0)
                # ts 缺失/为 0（旧版本条目）视为时效不可判定 → 保留（宁留勿误删，§15）
                if ts <= 0:
                    keep.append(it)
                    continue
                (expired if ts < cutoff else keep).append(it)
            if not expired:
                return 0
            self._items = keep
            self._delete_image_files(expired)
            self.mark_dirty()
            return len(expired)

    def _evict(self):
        """超额裁剪：只删最老的非置顶条目，置顶不受 max_items 限制。"""
        unpinned = [i for i, it in enumerate(self._items) if not it.get("pinned")]
        excess = len(unpinned) - self.max_items
        if excess > 0:
            oldest = unpinned[-excess:]                     # 最老的非置顶（只切一次）
            evicted = [self._items[i] for i in oldest]
            for i in sorted(oldest, reverse=True):
                del self._items[i]
            self._delete_image_files(evicted)

    def items(self) -> list:
        with _lock:
            return [dict(it) for it in self._items]

    def texts(self) -> list:
        """测试辅助：全部条目的 text 列表（生产 UI 走 items()/search()）。"""
        return [it["text"] for it in self.items()]

    def search(self, keyword: str = None, kind: str = None) -> list:
        """按关键字（大小写不敏感子串）与类型过滤。

        返回条目副本列表（置顶天然在前），每个副本额外带 "idx" 原始索引，
        供 UI 在过滤态下正确映射删除/置顶操作。
        """
        kw = (keyword or "").strip().lower()
        with _lock:
            pairs = [(i, dict(it)) for i, it in enumerate(self._items)]
        out = []
        for i, it in pairs:
            if kind and it.get("kind", "text") != kind:
                continue
            if kw and kw not in it.get("text", "").lower():
                continue
            it["idx"] = i
            out.append(it)
        return out

    def _resort(self):
        """置顶块在前；块内按时间倒序（稳定排序保证同秒条目相对顺序不变）。"""
        self._items.sort(key=lambda x: (0 if x.get("pinned") else 1,
                                        -int(x.get("ts", 0))))

    def toggle_pin(self, index: int) -> bool:
        """翻转第 index 条置顶状态并重排（置顶块在前，块内保持时间倒序）。"""
        with _lock:
            if not (0 <= index < len(self._items)):
                return False
            it = self._items[index]
            it["pinned"] = not it.get("pinned", False)
            self._resort()
            self._evict()
            self.mark_dirty()
            return True

    @staticmethod
    def _delete_image_files(items: list):
        """删除图片条目引用的 PNG 载荷（明文截图不应在磁盘无限期残留）。"""
        for it in items:
            name = it.get("data", "") if it.get("kind") == "image" else ""
            if name:
                try:
                    path = image_path(name)
                    if os.path.exists(path):
                        os.unlink(path)
                except OSError:
                    pass

    def remove(self, index: int) -> bool:
        with _lock:
            if 0 <= index < len(self._items):
                removed = self._items.pop(index)
                self._delete_image_files([removed])
                self.mark_dirty()
                return True
            return False

    def clear(self):
        with _lock:
            self._delete_image_files(self._items)
            self._items = []
            self.mark_dirty()

    @staticmethod
    def preview(text: str, n: int = 80) -> str:
        one_line = " ".join(str(text).split())
        if len(one_line) <= n:
            return one_line
        return one_line[: n - 1] + "…"


def flush_pending() -> int:
    """把所有存活实例的未落盘变更刷盘，返回发生刷盘的实例数。

    托盘「最近剪贴板」/命令面板读的是磁盘态：不先刷盘，最近一次复制
    （≤10s 防抖窗口内）恰恰是最可能想放回的条目——会看不到。"""
    n = 0
    for inst in list(_instances):
        try:
            if inst.flush_if_dirty():
                n += 1
        except Exception:
            LOGGER.debug("剪贴板 flush_pending 单实例失败", exc_info=True)
    return n
