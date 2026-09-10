import json
import os
import threading
import time

from . import config

# RLock：add/remove 在持锁时调用 save，必须可重入
_lock = threading.RLock()


class ClipboardHistory:
    """按时间倒序保存文本条目；连续重复内容只保留一条。"""

    def __init__(self, max_items: int = 50, max_len: int = 5000):
        self.max_items = max(1, int(max_items))
        self.max_len = max(64, int(max_len))
        self._items = []          # [{"ts": int, "text": str}]，最新在前
        self._path = os.path.join(config.APPDATA_DIR, "clipboard.json")
        self._load()

    def _load(self):
        try:
            with open(self._path, "r", encoding="utf-8") as f:
                data = json.load(f)
            if isinstance(data, list):
                self._items = [it for it in data
                               if isinstance(it, dict) and isinstance(it.get("text"), str)]
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

    def add(self, text) -> bool:
        """追加一条。空白/过长截断/与最新一条相同则不入库。返回是否新增。"""
        if not text or not str(text).strip():
            return False
        text = str(text)
        if len(text) > self.max_len:
            text = text[: self.max_len]
        with _lock:
            if self._items and self._items[0]["text"] == text:
                return False
            self._items.insert(0, {"ts": int(time.time()), "text": text})
            del self._items[self.max_items:]
            self.save()
        return True

    def items(self) -> list:
        with _lock:
            return [dict(it) for it in self._items]

    def texts(self) -> list:
        return [it["text"] for it in self.items()]

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
