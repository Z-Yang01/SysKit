"""管理员权限检测与自提权重启。"""
import ctypes
import sys

SW_SHOWNORMAL = 1


def is_admin() -> bool:
    if sys.platform != "win32":
        return False
    try:
        return bool(ctypes.windll.shell32.IsUserAnAdmin())
    except Exception:
        return False


def is_frozen() -> bool:
    return getattr(sys, "frozen", False)


def elevate_and_restart(cli_args: list = None) -> bool:
    """以管理员重启自身（触发 UAC）。成功则不返回。"""
    if sys.platform != "win32":
        return False
    params = " ".join(cli_args or sys.argv[1:])
    exe = sys.executable
    if is_frozen():
        target = exe
        params = " ".join(cli_args or sys.argv[1:])
    else:
        # 源码运行：python -m syskit ...
        target = exe
        params = "-m syskit " + (params or " ".join(sys.argv[1:]))
    ret = ctypes.windll.shell32.ShellExecuteW(None, "runas", target, params, None, SW_SHOWNORMAL)
    return ret > 32
