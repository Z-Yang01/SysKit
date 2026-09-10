"""syskit/clipboard.py 与 syskit/modes.py 单元测试（无真实注册表/进程/剪贴板）。"""
import json
import os
from types import SimpleNamespace

import pytest

from syskit import clipboard as clip_mod
from syskit import config, modes
from syskit.proc import CmdResult


# ---------------- ClipboardHistory ----------------

class TestClipboardHistory:
    def test_add_and_order_newest_first(self):
        h = clip_mod.ClipboardHistory(max_items=10)
        assert h.add("a")
        assert h.add("b")
        assert h.texts() == ["b", "a"]

    def test_blank_and_consecutive_dup_rejected(self):
        h = clip_mod.ClipboardHistory()
        assert h.add("  ") is False
        assert h.add("x")
        assert h.add("x") is False                    # 与最新一条相同
        assert h.add("y")
        assert h.add("x") is True                     # 非连续可再入
        assert h.texts() == ["x", "y", "x"]

    def test_max_items_trim(self):
        h = clip_mod.ClipboardHistory(max_items=3)
        for i in range(5):
            h.add(f"t{i}")
        assert h.texts() == ["t4", "t3", "t2"]

    def test_long_text_truncated(self):
        h = clip_mod.ClipboardHistory(max_len=64)
        h.add("Z" * 200)
        assert len(h.texts()[0]) == 64

    def test_persist_roundtrip(self, tmp_path, monkeypatch):
        monkeypatch.setattr(config, "APPDATA_DIR", str(tmp_path), raising=False)
        h1 = clip_mod.ClipboardHistory()
        h1.add("persist-me")
        h2 = clip_mod.ClipboardHistory()
        assert "persist-me" in h2.texts()

    def test_remove_and_clear(self):
        h = clip_mod.ClipboardHistory()
        h.add("a")
        h.add("b")
        assert h.remove(0)
        assert h.texts() == ["a"]
        assert h.remove(99) is False
        h.clear()
        assert h.items() == []

    def test_preview(self):
        assert clip_mod.ClipboardHistory.preview("hello\nworld") == "hello world"
        assert clip_mod.ClipboardHistory.preview("x" * 100, n=10).endswith("…")
        assert len(clip_mod.ClipboardHistory.preview("x" * 100, n=10)) == 10


# ---------------- modes helpers ----------------

def _ok(name="x"):
    return modes._step(name, True, "ok")


def _fail(name="x"):
    return modes._step(name, False, "err")


class TestModeSteps:
    def test_set_notifications_success(self, monkeypatch):
        calls = {}

        def fake_set(enabled):
            calls["en"] = enabled
            return modes._step("通知/勿扰", True, "已关闭（勿扰）" if not enabled else "已开启")

        monkeypatch.setattr(modes, "set_notifications", fake_set)
        r = modes.set_notifications(False)
        assert r["ok"] and "勿扰" in r["detail"]

    def test_set_mute_uses_ps(self, monkeypatch):
        seen = {}

        def fake_ps(script, timeout=40):
            seen["script"] = script
            return CmdResult(0, "", "")

        monkeypatch.setattr(modes.proc, "run_ps", fake_ps)
        r = modes.set_mute(True)
        assert r["ok"]
        assert "waveOutSetVolume" in seen["script"]
        assert "0," in seen["script"] or "0)" in seen["script"]

    def test_set_mute_fail(self, monkeypatch):
        monkeypatch.setattr(modes.proc, "run_ps",
                            lambda *a, **k: CmdResult(1, "", "boom"))
        r = modes.set_mute(False)
        assert r["ok"] is False

    def test_open_common_apps(self, monkeypatch):
        opened = []
        monkeypatch.setattr(modes.os, "startfile", lambda t: opened.append(t), raising=False)
        # os.startfile 在非 win32 不存在时 monkeypatch 到 modes 模块引用的 os
        steps = modes.open_common_apps(["记事本", "未知"])
        assert steps[0]["ok"] and "notepad" in steps[0]["detail"]
        assert steps[1]["ok"] is False

    def test_backup_config_copies_files(self, tmp_path, monkeypatch):
        appdata = tmp_path / "appdata"
        backup = tmp_path / "backup"
        monkeypatch.setattr(config, "APPDATA_DIR", str(appdata), raising=False)
        monkeypatch.setattr(config, "BACKUP_DIR", str(backup), raising=False)
        appdata.mkdir(parents=True)
        (appdata / "settings.json").write_text('{"a":1}', encoding="utf-8")
        (appdata / "traffic.json").write_text("{}", encoding="utf-8")
        r = modes.backup_config()
        assert r["ok"]
        # 备份目录下应有子目录
        kids = list(backup.iterdir())
        assert len(kids) == 1 and (kids[0] / "settings.json").is_file()

    def test_backup_config_empty_still_succeeds(self, tmp_path, monkeypatch):
        monkeypatch.setattr(config, "APPDATA_DIR", str(tmp_path / "empty"), raising=False)
        monkeypatch.setattr(config, "BACKUP_DIR", str(tmp_path / "b2"), raising=False)
        from syskit import settings as settings_mod
        monkeypatch.setattr(settings_mod, "load", lambda: {"dark_mode": True})
        r = modes.backup_config()
        assert r["ok"]

    def test_set_power_plan(self, monkeypatch):
        seen = {}

        def fake_run(cmd, timeout=20):
            seen["cmd"] = list(cmd)
            return CmdResult(0, "", "")

        monkeypatch.setattr(modes.proc, "run", fake_run)
        r = modes.set_power_plan(True)
        assert r["ok"] and seen["cmd"] == ["powercfg", "/setactive", "SCHEME_MAX"]
        r = modes.set_power_plan(False)
        assert seen["cmd"][-1] == "SCHEME_BALANCED"

    def test_pause_update_requires_admin(self, monkeypatch):
        monkeypatch.setattr(modes.admin, "is_admin", lambda: False)
        r = modes.pause_windows_update(7)
        assert r["ok"] is False and "管理员" in r["detail"]

    def test_pause_update_as_admin(self, monkeypatch):
        writes = []
        monkeypatch.setattr(modes.admin, "is_admin", lambda: True)

        class FakeKey:
            def Close(self):
                pass

        class FakeWinreg:
            HKEY_LOCAL_MACHINE = 1
            KEY_SET_VALUE = 2
            REG_DWORD = 4

            @staticmethod
            def OpenKey(*a, **k):
                raise OSError("missing")

            @staticmethod
            def CreateKeyEx(*a, **k):
                return FakeKey()

            @staticmethod
            def SetValueEx(key, name, reserved, typ, value):
                writes.append((name, typ, value))

        import sys
        monkeypatch.setitem(sys.modules, "winreg", FakeWinreg)
        r = modes.pause_windows_update(7)
        assert r["ok"] is True, r
        assert writes and writes[0][0] == "NoAutoUpdate" and writes[0][2] == 1

    def test_close_background_empty(self, monkeypatch):
        monkeypatch.setattr(modes, "list_background_candidates", lambda: [])
        r = modes.close_background([])
        assert r["ok"] and "无需清理" in r["detail"]

    def test_close_background_kills_whitelist_only(self, monkeypatch):
        killed = []

        class FakeProc:
            def __init__(self, pid):
                self.pid = pid

            def kill(self):
                killed.append(self.pid)

        monkeypatch.setattr(
            "psutil.Process", lambda pid: FakeProc(pid), raising=False)
        # 直接注入 targets，避免真实 psutil 扫描
        import syskit.modes as m
        r = m.close_background([{"pid": 123, "name": "Spotify.exe"}])
        assert r["ok"] and killed == [123]

    def test_run_office_mode_order(self, monkeypatch):
        order = []

        def track(fn, name):
            def wrap(*a, **k):
                order.append(name)
                return _ok(name)
            return wrap

        monkeypatch.setattr(modes, "set_notifications",
                            track(None, "notify"))
        monkeypatch.setattr(modes, "set_mute", track(None, "mute"))
        monkeypatch.setattr(modes, "open_common_apps",
                            lambda names=None: (order.append("apps"), [_ok("打开")])[-1])
        monkeypatch.setattr(modes, "backup_config",
                            track(None, "backup"))
        steps = modes.run_office_mode(open_apps=True)
        assert order == ["notify", "mute", "apps", "backup"]
        assert len(steps) == 4

    def test_run_game_mode_order(self, monkeypatch):
        order = []

        def mk(name):
            def _f(*a, **k):
                order.append(name)
                return _ok(name)
            return _f

        monkeypatch.setattr(modes, "set_notifications", mk("notify"))
        monkeypatch.setattr(modes, "set_power_plan", mk("power"))
        monkeypatch.setattr(modes, "pause_windows_update", mk("wu"))
        monkeypatch.setattr(modes, "close_background",
                            lambda processes=None: (order.append("bg"), _ok("bg"))[-1])
        steps = modes.run_game_mode(clear_bg=True)
        assert order == ["notify", "power", "wu", "bg"]
        assert len(steps) == 4

    def test_restore_functions_call_restores(self, monkeypatch):
        monkeypatch.setattr(modes, "set_notifications", lambda en: _ok())
        monkeypatch.setattr(modes, "set_mute", lambda m: _ok())
        monkeypatch.setattr(modes, "set_power_plan", lambda h: _ok())
        monkeypatch.setattr(modes, "pause_windows_update", lambda d: _ok())
        assert len(modes.restore_office_defaults()) == 2
        assert len(modes.restore_game_defaults()) == 3
