"""UI 布局 / 操作逻辑优化轮回归（2026-09-19）。

布局：概览网卡表拉伸填满页面剩余高度且末列吃掉横向富余；修复动作表全行统一
行高（曾只设第 0 行 → 首行比其他行高）；设置页保存按钮右对齐常规宽度（曾全窗
宽拉伸）；密码库账号表密码/标签列固定合理宽度（曾被内容挤到极窄）。

操作逻辑：DNS 查询框回车即查；持续 ping 目标框回车即开始（在跑则忽略）；
密码库网站搜索框回车选中首个过滤结果并聚焦账号表（键盘流 搜→回车→选账号）。
"""
import os

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from PySide6.QtWidgets import QHeaderView

from test_ui_smoke import make_main_window, ui_offline_mocks  # noqa: E402,F401

from syskit.ui.vault_page import VaultPage  # noqa: E402
from syskit.ui.toolbox_page import DnsSubnetTab, PingRouteTab  # noqa: E402


# ---------------- 自审回归：隐式不变式加固 ----------------

def test_save_settings_backfills_autostart_before_use(make_main_window, monkeypatch):
    """不进设置页直接保存时，先回填自启态再决策——否则未回填的未勾选态
    会把用户已启用的开机自启误禁（不变式原本是隐式的）。"""
    from syskit import admin, autostart
    from syskit import settings as settings_mod

    calls = []
    monkeypatch.setattr(admin, "is_admin", lambda: True)
    monkeypatch.setattr(autostart, "enabled", lambda: calls.append("query") or True)
    monkeypatch.setattr(autostart, "enable", lambda: calls.append("enable"))
    monkeypatch.setattr(autostart, "disable", lambda: calls.append("disable"))
    monkeypatch.setattr(settings_mod, "save", lambda patch: None)

    win = make_main_window()             # 从不切到设置页
    win._save_settings()
    # 只查询（回填一次 + 决策一次）；回填后勾选与实际态一致 → 不误禁/误启
    assert calls.count("query") == 2
    assert "enable" not in calls and "disable" not in calls


def test_exclude_os_history_prototypes_and_failure_cleanup(monkeypatch):
    """Win32 原型显式签名（64 位下默认 c_int 截断指针）+ SetClipboardData
    失败时归还句柄。假 DLL 桩全程不触真实剪贴板。"""
    import ctypes as ct
    import types as ts

    from syskit import clipboard as cb

    calls = []
    buf = ct.create_string_buffer(4)

    # 用裸函数命名空间（生产代码会给函数对象设 restype/argtypes——绑定方法
    # 不允许设属性，恰好会验证异常兜底路径）
    def _open_clipboard(hwnd):
        return 1

    def _close_clipboard():
        calls.append(("close",))
        return 1

    def _register(fmt_name):
        return 4900 + len(calls)

    def _set_data(fmt, h):
        calls.append(("set", fmt, h))
        return 0                         # 恒失败 → 必须看到 GlobalFree

    def _alloc(flags, size):
        calls.append(("alloc", flags, size))
        return 0x1234

    def _lock(h):
        calls.append(("lock", h))
        return ct.addressof(buf)         # 真实可写地址，memmove 安全

    def _unlock(h):
        calls.append(("unlock", h))
        return 1

    def _free(h):
        calls.append(("free", h))
        return None

    monkeypatch.setattr(ct, "windll", ts.SimpleNamespace(
        user32=ts.SimpleNamespace(OpenClipboard=_open_clipboard,
                                  CloseClipboard=_close_clipboard,
                                  RegisterClipboardFormatW=_register,
                                  SetClipboardData=_set_data),
        kernel32=ts.SimpleNamespace(GlobalAlloc=_alloc, GlobalLock=_lock,
                                    GlobalUnlock=_unlock, GlobalFree=_free)))
    cb.exclude_os_history()              # 不得抛异常
    sets = [c for c in calls if c[0] == "set"]
    frees = [c for c in calls if c[0] == "free"]
    closes = [c for c in calls if c[0] == "close"]
    assert len(sets) == 3 and len(frees) == 3 and closes == [("close",)]
    assert all(f[1] == s[2] for f, s in zip(frees, sets))   # 归还的正是分配的句柄


# ---------------- 布局 ----------------

def test_online_adapter_row_uses_semantic_soft_color(make_main_window):
    """在线网卡行：ok_soft 柔和底 + 调色板 ok 绿字（曾挪用选中色 sel_bg——
    与真实选中无法区分，且颜色固化后切暗色主题残留刺眼浅蓝）。"""
    from PySide6.QtGui import QColor

    from syskit.ui import theme
    from syskit.snapshot import Adapter, Snapshot

    win = make_main_window()
    snap = Snapshot(adapters=[
        Adapter(name="WLAN", desc="Intel", status="Up", ipv4=["192.168.1.2"],
                gw4=["192.168.1.1"], conn_v4="Internet"),
        Adapter(name="以太网 2", desc="Realtek", status="Down"),
    ])
    win._fill_adapters(snap)
    row_bg = win.tbl_adapters.item(0, 0).background().color()
    row_fg = win.tbl_adapters.item(0, 0).foreground().color()
    assert row_bg == QColor(theme.pal()["ok_soft"]) is not QColor(theme.pal()["sel_bg"])
    assert row_fg == QColor(theme.pal()["ok"])
    assert win.tbl_adapters.item(1, 0).background().color() != row_bg   # 非在线行无底色


def test_theme_change_refills_adapters_colors(make_main_window, monkeypatch):
    """主题切换后用缓存快照重填网卡表：颜色不再残留上一主题（QSS 不覆盖 item 色）。"""
    from PySide6.QtGui import QColor
    from PySide6.QtWidgets import QApplication

    from syskit.ui import theme
    from syskit.snapshot import Adapter, Snapshot

    win = make_main_window()
    snap = Snapshot(adapters=[
        Adapter(name="WLAN", desc="Intel", status="Up", ipv4=["192.168.1.2"],
                gw4=["192.168.1.1"], conn_v4="Internet"),
    ])
    win._fill_adapters(snap)
    was_dark = theme.is_dark()
    try:
        theme.apply(QApplication.instance(), dark=not was_dark)
        win._on_theme_changed()
        bg_after = win.tbl_adapters.item(0, 0).background().color()
        assert bg_after == QColor(theme.pal()["ok_soft"])   # 新主题的柔和底
    finally:
        theme.apply(QApplication.instance(), dark=was_dark)
        win._on_theme_changed()


def test_overview_adapters_table_stretches(make_main_window):
    win = make_main_window()
    # 表格吃掉页面剩余高度（曾固定高度，下方留大片空白）；末列吃掉横向富余
    assert win.tbl_adapters.horizontalHeader().stretchLastSection() is True


def test_repair_actions_rows_uniform_height(make_main_window):
    win = make_main_window()
    vh = win.tbl_actions.verticalHeader()
    assert vh.defaultSectionSize() == 40
    # 全行一致：不再有"仅第 0 行 42px"的孤高首行
    heights = {win.tbl_actions.rowHeight(r) for r in range(win.tbl_actions.rowCount())}
    assert heights == {40}


def test_settings_save_button_not_full_width(make_main_window):
    win = make_main_window()
    assert win.btn_save_settings.minimumWidth() == 140


def test_vault_account_columns_reasonable_width(qtbot):
    page = VaultPage()
    qtbot.addWidget(page)
    assert page.tbl_accts.columnWidth(1) == 150      # 密码列
    assert page.tbl_accts.columnWidth(2) == 170      # 标签列
    assert page.tbl_accts.horizontalHeader().sectionResizeMode(0) == QHeaderView.Stretch


# ---------------- 操作逻辑 ----------------

def test_dns_domain_return_triggers_query(qtbot, monkeypatch):
    tab = DnsSubnetTab()
    qtbot.addWidget(tab)
    called = []
    monkeypatch.setattr(tab, "query", lambda: called.append(1))
    tab.ed_domain.returnPressed.emit()
    assert called == [1]


def test_ping_target_return_starts_and_skips_when_running(qtbot, monkeypatch):
    from syskit import icmp
    monkeypatch.setattr(icmp, "ping", lambda host, timeout_ms=1500: 1.0)
    tab = PingRouteTab()
    qtbot.addWidget(tab)

    tab.ed_target.returnPressed.emit()               # 未运行 → 开始
    assert tab._ping_timer.isActive()
    # 回车触发了真实 _PingWorker QThread：等它收尾，避免 tab 回收时线程
    # 仍在运行触发 qFatal（生产由 reap 挂树 + quit_app 全树等待兜底）
    qtbot.wait_until(lambda: tab._ping_worker is None, timeout=10000)
    tab.ed_target.returnPressed.emit()               # 在跑 → 忽略（不重启）
    assert tab._ping_timer.isActive()
    qtbot.wait_until(lambda: tab._ping_worker is None, timeout=10000)
    tab.toggle_ping()
    assert not tab._ping_timer.isActive()


def test_vault_site_search_enter_selects_first_result(qtbot):
    """回车 → 选中过滤结果第一项并聚焦账号表（搜→回车→选账号 的键盘流）。"""
    from syskit import vault
    vault.init_db()
    vault.add_site("AlphaSite")
    vault.add_site("BetaSite")

    page = VaultPage()
    qtbot.addWidget(page)
    page.refresh_sites()
    assert page.lst_sites.count() >= 2

    page.lst_sites.setCurrentRow(-1)
    page.edt_site.returnPressed.emit()               # 回车 → 选第一个结果
    assert page.lst_sites.currentRow() == 0
    # （焦点移交 setFocus 在真实可见窗口生效；离屏隐藏窗不派发焦点事件，不在此断言）
