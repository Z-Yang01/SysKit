"""JSON 设置持久化。"""
import json
import os
import threading

from . import config
from .logger import LOGGER

_lock = threading.Lock()
_cache = None


def _typed(data: dict) -> dict:
    """类型收敛：DEFAULTS 已知键的值与默认类型不兼容时收敛到默认类型，
    无法收敛（语义损坏）则回退默认值并留痕。

    为什么必须有这一层：文件半截 JSON 有兜底，但「合法 JSON + 错误类型」
    （手改/被其他工具写坏，如 poll_interval_sec="很快"）会原样穿透到消费方——
    monitor.settings() 的 int() 在 run() 循环的 try 之外，一个坏键就让整个
    监控线程静默死亡；vault_page/clip_paste/palette 等处的 int() 同样裸奔。
    只校验 DEFAULTS 已知键：last_page / modes_prev_volume 等动态键原样保留。
    """
    out = dict(data)
    for key, default in config.DEFAULTS.items():
        if key not in out:
            continue
        v = out[key]
        if isinstance(default, bool):
            if isinstance(v, bool):
                continue
            if isinstance(v, int) and v in (0, 1):
                out[key] = bool(v)          # 手改 0/1 → 按意图收敛
                continue
        elif isinstance(default, int):
            if isinstance(v, bool):         # true/false 写进 int 键
                out[key] = int(v)
                continue
            if isinstance(v, int):
                continue
            if isinstance(v, float) and float(v).is_integer():
                out[key] = int(v)           # 60.0 → 60
                continue
            if isinstance(v, str):
                try:
                    out[key] = int(v.strip())
                    continue                # "50" → 50（保留手改意图）
                except ValueError:
                    pass
        else:                               # str 键：只认字符串（数字/None 视为损坏）
            if isinstance(v, str):
                continue
        LOGGER.warning("settings: 键 {} 值类型异常({})，已回退默认 {!r}",
                       key, type(v).__name__, default)
        out[key] = default
    return out


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
    except FileNotFoundError:
        pass
    except Exception:
        # 设置文件损坏（半截 JSON/权限）→ 回退默认值，但必须留痕，否则用户的
        # 个性化设置无痕丢失、无从排查
        LOGGER.warning("settings.json 读取失败，已回退默认设置: {}", config.SETTINGS_FILE,
                       exc_info=True)
    data = _typed(data)
    _cache = data
    return dict(_cache)


def save(patch: dict):
    """合并写入设置。写盘失败时仍保留内存缓存（本次改动不丢），并向上层抛错。"""
    global _cache
    with _lock:
        data = _typed({**load(), **patch})   # 补丁同样过类型收敛：坏值不进缓存/磁盘
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
