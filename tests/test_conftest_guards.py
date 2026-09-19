"""conftest 安全网自身的回归（红线夹具的"体检"）。

整套测试的安全性建立在「未 mock 的系统调用一律炸」这一条上；这条一旦因重构
被悄悄改坏（少 patch 一个入口、改名、fixture 顺序变了），全套用例就开始跑真实
系统而没人察觉。所以哨兵本身必须有测试：装漏一条即失败。

本文件只调用被禁的入口、断言它们抛 AssertionError，不产生任何真实副作用。
"""
import os
import socket
import subprocess
import urllib.request

import pytest

from syskit import proc


def _blocked(fn, name, *args, **kwargs):
    """调用 fn 并确认它被 conftest 哨兵拦下（同时校验拦住的是哪个入口）。"""
    with pytest.raises(AssertionError) as ei:
        fn(*args, **kwargs)
    msg = str(ei.value)
    assert name in msg, f"报错没点名 {name}()：可能拦到别处或根本没装哨兵"
    assert "红线禁止" in msg


def test_proc_run_and_run_ps_are_blocked():
    _blocked(proc.run, "proc.run", ["ipconfig"])
    _blocked(proc.run_ps, "proc.run_ps", "Get-NetIPConfiguration")


def test_subprocess_level_backstop_is_blocked():
    """业务代码绕过 proc 直接起进程也拦得住（这才是进程级兜底的意义）。"""
    for fn in ("run", "check_call", "check_output", "call"):
        _blocked(getattr(subprocess, fn), f"subprocess.{fn}", ["cmd.exe", "/c", "dir"])
    _blocked(subprocess.Popen, "subprocess.Popen", ["cmd.exe", "/c", "dir"])


def test_os_launch_helpers_are_blocked():
    _blocked(os.system, "os.system", "echo hi")
    # startfile 会拉起 ShellExecute（打开浏览器 / 跑程序），同样属于真实副作用
    _blocked(os.startfile, "os.startfile", "notepad.exe")


def test_winreg_writes_are_blocked():
    winreg = pytest.importorskip("winreg")
    for fn in ("SetValue", "SetValueEx", "CreateKey", "CreateKeyEx",
               "DeleteKey", "DeleteKeyEx", "DeleteValue", "SaveKey", "LoadKey"):
        # conftest 用 hasattr 逐个装，名字写错就等于漏装且无人察觉
        assert hasattr(winreg, fn), f"winreg 没有 {fn}()：名单写错了"
        _blocked(getattr(winreg, fn), f"winreg.{fn}", *([0] * 6))


def test_socket_egress_is_blocked():
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        _blocked(s.connect, "socket.socket.connect", ("1.2.3.4", 53))
        _blocked(s.sendto, "socket.socket.sendto", (b"x", ("1.2.3.4", 53)))
    finally:
        s.close()
    # 名字解析不经 socket 实例：只拦 connect 会漏掉直接查 DNS 的代码路径
    _blocked(socket.getaddrinfo, "socket.getaddrinfo", "example.com", 80)
    _blocked(socket.gethostbyname, "socket.gethostbyname", "example.com")


def test_url_paths_that_bypass_urlopen_are_blocked_too():
    """只拦 urlopen 等于没拦：urlretrieve 与 build_opener().open() 都不走它。"""
    _blocked(urllib.request.urlretrieve, "urllib.request.urlretrieve",
             "http://127.0.0.1/x")
    _blocked(urllib.request.build_opener().open, "urllib.request.OpenerDirector.open",
             "http://127.0.0.1/x")
    _blocked(urllib.request.urlopen, "urllib.request.urlopen", "http://127.0.0.1/x")


def test_sound_playback_is_blocked():
    winsound = pytest.importorskip("winsound")
    _blocked(winsound.PlaySound, "winsound.PlaySound", None, 0)
    _blocked(winsound.Beep, "winsound.Beep", 440, 1)


def test_persistence_paths_point_into_the_test_tmp_dir(tmp_path):
    """红线 2：%APPDATA%\\SysKit 与 %LOCALAPPDATA%\\SysKit 必须已重定向到临时目录。"""
    from syskit import config

    for attr in ("APPDATA_DIR", "LOCALAPPDATA_DIR", "SETTINGS_FILE",
                 "LOG_DIR", "BACKUP_DIR"):
        path = getattr(config, attr)
        assert path.startswith(str(tmp_path)), f"{attr} 未重定向：{path}"
        assert "SysKit" in path


def test_settings_cache_is_reset_per_test(tmp_path):
    """settings 模块级缓存每个用例复位一次，否则上一条用例的写入会串到下一条。"""
    from syskit import settings as settings_mod

    assert settings_mod._cache is None or settings_mod._cache == {}
