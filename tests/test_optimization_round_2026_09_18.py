"""2026-09-18 优化轮回归：启动懒加载 / 日志页增量读 / 托盘剪贴板 mtime 缓存。

本文件锁定「优化会话」的三项 UI 响应性改造语义：
- 设置页开机自启（schtasks）与修复页 WLAN 配置文件枚举（netsh）不在 MainWindow
  构造期执行，首次切到对应页才查——加快主窗体首帧；
- 日志页增量读：首访尾部载入、之后只追加新字节、轮转（文件变小）重读——
  此前每 5s 全量 readlines（最大 5MB）且 setPlainText 复位滚动位置；
- 托盘「最近剪贴板」子菜单按 clipboard.json 的 mtime 缓存，未变化不重读
  DPAPI（与 palette 同款缓存）。

安全（叠加 conftest 哨兵与 smoke 的 ui_offline_mocks）：无真实 schtasks/netsh/
注册表/socket；文件读写全部落在 pytest 临时目录。
"""
import os

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from PySide6.QtWidgets import QMenu, QSystemTrayIcon

# 复用 smoke 的离线 autouse 夹具与 MainWindow 工厂夹具（含后台线程回收）
from test_ui_smoke import main_win, make_main_window, ui_offline_mocks  # noqa: E402,F401

QSystemTrayIcon.showMessage = lambda self, *a, **k: None  # noqa: E305 离屏无托盘实现


# ---------------- 本轮新增后台 worker 的 run() 同步直跑（信号契约） ----------------

def test_plain_cmd_worker_run_success_and_exception(monkeypatch):
    from syskit import proc
    from syskit.ui import workers as ws

    monkeypatch.setattr(proc, "run",
                        lambda cmd, timeout=30: proc.CmdResult(0, " out ", ""))
    got = []
    w = ws.PlainCmdWorker("echo x")
    w.done.connect(lambda ok, out: got.append((ok, out)))
    w.run()
    assert got == [(True, "out")]

    def boom(cmd, timeout=30):
        raise RuntimeError("no exec")

    monkeypatch.setattr(proc, "run", boom)
    got2 = []
    w2 = ws.PlainCmdWorker("x")
    w2.done.connect(lambda ok, out: got2.append((ok, out)))
    w2.run()
    assert got2 == [(False, "no exec")]


def test_ping_and_dns_worker_run_bodies(monkeypatch):
    from syskit import dnsquery, icmp
    from syskit.ui import toolbox_page as tb

    # _PingWorker：正常回报
    monkeypatch.setattr(icmp, "ping", lambda host, timeout_ms=1500: 3.5)
    got = []
    p = tb._PingWorker("1.2.3.4")
    p.done.connect(got.append)
    p.run()
    assert got == [3.5]

    # _DnsQueryWorker：系统默认 / 指定服务器 / 异常转失败行
    import socket as socket_mod
    monkeypatch.setattr(socket_mod, "getaddrinfo",
                        lambda *a, **k: [(socket_mod.AF_INET, 1, 6, "", ("2.2.2.2", 0))])
    got_d = []
    d = tb._DnsQueryWorker("x.com", "系统默认")
    d.done.connect(got_d.append)
    d.run()
    assert got_d == [["2.2.2.2"]]

    monkeypatch.setattr(dnsquery, "query",
                        lambda host, server, timeout=3.0, qtype=1: ["9.9.9.9"])
    got_d2 = []
    d2 = tb._DnsQueryWorker("x.com", "9.9.9.9")
    d2.done.connect(got_d2.append)
    d2.run()
    assert got_d2 == [["9.9.9.9"]]

    def boom_q(*a, **k):
        raise RuntimeError("timeout!")

    monkeypatch.setattr(dnsquery, "query", boom_q)
    got_d3 = []
    d3 = tb._DnsQueryWorker("x.com", "9.9.9.9")
    d3.done.connect(got_d3.append)
    d3.run()
    assert got_d3 == [["查询失败: timeout!"]]


def test_wifi_share_workers_run_bodies(monkeypatch):
    from syskit import repairs
    from syskit.ui import wifi_page as wp

    monkeypatch.setattr(repairs, "wlan_profiles", lambda: ["A", "B"])
    got = []
    pf = wp._ProfilesWorker()
    pf.done.connect(got.append)
    pf.run()
    assert got == [["A", "B"]]

    def boom_profiles():
        raise RuntimeError("netsh gone")

    monkeypatch.setattr(repairs, "wlan_profiles", boom_profiles)
    got2 = []
    pf2 = wp._ProfilesWorker()
    pf2.done.connect(got2.append)
    pf2.run()
    assert got2 == [[]]                       # 异常 → 空列表兜底

    monkeypatch.setattr(wp, "profile_password", lambda name: ("WPA2", "pw"))
    got3 = []
    pp = wp._ProfilePwdWorker("Home")
    pp.done.connect(lambda n, a, p: got3.append((n, a, p)))
    pp.run()
    assert got3 == [("Home", "WPA2", "pw")]   # 带回 profile 名：完成时可校验错配

    def boom_pw(name):
        raise RuntimeError("x")

    monkeypatch.setattr(wp, "profile_password", boom_pw)
    got4 = []
    pp2 = wp._ProfilePwdWorker("Home")
    pp2.done.connect(lambda n, a, p: got4.append((n, a, p)))
    pp2.run()
    assert got4 == [("Home", "", "")]         # 异常 → 双空兜底


def test_scan_networks_bare_ssid_compat_line(monkeypatch):
    """裸 `SSID:` 行兼容分支（防御保留；真实 show networks 输出走带索引分支）。"""
    from syskit import proc

    import syskit.ui.wifi_page as wp

    out = "SSID: Legacy\n    BSSID 1: aa:aa:aa:aa:aa:aa\n        信号: 10%\n"
    monkeypatch.setattr(proc, "run",
                        lambda cmd, timeout=20: proc.CmdResult(code=0, stdout=out, stderr=""))
    aps = wp.scan_networks()
    assert [a["ssid"] for a in aps] == ["Legacy"]


# ---------------- 启动懒加载（main_window） ----------------

def test_settings_autostart_and_repair_profiles_lazy_load(make_main_window, monkeypatch):
    """schtasks（开机自启态）与 netsh（WLAN 配置文件）不在构造期执行。"""
    from syskit import autostart, repairs

    enabled_calls = []
    monkeypatch.setattr(autostart, "enabled", lambda: enabled_calls.append(1) or True)
    prof_calls = []
    monkeypatch.setattr(repairs, "wlan_profiles",
                        lambda: prof_calls.append(1) or ["HomeWiFi"])

    win = make_main_window()
    assert enabled_calls == [] and prof_calls == []      # 构造期零系统调用

    win.stack.setCurrentWidget(win.page_repair)          # 首访修复页 → 枚举配置文件
    assert prof_calls == [1]
    assert win.cmb_profile.currentText() == "HomeWiFi"
    assert enabled_calls == []

    win.stack.setCurrentWidget(win.page_settings)        # 首访设置页 → 查询自启态
    assert enabled_calls == [1]
    assert win.chk_start.isChecked() is True

    win.stack.setCurrentWidget(win.page_dash)
    win.stack.setCurrentWidget(win.page_settings)        # 再次进入不重复查询
    win.stack.setCurrentWidget(win.page_repair)
    assert enabled_calls == [1] and prof_calls == [1]


def test_save_settings_uses_backfilled_autostart_state(make_main_window, monkeypatch):
    """懒回填后的勾选态参与保存：勾选态与实际自启态不一致时发出对应启停。"""
    from syskit import admin, autostart, settings as settings_mod

    enabled_calls = []
    state = {"enabled": True}                        # 实际：当前已注册自启

    def fake_enabled():
        enabled_calls.append(1)
        return state["enabled"]

    calls = []
    monkeypatch.setattr(autostart, "enabled", fake_enabled)
    monkeypatch.setattr(autostart, "enable", lambda: calls.append("enable"))
    monkeypatch.setattr(autostart, "disable", lambda: calls.append("disable"))
    monkeypatch.setattr(settings_mod, "save", lambda patch: None)
    monkeypatch.setattr(admin, "is_admin", lambda: True)

    win = make_main_window()
    assert enabled_calls == []                       # 构造期不查
    win.stack.setCurrentWidget(win.page_settings)    # 懒回填：chk_start ← True
    assert win.chk_start.isChecked() is True

    state["enabled"] = False                         # 实际已丢失自启（如任务被删）
    win._save_settings()
    assert calls == ["enable"]                       # 勾选态 True vs 实际 False → 重新启用


# ---------------- 日志页增量读（main_window） ----------------

def test_log_page_incremental_reload(make_main_window):
    """首访尾部载入 → 追加只读增量 → 轮转（变小）重读 → 切源重载。"""
    from syskit import config

    win = make_main_window()
    log_path = os.path.join(config.LOG_DIR, "syskit.log")
    os.makedirs(config.LOG_DIR, exist_ok=True)
    with open(log_path, "w", encoding="utf-8") as f:
        f.write("".join(f"line-{i}\n" for i in range(5)))

    win.stack.setCurrentWidget(win.page_logs)            # 首访：尾部载入
    text = win.txt_log.toPlainText()
    assert "line-0" in text and "line-4" in text
    assert win._log_state[0] == "syskit.log"
    assert win._log_state[1] == os.path.getsize(log_path)

    with open(log_path, "a", encoding="utf-8") as f:     # 追加 → 只读增量
        f.write("line-5\nline-6\n")
    win._reload_log()
    assert "line-6" in win.txt_log.toPlainText()
    assert "line-0" in win.txt_log.toPlainText()         # 旧内容保留（追加而非重置）
    assert win._log_state[1] == os.path.getsize(log_path)

    win._reload_log()                                    # 无新内容 → 视图不变
    assert win._log_state[1] == os.path.getsize(log_path)

    with open(log_path, "w", encoding="utf-8") as f:     # 轮转：文件变小 → 重读
        f.write("fresh-0\n")
    win._reload_log()
    assert "fresh-0" in win.txt_log.toPlainText()
    assert "line-4" not in win.txt_log.toPlainText()

    win.cmb_log_src.setCurrentIndex(1)                   # 切到 error.log → 独立重载
    win._reload_log()
    assert win._log_state[0] == "error.log"


# ---------------- 托盘剪贴板子菜单 mtime 缓存（tray） ----------------

def test_tray_clip_menu_uses_mtime_cache(main_win, monkeypatch):
    from syskit.ui import tray as tray_mod

    tray = main_win.tray
    real_cls = tray_mod.clipboard_mod.ClipboardHistory
    calls = {"n": 0}

    class Counting(real_cls):
        def __init__(self, *a, **k):
            calls["n"] += 1
            super().__init__(*a, **k)

    monkeypatch.setattr(tray_mod.clipboard_mod, "ClipboardHistory", Counting)

    m = QMenu()
    m.setParent(main_win)
    try:
        tray._fill_clip_menu(m)                          # 首次：读盘 + DPAPI 解密
        assert calls["n"] == 1
        tray._fill_clip_menu(m)                          # mtime 未变 → 走缓存
        assert calls["n"] == 1
    finally:
        m.deleteLater()
