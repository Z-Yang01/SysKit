"""loguru 日志：普通/错误双文件、轮转、7 天自动清理。

- syskit.log  : INFO 及以上（正常日志）
- error.log    : ERROR 及以上（错误日志，含堆栈）
- 每次启动清扫一次过期日志；常驻运行时由 monitor 每日再清扫
"""
import os
import time

from loguru import logger as LOGGER

from . import config

LOG_RETENTION_DAYS = 7
_FMT_FILE = "{time:YYYY-MM-DD HH:mm:ss} [{level}] {name}:{function}:{line} - {message}"
_FMT_CONSOLE = "<level>{message}</level>"


def setup(verbose_console: bool = False):
    """配置 sinks。GUI 与 CLI 启动时各调用一次，重复调用安全（先移除旧 sink）。"""
    os.makedirs(config.LOG_DIR, exist_ok=True)
    LOGGER.remove()
    LOGGER.add(
        os.path.join(config.LOG_DIR, "syskit.log"),
        level="INFO", rotation="5 MB", retention=f"{LOG_RETENTION_DAYS} days",
        encoding="utf-8", enqueue=True, format=_FMT_FILE,
    )
    LOGGER.add(
        os.path.join(config.LOG_DIR, "error.log"),
        level="ERROR", rotation="5 MB", retention=f"{LOG_RETENTION_DAYS} days",
        encoding="utf-8", enqueue=True, backtrace=True, format=_FMT_FILE,
    )
    import sys
    if verbose_console and sys.stderr is not None:
        LOGGER.add(sys.stderr, level="DEBUG", format=_FMT_CONSOLE)
    sweep_old_logs()
    return LOGGER


def sweep_old_logs(days: int = LOG_RETENTION_DAYS):
    """删除超过保留期的日志文件（含轮转备份），返回删除数。"""
    removed = 0
    deadline = time.time() - days * 86400
    try:
        names = os.listdir(config.LOG_DIR)
    except OSError:
        return 0
    for name in names:
        if not name.startswith(("syskit.log", "error.log", "stdio.log")):
            continue
        path = os.path.join(config.LOG_DIR, name)
        try:
            if os.path.isfile(path) and os.path.getmtime(path) < deadline:
                os.remove(path)
                removed += 1
        except OSError:
            continue
    if removed:
        LOGGER.info("日志清理：删除 {} 个超过 {} 天的旧日志", removed, days)
    return removed
