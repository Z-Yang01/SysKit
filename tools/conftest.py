"""tools/ 目录最小 conftest：真机冒烟驱动的开关与安全网。

根 conftest.py（Python 回归测试的隔离/哨兵网）随 Python 版归档删除；
Python 版退役后本目录仅剩真机冒烟驱动（ctypes 自足，无 PySide6/syskit
依赖），本文件只保留它运行所需的两件事：

1. 注册 --run-real 选项与 real 标记——未注册时 pytest 对未知参数直接
   usage error 退出（exit 4），冒烟脚本连收集都进不去。
2. 系统调用哨兵：默认拦截 subprocess / os.startfile / winreg 写 / socket
   出网等真实副作用。唯一豁免：@pytest.mark.real + --run-real **同时**
   满足——缺一即照常武装，误运行（忘带 --run-real）时第一发 os.startfile
   即被击落，而不是把键鼠注入真实桌面。
"""
import os
import sys

import pytest


def pytest_addoption(parser):
    try:
        parser.addoption("--run-real", action="store_true", default=False,
                         help="真机冒烟开关：允许 @pytest.mark.real 用例实际运行"
                              "（向真实桌面注入按键，须人工在场）")
    except ValueError:
        pass  # 根 conftest.py 尚存（归档过渡期）时已注册，避免双注册 usage error


def pytest_configure(config):
    if "real" not in str(config.getini("markers")):
        config.addinivalue_line(
            "markers", "real: 真机冒烟（人工在场 + --run-real，缺一自动 skip/拦截）")


def _sentinel(name):
    def _blocked(*args, **kwargs):
        raise AssertionError(
            f"测试试图执行真实系统调用 {name}() —— 红线禁止。真机冒烟请用"
            " python -m pytest tools/test_smoke_real_rs.py --run-real -v（人工在场）。")
    return _blocked


def _sentinels_armed_for(request) -> bool:
    """豁免须同时满足：用例带 @pytest.mark.real 且命令行显式 --run-real。"""
    if request.node.get_closest_marker("real") is None:
        return True
    try:
        run_real = bool(request.config.getoption("--run-real"))
    except (AttributeError, ValueError):    # 非常规 request（桩）
        run_real = "--run-real" in sys.argv
    return not run_real


@pytest.fixture(autouse=True)
def block_system_calls(request, monkeypatch):
    """未豁免时，真实副作用一律抛 AssertionError（而非真实执行）。"""
    if not _sentinels_armed_for(request):
        yield
        return

    import subprocess

    for fn in ("run", "Popen", "check_call", "check_output", "call"):
        if hasattr(subprocess, fn):
            monkeypatch.setattr(subprocess, fn, _sentinel(f"subprocess.{fn}"))
    monkeypatch.setattr(os, "system", _sentinel("os.system"))
    if hasattr(os, "startfile"):
        # 启动外部程序 / ShellExecute 打开链接都是真实副作用（还会拉起浏览器）
        monkeypatch.setattr(os, "startfile", _sentinel("os.startfile"))
    # spawn/exec/popen 兜底：os 层直启进程不经 subprocess（nt 模块直通 CRT）
    for fn in ("spawnv", "spawnve",
               "spawnl", "spawnle",
               "execv", "execve", "execvp", "execvpe",
               "execl", "execle", "execlp", "execlpe",
               "popen"):
        if hasattr(os, fn):
            monkeypatch.setattr(os, fn, _sentinel(f"os.{fn}"))

    # 注册表写操作哨兵（读操作放行）
    try:
        import winreg
    except ImportError:
        winreg = None
    if winreg:
        for fn in ("SetValue", "SetValueEx", "CreateKey", "CreateKeyEx",
                   "DeleteKey", "DeleteKeyEx", "DeleteValue", "SaveKey", "LoadKey"):
            if hasattr(winreg, fn):
                monkeypatch.setattr(winreg, fn, _sentinel(f"winreg.{fn}"))

    # 网络连接哨兵：任何真实出网连接/解析都视为测试缺陷
    import socket
    for fn in ("connect", "connect_ex", "sendto"):
        monkeypatch.setattr(socket.socket, fn, _sentinel(f"socket.socket.{fn}"))
    for fn in ("getaddrinfo", "gethostbyname"):
        monkeypatch.setattr(socket, fn, _sentinel(f"socket.{fn}"))
    try:
        import urllib.request
        # urlopen 只是 OpenerDirector.open 的薄封装：连 open 才拦得住 build_opener
        monkeypatch.setattr(urllib.request, "urlopen",
                            _sentinel("urllib.request.urlopen"))
        monkeypatch.setattr(urllib.request, "urlretrieve",
                            _sentinel("urllib.request.urlretrieve"))
        monkeypatch.setattr(urllib.request.OpenerDirector, "open",
                            _sentinel("urllib.request.OpenerDirector.open"))
    except ImportError:
        pass

    yield
