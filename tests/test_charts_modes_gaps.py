"""覆盖率补强：syskit/ui/charts.py、syskit/modes.py、syskit/proxy.py、syskit/settings.py。

与既有测试互补，只补缺口：
- charts：LineChart/BarChart 的 paintEvent 离屏真实渲染（多序列/断线/空数据/图例/空标题），
  及 drawLine 相邻点分支（此前仅 drawPoint 被触发）。
- modes：set_notifications 真本体（假 winreg）、startfile/makedirs 异常分支、
  list_background_candidates、close_background 的 pid<=0/杀失败/失败计数、
  组合入口的失败步骤日志分支、pause_windows_update 意外异常分支。
- proxy：winregread.values 抛异常兜底、DeleteValue 失败回退写空串、备份落盘失败返回 ""。
- settings：load 的坏 JSON/非 dict/合法 dict 三分支、save 写盘失败仍保留缓存并抛错。

安全：注册表写/进程操作/urlopen 全部显式 mock（叠加 conftest 哨兵），零系统副作用。
"""
import json
import os
import sys
import types

import pytest

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

qApp = None


def _install_qt_msg_sink():
    """与 test_ui_system_units 相同：接管 Qt 消息，避免 pytest 捕获环境下 fail-fast。"""
    try:
        from PySide6.QtCore import qInstallMessageHandler
    except ImportError:
        return
    import tempfile

    def _sink(msg_type, context, message):
        try:
            with open(os.path.join(tempfile.gettempdir(), "syskit_gaps_sink.log"),
                      "a", encoding="utf-8") as fh:
                fh.write("[qt] {}\n".format(message))
        except Exception:
            pass

    qInstallMessageHandler(_sink)


_install_qt_msg_sink()


# ================= charts.py =================

class TestLineChartRender:
    def test_render_series_break_and_trend_line(self, qtbot):
        """多序列 + 相邻两点 drawLine + None 断线 + 图例 + 固定量程。"""
        from syskit.ui.charts import LineChart

        c = LineChart("CPU / 内存", series=[("CPU", "#3576e0"), ("内存", "#8e5ce6")],
                      unit="%", seconds=10, y_fixed=(0, 100), fmt=lambda v: f"{v:.0f}%")
        qtbot.addWidget(c)
        c.resize(420, 220)
        assert list(c._series[0][2]) == [None] * 10        # 初始全部断线占位

        c.push([10, 20])
        c.push([30, None])          # CPU 相邻两点 → drawLine；内存最新点为 None → 断线
        pix = c.grab()
        assert not pix.isNull()
        assert list(c._series[0][2])[-2:] == [10, 30]

        c.clear_data()
        assert all(v is None for s in c._series for v in s[2])
        assert not c.grab().isNull()                        # 清空后渲染（空数据量程 0..1）

    def test_render_auto_range_zero_and_no_title(self, qtbot):
        """无 y_fixed 自动量程（空数据兜底 0..1 / 全 0 数据 → hi=1 / 大值默认 fmt）。"""
        from syskit.ui.charts import LineChart

        c = LineChart()                                     # 无标题、无序列、无 fmt
        qtbot.addWidget(c)
        c.resize(360, 200)
        c.add_series("读", "#2e9e44")
        assert not c.grab().isNull()                        # 有序列但无数据 → 量程 0..1
        c.push([0])                                         # 全 0 → max*1.2 or 1 → 1
        c.push([512 * 1024])                                # 大值走 fmt 默认 f"{v:.0f}"
        assert not c.grab().isNull()
        # 单点序列：drawPoint 分支（prev=None → 画点不画线）
        c2 = LineChart("", seconds=5)
        qtbot.addWidget(c2)
        c2.add_series("x", "#3576e0")
        c2.push([42])
        assert not c2.grab().isNull()


class TestBarChartRender:
    def test_render_empty_then_bars_with_y_label(self, qtbot):
        """空数据提示分支 + 柱体/数值/横轴标签/y 轴标题渲染。"""
        from syskit.ui.charts import BarChart

        b = BarChart("信道占用", y_label="AP 数", fmt=lambda v: str(int(v)))
        qtbot.addWidget(b)
        b.resize(420, 240)
        assert b._bars == []
        assert not b.grab().isNull()                        # "暂无数据，请先扫描" 分支

        b.set_bars([("1", 0, "#3576e0"), ("6", 3, "#2e9e44"), ("11", 7, "#ef6c00")])
        pix = b.grab()
        assert not pix.isNull()
        assert len(b._bars) == 3
        b.set_bars([])                                      # 回到空分支（set_bars 后重绘）
        assert not b.grab().isNull()


# ================= modes.py =================

class _FakeKey:
    def __init__(self, records=None):
        self.records = records if records is not None else []

    def Close(self):
        pass


class TestNotifications:
    def test_openkey_success_and_createkey_fallback(self, monkeypatch):
        import winreg

        from syskit import modes
        set_calls = []
        monkeypatch.setattr(winreg, "OpenKey", lambda hive, path, *a: _FakeKey())
        monkeypatch.setattr(winreg, "SetValueEx",
                            lambda k, name, res, typ, val: set_calls.append((name, typ, val)))

        r = modes.set_notifications(False)
        assert r["ok"] and "勿扰" in r["detail"]
        assert set_calls == [(modes.NOTIFY_GLOBAL_VALUE, winreg.REG_DWORD, 0)]

        # OpenKey 失败 → CreateKeyEx 兜底建键
        created = []

        def _open_fail(hive, path, *a):
            raise OSError("missing key")

        def _create(root, path, *a):
            created.append(path)
            return _FakeKey()

        monkeypatch.setattr(winreg, "OpenKey", _open_fail)
        monkeypatch.setattr(winreg, "CreateKeyEx", _create)
        r2 = modes.set_notifications(True)
        assert r2["ok"] and "已开启" in r2["detail"]
        assert created == [modes.NOTIFY_SETTINGS]
        assert set_calls[-1] == (modes.NOTIFY_GLOBAL_VALUE, winreg.REG_DWORD, 1)

    def test_registry_error_reports_failed_step(self, monkeypatch):
        import winreg

        from syskit import modes

        def _boom(*a, **k):
            raise RuntimeError("reg denied")

        monkeypatch.setattr(winreg, "OpenKey", _boom)
        monkeypatch.setattr(winreg, "CreateKeyEx", _boom)
        r = modes.set_notifications(True)
        assert r["ok"] is False and "reg denied" in r["detail"]


class TestModeErrorBranches:
    def test_open_common_apps_startfile_oserror(self, monkeypatch):
        from syskit import modes

        def _boom(target):
            raise OSError("no association")

        monkeypatch.setattr(modes.os, "startfile", _boom, raising=False)
        steps = modes.open_common_apps(["记事本"])
        assert steps[0]["ok"] is False and "no association" in steps[0]["detail"]

    def test_backup_config_makedirs_oserror(self, monkeypatch):
        from syskit import modes

        def _boom(path, *a, **k):
            raise OSError("cannot create dir")

        monkeypatch.setattr(modes.os, "makedirs", _boom)
        r = modes.backup_config()
        assert r["ok"] is False and "cannot create dir" in r["detail"]

    def test_pause_windows_update_unexpected_exception(self, monkeypatch):
        import winreg

        from syskit import modes
        monkeypatch.setattr(modes.admin, "is_admin", lambda: True)
        monkeypatch.setattr(winreg, "OpenKey",
                            lambda *a, **k: (_ for _ in ()).throw(OSError("no key")))
        monkeypatch.setattr(winreg, "CreateKeyEx",
                            lambda *a, **k: (_ for _ in ()).throw(ValueError("bad hive")))
        r = modes.pause_windows_update(7)
        assert r["ok"] is False and "bad hive" in r["detail"]

    def test_set_mute_stderr_truncated_detail(self, monkeypatch):
        from syskit import modes
        monkeypatch.setattr(modes.proc, "run_ps",
                            lambda *a, **k: modes.proc.CmdResult(1, "", "e" * 300))
        r = modes.set_mute(True)
        assert r["ok"] is False and len(r["detail"]) == 120 and set(r["detail"]) == {"e"}

    def test_set_power_plan_failure_detail_from_stderr(self, monkeypatch):
        from syskit import modes
        monkeypatch.setattr(modes.proc, "run",
                            lambda cmd, timeout=10: modes.proc.CmdResult(1, "", "access denied"))
        r = modes.set_power_plan(False)
        assert r["ok"] is False and r["detail"] == "access denied"


class TestBackgroundCleanup:
    def test_list_background_candidates_filters_whitelist(self, monkeypatch):
        import psutil

        from syskit import modes

        class _P:
            def __init__(self, pid, name):
                self.pid = pid
                self.info = {"pid": pid, "name": name}

        procs = [_P(1, "OneDrive.exe"), _P(2, "explorer.exe"),
                 _P(3, None), _P(4, "Spotify.EXE")]
        monkeypatch.setattr(psutil, "process_iter", lambda attrs=None: iter(procs))
        out = modes.list_background_candidates()
        assert out == [{"pid": 1, "name": "OneDrive.exe"},
                       {"pid": 4, "name": "Spotify.EXE"}]

        # 枚举异常 → 吞掉返回空
        def _boom(attrs=None):
            raise RuntimeError("wmi broken")

        monkeypatch.setattr(psutil, "process_iter", _boom)
        assert modes.list_background_candidates() == []

    def test_close_background_pid_guard_kill_failures_and_detail(self, monkeypatch):
        import psutil

        from syskit import modes
        killed = []

        class _FakeProc:
            def __init__(self, pid):
                self.pid = pid

            def kill(self):
                if self.pid == 2:
                    raise psutil.NoSuchProcess(2)
                if self.pid == 3:
                    raise RuntimeError("access denied")
                killed.append(self.pid)

        monkeypatch.setattr(psutil, "Process", _FakeProc)
        r = modes.close_background([
            {"pid": 1, "name": "A.exe"},
            {"pid": 2, "name": "B.exe"},     # NoSuchProcess → failed
            {"pid": 3, "name": "C.exe"},     # 其它异常 → failed
            {"pid": 0, "name": "D.exe"},     # pid<=0 → 跳过
        ])
        assert killed == [1]
        assert r["ok"] is True
        assert "已结束 1 个" in r["detail"] and "A.exe" in r["detail"]
        assert "失败 2" in r["detail"]

    def test_close_background_all_failed_reports_not_ok(self, monkeypatch):
        import psutil

        from syskit import modes

        class _Denied:
            def __init__(self, pid):
                pass

            def kill(self):
                raise psutil.AccessDenied(pid=1)

        monkeypatch.setattr(psutil, "Process", _Denied)
        r = modes.close_background([{"pid": 9, "name": "X.exe"}])
        assert r["ok"] is False and "失败 1" in r["detail"]


class TestCompositeModes:
    def test_run_office_mode_logs_failed_step(self, monkeypatch):
        from syskit import modes
        monkeypatch.setattr(modes, "set_notifications",
                            lambda en: modes._step("通知/勿扰", True, "已关闭（勿扰）"))
        monkeypatch.setattr(modes, "set_mute",
                            lambda m: modes._step("系统静音", False, "ps fail"))
        monkeypatch.setattr(modes, "backup_config",
                            lambda: modes._step("备份配置", True, "1 个文件"))
        steps = modes.run_office_mode(open_apps=False)
        assert [s["ok"] for s in steps] == [True, False, True]

    def test_run_game_mode_logs_failed_step(self, monkeypatch):
        from syskit import modes
        monkeypatch.setattr(modes, "set_notifications",
                            lambda en: modes._step("通知/勿扰", True, "已关闭（勿扰）"))
        monkeypatch.setattr(modes, "set_power_plan",
                            lambda h: modes._step("电源计划", False, "powercfg fail"))
        monkeypatch.setattr(modes, "pause_windows_update",
                            lambda d: modes._step("暂停 Windows 更新", True, "已暂停"))
        monkeypatch.setattr(modes, "close_background",
                            lambda processes=None: modes._step("清理后台", True, "无需清理"))
        steps = modes.run_game_mode(clear_bg=True)
        assert [s["ok"] for s in steps] == [True, False, True, True]


# ================= proxy.py =================

class _ProxyWininet:
    def __init__(self):
        self.opts = []

    def InternetSetOptionW(self, *a):
        self.opts.append(a[1])


class TestProxyGaps:
    def test_disable_proxy_read_values_raises_uses_empty_original(self, monkeypatch):
        """winregread.values 抛异常 → original={} → 不做备份，仍成功关闭。"""
        import winreg

        from syskit import proxy
        written = []
        wi = _ProxyWininet()

        monkeypatch.setattr(proxy.winregread, "values",
                            lambda path: (_ for _ in ()).throw(RuntimeError("reg boom")))
        monkeypatch.setattr(winreg, "OpenKey", lambda *a, **k: _FakeKey())
        monkeypatch.setattr(winreg, "SetValueEx",
                            lambda k, name, res, typ, val: written.append((name, val)))
        monkeypatch.setattr(proxy.ctypes, "windll",
                            types.SimpleNamespace(wininet=wi))
        assert proxy.disable_proxy() is True
        assert written == [("ProxyEnable", 0)]
        assert wi.opts == [39, 37]

    def test_disable_proxy_pac_delete_fails_falls_back_to_empty(self, monkeypatch, tmp_path):
        """DeleteValue 失败（值已被并发清除等）→ 回退写空串，仍视为成功。"""
        import winreg

        from syskit import proxy
        written, deleted = [], []

        monkeypatch.setattr(proxy.winregread, "values",
                            lambda path: {"ProxyEnable": 1,
                                          "AutoConfigURL": "http://127.0.0.1:7890/pac"})
        monkeypatch.setattr(winreg, "OpenKey", lambda *a, **k: _FakeKey())
        monkeypatch.setattr(winreg, "SetValueEx",
                            lambda k, name, res, typ, val: written.append((name, typ, val)))

        def _delete_fail(key, name):
            deleted.append(name)
            raise OSError("gone")

        monkeypatch.setattr(winreg, "DeleteValue", _delete_fail)
        monkeypatch.setattr(proxy.ctypes, "windll",
                            types.SimpleNamespace(wininet=_ProxyWininet()))
        assert proxy.disable_proxy() is True
        assert deleted == ["AutoConfigURL"]
        assert ("AutoConfigURL", winreg.REG_SZ, "") in written

    def test_backup_values_failure_returns_empty(self, monkeypatch, tmp_path):
        """备份目录创建失败 → 返回 ""，不向上抛。"""
        from syskit import config, proxy

        blocker = tmp_path / "blocker"
        blocker.write_text("x", encoding="utf-8")       # 文件占据父路径 → makedirs 必败
        monkeypatch.setattr(config, "BACKUP_DIR", str(blocker / "sub" / "stamp"))
        assert proxy._backup_values({"ProxyEnable": 1}) == ""


# ================= settings.py =================

class TestSettingsGaps:
    def test_load_bad_json_non_dict_and_valid_dict(self, tmp_path):
        """load 的三个解析分支：坏 JSON / 非 dict JSON / 合法 dict 合并。"""
        from syskit import config, settings

        os.makedirs(config.APPDATA_DIR, exist_ok=True)

        # 坏 JSON → json.load 抛异常被吞
        with open(config.SETTINGS_FILE, "w", encoding="utf-8") as f:
            f.write("{bad json")
        settings._cache = None
        assert settings.load() == dict(config.DEFAULTS)

        # 合法 JSON 但不是 dict → 忽略
        with open(config.SETTINGS_FILE, "w", encoding="utf-8") as f:
            json.dump([1, 2], f)
        settings._cache = None
        assert settings.load() == dict(config.DEFAULTS)

        # 合法 dict → 合并进默认值
        with open(config.SETTINGS_FILE, "w", encoding="utf-8") as f:
            json.dump({"poll_interval_sec": 99}, f)
        settings._cache = None
        data = settings.load()
        assert data["poll_interval_sec"] == 99
        assert data["dark_mode"] == config.DEFAULTS["dark_mode"]

    def test_save_oserror_keeps_cache_removes_tmp_and_raises(self, monkeypatch):
        """写盘失败：tmp 被清理、异常上抛，但内存缓存仍保留本次改动。"""
        from syskit import config, settings

        settings._cache = None
        settings.save({"first": 1})                     # 先落一次盘建好目录
        assert settings.load()["first"] == 1

        def _boom(src, dst):
            raise OSError("disk full")

        monkeypatch.setattr(os, "replace", _boom)
        with pytest.raises(OSError):
            settings.save({"second": 2})
        assert settings._cache["second"] == 2           # 缓存未丢
        assert settings.load()["second"] == 2           # load 命中缓存（不读半截文件）
        assert not os.path.exists(config.SETTINGS_FILE + ".tmp")

    def test_save_unlink_tmp_failure_still_raises(self, monkeypatch):
        """tmp 清理本身也失败 → 异常被吞、原始 OSError 上抛（不二次炸穿）。"""
        from syskit import settings

        settings._cache = None

        def _boom_replace(src, dst):
            raise OSError("disk full")

        def _boom_unlink(path):
            raise OSError("tmp locked")

        monkeypatch.setattr(os, "replace", _boom_replace)
        monkeypatch.setattr(os, "unlink", _boom_unlink)
        with pytest.raises(OSError):
            settings.save({"third": 3})
        assert settings._cache["third"] == 3
