"""打包/安装/配置链路回归（2026-09-19 打包链路审计）。

用文本断言锁定安装器与构建脚本的的关键不变式（不执行 ISCC/schtasks）：
- ISS 自启任务 /TR 必须带内层引号（schtasks 取首个空格前 token 当可执行文件，
  默认 C:\Program Files\SysKit 路径下曾必然失效，且留下 Program.exe 劫持面）；
- CU 免管理员安装不创建 /RL HIGHEST 任务（创建/卸载都无权，只会残留）；
- AppMutex 与应用互斥体名一致（覆盖安装友好提示）；
- 便携任务描述不得再承诺「卸载即清」（Inno 只删空目录，承诺曾为假）；
- .iss 必须带 UTF-8 BOM（ISCC 6.0-6.2 对无 BOM 文件按 ANSI 解码 → 中文乱码）；
- build.bat 版本捕获失败必须中止（曾产出 SysKit--setup.exe 空版本产物）。
另锁定 main.py 未知选项报错与提权重启后旧实例退出。
"""
import os
import sys

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

import pytest

from test_ui_smoke import main_win, make_main_window, ui_offline_mocks  # noqa: E402,F401

ISS = open(os.path.join(os.path.dirname(__file__), "..", "installer", "SysKit.iss"),
           "rb").read().decode("utf-8-sig")
BUILD_BAT = open(os.path.join(os.path.dirname(__file__), "..", "build.bat"),
                 "rb").read().decode("utf-8")


def test_iss_autostart_tr_has_inner_quotes():
    """/TR 值 = "\"C:\\...\\SysKit.exe\" --minimized"：内层引号不可省。"""
    assert '/TR "\\' in ISS                     # 外层引号内的内层引号起始
    assert '\\SysKit.exe\\" --minimized"' in ISS


def test_iss_autostart_gated_on_isadmin():
    """/CURRENTUSER 免管理员安装：创建与卸载都无权操作根目录任务 → 不创建。"""
    assert "WizardIsTaskSelected('autostart') and IsAdmin()" in ISS


def test_iss_app_mutex_matches_app():
    assert "AppMutex=SysKit_SingleInstance_Mutex" in ISS
    from syskit.ui import app as app_mod
    src = open(app_mod.__file__, encoding="utf-8").read()
    assert '"SysKit_SingleInstance_Mutex"' in src        # 两处名字一致


def test_iss_portable_task_no_false_uninstall_promise():
    """曾描述「卸载即清」：Inno 只删空目录，应用写出的 data\* 会全部残留。"""
    assert "卸载即清" not in ISS


def test_iss_has_utf8_bom():
    raw = open(os.path.join(os.path.dirname(__file__), "..", "installer", "SysKit.iss"),
               "rb").read()
    assert raw.startswith(b"\xef\xbb\xbf")   # ISCC 6.0-6.2 对无 BOM 按 ANSI 解码


def test_build_bat_aborts_on_empty_version():
    assert "if not defined VER goto :err" in BUILD_BAT


def test_iss_uninstall_removes_autostart_task():
    """install.bat 已移除（setup.exe 唯一安装路径）；卸载器必须清自启任务。"""
    assert "/Delete /TN SysKitAutostart" in ISS


# ---------------- main.py 未知选项 ----------------

def test_main_unknown_option_exits_2(monkeypatch, capsys):
    from syskit import main as main_mod
    monkeypatch.setattr(sys, "argv", ["syskit", "--minimizd"])
    with pytest.raises(SystemExit) as ei:
        main_mod.run()
    assert ei.value.code == 2
    assert "未知选项" in capsys.readouterr().out


def test_main_unknown_subcommand_still_exits_2(monkeypatch, capsys):
    from syskit import main as main_mod
    monkeypatch.setattr(sys, "argv", ["syskit", "dianose"])
    with pytest.raises(SystemExit) as ei:
        main_mod.run()
    assert ei.value.code == 2
    assert "未知命令" in capsys.readouterr().out


def test_main_minimized_still_reaches_gui(monkeypatch):
    """--minimized 白名单语义不回退：透传给 GUI 且不报未知选项。"""
    from syskit import main as main_mod
    seen = {}
    fake_app = types_module()
    monkeypatch.setitem(sys.modules, "syskit.ui.app", fake_app)
    monkeypatch.setattr(fake_app, "run_gui",
                        lambda: seen.setdefault("minimized", "--minimized" in sys.argv),
                        raising=False)
    monkeypatch.setattr(sys, "argv", ["syskit", "--minimized"])
    main_mod.run()
    assert seen["minimized"] is True


def types_module():
    import types
    return types.ModuleType("syskit.ui.app")


# ---------------- 提权重启后旧实例退出 ----------------

def test_elevate_restart_quits_old_instance(main_win, monkeypatch):
    """UAC 成功 → 本实例 quit（否则新实例撞单例互斥体自灭——曾整体失效）；
    UAC 拒绝 → 状态栏提示且不退出。"""
    from syskit import admin
    from syskit.ui.main_window import MainWindow

    quitted = []
    monkeypatch.setattr(admin, "elevate_and_restart", lambda args: True)
    monkeypatch.setattr(MainWindow, "quit_app", lambda self: quitted.append(1))
    main_win._elevate_restart()
    assert quitted == [1]

    quitted.clear()
    monkeypatch.setattr(admin, "elevate_and_restart", lambda args: False)
    main_win._elevate_restart()
    assert quitted == []                       # 失败不退出
    assert "UAC" in main_win.statusBar().currentMessage() or \
           "提权" in main_win.statusBar().currentMessage()


# ---------------- 配置死键 ----------------

def test_no_ghost_start_with_windows_key():
    """DEFAULTS 的 start_with_windows 曾是永不读取的死键（自启走 autostart 实时查询）。"""
    from syskit import config
    assert "start_with_windows" not in config.DEFAULTS
