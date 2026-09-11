"""剪贴板历史：多格式条目（文本/图片/文件/HTML）、置顶、搜索、敏感内容防护。

条目 schema：{"ts", "text", "kind": "text|image|file|html", "pinned": bool, "data"}
- image 条目：text 为可搜索的占位描述（如 "[图片 128×96]"），data 为 PNG 文件名，
  实际字节存 %APPDATA%\\SysKit\\clipboard_files\\（md5 命名去重）。
- html 条目：text 为纯文本预览，data 为原始 HTML。
- 兼容旧格式 JSON（无 kind/pinned 字段）。
"""
import hashlib
import json
import os
import sys
import threading
import time

from . import config

# RLock：add/remove 在持锁时调用 save，必须可重入
_lock = threading.RLock()

# 敏感内容摘要：[(digest, until_ts)]，过期自动清理。密码库等来源标记后监听跳过。
_sensitive = []

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


def mark_sensitive(text, ttl: int = 25) -> None:
    """标记文本为敏感：ttl 秒内监听到相同内容不记录（密码库复制用）。"""
    if text is None:
        return
    digest = hashlib.sha256(str(text).encode("utf-8", "ignore")).hexdigest()
    with _lock:
        _sensitive.append({"digest": digest, "until": time.time() + max(1, int(ttl))})
        _sensitive[:] = [s for s in _sensitive if s["until"] > time.time()]


def _is_sensitive(text) -> bool:
    digest = hashlib.sha256(str(text).encode("utf-8", "ignore")).hexdigest()
    with _lock:
        _sensitive[:] = [s for s in _sensitive if s["until"] > time.time()]
        return any(s["digest"] == digest for s in _sensitive)


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
        self._path = os.path.join(config.APPDATA_DIR, "clipboard.json")
        self._load()

    def _load(self):
        try:
            with open(self._path, "r", encoding="utf-8") as f:
                data = json.load(f)
            if isinstance(data, list):
                items = []
                for it in data:
                    if isinstance(it, dict) and isinstance(it.get("text"), str):
                        items.append({
                            "ts": int(it.get("ts", 0)),
                            "text": it["text"],
                            "kind": it.get("kind") or "text",
                            "pinned": bool(it.get("pinned", False)),
                            "data": it.get("data", "") or "",
                        })
                self._items = items
        except Exception:
            self._items = []

    def save(self):
        with _lock:
            try:
                os.makedirs(config.APPDATA_DIR, exist_ok=True)
                tmp = self._path + ".tmp"
                with open(tmp, "w", encoding="utf-8") as f:
                    json.dump(self._items, f, ensure_ascii=False)
                os.replace(tmp, self._path)
            except OSError:
                pass

    def add(self, text, kind: str = "text", data: str = "") -> bool:
        """追加一条。空白/敏感内容/与最新条相同/与置顶条目相同则不入库。"""
        if not text or not str(text).strip():
            return False
        text = str(text)
        if _is_sensitive(text):
            return False
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
            self.save()
        return True

    def _evict(self):
        """超额裁剪：只删最老的非置顶条目，置顶不受 max_items 限制。"""
        unpinned = [i for i, it in enumerate(self._items) if not it.get("pinned")]
        excess = len(unpinned) - self.max_items
        if excess > 0:
            for i in sorted(unpinned[-excess:], reverse=True):
                del self._items[i]

    def items(self) -> list:
        with _lock:
            return [dict(it) for it in self._items]

    def texts(self) -> list:
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
            self.save()
            return True

    def remove(self, index: int) -> bool:
        with _lock:
            if 0 <= index < len(self._items):
                del self._items[index]
                self.save()
                return True
            return False

    def clear(self):
        with _lock:
            self._items = []
            self.save()

    @staticmethod
    def preview(text: str, n: int = 80) -> str:
        one_line = " ".join(str(text).split())
        if len(one_line) <= n:
            return one_line
        return one_line[: n - 1] + "…"
