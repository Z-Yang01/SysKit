"""syskit/main.py 与 __main__ 分流测试：CLI 子命令 / help / GUI 路径（不启动真实 GUI）。"""
import builtins
import sys

import pytest


@pytest.fixture
def restore_argv(monkeypatch):
    monkeypatch.setattr(sys, "argv", ["syskit"])
    yield


def test_cli_subcommand_routes_to_cli_main(monkeypatch):
    from syskit import main as main_mod
    from syskit import cli as cli_mod

    seen = {}

    def fake_cli_main(argv=None):
        seen["argv"] = list(argv or [])
        return 0

    monkeypatch.setattr(cli_mod, "main", fake_cli_main)
    monkeypatch.setattr(sys, "argv", ["syskit", "diagnose", "--quick"])

    with pytest.raises(SystemExit) as ei:
        main_mod.run()
    assert ei.value.code == 0
    assert seen["argv"] == ["diagnose", "--quick"]


def test_repair_subcommand_routes(monkeypatch):
    from syskit import main as main_mod
    from syskit import cli as cli_mod

    seen = {}

    def fake_cli_main(argv=None):
        seen["argv"] = list(argv or [])
        return 0

    monkeypatch.setattr(cli_mod, "main", fake_cli_main)
    monkeypatch.setattr(sys, "argv", ["syskit", "repair", "stack", "-y"])

    with pytest.raises(SystemExit) as ei:
        main_mod.run()
    assert ei.value.code == 0
    assert seen["argv"] == ["repair", "stack", "-y"]


@pytest.mark.parametrize("flag", ["help", "--help", "-h"])
def test_help_flags_route_to_empty_cli(monkeypatch, flag):
    """help/*-h → cli.main([])（argparse 自己打印帮助）。"""
    from syskit import main as main_mod
    from syskit import cli as cli_mod

    seen = {}

    def fake_cli_main(argv=None):
        seen["argv"] = list(argv) if argv is not None else None
        return 0

    monkeypatch.setattr(cli_mod, "main", fake_cli_main)
    monkeypatch.setattr(sys, "argv", ["syskit", flag])

    with pytest.raises(SystemExit) as ei:
        main_mod.run()
    assert ei.value.code == 0
    assert seen["argv"] == []


def test_no_args_runs_gui(monkeypatch):
    from syskit import main as main_mod
    from syskit.ui import app as app_mod

    calls = []
    monkeypatch.setattr(app_mod, "run_gui", lambda: calls.append("gui"))
    monkeypatch.setattr(sys, "argv", ["syskit"])

    main_mod.run()  # GUI 路径不 sys.exit
    assert calls == ["gui"]


def test_minimized_passthrough_still_gui(monkeypatch):
    """--minimized 不是 CLI 子命令 → 走 GUI（透传给 app.py）。"""
    from syskit import main as main_mod
    from syskit.ui import app as app_mod

    calls = []
    monkeypatch.setattr(app_mod, "run_gui", lambda: calls.append(sys.argv[1:]))
    monkeypatch.setattr(sys, "argv", ["syskit", "--minimized"])

    main_mod.run()
    assert calls and calls[0] == ["--minimized"]


def test_gui_import_error_exits_1(monkeypatch):
    from syskit import main as main_mod

    real_import = builtins.__import__

    def fake_import(name, globals=None, locals=None, fromlist=(), level=0):
        # from .ui.app import run_gui
        if fromlist and "run_gui" in fromlist:
            raise ImportError("PySide6 未安装")
        if "ui.app" in name:
            raise ImportError("PySide6 未安装")
        return real_import(name, globals, locals, fromlist, level)

    monkeypatch.setattr(builtins, "__import__", fake_import)
    monkeypatch.setattr(sys, "argv", ["syskit"])

    with pytest.raises(SystemExit) as ei:
        main_mod.run()
    assert ei.value.code == 1


def test_main_module_run_guard(monkeypatch):
    """syskit/main.py 的 if __name__ 守卫：作为模块导入时不自动 run()。"""
    import syskit.main as main_mod
    # 导入后 run 应可引用且非自动执行
    assert callable(main_mod.run)


def test_cli_main_no_argv_uses_sys_argv(monkeypatch):
    """cli.main(None) 时读取 sys.argv[1:]。"""
    from syskit import cli as cli_mod
    from syskit import admin as admin_mod

    seen = {}

    def fake_diagnose(quick=False):
        seen["quick"] = quick
        return 42

    monkeypatch.setattr(cli_mod, "cmd_diagnose", fake_diagnose)
    monkeypatch.setattr(cli_mod.logger, "setup", lambda **k: None)
    monkeypatch.setattr(admin_mod, "is_admin", lambda: True)
    monkeypatch.setattr(sys, "argv", ["syskit", "diagnose"])

    rc = cli_mod.main(None)
    assert rc == 42
    assert seen["quick"] is False


def test_unknown_subcommand_errors_instead_of_gui(monkeypatch, capsys):
    """回归：未知子命令明确报错 exit 2（曾静默弹整个 GUI 掩盖拼写错误）。"""
    from syskit import main as main_mod

    monkeypatch.setattr(sys, "argv", ["syskit", "dianose"])   # 拼写错误
    # 不真跑 run_gui：patch 掉 GUI 导入路径的行为不可行（函数内 import），
    # 直接断言 run() 的 sys.exit(2) 且未进入 GUI
    monkeypatch.setattr(sys, "exit", lambda code=0: (_ for _ in ()).throw(
        SystemExit(code)))
    with pytest.raises(SystemExit) as ei:
        main_mod.run()
    assert ei.value.code == 2
    out = capsys.readouterr().out
    assert "未知命令" in out and "dianose" in out


def test_minimized_flag_still_launches_gui(monkeypatch):
    """--minimized 不被未知命令误判：仍走 GUI。"""
    from syskit import main as main_mod
    monkeypatch.setattr(sys, "argv", ["syskit", "--minimized"])
    called = []
    import types
    fake_app = types.ModuleType("syskit.ui.app")
    fake_app.run_gui = lambda: called.append(1)
    fake_ui = types.ModuleType("syskit.ui")
    fake_ui.app = fake_app
    monkeypatch.setitem(sys.modules, "syskit.ui.app", fake_app)
    monkeypatch.setitem(sys.modules, "syskit.ui", fake_ui)
    monkeypatch.setattr(sys, "exit", lambda code=0: None)
    main_mod.run()
    assert called == [1]
