"""网络质量日报 + 测速历史持久化 两个新功能的 UI 集成回归（2026-09-19）。"""
import os
import time

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from PySide6.QtWidgets import QMessageBox

from test_ui_smoke import make_main_window, ui_offline_mocks  # noqa: E402,F401

QMessageBox.question = staticmethod(  # noqa: E305 清空历史确认框默认 Yes
    lambda *a, **k: QMessageBox.StandardButton.Yes)

from syskit import stores  # noqa: E402
from syskit.stores import EventStore, TrafficStore  # noqa: E402
from syskit.ui.dashboard_page import DashboardPage  # noqa: E402
from syskit.ui.toolbox_page import SpeedTestTab  # noqa: E402


# ---------------- 网络质量日报（dashboard 双 Tab） ----------------

def test_dashboard_report_tab_aggregates_events_and_traffic(qtbot, monkeypatch):
    today = time.strftime("%Y-%m-%d")
    store = EventStore()
    store.add("ok", "已恢复联网")                       # 最新在前：ok 在后
    store.add("no_net", "探测失败 ×2")
    traffic = TrafficStore()
    traffic.add(300, 700)

    page = DashboardPage(store, traffic=traffic)
    qtbot.addWidget(page)
    assert page._hist_tabs.count() == 2                 # 时间线 / 日报
    assert page.list_events.count() == 2                # 原 Tab 属性名不变（ok+no_net）
    rows = page.tbl_report.rowCount()
    assert rows >= 1
    # 今天行：断网 1 次、时长 0（无恢复对——这里只有 no_net→ok？add 顺序 ok 在最新，
    # 时间正序为 no_net→ok = 一次 0 秒断网）、流量 700/300
    dates = {page.tbl_report.item(r, 0).text(): r for r in range(rows)}
    r = dates[today]
    assert page.tbl_report.item(r, 1).text() == "1"
    assert page.tbl_report.item(r, 3).text() != "—" and page.tbl_report.item(r, 4).text() != "—"

    # 7 天 / 30 天切换：行数变化且 primary 高亮跟随
    page.btn_report_30.click()
    assert page.tbl_report.rowCount() == 30
    page.btn_report_7.click()
    assert page.tbl_report.rowCount() == 7


def test_dashboard_report_without_traffic_store_shows_placeholder(qtbot):
    page = DashboardPage(EventStore())                  # 未传 traffic（兼容旧签名）
    qtbot.addWidget(page)
    page.reload_events()
    assert page.tbl_report.rowCount() == 7
    assert page.tbl_report.item(6, 3).text() == "—"     # 无流量数据 → 占位


def test_dashboard_reset_if_stale_clears_charts(qtbot, monkeypatch):

    page = DashboardPage(EventStore())
    qtbot.addWidget(page)
    page.update_tick({"lat_gw": 5.0, "lat_pub": 9.0, "gateway": "192.168.1.1",
                      "down": 1024.0, "up": 512.0, "wifi_signal": 70,
                      "today_sent": 1, "today_recv": 1})
    assert any(v is not None for _n, _c, dq in page.chart_lat._series for v in dq)

    page.reset_if_stale(max_gap_sec=600)                # 刚更新过 → 不清
    assert any(v is not None for _n, _c, dq in page.chart_lat._series for v in dq)

    page._last_tick -= 700                              # 模拟断流超窗 → 清空
    page.reset_if_stale(max_gap_sec=600)
    assert all(v is None for _n, _c, dq in page.chart_lat._series for v in dq)


def test_system_page_reset_if_stale_delegates_to_perf(qtbot):
    from syskit.ui.system_page import SystemPage
    page = SystemPage()
    qtbot.addWidget(page)
    page.update_tick({"cpu": 5.0, "mem": 30.0, "disk_r": 10, "disk_w": 5})
    assert any(v is not None for _n, _c, dq in page.perf.chart_sys._series for v in dq)
    page.perf._last_tick -= 700
    page.reset_if_stale()                               # SystemPage → PerfTab 委托
    assert all(v is None for _n, _c, dq in page.perf.chart_sys._series for v in dq)


# ---------------- 测速历史持久化 ----------------

def test_speed_history_persist_and_reload(qtbot, tmp_path, monkeypatch):
    monkeypatch.setattr(stores.config, "APPDATA_DIR", str(tmp_path))
    tab = SpeedTestTab()
    qtbot.addWidget(tab)
    assert tab.list_hist.count() == 0                   # 首次：空历史

    tab._on_done(True, 2_000_000.0)                     # 16 Mbps
    tab._on_done(False, 0.0)
    assert tab.list_hist.count() == 2
    assert "测速失败" in tab.list_hist.item(0).text()   # 失败也记录（最新在前）
    assert "Mbps" in tab.list_hist.item(1).text()

    # 模拟重启：新实例从盘上读回
    tab2 = SpeedTestTab()
    qtbot.addWidget(tab2)
    assert tab2.list_hist.count() == 2
    assert tab2.list_hist.item(0).text() == tab.list_hist.item(0).text()


def test_speed_history_cap_200(qtbot, tmp_path, monkeypatch):
    monkeypatch.setattr(stores.config, "APPDATA_DIR", str(tmp_path))
    tab = SpeedTestTab()
    qtbot.addWidget(tab)
    for i in range(230):
        tab._on_done(True, 1_000_000.0 + i)
    assert tab.list_hist.count() == 200                 # 上限裁剪（UI 侧）
    assert len(tab._history) == 200                     # 持久化侧同步裁剪
    assert tab.list_hist.item(0).text().startswith(time.strftime("%m-%d %H:%M"))


def test_speed_history_clear_button(qtbot, tmp_path, monkeypatch):
    monkeypatch.setattr(stores.config, "APPDATA_DIR", str(tmp_path))
    tab = SpeedTestTab()
    qtbot.addWidget(tab)
    tab._on_done(True, 2_000_000.0)
    assert tab.list_hist.count() == 1
    tab.btn_hist_clear.click()                          # question 桩默认 Yes
    assert tab.list_hist.count() == 0
    assert stores.load_json("speedtest.json", None) == []
    assert SpeedTestTab().list_hist.count() == 0        # 重启后为空


def test_speed_history_survives_corrupt_file(qtbot, tmp_path, monkeypatch):
    monkeypatch.setattr(stores.config, "APPDATA_DIR", str(tmp_path))
    (tmp_path / "speedtest.json").write_text("{半截", encoding="utf-8")
    tab = SpeedTestTab()                                # 损坏 → 回退空列表不崩
    qtbot.addWidget(tab)
    assert tab.list_hist.count() == 0
