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


# ---------------- 布局 ----------------

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
