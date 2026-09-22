"""定向补盲（无人值守轮次13·方向B覆盖率残余）。

取证方式：按模块定向跑覆盖率（触及该模块的全部测试文件一次跑齐，避免假盲区），
只补「主线程可达、有契约价值」的分支。注意：QThread.run() 体内的行（如
toolbox_page 的 _AppNetWorker.run 与各 job 闭包、_scan 的 ThreadPoolExecutor 段）
coverage 天然测不到——Qt 线程不走 threading.settrace——test_ui_appnet 等已真实
exercise 过它们，不属盲区，本文件不重复补。

本轮 5 个契约族：
A. SpeedTest 历史档元素级防护与写盘失败留痕（_clean_history 非法档不炸构造、
   _record_history/_clear_history 写盘失败不炸界面、用户拒绝不清空）；
B. 局域网扫描：全部网卡名命中虚拟排除表时回退旧枚举——VPN-only 机器也能扫到
   真实网段（被排除网卡的地址仍参与选段）；
C. 命令面板本地命令防重入：上一条未完成时气泡提示且不叠加第二条 worker；
D. 设置保存：快捷粘贴热键填非法值时不动既有绑定，其余设置照常落盘。
"""
import socket
import subprocess
import types

import pytest
from PySide6.QtWidgets import QMessageBox

from syskit.ui.toolbox_page import SpeedTestTab, _LanScanWorker, _clean_history

from test_ui_smoke import make_main_window, ui_offline_mocks  # noqa: F401  复用工厂夹具

_KEEP = []          # 保活同步跑过 run() 的 QThread，防收尾期 "destroyed while running"


# ---------------- A. SpeedTest 历史档健壮性 ----------------

def test_clean_history_rejects_non_list_and_dirty_elements():
    """speedtest.json 元素级防护：整档不是 list → []；元素非 dict 逐条剔除。"""
    assert _clean_history("junk") == []
    assert _clean_history({"mbps": 1}) == []
    assert _clean_history(["x", 3, {"ok": True}, None]) == [{"ok": True}]


def test_speed_tab_constructs_with_dirty_history_file(qtbot, monkeypatch):
    """历史档整体脏（合法 JSON 但非 list）时构造不炸：首屏不被 speedtest.json 打挂。"""
    from syskit import stores
    monkeypatch.setattr(stores, "load_json", lambda name, default: "junk")
    tab = SpeedTestTab()
    qtbot.addWidget(tab)
    assert tab.list_hist.count() == 0
    assert tab._history == []


def test_speed_history_write_failure_and_user_decline(qtbot, monkeypatch):
    """写盘失败只留痕不炸界面；用户在确认框点「否」时历史原样保留。"""
    from syskit import stores
    monkeypatch.setattr(stores, "save_json", lambda name, data: False)
    tab = SpeedTestTab()
    qtbot.addWidget(tab)

    tab._record_history(True, 42.5)                    # 写盘失败路径：UI 照常入列
    assert tab._history[0]["mbps"] == 42.5
    assert tab.list_hist.count() == 1

    answers = iter([QMessageBox.StandardButton.No, QMessageBox.StandardButton.Yes])
    monkeypatch.setattr(QMessageBox, "question",
                        staticmethod(lambda *a, **k: next(answers)))
    tab._clear_history()                               # 拒绝 → 不清空
    assert tab._history and tab.list_hist.count() == 1
    tab._clear_history()                               # 确认 → 界面清空（盘写失败也清）
    assert tab._history == [] and tab.list_hist.count() == 0


# ---------------- B. 局域网扫描：虚拟网卡全排除时回退旧枚举 ----------------

def test_lan_scan_falls_back_when_all_adapters_excluded(monkeypatch):
    """全部网卡名都命中排除表（Wintun/vEthernet）时走回退分支：仍按其地址选段扫描。"""
    import psutil
    from syskit import icmp

    import syskit.ui.toolbox_page as tp

    monkeypatch.setattr(psutil, "net_if_addrs", lambda: {
        "Wintun Gate": [types.SimpleNamespace(family=socket.AF_INET,
                                              address="10.8.0.2")],
        "vEthernet (WSL)": [types.SimpleNamespace(family=socket.AF_INET,
                                                  address="172.20.0.1")]})
    monkeypatch.setattr(icmp, "ping",
                        lambda ip, timeout_ms=400: 5.0 if ip == "10.8.0.1" else None)
    monkeypatch.setattr(subprocess, "run",
                        lambda *a, **k: types.SimpleNamespace(stdout=b""))
    w = tp._LanScanWorker()
    _KEEP.append(w)
    out = []
    w.done.connect(out.append)
    w.run()
    # 网段取自被排除名网卡的地址（10.8.0.0/24），只有 10.8.0.1 存活、MAC 列留 ?
    assert out == [[("10.8.0.1", "?", "5")]]


# ---------------- C. 命令面板本地命令防重入 ----------------

def test_run_palette_command_reentry_guard(make_main_window, monkeypatch):
    """上一条本地命令未完成时再下发：气泡提示「请稍候」，不叠加第二条 worker。"""
    win = make_main_window()
    sentinel = object()
    win._cmd_worker = sentinel                         # 模拟上一条仍在执行
    notes = []
    monkeypatch.setattr(win.tray, "notify", lambda *a, **k: notes.append(a))
    win.run_palette_command("calc.exe")                # 非 URL → 确认框已由夹具打桩 Yes
    assert len(notes) == 1 and "请稍候" in notes[0][1]
    assert win._cmd_worker is sentinel                 # 未被新 worker 覆盖
    win._cmd_worker = None                             # 还原，防影响同窗收尾


# ---------------- D. 设置保存：快捷粘贴热键非法输入不破坏既有绑定 ----------------

def test_save_settings_invalid_paste_hotkey_keeps_old_binding(make_main_window,
                                                               monkeypatch):
    """快捷粘贴热键填非法值：本次保存不改全局热键，其余设置照常落盘。"""
    from syskit import admin
    from syskit import hotkey as hotkey_mod
    from syskit import settings
    monkeypatch.setattr(hotkey_mod, "register", lambda m, v, hotkey_id=1: True)
    monkeypatch.setattr(admin, "is_admin", lambda: False)
    win = make_main_window()
    settings.save({"clip_paste_hotkey": "Ctrl+Alt+V"})   # 既有绑定（回退基线）
    win.cmb_paste_hotkey.setCurrentText("Ctrl+F25")      # parse_hotkey → None
    win.spin_interval.setValue(45)
    win._save_settings()
    saved = settings.load()
    assert saved["clip_paste_hotkey"] == "Ctrl+Alt+V"    # 非法输入不覆盖旧键
    assert saved["poll_interval_sec"] == 45              # 其余设置照常保存
