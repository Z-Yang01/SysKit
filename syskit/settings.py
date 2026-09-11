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
    """合并写入设置。写盘失败时仍保留内存缓存（本次改动不丢），并向上层抛错。"""
    global _cache
    with _lock:
        data = load()
        data.update(patch)
        _cache = data          # 先更新缓存：即使写盘失败，本次改动在进程内仍然生效
        os.makedirs(config.APPDATA_DIR, exist_ok=True)
        tmp = config.SETTINGS_FILE + ".tmp"
        try:
            with open(tmp, "w", encoding="utf-8") as f:
                json.dump(data, f, ensure_ascii=False, indent=2)
            os.replace(tmp, config.SETTINGS_FILE)   # 原子替换，避免半截 JSON 破坏配置
        except OSError:
            try:
                os.unlink(tmp)
            except OSError:
                pass
            raise
    return dict(_cache)
