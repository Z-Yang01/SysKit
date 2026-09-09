"""阶段一示例测试：验证 conftest 安全网（隔离 + 哨兵 + offscreen Qt）本身有效。"""
import os

import pytest


def test_config_identity():
    from syskit import config
    assert config.APP_NAME == "SysKit"
    assert "阿里" in config.DNS_PRESETS


def test_appdata_isolated_to_tmp():
    """红线 2 验证：持久化路径已指向临时目录，settings 落盘不进真实 APPDATA。"""
    from syskit import config, settings
    assert "pytest" in config.APPDATA_DIR or os.sep + "appdata" in config.APPDATA_DIR
    assert "SysKit" in config.SETTINGS_FILE

    saved = settings.save({"dark_mode": True})
    assert saved["dark_mode"] is True
    assert os.path.isfile(config.SETTINGS_FILE)          # 写进了临时目录
    assert settings.load()["dark_mode"] is True


def test_settings_cache_reset_between_tests():
    """上一个测试写入的 dark_mode 不应泄漏到本测试（autouse 复位 _cache）。"""
    from syskit import settings
    assert settings.load()["dark_mode"] is False   # DEFAULTS 默认值


def test_proc_run_blocked_by_sentinel():
    """红线 1 验证：未显式 mock 的系统命令调用直接失败而非真实执行。"""
    from syskit import proc
    with pytest.raises(AssertionError, match="proc.run"):
        proc.run(["netsh", "wlan", "show"])


def test_subprocess_blocked_by_sentinel():
    import subprocess
    with pytest.raises(AssertionError, match="subprocess"):
        subprocess.run(["cmd", "/c", "echo hi"])


def test_winreg_write_blocked_by_sentinel():
    import winreg
    with pytest.raises(AssertionError, match="winreg"):
        winreg.SetValueEx(None, "x", 0, winreg.REG_SZ, "y")


def test_socket_connect_blocked_by_sentinel():
    import socket
    with pytest.raises(AssertionError, match="socket"):
        socket.socket().connect(("127.0.0.1", 80))


def test_qtbot_offscreen(qtbot):
    """pytest-qt 在 offscreen 平台可用：控件可创建、可显示。"""
    from PySide6.QtWidgets import QLabel
    label = QLabel("pytest-qt ok")
    qtbot.addWidget(label)
    label.show()
    assert label.isVisible()
    assert os.environ.get("QT_QPA_PLATFORM") == "offscreen"
