"""syskit/ui/system_page.py 与 syskit/ui/app.py 单元测试（离屏、全 mock、零系统副作用）。

与 tests/test_ui_smoke.py / test_clipboard_modes.py 互补，不重复冒烟内容，重点补：
- system_page：StartupTab 枚举/禁用/启用/异常降级/无管理员拦截、Battery 解析异常
  兜底、Cleanup 扫描与清理 worker 同步链路 + 防重入 + 空态 + 自动复扫、
  ClipboardTab 监听/复制/删除/清空与各守卫、ModesTab 四个入口的确认流、
  无管理员降级、结果表着色与防重入。
- app：_enable_crash_log 的异常钩子本体与降级分支；run_gui 单实例互斥分支与
  完整装配链（ctypes.windll / QApplication / MainWindow 全部注入假件 —— 绝不
  创建真实 Win32 互斥量、不进入真实事件循环、不弹真实模态框）。

Mock 策略（叠加在仓库 conftest 哨兵之上）：
- autouse 夹具 msgbox_recorder：QMessageBox 四个静态弹窗替换为记录调用并返回
  默认按钮（offscreen 下模态 exec 会永久挂起），返回 recorder 列表供断言。
- 后台 worker（_ScanWorker/_CleanWorker）不调用 start()，同步直接 run() 验证
  信号链路；唯一例外是 CleanupTab.clean() 走真实 QThread（内部只清理 pytest
  临时目录），用 qtbot.wait_until 等待线程真正收尾。
- 注册表写操作 / proc.run_ps / os.remove 等全部在用例内显式 monkeypatch。
"""
import os
import sys
import threading
import time
import winreg
from types import SimpleNamespace

import pytest

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from PySide6.QtCore import QObject, Signal, qInstallMessageHandler
from PySide6.QtGui import QColor
from PySide6.QtWidgets import QApplication, QMessageBox, QTableWidgetItem
from PySide6 import QtWidgets

def _qt_message_sink(msg_type, context, message):
    """接管 Qt 消息：默认处理器在 pytest 捕获环境下会 fail-fast（README 已知坑）。"""
    try:
        with open(os.path.join(os.environ.get("TEMP", "."),
                               "syskit_qt_units_sink.log"),
                  "a", encoding="utf-8") as fh:
            fh.write("[qt] {}\n".format(message))
    except Exception:
        pass


qInstallMessageHandler(_qt_message_sink)


# ---------------- autouse：弹窗记录器 ----------------


@pytest.fixture(autouse=True)
def msgbox_recorder(monkeypatch):
    """记录全部静态弹窗调用并返回默认按钮（question 默认 Yes）。"""
    sb = QMessageBox.StandardButton
    calls = []

    def _mk(kind, ret):
        def _f(*a, **k):
            calls.append((kind, a))
            return ret
        return _f

    monkeypatch.setattr(QMessageBox, "information",
                        staticmethod(_mk("information", sb.Ok)))
    monkeypatch.setattr(QMessageBox, "warning",
                        staticmethod(_mk("warning", sb.Ok)))
    monkeypatch.setattr(QMessageBox, "critical",
                        staticmethod(_mk("critical", sb.Ok)))
    monkeypatch.setattr(QMessageBox, "about", staticmethod(_mk("about", None)))
    monkeypatch.setattr(QMessageBox, "question",
                        staticmethod(_mk("question", sb.Yes)))
    return calls


def recorder_kinds(recorder):
    return [kind for kind, _args in recorder]


def _no_win_temp(monkeypatch):
    """isdir('C:\\Windows\\Temp') → False，避免 glob 触碰真实系统临时目录。"""
    real_isdir = os.path.isdir

    def fake_isdir(p):
        return False if os.path.normcase(p) == r"c:\windows\temp" else real_isdir(p)

    monkeypatch.setattr(os.path, "isdir", fake_isdir)


# ---------------- StartupTab ----------------


class _RegKey:
    """winreg.OpenKey 假键：可枚举固定值、支持上下文协议与 Close。"""

    def __init__(self, vals):
        self.vals = list(vals.items())

    def Close(self):
        pass

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        return False


def test_startup_enum_lists_all_sources_and_greys_disabled(qtbot, monkeypatch):
    """_enum 聚合 HKCU/HKLM Run + 启动文件夹 + 已禁用键；禁用行灰显。"""
    from syskit.ui import system_page as sp

    hives = {"HKCU": "HIVE_CU", "HKLM": "HIVE_LM", "HKCR": "HIVE_CR", "HKU": "HIVE_U"}
    monkeypatch.setattr(sp.winregread, "_HIVES", hives)
    reg_values = {"HIVE_CU": {"AppA": "C:\\a.exe", "AppB": "C:\\b.exe"},
                  "HIVE_LM": {"SvcX": "C:\\x.exe"}}

    def fake_openkey(hive, path, *a):
        assert path == sp.RUN_KEY
        if hive == "HIVE_LM":
            raise OSError("hklm blocked")          # 命中 _enum 的 except 分支
        return _RegKey(reg_values[hive])

    def fake_enumvalue(k, i):
        if i >= len(k.vals):
            raise OSError(22, "no more")
        name, value = k.vals[i]
        return name, value, 1

    monkeypatch.setattr(winreg, "OpenKey", fake_openkey)
    monkeypatch.setattr(winreg, "EnumValue", fake_enumvalue)
    # 已禁用键读取（DISABLED_KEY 每条格式 "loc|cmd"；坏条目被跳过）
    monkeypatch.setattr(sp.winregread, "values",
                        lambda path: {"AppB": "HKCU-Run|C:\\b.exe",
                                      "Bad": "no-delimiter"}
                        if sp.DISABLED_KEY in path else {})
    monkeypatch.setattr(sp.glob, "glob", lambda pattern: ["C:\\Startup\\mytool.lnk"])
    monkeypatch.setattr(sp, "USER_STARTUP", "C:\\Startup")

    items = sp.StartupTab._enum()
    tags = {(loc, name) for loc, name, _cmd, _en in items}
    assert ("HKCU-Run", "AppA") in tags
    assert ("HKLM-Run", "SvcX") not in tags      # OpenKey 失败被跳过
    assert ("启动文件夹", "mytool.lnk") in tags
    disabled = [it for it in items if not it[3]]
    assert disabled == [("HKCU-Run", "AppB", "C:\\b.exe", False)]

    tab = sp.StartupTab()                        # 构造即用同一套假数据 reload
    qtbot.addWidget(tab)
    assert tab.tbl.rowCount() == 4
    row = next(r for r in range(tab.tbl.rowCount())
               if tab.tbl.item(r, 3).text() == "已禁用")
    assert tab.tbl.item(row, 0).text() == "AppB"
    fg = tab.tbl.item(row, 0).foreground().color().name().lower()
    assert fg == "#9aa4b2"                       # 禁用项灰显


def test_startup_reload_swallows_enum_error(qtbot, monkeypatch):
    """_enum 抛异常时 reload 记日志并保持空表（不崩）。"""
    from syskit.ui import system_page as sp
    tab = sp.StartupTab()                        # 构造走真实只读枚举（放行）
    qtbot.addWidget(tab)

    def boom():
        raise RuntimeError("enum failed")

    monkeypatch.setattr(tab, "_enum", boom)
    tab.reload()
    assert tab.tbl.rowCount() == 0


def test_startup_disable_requires_admin(qtbot, monkeypatch, msgbox_recorder):
    """无管理员点「禁用选中项」：仅弹警告，不执行任何操作。"""
    from syskit.ui import system_page as sp
    monkeypatch.setattr(sp.admin, "is_admin", lambda: False)
    tab = sp.StartupTab()
    qtbot.addWidget(tab)
    tab.tbl.setRowCount(1)
    for col, text in enumerate(("AppA", "HKCU-Run", "C:\\a.exe", "已启用")):
        tab.tbl.setItem(0, col, QTableWidgetItem(text))
    tab.tbl.selectRow(0)

    tab._toggle_selected(True)
    assert recorder_kinds(msgbox_recorder) == ["warning"]

    # 无选中项（管理员可用时）：提示先选中
    monkeypatch.setattr(sp.admin, "is_admin", lambda: True)
    tab.tbl.clearSelection()
    tab._toggle_selected(False)
    assert recorder_kinds(msgbox_recorder) == ["warning", "information"]


def test_startup_toggle_dispatch_and_error_swallow(qtbot, monkeypatch):
    """启用/禁用按行状态正确分发；单步异常被吞掉并继续。"""
    from syskit.ui import system_page as sp
    monkeypatch.setattr(sp.admin, "is_admin", lambda: True)
    monkeypatch.setattr(sp.StartupTab, "_enum", staticmethod(lambda: [
        ("HKCU-Run", "En", "C:\\e.exe", True),
        ("HKCU-Run", "Dis", "C:\\d.exe", False)]))
    tab = sp.StartupTab()
    qtbot.addWidget(tab)
    calls = []
    monkeypatch.setattr(tab, "_disable_one",
                        lambda loc, name: calls.append(("dis", loc, name)))
    monkeypatch.setattr(tab, "_enable_one",
                        lambda loc, name: calls.append(("en", loc, name)))

    tab.tbl.selectRow(0)                         # 已启用行 → 禁用
    tab._toggle_selected(True)
    assert calls == [("dis", "HKCU-Run", "En")]

    calls.clear()
    tab.tbl.selectRow(1)                         # 已禁用行 → 启用
    tab._toggle_selected(False)
    assert calls == [("en", "HKCU-Run", "Dis")]

    calls.clear()
    tab.tbl.selectRow(0)
    tab._toggle_selected(False)                  # 已启用行点启用 → 无操作
    tab.tbl.selectRow(1)
    tab._toggle_selected(True)                   # 已禁用行点禁用 → 无操作
    assert calls == []

    def boom(loc, name):
        raise OSError("registry denied")

    monkeypatch.setattr(tab, "_disable_one", boom)
    tab.tbl.selectRow(0)
    tab._toggle_selected(True)                   # 异常被 LOGGER 吞掉，不抛
    assert tab.tbl.rowCount() == 2               # 末尾已 reload


def test_startup_disable_enable_registry_fakes(qtbot, monkeypatch):
    """_disable_one/_enable_one 的注册表读写全部走假 winreg。"""
    from syskit.ui import system_page as sp
    monkeypatch.setattr(sp.winregread, "_HIVES",
                        {"HKCU": "HIVE_CU", "HKLM": "HIVE_LM",
                         "HKCR": "HIVE_CR", "HKU": "HIVE_U"})
    events = []

    class CtxKey:
        def __init__(self, name):
            self.name = name

        def __enter__(self):
            return self

        def __exit__(self, *exc):
            return False

    def fake_openkey(hive, path, *a):
        events.append(("open", str(hive), path))
        return CtxKey(path)

    def fake_query(key, name):
        return ("C:\\val.exe", winreg.REG_SZ)

    def fake_createkeyex(root, path, *a):
        events.append(("create", path))
        return CtxKey(path)

    def fake_set(key, name, reserved, typ, value):
        events.append(("set", key.name, name, value))

    def fake_delete(key, name):
        events.append(("delete", key.name, name))

    def fake_enum(k, i):
        raise OSError(22, "no more")

    for attr, fn in (("OpenKey", fake_openkey), ("QueryValueEx", fake_query),
                     ("CreateKeyEx", fake_createkeyex), ("SetValueEx", fake_set),
                     ("DeleteValue", fake_delete), ("EnumValue", fake_enum)):
        monkeypatch.setattr(winreg, attr, fn)

    tab = sp.StartupTab()
    qtbot.addWidget(tab)

    tab._disable_one("HKCU-Run", "AppZ")         # 备份到 DISABLED_KEY 再删 Run 值
    assert ("set", sp.DISABLED_KEY, "AppZ", "HKCU-Run|C:\\val.exe") in events
    assert ("delete", sp.RUN_KEY, "AppZ") in events

    monkeypatch.setattr(sp.winregread, "value",
                        lambda path, name, default=None: "HKLM-Run|C:\\y.exe")
    tab._enable_one("HKLM-Run", "AppZ")          # 从 DISABLED_KEY 还原
    assert ("set", sp.RUN_KEY, "AppZ", "C:\\y.exe") in events
    assert ("delete", sp.DISABLED_KEY, "AppZ") in events

    before = len(events)
    monkeypatch.setattr(sp.winregread, "value", lambda *a, **k: None)
    tab._enable_one("HKCU-Run", "Ghost")         # 无备份记录 → 直接返回
    assert len(events) == before


def test_startup_folder_disable_enable_real_tmp(tmp_path, qtbot, monkeypatch):
    """启动文件夹项的禁用/启用在 pytest 临时目录内真实改名。"""
    from syskit.ui import system_page as sp
    startup = tmp_path / "startup"
    startup.mkdir()
    target = startup / "tool.lnk"
    target.write_text("x", encoding="utf-8")
    monkeypatch.setattr(sp, "USER_STARTUP", str(startup))

    tab = sp.StartupTab()
    qtbot.addWidget(tab)
    tab._disable_one("启动文件夹", "tool.lnk")
    assert (startup / "tool.lnk.disabled").is_file() and not target.exists()
    tab._enable_one("启动文件夹", "tool.lnk")
    assert target.is_file() and not (startup / "tool.lnk.disabled").exists()
    tab._disable_one("启动文件夹", "ghost.lnk")   # 目标不存在 → no-op
    tab._enable_one("启动文件夹", "ghost.lnk")


# ---------------- BatteryTab ----------------


def test_battery_parse_error_falls_back_to_no_battery(qtbot, monkeypatch):
    """isdigit 为真但 int 抛 ValueError（如 "³"）→ 命中解析兜底分支。"""
    from syskit import proc
    from syskit.ui.system_page import BatteryTab
    monkeypatch.setattr(proc, "run_ps",
                        lambda script, timeout=30, resident=False:
                        proc.CmdResult(0, "40000,32000,³", ""))
    monkeypatch.setattr("syskit.ui.system_page._BatteryWorker.start",
                        lambda self: self.run())            # worker 同步执行（项目惯例）
    tab = BatteryTab()                           # 懒加载：构造期不查询
    qtbot.addWidget(tab)
    tab.refresh()                                # 显式触发（首显由 showEvent 自动）
    assert "未检测到电池" in tab.lbl_design.text()
    assert tab.lbl_full.text() == "—"
    assert "电池" in tab.lbl_note.text()


# ---------------- Cleanup worker（同步 run） ----------------


def test_scan_worker_sizes_all_targets(qtbot, monkeypatch, tmp_path):
    from syskit import proc
    from syskit.ui import system_page as sp
    _no_win_temp(monkeypatch)
    results = iter([proc.CmdResult(0, "4096", ""),
                    proc.CmdResult(0, "", "")])
    monkeypatch.setattr(proc, "run_ps", lambda s, timeout=30: next(results))

    d = tmp_path / "temp"
    d.mkdir()
    (d / "a.tmp").write_bytes(b"x" * 100)
    (d / "b.tmp").write_bytes(b"y" * 30)
    monkeypatch.setenv("TEMP", str(d))
    monkeypatch.setenv("LOCALAPPDATA", str(tmp_path / "no-such-la"))

    w = sp._ScanWorker()
    got = []
    w.done.connect(got.append)
    w.run()                                      # 同步执行，不 start()
    sizes = got[0]
    assert sizes["user_temp"] == 130
    assert sizes["win_temp"] == 0                # isdir 假件 → 直接 0
    assert sizes["thumbcache"] == 0              # LOCALAPPDATA 指向不存在目录
    assert sizes["recycle"] == 4096              # 假 run_ps 返回值


def test_scan_worker_recycle_valueerror_branch(monkeypatch):
    """回收站脚本返回非数字 → float() ValueError → 按 0 处理。

    glob 一并打桩：本用例不碰 TEMP，默认会让 _ScanWorker 递归扫真实用户临时目录
    （实测 33 秒，全量套件的离群慢例就是它）。
    """
    from syskit import proc
    from syskit.ui import system_page as sp
    monkeypatch.setattr(proc, "run_ps",
                        lambda s, timeout=30: proc.CmdResult(0, "not-a-number", ""))
    monkeypatch.setattr(sp.glob, "glob", lambda *a, **k: [])
    w = sp._ScanWorker()
    got = []
    w.done.connect(got.append)
    w.run()
    assert got[0]["recycle"] == 0


def test_scan_worker_getsize_oserror_skipped(monkeypatch, tmp_path):
    """遍历中单个文件 getsize 抛 OSError → 跳过并继续累计。"""
    from syskit import proc
    from syskit.ui import system_page as sp
    d = tmp_path / "t"
    d.mkdir()
    monkeypatch.setenv("TEMP", str(d))
    monkeypatch.setattr(sp.glob, "glob", lambda *a, **k: [str(d / "gone.bin")])
    monkeypatch.setattr(os.path, "isfile", lambda p: True)

    def boom(p):
        raise OSError("file vanished")

    monkeypatch.setattr(os.path, "getsize", boom)
    monkeypatch.setattr(proc, "run_ps", lambda s, timeout=30: proc.CmdResult(0, "0", ""))
    w = sp._ScanWorker()
    got = []
    w.done.connect(got.append)
    w.run()
    assert got[0]["user_temp"] == 0


def test_clean_worker_run_sync(monkeypatch, tmp_path):
    """_CleanWorker：回收站走假 run_ps、白名单过滤、被占用文件计入 errors。"""
    from syskit import proc
    from syskit.ui import system_page as sp
    _no_win_temp(monkeypatch)
    d = tmp_path / "temp"
    d.mkdir()
    (d / "keep.txt").write_bytes(b"z" * 10)
    (d / "locked.log").write_bytes(b"z" * 5)
    monkeypatch.setenv("TEMP", str(d))

    explorer = tmp_path / "la" / "Microsoft" / "Windows" / "Explorer"
    explorer.mkdir(parents=True)
    (explorer / "thumbcache_1.db").write_bytes(b"a" * 7)
    (explorer / "other.db").write_bytes(b"a" * 3)
    monkeypatch.setenv("LOCALAPPDATA", str(tmp_path / "la"))

    ps_scripts = []
    monkeypatch.setattr(proc, "run_ps",
                        lambda s, timeout=60: ps_scripts.append(s)
                        or proc.CmdResult(0, "", ""))

    real_remove = os.remove

    def fake_remove(path):
        if str(path).endswith("locked.log"):
            raise OSError("in use")
        return real_remove(path)

    monkeypatch.setattr(os, "remove", fake_remove)

    w = sp._CleanWorker(["recycle", "user_temp", "win_temp", "thumbcache"])
    progress, done = [], []
    w.progress.connect(progress.append)
    w.done.connect(lambda freed, errors: done.append((freed, errors)))
    w.run()
    freed, errors = done[0]
    assert freed == 17                           # keep.txt 10 + thumbcache_1.db 7
    assert errors == ["locked.log"]
    assert not (d / "keep.txt").exists()
    assert (explorer / "other.db").exists()      # 非 thumbcache 前缀被跳过
    assert any("回收站" in p for p in progress)
    assert any("Windows" in p for p in progress)  # win_temp root 不存在 → 跳过分支
    assert ps_scripts and "Clear-RecycleBin" in ps_scripts[0]


# ---------------- CleanupTab（控件流 + 一次真实线程收尾） ----------------


def test_cleanup_tab_guards_empty_state_and_clean_flow(qtbot, monkeypatch,
                                                       tmp_path, msgbox_recorder):
    """clean/scan 完整编排：worker.start 注入为同步执行（worker 内部逻辑已由
    上面的同步 run() 用例覆盖，这里不真开线程，规避离屏环境原生崩溃）。"""
    from syskit import proc
    from syskit.ui import system_page as sp
    monkeypatch.setattr(proc, "run_ps",
                        lambda s, timeout=30: proc.CmdResult(0, "0", ""))

    def sync_start(self):                        # start() → 主线程同步 run()
        self.run()

    monkeypatch.setattr(sp._CleanWorker, "start", sync_start)
    monkeypatch.setattr(sp._ScanWorker, "start", sync_start)

    tab = sp.CleanupTab()
    qtbot.addWidget(tab)

    # 防重入：已有 worker 时 scan/clean 直接返回
    tab._scan_worker = object()
    tab.scan()
    assert tab.btn_scan.isEnabled()              # 守卫先于按钮禁用
    tab._scan_worker = None
    tab._clean_worker = object()
    tab.clean()
    assert tab._clean_worker is not None
    tab._clean_worker = None

    # 空态：未勾选任何项 → 仅提示
    tab.clean()
    assert tab._clean_worker is None
    assert recorder_kinds(msgbox_recorder) == ["information"]
    assert tab.btn_clean.isEnabled()

    # 勾选 user_temp（TEMP 指向 pytest 临时目录）→ 完整编排只清临时目录
    d = tmp_path / "temp"
    d.mkdir()
    (d / "junk.bin").write_bytes(b"q" * 64)
    monkeypatch.setenv("TEMP", str(d))
    tab.checks["user_temp"].setChecked(True)

    real_clean_run = sp._CleanWorker.run

    def spying_clean_run(self):
        assert not tab.btn_clean.isEnabled() and not tab.pb.isHidden()
        real_clean_run(self)

    monkeypatch.setattr(sp._CleanWorker, "run", spying_clean_run)
    tab.clean()
    assert tab._clean_worker is None             # 同步跑完即复位
    assert not (d / "junk.bin").exists()
    assert tab.pb.isHidden() and tab.btn_clean.isEnabled()
    assert "0B" in tab.checks["user_temp"].text()   # 末尾自动复扫已回填

    # _clean_done 直调：结果文案 + 跳过计数。置 scan 守卫防自动复扫覆盖文案。
    tab._scan_worker = object()
    tab._clean_done(12345678, ["a", "b"])
    assert "清理完成" in tab.lbl_result.text()
    assert "11.8MB" in tab.lbl_result.text()
    assert "跳过 2" in tab.lbl_result.text()
    tab._scan_worker = None
    tab.scan()                                   # 手动复扫
    tab._scan_done({"user_temp": 12345678})      # 复选框文案回填
    assert "11.8MB" in tab.checks["user_temp"].text()


# ---------------- ClipboardTab ----------------


class _FakeClip(QObject):
    dataChanged = Signal()

    def __init__(self, parent=None):
        super().__init__(parent)
        self._text = ""
        self.set_calls = []

    def text(self):
        return self._text

    def setText(self, t):
        self.set_calls.append(t)
        self._text = t


def _make_clip_tab(qtbot):
    from syskit.ui.system_page import ClipboardTab
    tab = ClipboardTab()
    qtbot.addWidget(tab)
    tab.hist.clear()
    return tab


def test_battery_tab_lazy_loads_on_show(qtbot, monkeypatch):
    """回归：电池 Tab 懒加载——构造期不跑 PowerShell，首次显示才查询。"""
    from syskit import proc
    from syskit.ui.system_page import BatteryTab
    calls = []
    monkeypatch.setattr(proc, "run_ps",
                        lambda script, timeout=30, resident=False:
                        calls.append(script) or proc.CmdResult(0, "40000,32000,80", ""))
    monkeypatch.setattr("syskit.ui.system_page._BatteryWorker.start",
                        lambda self: self.run())            # worker 同步执行（项目惯例）
    tab = BatteryTab()
    qtbot.addWidget(tab)
    assert calls == []                            # 构造期零系统调用
    tab.show()                                    # 首次显示 → 自动查询一次
    assert len(calls) == 1
    tab.hide()
    tab.show()                                    # 再次显示不重复查询（手动刷新除外）
    assert len(calls) == 1


def test_clipboard_purge_hint_survives_initial_reload(qtbot, monkeypatch):
    """回归：构造序列 reload 在 purge 之后执行——清理提示不得被 reload 覆盖。"""
    import time
    from syskit.ui.system_page import ClipboardTab

    tab = ClipboardTab()
    qtbot.addWidget(tab)
    tab.hist.clear()
    tab.hist._items = [
        {"ts": int(time.time()) - 5 * 86400, "text": "old", "kind": "text", "pinned": False, "data": ""},
        {"ts": int(time.time()), "text": "fresh", "kind": "text", "pinned": False, "data": ""},
    ]
    # 按构造序列：先 reload 再 purge（与 __init__ 一致）
    tab.reload()
    tab._purge_expired()
    assert "清理 1 条" in tab.lbl.text()      # 修复前此处被 "共 1 条" 覆盖
    assert [it["text"] for it in tab.hist.items()] == ["fresh"]


def test_clipboard_purge_expired_on_setup_and_refresh(qtbot, monkeypatch):
    """按天过期：构造期清理一次（过期条目被删并提示），设置 0 天则不清理。"""
    import time
    from syskit import settings as settings_mod
    from syskit.ui.system_page import ClipboardTab

    tab = ClipboardTab()
    qtbot.addWidget(tab)
    tab.hist.clear()
    now = time.time()
    tab.hist._items = [
        {"ts": int(now - 5 * 86400), "text": "old", "kind": "text", "pinned": False, "data": ""},
        {"ts": int(now), "text": "fresh", "kind": "text", "pinned": False, "data": ""},
    ]
    tab._purge_expired()                          # 默认保留 1 天 → old 删除
    assert [it["text"] for it in tab.hist.items()] == ["fresh"]
    assert "清理 1 条" in tab.lbl.text()

    settings_mod.save({"clipboard_retain_days": 0})
    tab.hist._items.append(
        {"ts": int(now - 9 * 86400), "text": "ancient", "kind": "text", "pinned": False, "data": ""})
    tab._purge_expired()                          # 0 天 = 永不过期
    assert any(it["text"] == "ancient" for it in tab.hist.items())


def test_clipboard_reload_rows_preview_and_bad_ts(qtbot):
    tab = _make_clip_tab(qtbot)
    tab.hist._items = [{"ts": int(time.time()), "text": "hello\nworld  foo"},
                       {"ts": "bad", "text": "x" * 200}]
    tab.reload()
    assert tab.tbl.rowCount() == 2
    assert tab.tbl.item(0, 2).text() == "hello world foo"          # 预览压成单行
    assert tab.tbl.item(0, 1).text() == "📄 文本"                   # 类型列
    assert tab.tbl.item(0, 0).text().count(":") == 2               # 时间格式
    assert tab.tbl.item(1, 2).text().endswith("…")                 # 长文截断
    assert tab.tbl.item(1, 2).toolTip().startswith("x")
    assert tab.tbl.item(1, 0).text() == "bad"                      # 非法 ts 兜底
    assert "共 2 条" in tab.lbl.text()


def test_clipboard_watch_change_and_guards(qtbot, monkeypatch):
    from syskit import clipboard as clip_mod
    monkeypatch.setattr(clip_mod, "should_record_now", lambda *a, **k: True)
    tab = _make_clip_tab(qtbot)
    fake = _FakeClip(tab)
    tab._clip = fake

    fake._text = "aaa"
    tab.chk_watch.setChecked(False)
    tab._on_clip_change()                        # 未勾选监听 → 吞进 _last（不补录语义）
    assert tab.hist.items() == []

    tab.chk_watch.setChecked(True)
    tab._on_clip_change()                        # 同内容已被吞 → 不入库（不补录）
    assert tab.hist.items() == []

    fake._text = "new-after-resume"
    tab._on_clip_change()                        # 恢复监听后新文本 → 入库
    assert tab.hist.texts() == ["new-after-resume"]
    assert tab.tbl.rowCount() == 1

    tab._on_clip_change()                        # 与最新一条相同 → 不重复
    assert len(tab.hist.items()) == 1

    fake._text = ""
    tab._on_clip_change()                        # 空文本 → 忽略
    assert len(tab.hist.items()) == 1

    class Boom:
        def text(self):
            raise RuntimeError("clipboard gone")

    tab._clip = Boom()
    tab._on_clip_change()                        # 读取异常 → 安全返回


def test_clipboard_copy_delete_clear(qtbot, monkeypatch, msgbox_recorder):
    tab = _make_clip_tab(qtbot)
    fake = _FakeClip(tab)
    tab._clip = fake
    tab.hist.add("one")
    tab.hist.add("two")
    tab.reload()
    tab.tbl.setCurrentCell(-1, -1)               # 清空选择

    tab._copy_selected()                         # 无选中 → no-op
    assert fake.set_calls == []
    tab._delete_selected()
    assert tab.tbl.rowCount() == 2

    tab._clip = None
    tab.tbl.selectRow(0)
    tab._copy_selected()                         # 无剪贴板句柄 → no-op
    assert fake.set_calls == []

    tab._clip = fake
    tab._copy_selected()                         # 选中第 0 行（two）→ 复制
    assert fake.set_calls == ["two"]
    assert "已复制" in tab.lbl.text()

    tab._delete_selected()                       # 删除选中行
    assert tab.hist.texts() == ["one"]
    assert tab.tbl.rowCount() == 1

    # 表格行数与历史不一致时（idx 超界）复制 no-op
    tab.hist.clear()
    tab.tbl.setRowCount(1)
    tab.tbl.setItem(0, 0, QTableWidgetItem("t"))
    tab.tbl.setCurrentCell(0, 0)
    tab._copy_selected()
    assert len(fake.set_calls) == 1

    # 清空：确认弹窗 Yes / No 两条路径
    tab._clear_all()                             # Yes → 清空
    assert tab.hist.items() == []
    tab.hist.add("left")
    tab.reload()
    monkeypatch.setattr(QMessageBox, "question",
                        staticmethod(lambda *a, **k: QMessageBox.StandardButton.No))
    tab._clear_all()                             # 取消 → 不清空
    assert tab.hist.texts() == ["left"]
    assert recorder_kinds(msgbox_recorder).count("question") == 1


def test_clipboard_search_filter_and_row_index_mapping(qtbot):
    tab = _make_clip_tab(qtbot)
    for t in ("alpha one", "beta two", "gamma ALPHA"):
        tab.hist.add(t)
    tab.reload()
    tab.edt_search.setText("ALPHA")              # 大小写不敏感 → 2 条
    assert tab.tbl.rowCount() == 2
    # 过滤态下行号≠历史索引：删第 0 行应命中 gamma（最新），而非行号对应的错误条目
    tab.tbl.selectRow(0)
    tab._delete_selected()
    texts = tab.hist.texts()
    assert "gamma ALPHA" not in texts and "beta two" in texts
    assert tab.tbl.rowCount() == 1               # 过滤仍生效
    tab.edt_search.setText("")
    assert tab.tbl.rowCount() == 2


def test_clipboard_kind_filter(qtbot):
    tab = _make_clip_tab(qtbot)
    tab.hist.add("[图片 4×4]", kind="image", data="a.png")
    tab.hist.add("plain")
    tab.reload()
    assert tab.tbl.rowCount() == 2
    tab.cmb_kind.setCurrentIndex(2)              # 图片
    assert tab.tbl.rowCount() == 1
    assert tab.tbl.item(0, 1).text() == "🖼 图片"
    tab.cmb_kind.setCurrentIndex(0)
    assert tab.tbl.rowCount() == 2


def test_clipboard_pin_protects_from_eviction(qtbot):
    from syskit.clipboard import ClipboardHistory
    tab = _make_clip_tab(qtbot)
    tab.hist = ClipboardHistory(max_items=2)
    tab.hist.add("keep")
    tab.hist.add("m1")
    tab.reload()
    tab.tbl.selectRow(1)                         # keep（最老）
    tab._toggle_pin()
    assert tab.tbl.item(0, 0).text().startswith("📌")
    assert tab.tbl.item(0, 2).text() == "keep"
    tab.hist.add("m2")                           # 非置顶未超上限
    tab.reload()
    assert [tab.tbl.item(r, 2).text() for r in range(tab.tbl.rowCount())] == \
        ["keep", "m2", "m1"]
    tab.hist.add("m3")                           # 非置顶超额 → m1 淘汰，keep 保留
    tab.reload()
    assert [tab.tbl.item(r, 2).text() for r in range(tab.tbl.rowCount())] == \
        ["keep", "m3", "m2"]
    tab.tbl.selectRow(0)
    tab._toggle_pin()                            # 取消置顶恢复普通排序
    assert not tab.tbl.item(0, 0).text().startswith("📌")


def test_clipboard_pause_toggles_and_blacklist(qtbot, monkeypatch):
    from syskit import clipboard as clip_mod
    monkeypatch.setattr(clip_mod, "should_record_now", lambda *a, **k: True)
    tab = _make_clip_tab(qtbot)
    fake = _FakeClip(tab)
    tab._clip = fake

    tab._toggle_pause()
    assert not tab.chk_watch.isChecked()
    assert tab.btn_pause.text() == "恢复监听"
    fake._text = "while-paused"
    tab._on_clip_change()                        # 暂停中不记录
    assert tab.hist.items() == []
    tab._toggle_pause()
    assert tab.chk_watch.isChecked()
    assert tab.btn_pause.text() == "隐私暂停"

    monkeypatch.setattr(clip_mod, "should_record_now", lambda *a, **k: False)
    fake._text = "from-keepass"
    tab._on_clip_change()                        # 黑名单来源 → 吞掉不入库
    assert tab.hist.items() == []
    monkeypatch.setattr(clip_mod, "should_record_now", lambda *a, **k: True)
    fake._text = "normal"
    tab._on_clip_change()
    assert tab.hist.texts() == ["normal"]


def test_clipboard_detail_rules_and_json_action(qtbot):
    tab = _make_clip_tab(qtbot)
    tab.hist.add("just words")
    tab.hist.add('{"a": 1}')                     # 最新在前 → 行 0
    tab.reload()
    tab.tbl.selectRow(0)                         # 最新在前 → JSON 条目
    labels = [tab.rules_row.itemAt(i).widget().text()
              for i in range(tab.rules_row.count())]
    assert any("格式化 JSON" in x for x in labels)
    tab.tbl.selectRow(1)                         # 无可识别操作
    assert tab.rules_row.count() == 0
    assert "无可识别操作" in tab.lbl_detail.text()
    # 点击「格式化 JSON」→ 结果写回剪贴板
    fake = _FakeClip(tab)
    tab._clip = fake
    tab.tbl.selectRow(0)
    tab.rules_row.itemAt(0).widget().click()
    assert fake.set_calls == ['{\n  "a": 1\n}']
    assert "已复制" in tab.lbl.text()


def test_clipboard_image_preview_and_copy_back(qtbot):
    from PySide6.QtCore import QBuffer
    from PySide6.QtGui import QImage
    from syskit import clipboard as clip_mod
    tab = _make_clip_tab(qtbot)
    img = QImage(4, 4, QImage.Format_ARGB32)
    img.fill(0xFF0000FF)
    buf = QBuffer()
    buf.open(QBuffer.WriteOnly)
    img.save(buf, "PNG")
    name = clip_mod.save_image_bytes(bytes(buf.data()))
    assert name.endswith(".png")
    tab.hist.add("[图片 4×4]", kind="image", data=name)
    tab.reload()
    tab.tbl.selectRow(0)
    assert not tab.lbl_thumb.isHidden()          # 图片条目显示缩略图
    fake = _FakeClip(tab)
    fake.images = []
    fake.setImage = lambda im: fake.images.append(im)
    tab._clip = fake
    tab._copy_selected()                         # 图片条目复制 → 放回剪贴板
    assert len(fake.images) == 1


# ---------------- ModesTab ----------------


def test_modes_show_steps_colors_and_summary(qtbot, msgbox_recorder):
    from syskit.ui import theme
    from syskit.ui.system_page import ModesTab
    tab = ModesTab()
    qtbot.addWidget(tab)
    assert tab.lbl.text().startswith("尚未执行")
    pal = theme.pal()
    tab._show_steps([{"name": "关通知", "ok": True, "detail": "已关闭"},
                     {"name": "暂停更新", "ok": False, "detail": "需要管理员"}],
                    "进入游戏模式")
    assert tab.txt.rowCount() == 2
    assert tab.txt.item(0, 0).text() == "关通知"
    assert tab.txt.item(0, 1).text() == "√ 成功"
    assert tab.txt.item(0, 1).foreground().color() == QColor(pal["ok"])
    assert tab.txt.item(1, 1).text() == "× 失败"
    assert tab.txt.item(1, 1).foreground().color() == QColor(pal["err"])
    assert tab.txt.item(1, 2).text() == "需要管理员"
    assert tab.lbl.text() == "进入游戏模式：1/2 步成功"
    assert recorder_kinds(msgbox_recorder) == ["information"]


def test_modes_busy_guards_and_theme_fallback(qtbot, monkeypatch, msgbox_recorder):
    """_busy 时四个入口全部直接返回；theme 导入失败时用默认色。"""
    import syskit.ui.system_page as sp
    from syskit.ui.system_page import ModesTab
    tab = ModesTab()
    qtbot.addWidget(tab)

    calls = []
    for name in ("run_office_mode", "restore_office_defaults",
                 "run_game_mode", "restore_game_defaults"):
        monkeypatch.setattr(sp.modes, name,
                            lambda *a, _n=name, **k: calls.append(_n) or [])
    tab._busy = True                              # 模拟执行中
    tab._enter_office()
    tab._exit_office()
    tab._enter_game()
    tab._exit_game()
    assert calls == []                            # 全部被防重入守卫拦截
    assert tab._busy is True
    tab._busy = False

    # theme 导入失败 → _show_steps 走默认颜色兜底分支（同时屏蔽包属性与
    # sys.modules 缓存，强制 `from . import theme` 抛 ImportError）
    import syskit.ui as ui_pkg
    monkeypatch.setitem(sys.modules, "syskit.ui.theme", None)
    monkeypatch.delattr(ui_pkg, "theme")
    tab._show_steps([{"name": "s", "ok": True, "detail": ""}], "标题")
    assert tab.txt.item(0, 1).foreground().color() == QColor("#2e9e44")
    tab._show_steps([{"name": "s", "ok": False, "detail": ""}], "标题")
    assert tab.txt.item(0, 1).foreground().color() == QColor("#d94040")


def test_modes_office_flow_reentry_and_error_recovery(qtbot, monkeypatch,
                                                      msgbox_recorder):
    from syskit.ui import system_page as sp
    from syskit.ui.system_page import ModesTab
    tab = ModesTab()
    qtbot.addWidget(tab)
    def _sync_worker(monkeypatch):
        """_ModeWorker.start() 同步执行（emit 直连回填，便于断言）。"""
        from syskit.ui import system_page as sp
        monkeypatch.setattr(sp._ModeWorker, "start",
                            lambda self: self.run())


    calls = []

    def fake_office(open_apps):
        calls.append(open_apps)
        assert tab._busy                         # 执行期间忙碌标志置位
        assert not tab.btn_game.isEnabled()      # 四个按钮全部禁用
        return [{"name": "s", "ok": True, "detail": ""}]

    monkeypatch.setattr(sp.modes, "run_office_mode", fake_office)
    monkeypatch.setattr(sp.modes, "restore_office_defaults",
                        lambda: [{"name": "r", "ok": True, "detail": ""}])
    _sync_worker(monkeypatch)

    tab._enter_office()                          # 默认勾选打开常用软件
    assert calls == [True]
    assert not tab._busy and tab.btn_office.isEnabled()

    tab.chk_open_apps.setChecked(False)
    tab._enter_office()
    assert calls[-1] is False

    # 防重入：执行期间再次点击直接返回（内层不触发 run_office_mode）
    counter = {"n": 0}

    def reentry(open_apps):
        counter["n"] += 1
        tab._enter_office()
        return []

    monkeypatch.setattr(sp.modes, "run_office_mode", reentry)
    tab._enter_office()
    assert counter["n"] == 1

    # 执行异常 → 后台线程兜底为失败步骤（不再向 UI 线程抛异常），按钮恢复不锁死
    def bad(open_apps):
        raise RuntimeError("mode failed")

    monkeypatch.setattr(sp.modes, "run_office_mode", bad)
    tab._enter_office()
    assert not tab._busy and tab.btn_office.isEnabled()
    assert "进入办公模式：0/1 步成功" in tab.lbl.text()   # 异常兜底为失败步骤

    tab._exit_office()                           # 退出模式走恢复链
    assert recorder_kinds(msgbox_recorder)[-1] == "information"
    assert tab.lbl.text() == "退出办公模式：1/1 步成功"


def test_modes_game_admin_downgrade_and_restore(qtbot, monkeypatch,
                                                msgbox_recorder):
    from syskit.ui import system_page as sp
    from syskit.ui.system_page import ModesTab
    tab = ModesTab()
    qtbot.addWidget(tab)

    game_calls = []
    monkeypatch.setattr(sp.modes, "run_game_mode",
                        lambda clear_bg: game_calls.append(clear_bg)
                        or [{"name": "g", "ok": True, "detail": ""}])
    monkeypatch.setattr(sp.modes, "restore_game_defaults",
                        lambda: [{"name": "rg", "ok": False, "detail": ""}])
    monkeypatch.setattr(sp.admin, "is_admin", lambda: False)

    def _sync_worker(monkeypatch):
        monkeypatch.setattr(sp._ModeWorker, "start", lambda self: self.run())
    _sync_worker(monkeypatch)

    tab._enter_game()                            # 无管理员：先警告再继续执行
    assert game_calls == [True]
    kinds = recorder_kinds(msgbox_recorder)
    assert kinds[0] == "warning" and kinds[-1] == "information"
    assert "管理员" in msgbox_recorder[0][1][2]

    msgbox_recorder.clear()
    monkeypatch.setattr(sp.admin, "is_admin", lambda: True)
    tab._enter_game()                            # 有管理员：无警告
    assert recorder_kinds(msgbox_recorder) == ["information"]

    msgbox_recorder.clear()
    monkeypatch.setattr(sp.modes, "run_game_mode",
                        lambda clear_bg: game_calls.append(clear_bg) or [])
    tab.chk_clear_bg.setChecked(False)
    tab._enter_game()                            # 不清理后台（假实现返回空步骤）
    assert game_calls[-1] is False
    assert "进入游戏模式：0/0 步成功" in tab.lbl.text()
    assert recorder_kinds(msgbox_recorder) == ["information"]

    msgbox_recorder.clear()
    tab._exit_game()                             # 退出游戏模式（含失败步也弹窗）
    assert recorder_kinds(msgbox_recorder) == ["information"]
    assert tab.lbl.text() == "退出游戏模式：0/1 步成功"


def test_enter_game_runs_via_mode_worker(qtbot, monkeypatch, msgbox_recorder):
    """进入游戏模式必须走 _start_mode/_ModeWorker 后台执行。

    回归：四入口异步化时漏掉了本入口——它曾在 UI 线程同步跑 run_game_mode
    （powercfg + PowerShell + 进程清理，冻结 UI 数秒~数十秒），且旧测试的
    _sync_worker 补丁对该路径是 no-op，掩盖了分叉。
    """
    from syskit.ui import system_page as sp
    from syskit.ui.system_page import ModesTab
    tab = ModesTab()
    qtbot.addWidget(tab)

    seen = {}
    monkeypatch.setattr(sp.admin, "is_admin", lambda: True)

    def fake_run(clear_bg):
        seen["clear_bg"] = clear_bg
        return [{"name": "g", "ok": True, "detail": ""}]

    monkeypatch.setattr(sp.modes, "run_game_mode", fake_run)
    monkeypatch.setattr(sp._ModeWorker, "start", lambda self: self.run())

    tab.chk_clear_bg.setChecked(True)
    tab._enter_game()
    assert seen["clear_bg"] is True              # 参数经 lambda 传入 worker
    assert not tab._busy and tab._mw is None     # _start_mode 的 worker 生命周期收尾
    assert tab.txt.rowCount() == 1 and tab.btn_game.isEnabled()


# ---------------- app.py ----------------


_REAL_QAPP_CLASS = QApplication        # 模块导入时的真实 QApplication 类


class _FakeApp:
    """run_gui 装配链的 QApplication 假件（离屏进程已存在真实单例，不可再建）。"""

    policy = None
    last = None

    @staticmethod
    def instance():
        # pytest-qt 的 setup/teardown 钩子会调用 QtWidgets.QApplication.instance()，
        # 必须委托回真实类，否则整个测试进程的钩子被打断。
        return _REAL_QAPP_CLASS.instance()

    def __init__(self, argv):
        type(self).last = self
        self.argv = list(argv)
        self.styles = []
        self.font = None
        self.qss = ""
        self.app_name = None
        self.quit_on_close = None

    @classmethod
    def setHighDpiScaleFactorRoundingPolicy(cls, p):
        cls.policy = p

    def setStyle(self, s):
        self.styles.append(s)

    def setFont(self, f):
        self.font = f

    def setStyleSheet(self, qss):
        self.qss = qss

    def setApplicationName(self, n):
        self.app_name = n

    def setWindowIcon(self, icon):
        self.window_icon = icon

    def setQuitOnLastWindowClosed(self, b):
        self.quit_on_close = b

    def exec(self):
        return 7


class _FakeWin:
    instances = []

    def __init__(self, minimized=False):
        self.minimized = minimized
        self.shown = False
        type(self).instances.append(self)

    def show(self):
        self.shown = True


@pytest.fixture
def patched_gui(monkeypatch, msgbox_recorder):
    """注入假 kernel32 / QApplication / MainWindow，并保护 Qt 消息钩子与主题。"""
    import ctypes
    import syskit.logger as logger_mod
    import syskit.ui.main_window as mw_mod
    from syskit.ui import theme

    monkeypatch.setattr(ctypes, "windll",
                        SimpleNamespace(kernel32=SimpleNamespace(
                            CreateMutexW=lambda *a: 1,
                            GetLastError=lambda: 0)))
    monkeypatch.setattr(mw_mod, "MainWindow", _FakeWin)
    monkeypatch.setattr(QtWidgets, "QApplication", _FakeApp)
    setup_calls = []
    monkeypatch.setattr(logger_mod, "setup",
                        lambda verbose_console=False: setup_calls.append(True))
    _FakeApp.policy = None
    _FakeWin.instances = []
    prev_handler = qInstallMessageHandler(lambda *a: None)   # run_gui 内部会再装
    yield SimpleNamespace(setup_called=lambda: bool(setup_calls))
    qInstallMessageHandler(prev_handler)                     # 还原消息钩子
    real_app = QApplication.instance()                       # 无 qtbot 时可能无实例
    if real_app is not None:
        theme.apply(real_app, dark=False)                    # 主题状态还原浅色


def test_app_crash_hook_writes_traceback_to_isolated_log():
    """_enable_crash_log 的异常钩子本体把 traceback 落到隔离目录 crash.log。"""
    import faulthandler
    from syskit import config
    from syskit.ui import app as app_mod
    old_hook, old_thread_hook = sys.excepthook, threading.excepthook
    fh = None
    try:
        fh = app_mod._enable_crash_log()
        assert fh is not None
        try:
            raise ValueError("boom-crash")
        except ValueError:
            sys.excepthook(*sys.exc_info())      # 直接调用钩子（覆盖 _hook 本体）
        fh.flush()
        content = open(os.path.join(config.LOCALAPPDATA_DIR, "crash.log"),
                       encoding="utf-8").read()
        assert "boom-crash" in content and "ValueError" in content
    finally:
        if fh:
            fh.close()
        faulthandler.disable()
        sys.excepthook = old_hook
        threading.excepthook = old_thread_hook


def test_app_enable_crash_log_degrades_on_error(monkeypatch):
    """目录创建失败时返回 None（windowed 打包的兜底路径）。"""
    from syskit.ui import app as app_mod

    def boom(*a, **k):
        raise OSError("access denied")

    monkeypatch.setattr(app_mod.os, "makedirs", boom)
    assert app_mod._enable_crash_log() is None


def test_run_gui_single_instance_shows_hint_and_returns(patched_gui, monkeypatch,
                                                        msgbox_recorder):
    """互斥量已存在（GetLastError==183）：只弹提示并 return，不进事件循环。"""
    import ctypes
    from syskit import config
    from syskit.ui import app as app_mod
    monkeypatch.setattr(ctypes.windll.kernel32, "GetLastError", lambda: 183)

    ret = app_mod.run_gui()
    assert ret is None
    assert _FakeApp.last is not None             # 仅创建了用于弹窗的 QApplication
    assert _FakeWin.instances == []              # 主窗口未创建、未进入事件循环
    kinds_args = msgbox_recorder[0][1]
    assert recorder_kinds(msgbox_recorder) == ["information"]
    assert kinds_args[1] == config.DISPLAY_NAME
    assert "已在运行" in kinds_args[2]


def test_run_gui_qt_msg_handler_swallows_logger_errors(patched_gui, monkeypatch):
    """Qt 消息钩子内 LOGGER 故障时异常被吞（windowed 环境不能因日志崩进程）。"""
    import syskit.logger as logger_mod
    from syskit.ui import app as app_mod

    with pytest.raises(SystemExit):
        app_mod.run_gui()

    class BoomLogger:
        def debug(self, *a, **k):
            raise RuntimeError("no sink")

    monkeypatch.setattr(logger_mod, "LOGGER", BoomLogger())
    from PySide6.QtCore import qWarning
    qWarning("[test] logger error must be swallowed")   # 不应抛出


def test_run_gui_full_bootstrap_and_minimized(patched_gui, monkeypatch):
    """完整装配链：DPI 策略 / Fusion / 主题 QSS / 应用名 / 托盘常驻 / 主窗口。"""
    from syskit import config
    from syskit.ui import app as app_mod

    with pytest.raises(SystemExit) as ei:
        app_mod.run_gui()                        # pytest argv 无 --minimized → show
    assert ei.value.code == 7                    # FakeApp.exec 的返回值
    assert patched_gui.setup_called()
    app = _FakeApp.last
    assert app.argv == sys.argv
    assert app.styles == ["Fusion"]
    assert app.font is not None                  # theme.apply 设置了字体
    assert "QMainWindow" in app.qss              # theme.apply 写入 QSS
    assert app.app_name == config.APP_NAME
    assert app.quit_on_close is False            # 托盘常驻
    assert _FakeApp.policy is not None           # HighDPI 取整策略
    win = _FakeWin.instances[-1]
    assert win.minimized is False and win.shown is True

    # run_gui 安装的 Qt 消息钩子仍在位：发一条 Qt 消息应被接管进日志而非
    # 走默认 fail-fast 处理器（覆盖 _qt_msg_handler 本体）。
    from PySide6.QtCore import qWarning
    qWarning("[test] routed through qt msg handler")

    # --minimized：主窗口不 show，直接进事件循环
    _FakeWin.instances = []
    monkeypatch.setattr(sys, "argv", ["syskit", "--minimized"])
    with pytest.raises(SystemExit):
        app_mod.run_gui()
    win2 = _FakeWin.instances[-1]
    assert win2.minimized is True and win2.shown is False


# ---------------- app.py 防御分支收口（2026-09-16 夜间批）----------------


def test_prepare_data_dirs_outer_exception_is_swallowed(monkeypatch):
    """数据目录准备整体异常（如目录所在卷只读）→ 记日志不外抛，启动不被阻断。"""
    import os as _os
    import syskit.logger as logger_mod
    from syskit.ui import app as app_mod

    calls = []

    class SpyLogger:
        @staticmethod
        def exception(*a, **k):
            calls.append(a)

    monkeypatch.setattr(logger_mod, "LOGGER", SpyLogger)

    def boom(*a, **k):
        raise OSError("readonly volume")

    monkeypatch.setattr(_os, "makedirs", boom)
    assert app_mod._prepare_data_dirs() is None
    assert calls, "异常路径必须留下 LOGGER.exception 记录"


def test_run_gui_qt_fatal_branch_writes_crash_log(patched_gui, monkeypatch):
    """QtFatalMsg：同步直写 crash.log 并 flush/fsync；句柄缓存复用；写失败被吞。"""
    import os
    from PySide6 import QtCore
    from syskit import config
    from syskit.ui import app as app_mod

    captured = []
    real_install = QtCore.qInstallMessageHandler
    monkeypatch.setattr(
        QtCore, "qInstallMessageHandler",
        lambda handler: (captured.append(handler),
                         real_install(lambda *a: None))[1])

    with pytest.raises(SystemExit):
        app_mod.run_gui()
    assert captured, "run_gui 必须安装 Qt 消息钩子"
    handler = captured[0]

    crash_log = os.path.join(config.LOCALAPPDATA_DIR, "crash.log")

    handler(3, None, "[test] fatal one")      # QtFatalMsg：开句柄落盘并缓存
    handler(3, None, "[test] fatal two")      # 第二条：复用缓存句柄
    assert os.path.exists(crash_log)
    data = open(crash_log, "rb").read().decode("utf-8", errors="replace")
    assert "QtFATAL: [test] fatal one" in data
    assert "QtFATAL: [test] fatal two" in data
    assert getattr(handler, "_fatal_fh", None) is not None

    # 句柄失效（文件已关）→ 写失败被吞，不影响后续 LOGGER.debug 兜底
    handler._fatal_fh.close()
    handler(3, None, "[test] fatal after close")


def _png_bytes_4x4():
    from PySide6.QtCore import QBuffer
    from PySide6.QtGui import QImage
    img = QImage(4, 4, QImage.Format_ARGB32)
    img.fill(0xFF00FF00)
    buf = QBuffer()
    buf.open(QBuffer.WriteOnly)
    img.save(buf, "PNG")
    return bytes(buf.data())


def _rules_row_texts(tab):
    out = []
    for i in range(tab.rules_row.count()):
        w = tab.rules_row.itemAt(i).widget()
        if w is not None:
            out.append(w.text())
    return out


def test_clipboard_image_row_thumbnail_and_detail_actions(qtbot):
    """图片条目行内缩略图 + 详情区「放大预览/另存为…」按钮
    （补57：此前列表是纯文字占位、详情区对图片直接 return 无任何动作）。"""
    from syskit import clipboard as clip_mod
    tab = _make_clip_tab(qtbot)
    name = clip_mod.save_image_bytes(_png_bytes_4x4())
    tab.hist.add("[图片 4×4]", kind="image", data=name)
    tab.reload()
    assert tab.tbl.rowCount() == 1
    assert not tab.tbl.item(0, 2).icon().isNull()       # 行内缩略图已挂
    tab.tbl.selectRow(0)
    texts = _rules_row_texts(tab)
    assert "放大预览" in texts and "另存为…" in texts


def test_clipboard_image_preview_dialog_and_save_as(qtbot, monkeypatch, tmp_path):
    from PySide6.QtWidgets import QDialog, QFileDialog
    from syskit import clipboard as clip_mod
    tab = _make_clip_tab(qtbot)
    name = clip_mod.save_image_bytes(_png_bytes_4x4())
    path = clip_mod.image_path(name)
    monkeypatch.setattr(QDialog, "exec", lambda self: 0)
    tab._show_image_preview(path)                        # 不抛即通过
    target = tmp_path / "out.png"
    monkeypatch.setattr(QFileDialog, "getSaveFileName",
                        lambda *a, **k: (str(target), "PNG 图片 (*.png)"))
    tab._save_image_as(path)
    assert target.exists()                               # 真实落盘


def test_clipboard_context_menu_lists_and_wires_actions(qtbot, monkeypatch):
    """右键菜单列出该条目的可用操作并真实接线（触发「删除」→ 行消失）；
    文本条目不出现「另存为…」（图专属）。"""
    import syskit.ui.system_page as sp_mod
    from PySide6.QtCore import QPoint
    tab = _make_clip_tab(qtbot)
    tab.hist.add("hello")
    tab.reload()
    tab.tbl.selectRow(0)
    captured = {}

    class _FakeMenu:
        def __init__(self, parent=None):
            self.items = []

        def addAction(self, text, cb=None):
            self.items.append((text, cb))

        def addSeparator(self):
            self.items.append(("", None))

        def exec(self, pos=None):
            captured["texts"] = [t for t, _cb in self.items]
            for t, cb in self.items:
                if t == "删除" and cb:
                    cb()                                # 触发删除：验证回调接线
            return 0
    monkeypatch.setattr(sp_mod, "QMenu", _FakeMenu)
    tab._show_context_menu(QPoint(5, 5))
    for need in ("复制", "置顶", "预览", "删除"):
        assert need in captured["texts"]
    assert "另存为…" not in captured["texts"]
    assert tab.tbl.rowCount() == 0                      # 删除回调真的删了


def test_clipboard_text_full_preview_dialog(qtbot, monkeypatch):
    """长文本条目全文预览：对话框拿到完整内容（列表只显示 80 字折叠）。"""
    from PySide6.QtWidgets import QDialog, QTextEdit
    tab = _make_clip_tab(qtbot)
    long_text = "abcdef" * 100
    tab.hist.add(long_text)
    tab.reload()
    tab.tbl.selectRow(0)
    shown = {}
    orig_set = QTextEdit.setPlainText

    def spy_set(self, t):
        shown["text"] = t
        orig_set(self, t)
    monkeypatch.setattr(QTextEdit, "setPlainText", spy_set)
    monkeypatch.setattr(QDialog, "exec", lambda self: 0)
    tab._preview_selected()
    assert shown["text"] == long_text
