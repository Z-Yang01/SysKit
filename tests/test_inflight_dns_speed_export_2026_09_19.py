"""在途改动回归（2026-09-19 续）：工具箱 DNS 测速 Tab + 诊断报告导出。

红线：不发真实 DNS 查询（dnsquery.query 打桩）、不弹真实文件框
（QFileDialog.getSaveFileName 打桩）、不写真实数据目录（conftest 重定向）。
"""
import os

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

import pytest
from PySide6.QtWidgets import QFileDialog, QMessageBox

from test_ui_smoke import main_win, make_main_window, ui_offline_mocks  # noqa: E402,F401

from syskit import dnsquery  # noqa: E402
from syskit.netcheck import CheckResult  # noqa: E402
from syskit.ui.toolbox_page import (DnsSpeedTab, ToolboxPage,  # noqa: E402
                                    _DNS_SPEED_SERVERS, _DnsSpeedWorker)


# ---------------- DNS 测速：后台聚合线程 ----------------

@pytest.fixture
def fake_dns(monkeypatch):
    """可编程假解析：fail 集合内的服务器一律抛错，calls 记录 (domain, server)。"""
    state = {"fail": set(), "calls": []}

    def _query(domain, server, timeout=1.5, attempts=1):
        state["calls"].append((domain, server))
        if timeout is None or timeout > 3:
            raise AssertionError("测速查询必须带硬超时，否则单台服务器能拖死整轮")
        if server in state["fail"]:
            raise OSError("模拟解析失败")
        return ["1.2.3.4"]

    monkeypatch.setattr(dnsquery, "query", _query)
    return state


def test_dns_speed_worker_probes_every_server_and_aggregates(fake_dns, qtbot):
    """结果经事件队列回主线程（池线程直调槽会碰 QTableWidget）：
    run() 返回时必须尚未派发，泵一次事件循环才拿到全部 6 条。"""
    from PySide6.QtWidgets import QApplication

    fake_dns["fail"] = {"8.8.8.8"}
    w = _DnsSpeedWorker("example.com", 3)
    got = []
    w.server_done.connect(lambda s, info: got.append((s, info)))
    w.run()                                  # 线程池已 join，事件仍堆在主线程队列
    assert got == []                         # 未派发 = Auto 连接判定为跨线程排队
    qtbot.wait_until(lambda: len(got) == len(_DNS_SPEED_SERVERS), timeout=3000)
    QApplication.processEvents()

    d = dict(got)
    assert sorted(d) == sorted(s for s, _n in _DNS_SPEED_SERVERS)
    assert d["8.8.8.8"] is None                                  # 全失败 → None
    assert d["223.5.5.5"]["ok"] == d["223.5.5.5"]["total"] == 3
    assert d["223.5.5.5"]["avg"] >= 0
    assert {dom for dom, _s in fake_dns["calls"]} == {"example.com"}


def test_dns_speed_worker_partial_failures_counted(fake_dns, monkeypatch, qtbot):
    """每台服务器首发失败：成功率必须如实反映 ok<total，而不是被平均值掩盖。"""
    per_server = {}

    def _flaky(domain, server, timeout=1.5, attempts=1):
        per_server[server] = per_server.get(server, 0) + 1
        if per_server[server] == 1:          # 同一台串行探测 → 与线程交错无关
            raise OSError("抖动")
        return ["1.2.3.4"]

    monkeypatch.setattr(dnsquery, "query", _flaky)
    w = _DnsSpeedWorker("d.com", 2)
    got = []
    w.server_done.connect(lambda s, info: got.append((s, info)))
    w.run()
    qtbot.wait_until(lambda: len(got) == 6, timeout=3000)

    assert all(info["ok"] == 1 and info["total"] == 2 for _s, info in got)


def test_dns_speed_times_clamped():
    assert _DnsSpeedWorker("d", 99)._times == 5
    assert _DnsSpeedWorker("d", 0)._times == 1
    assert _DnsSpeedWorker("d", 3)._times == 3


# ---------------- DNS 测速：Tab 渲染与推荐 ----------------

def test_dns_speed_tab_sorts_failures_last(qtbot):
    tab = DnsSpeedTab()
    qtbot.addWidget(tab)
    tab._server_done("223.5.5.5", {"avg": 40.0, "ok": 3, "total": 3})
    tab._server_done("8.8.8.8", None)
    tab._server_done("1.1.1.1", {"avg": 120.0, "ok": 2, "total": 3})

    assert tab.tbl.rowCount() == 3
    assert tab.tbl.item(0, 0).text().startswith("阿里")             # 最快在前
    assert tab.tbl.item(1, 0).text().startswith("Cloudflare")
    assert tab.tbl.item(2, 0).text().startswith("Google")           # 全失败垫底
    assert tab.tbl.item(1, 2).text() == "2/3"
    assert "部分丢包" in tab.tbl.item(1, 3).text()
    assert "全部超时" in tab.tbl.item(2, 3).text()
    assert tab.tbl.item(2, 1).text() == "—"                         # 无延迟不编数


def test_dns_speed_tab_recommend_after_done(qtbot):
    tab = DnsSpeedTab()
    qtbot.addWidget(tab)
    tab._server_done("1.1.1.1", {"avg": 120.0, "ok": 3, "total": 3})
    tab._server_done("223.5.5.5", {"avg": 40.0, "ok": 3, "total": 3})
    tab._finished()
    text = tab.lbl_recommend.text()
    assert "推荐：阿里（223.5.5.5）" in text and "40 ms" in text
    assert tab.btn_start.isEnabled()


def test_dns_speed_tab_all_failed_says_so(qtbot):
    tab = DnsSpeedTab()
    qtbot.addWidget(tab)
    for server, _name in _DNS_SPEED_SERVERS:
        tab._server_done(server, None)
    tab._finished()
    assert "全部 DNS 均解析失败" in tab.lbl_recommend.text()


def test_dns_speed_start_empty_domain_aborts(qtbot):
    tab = DnsSpeedTab()
    qtbot.addWidget(tab)
    tab.ed_domain.setText("   ")
    tab.start()
    assert tab._worker is None
    assert tab.btn_start.isEnabled()


def test_dns_speed_start_ignores_reentry(qtbot, monkeypatch):
    started = []
    monkeypatch.setattr(_DnsSpeedWorker, "start", lambda self: started.append(self))
    tab = DnsSpeedTab()
    qtbot.addWidget(tab)
    tab.ed_domain.setText("example.com")
    tab.start()
    first = tab._worker
    assert not tab.btn_start.isEnabled()
    tab.start()                                  # 测速中再点：不起第二个线程
    assert tab._worker is first
    assert len(started) == 1


def test_dns_speed_count_frozen_while_running(qtbot, monkeypatch):
    """跑完前改「次数」档位，会让全失败那行的分母变成一次都没跑过的次数。"""
    monkeypatch.setattr(_DnsSpeedWorker, "start", lambda self: None)
    tab = DnsSpeedTab()
    qtbot.addWidget(tab)
    tab.ed_domain.setText("example.com")
    tab.spin_times.setValue(5)
    tab.start()
    assert tab._times == 5 and tab._worker._times == 5       # 交给线程的是开跑时的档位
    assert tab.spin_times.isEnabled() is False               # 控件同步锁住
    tab.spin_times.setValue(1)                               # 编程路径硬改，模拟绕过
    tab._server_done("8.8.8.8", None)
    assert tab.tbl.item(0, 2).text() == "0/5"                # 分母仍是本次实际次数
    tab._finished()
    assert tab.spin_times.isEnabled() is True


def test_toolbox_dns_speed_tab_index_frozen(qtbot):
    """DNS 测速钉在索引 6：ping/dns 命令面板跳转与既有测试依赖前 7 个 Tab 的顺序。
    （后来追加的「应用联网」只能排在它后面，所以这里断言索引而不是「末位」。）"""
    page = ToolboxPage()
    qtbot.addWidget(page)
    labels = [page.tabs.tabText(i) for i in range(page.tabs.count())]
    assert labels[6] == "DNS 测速"
    assert labels[4] == "ping / 路由"


# ---------------- 导出诊断报告 ----------------

def test_export_report_writes_utf8_file(main_win, tmp_path, monkeypatch):
    win = main_win
    out = tmp_path / "SysKit诊断报告.txt"
    picked = []

    def _save(*a, **k):
        picked.append(a)
        return str(out), "文本文件 (*.txt)"

    monkeypatch.setattr(QFileDialog, "getSaveFileName", staticmethod(_save))
    win._export_report()

    assert len(picked) == 1
    text = out.read_text(encoding="utf-8")                 # 中文不乱码
    assert "SysKit 诊断报告" in text and "204探测:OK" in text
    assert "结论：" in text
    assert "报告已导出" in win.statusBar().currentMessage()


def test_export_report_exported_file_matches_copy_to_clipboard(main_win, tmp_path,
                                                               monkeypatch):
    """复制与导出共用 _report_lines：同一份结果不得给出两份不同报告。"""
    from PySide6.QtWidgets import QApplication

    win = main_win
    out = tmp_path / "r.txt"
    monkeypatch.setattr(QFileDialog, "getSaveFileName",
                        staticmethod(lambda *a, **k: (str(out), "")))
    win._export_report()
    win._copy_report()
    clip = QApplication.clipboard().text()
    assert out.read_text(encoding="utf-8").strip() == clip.strip()


def test_export_report_aborted_result_not_judged(main_win, tmp_path, monkeypatch):
    """中止残片导出时必须明示「已中止」，不得让 verdict 误判成「外网不可达」。"""
    win = main_win
    win._diag_results[:] = [CheckResult("diag_aborted", "诊断中止", "skip", "用户中止")]
    out = tmp_path / "r2.txt"
    monkeypatch.setattr(QFileDialog, "getSaveFileName",
                        staticmethod(lambda *a, **k: (str(out), "")))
    win._export_report()
    text = out.read_text(encoding="utf-8")
    assert "诊断已中止" in text and "外网不可达" not in text


def test_export_report_guards_empty_and_cancel(main_win, tmp_path, monkeypatch):
    win = main_win
    win._diag_results.clear()
    called = []
    monkeypatch.setattr(QFileDialog, "getSaveFileName",
                        staticmethod(lambda *a, **k: called.append(1) or ("", "")))
    win._export_report()
    assert called == []                                   # 无结果时不弹保存框
    assert "暂无诊断结果" in win.statusBar().currentMessage()

    win._diag_results[:] = [CheckResult("real_conn", "真实连通性探测", "pass", "OK")]
    win._export_report()                                  # 用户取消
    assert called == [1]
    assert list(tmp_path.glob("*.txt")) == []             # 取消不落地文件


def test_report_refused_while_diagnosis_running(main_win, tmp_path, monkeypatch):
    """静默重诊（遥测/托盘）会清空 _diag_results 再逐项回填且不禁用按钮：
    中途取到的是缺项集合，verdict 会照着写出「结论」——必须拒绝出报告而不是照抄。"""
    win = main_win
    win._diag_worker = object()                          # 诊断进行中的哨兵
    called = []
    monkeypatch.setattr(QFileDialog, "getSaveFileName",
                        staticmethod(lambda *a, **k: called.append(1) or ("", "")))
    win._export_report()
    win._copy_report()
    assert called == []                                   # 不弹保存框
    assert "诊断进行中" in win.statusBar().currentMessage()
    assert list(tmp_path.glob("*.txt")) == []
    assert win._report_lines() is None


def test_export_report_write_failure_warns_without_crash(main_win, monkeypatch):
    win = main_win
    warnings = []
    monkeypatch.setattr(QMessageBox, "warning",
                        staticmethod(lambda *a, **k: warnings.append(a)))
    monkeypatch.setattr(QFileDialog, "getSaveFileName",
                        staticmethod(lambda *a, **k: ("Z:\\不存在\\路径.txt", "")))
    win._export_report()
    assert len(warnings) == 1
    assert "导出失败" in warnings[0][2]
