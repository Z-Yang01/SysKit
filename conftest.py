"""pytest 全局夹具：测试安全网。

三条红线（见任务说明）：
1. 任何测试不得执行真实的网络修复动作 —— 本文件用 autouse 夹具把
   proc.run / proc.run_ps / subprocess / winreg 写操作 / socket 网络连接
   全部替换为「遇调用即失败」的哨兵；测试需要系统调用返回值时，必须在
   自己的测试里显式 monkeypatch 目标函数返回假数据。
2. 不碰 %APPDATA%\\SysKit 与 %LOCALAPPDATA%\\SysKit 真实数据 —— autouse
   夹具把 config 中的全部持久化路径重定向到 pytest 临时目录。
   （config/settings/stores/logger 均在调用时动态读取 config.X，重定向即生效）
3. 不修改业务代码。

各分组测试请勿修改本文件（避免并行冲突）；需要的局部夹具写在自己的测试文件里。
"""
import os

# 必须在任意 Qt 导入 / QApplication 创建之前生效
os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import pytest


# ---------------- 红线 2：持久化隔离 ----------------

@pytest.fixture(autouse=True)
def isolated_appdata(tmp_path, monkeypatch):
    """把 SysKit 的全部持久化路径重定向到本测试的临时目录。

    同时复位 settings 模块缓存，保证测试间互不影响。
    """
    from syskit import config

    appdata = tmp_path / "appdata" / "SysKit"
    localappdata = tmp_path / "localappdata" / "SysKit"
    monkeypatch.setattr(config, "APPDATA_DIR", str(appdata), raising=False)
    monkeypatch.setattr(config, "LOCALAPPDATA_DIR", str(localappdata), raising=False)
    monkeypatch.setattr(config, "SETTINGS_FILE", str(appdata / "settings.json"), raising=False)
    monkeypatch.setattr(config, "LOG_DIR", str(localappdata / "logs"), raising=False)
    monkeypatch.setattr(config, "BACKUP_DIR", str(localappdata / "backup"), raising=False)

    from syskit import settings as settings_mod
    monkeypatch.setattr(settings_mod, "_cache", None, raising=False)
    yield


# ---------------- 红线 1：系统调用哨兵 ----------------

def _sentinel(name):
    def _blocked(*args, **kwargs):
        raise AssertionError(
            f"测试试图执行真实系统调用 {name}() —— 红线禁止。"
            "请在测试中显式 monkeypatch 目标模块函数返回假数据。")
    return _blocked


@pytest.fixture(autouse=True)
def block_system_calls(monkeypatch):
    """未显式 mock 的系统调用一律抛 AssertionError（而非真实执行）。"""
    from syskit import proc

    monkeypatch.setattr(proc, "run", _sentinel("proc.run"))
    monkeypatch.setattr(proc, "run_ps", _sentinel("proc.run_ps"))

    # 进程级兜底：直接拦 subprocess 与 os.system
    import subprocess
    for fn in ("run", "Popen", "check_call", "check_output", "call"):
        if hasattr(subprocess, fn):
            monkeypatch.setattr(subprocess, fn, _sentinel(f"subprocess.{fn}"))
    monkeypatch.setattr(os, "system", _sentinel("os.system"))

    # 注册表写操作哨兵（读操作放行，确定性由各组测试自行 mock）
    try:
        import winreg
    except ImportError:
        winreg = None
    if winreg:
        for fn in ("SetValueEx", "CreateKey", "CreateKeyEx", "DeleteKey",
                   "DeleteValue", "SaveKey"):
            if hasattr(winreg, fn):
                monkeypatch.setattr(winreg, fn, _sentinel(f"winreg.{fn}"))

    # 网络连接哨兵：任何真实出网连接都视为测试缺陷
    import socket
    monkeypatch.setattr(socket.socket, "connect",
                        _sentinel("socket.socket.connect"))
    try:
        import urllib.request
        monkeypatch.setattr(urllib.request, "urlopen",
                            _sentinel("urllib.request.urlopen"))
    except ImportError:
        pass
    yield


# ---------------- 通用测试夹具 ----------------

@pytest.fixture
def fake_cmd_result():
    """构造 proc.CmdResult 假对象的工厂。"""
    from syskit.proc import CmdResult

    def _make(code=0, stdout="", stderr="", timeout=False):
        return CmdResult(code=code, stdout=stdout, stderr=stderr, timeout=timeout)
    return _make


# ---------------- 会话收尾：QThread 排空（缓解原生 fail-fast） ----------------

@pytest.fixture(scope="session", autouse=True)
def _drain_qt_threads_on_session_end():
    """会话收尾兜底：等所有仍在运行的 QThread 结束。

    QThread 对象在「仍在运行」时被 GC / 被 Qt 析构会触发 qFatal → 整个 pytest
    进程以 0xC0000409 崩溃。该崩溃发生在**全部用例通过之后**的收尾阶段，却会把
    退出码污染成 127，掩盖真实结果（README / 各 UI 测试文件头均有记载）。

    各测试文件的局部夹具已尽力回收自己创建的线程，这里再做一个进程级兜底：
    扫描 GC 可见的 QThread，给它们一点时间收尾并泵一次事件循环。
    """
    yield
    try:
        import gc
        from PySide6.QtCore import QThread
        from PySide6.QtWidgets import QApplication

        deadline = time.time() + 8
        while time.time() < deadline:
            running = [o for o in gc.get_objects()
                       if isinstance(o, QThread) and o.isRunning()]
            if not running:
                break
            for t in running:
                try:
                    t.wait(500)
                except Exception:
                    pass
            app = QApplication.instance()
            if app is not None:
                app.processEvents()
        app = QApplication.instance()
        if app is not None:
            app.processEvents()
        gc.collect()
    except Exception:
        pass
