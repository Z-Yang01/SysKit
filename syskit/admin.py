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
    """以管理员拉起新实例（触发 UAC），返回是否成功发起。

    注意：ShellExecuteW 是异步的——返回 True 时新实例可能仍在启动中，
    调用方必须随后自行退出，否则新实例撞单实例互斥体弹「已在运行」后自灭
    （「以管理员重启」按钮曾因此整体失效）。"""
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
