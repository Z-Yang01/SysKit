"""SysKit 包初始化：windowed 模式下修复无效的标准流句柄。

PyInstaller --windowed 打包后进程没有控制台，CRT 的 stdout/stderr 句柄无效。
Qt/C++ 层访问这些句柄会直接 fail-fast（BEX64 / 0xc0000409），Python 层与
qInstallMessageHandler 都拦不住。这里在一切 Qt 导入之前用 dup2 把句柄
重定向到真实文件（stdio.log），同时兼作底层输出的取证留痕。
"""
import os
import sys


def _rotate_if_huge(path: str, limit: int = 5 * 1024 * 1024) -> None:
    """stdio.log 是 O_APPEND 追加且每次启动刷新 mtime——按天清扫永远轮不到
    它，长期驻留场景无界增长；超限时滚动为 .old（留一份最近取证即可）。"""
    try:
        if os.path.exists(path) and os.path.getsize(path) > limit:
            old = path + ".old"
            if os.path.exists(old):
                os.remove(old)
            os.replace(path, old)
    except OSError:
        pass


def _fix_windows_stdio():
    try:
        for stream in (sys.stdout, sys.stderr):
            if stream is not None and hasattr(stream, "fileno"):
                return  # 已有真实句柄（console 模式），无需处理
    except Exception:
        pass
    try:
        from . import config
        os.makedirs(config.LOG_DIR, exist_ok=True)
        path = os.path.join(config.LOG_DIR, "stdio.log")
        _rotate_if_huge(path)
        fd = os.open(path, os.O_APPEND | os.O_CREAT | os.O_WRONLY, 0o644)
        os.dup2(fd, 1)
        os.dup2(fd, 2)
        if fd > 2:
            os.close(fd)
        sys.stdout = os.fdopen(1, "w", buffering=1, encoding="utf-8", errors="replace")
        sys.stderr = os.fdopen(2, "w", buffering=1, encoding="utf-8", errors="replace")
    except Exception:
        pass


if sys.platform == "win32":
    _fix_windows_stdio()
