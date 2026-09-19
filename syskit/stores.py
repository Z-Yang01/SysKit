"""轻量持久化存储：每日流量 / 断网事件。JSON 文件位于 %APPDATA%\\SysKit。"""
import json
import os
import threading
import time

from . import config
from .logger import LOGGER

_lock = threading.Lock()


def _path(name: str) -> str:
    os.makedirs(config.APPDATA_DIR, exist_ok=True)
    return os.path.join(config.APPDATA_DIR, name)


def _load(name: str, default):
    try:
        with open(_path(name), "r", encoding="utf-8") as f:
            return json.load(f)
    except FileNotFoundError:
        return default
    except Exception:
        # 数据文件损坏 → 回退默认（流量/事件清零），留痕供排查
        LOGGER.warning("{} 读取失败，对应统计数据已重置", _path(name), exc_info=True)
        return default


def _save(name: str, data) -> bool:
    """原子写盘。成功返回 True；失败吞掉 OSError 并返回 False（调用方决定是否重试）。"""
    tmp = _path(name + ".tmp")
    try:
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump(data, f, ensure_ascii=False)
        os.replace(tmp, _path(name))
        return True
    except OSError:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        return False


def load_json(name: str, default):
    """公共 JSON 读接口（traffic/events 之外的新持久化文件，如 speedtest.json）。"""
    return _load(name, default)


def save_json(name: str, data) -> bool:
    """公共 JSON 原子写接口。成功 True；失败 False（调用方决定是否提示/重试）。"""
    return _save(name, data)


class TrafficStore:
    """按天累计上下行字节：{"2026-09-07": {"sent": n, "recv": n}}"""

    def __init__(self):
        self._data = _load("traffic.json", {})
        if not isinstance(self._data, dict):
            self._data = {}
        self._dirty = False
        self._today_cache = (None, 0, 0)    # (day, sent, recv)：秒级热路径免锁

    def add(self, sent_delta: int, recv_delta: int):
        with _lock:
            day = time.strftime("%Y-%m-%d")
            rec = self._data.setdefault(day, {"sent": 0, "recv": 0})
            ds, dr = max(0, int(sent_delta)), max(0, int(recv_delta))
            if ds == 0 and dr == 0:
                return                      # 无增量：不标脏（避免空转写盘）
            rec["sent"] += ds
            rec["recv"] += dr
            self._today_cache = (day, rec["sent"], rec["recv"])
            self._dirty = True
            # 只保留 62 天
            if len(self._data) > 62:
                for k in sorted(self._data)[:-62]:
                    del self._data[k]

    def today(self):
        day = time.strftime("%Y-%m-%d")
        cached_day, sent, recv = self._today_cache
        if cached_day == day:
            return sent, recv               # 每秒热路径：无锁快照
        with _lock:
            rec = self._data.get(day, {"sent": 0, "recv": 0})
            self._today_cache = (day, rec.get("sent", 0), rec.get("recv", 0))
            return self._today_cache[1], self._today_cache[2]

    def days(self) -> dict:
        """按天数据拷贝（{"YYYY-MM-DD": {"sent", "recv"}}）——网络日报聚合用。"""
        with _lock:
            return {k: dict(v) for k, v in self._data.items()}

    def save_if_dirty(self):
        with _lock:
            if self._dirty and _save("traffic.json", self._data):
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
