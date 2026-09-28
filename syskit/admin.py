"""管理员权限检测与自提权重启。"""
import ctypes
import sys
from subprocess import list2cmdline

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
    （「以管理员重启」按钮曾因此整体失效）。

    参数拼接用 list2cmdline 逐个加引号，不能 " ".join：argv 里的参数可能
    含空格（如 --open "C:\\Program Files\\a.txt"），裸 join 后新进程会把它
    拆成两个参数——提权重启后行为悄然变脸，且只在路径带空格的机器上复现。
    cli_args=None 才继承当前进程的 argv；显式传 [] 就是不带参数重启
    （旧实现 `cli_args or argv` 把两者混同，[] 会被偷换成继承）。
    """
    if sys.platform != "win32":
        return False
    args = list(cli_args) if cli_args is not None else list(sys.argv[1:])
    exe = sys.executable
    if is_frozen():
        params = list2cmdline(args)
    else:
        # 源码运行：python -m syskit ...
        params = ("-m syskit " + list2cmdline(args)) if args else "-m syskit "
    ret = ctypes.windll.shell32.ShellExecuteW(None, "runas", exe, params, None, SW_SHOWNORMAL)
    return ret > 32
