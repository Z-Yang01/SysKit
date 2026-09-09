"""轻量持久化存储：每日流量 / 断网事件。JSON 文件位于 %APPDATA%\\SysKit。"""
import json
import os
import threading
import time

from . import config

_lock = threading.Lock()


def _path(name: str) -> str:
    os.makedirs(config.APPDATA_DIR, exist_ok=True)
    return os.path.join(config.APPDATA_DIR, name)


def _load(name: str, default):
    try:
        with open(_path(name), "r", encoding="utf-8") as f:
            return json.load(f)
    except Exception:
        return default


def _save(name: str, data):
    tmp = _path(name + ".tmp")
    try:
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump(data, f, ensure_ascii=False)
        os.replace(tmp, _path(name))
    except OSError:
        pass


class TrafficStore:
    """按天累计上下行字节：{"2026-09-07": {"sent": n, "recv": n}}"""

    def __init__(self):
        self._data = _load("traffic.json", {})
        self._dirty = False

    def add(self, sent_delta: int, recv_delta: int):
        day = time.strftime("%Y-%m-%d")
        rec = self._data.setdefault(day, {"sent": 0, "recv": 0})
        rec["sent"] += max(0, int(sent_delta))
        rec["recv"] += max(0, int(recv_delta))
        self._dirty = True
        # 只保留 62 天
        if len(self._data) > 62:
            for k in sorted(self._data)[:-62]:
                del self._data[k]

    def today(self):
        rec = self._data.get(time.strftime("%Y-%m-%d"), {"sent": 0, "recv": 0})
        return rec.get("sent", 0), rec.get("recv", 0)

    def save_if_dirty(self):
        with _lock:
            if self._dirty:
                _save("traffic.json", self._data)
                self._dirty = False


class EventStore:
    """断网/恢复事件时间线：[{ts, state, detail}]，最新在前，上限 500 条。"""

    def __init__(self, limit: int = 500):
        self._limit = limit
        self._data = _load("events.json", [])
        if not isinstance(self._data, list):
            self._data = []

    def add(self, state: str, detail: str = ""):
        self._data.insert(0, {
            "ts": time.strftime("%Y-%m-%d %H:%M:%S"),
            "state": state,
            "detail": detail[:120],
        })
        del self._data[self._limit:]
        _save("events.json", self._data)

    def items(self) -> list:
        return list(self._data)

    def recent(self, n: int = 30) -> list:
        return self._data[:n]
