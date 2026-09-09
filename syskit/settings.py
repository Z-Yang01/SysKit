"""JSON 设置持久化。"""
import json
import os
import threading

from . import config

_lock = threading.Lock()
_cache = None


def load() -> dict:
    global _cache
    if _cache is not None:
        return dict(_cache)
    data = dict(config.DEFAULTS)
    try:
        with open(config.SETTINGS_FILE, "r", encoding="utf-8") as f:
            stored = json.load(f)
        if isinstance(stored, dict):
            data.update(stored)
    except Exception:
        pass
    _cache = data
    return dict(_cache)


def save(patch: dict):
    global _cache
    with _lock:
        data = load()
        data.update(patch)
        _cache = data
        os.makedirs(config.APPDATA_DIR, exist_ok=True)
        with open(config.SETTINGS_FILE, "w", encoding="utf-8") as f:
            json.dump(data, f, ensure_ascii=False, indent=2)
    return dict(_cache)
