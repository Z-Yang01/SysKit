"""工具箱「应用联网」Tab 的 UI 测试：按钮 / 表格 / 后台线程，命令层走有状态假防火墙。

不打桩 syskit.appnet 的函数，只打桩它底下的 proc.run / proc.run_ps 与 admin.is_admin
——这样「点击 → QThread → 组 netsh 命令 → 假防火墙真的改状态 → 回渲染」这条链路是真的，
只有操作系统是假的。假防火墙见 tests/appnet_fake.py：它让"写完回查""规则被外部删除"
这类语义真正被 exercised，而不是靠测试作者随手写的成功桩。
conftest 红线 1 保证：任何漏 mock 的真实命令调用会直接 AssertionError，
所以「构造期不发命令」这条性质由 AppNetTab() 能被安全构造本身来证明。
"""
import os

import pytest
from PySide6.QtCore import Qt
from PySide6.QtWidgets import QFileDialog

from appnet_fake import Recorder
from syskit import appnet
from syskit.ui.toolbox_page import AppNetTab, ToolboxPage, _appnet_state_text

PROG = r"C:\Program Files\Reader\阅读.exe"
PROG2 = r"C:\Tools\Notes\notes.exe"


@pytest.fixture
def rec(monkeypatch):
    r = Recorder()
    r.patch(monkeypatch, isfile=True)          # 假 exe 路径不必真的存在
    return r


@pytest.fixture
def ledger(monkeypatch):
    """把台账固定成一条记录（写盘也拦下来，避免用例间互相污染）。"""
    state = {"entries": []}

    def _load(name, default):
        return {"entries": [dict(e) for e in state["entries"]]}

    def _save(name, data):
        state["entries"] = [dict(e) for e in data["entries"]]
        return True

    monkeypatch.setattr(appnet.stores, "load_json", _load)
    monkeypatch.setattr(appnet.stores, "save_json", _save)
    return state


def _entry(program=PROG, blocked=True):
    return {"program": program, "rule": appnet.rule_name(program),
            "blocked": blocked, "added": "2026-09-19 10:00"}


@pytest.fixture
def tab(qtbot, rec):
    t = AppNetTab()
    qtbot.addWidget(t)
    return t


def _seed_firewall(rec, program=PROG, enabled=True, action="Block"):
    """在假防火墙上放一条 SysKit 规则，让"防火墙里的实际状态"有真值可查。"""
    rec.fw.seed(appnet.rule_name(program), program=program,
                enabled=enabled, action=action)


def _wait(qtbot, tab):
    qtbot.wait_until(lambda: tab._worker is None, timeout=10000)


# ---------------- 状态文案（纯函数） ----------------

def test_state_text_covers_every_firewall_state():
    key = appnet.norm_path(PROG)
    e = _entry(blocked=True)
    assert "未校验" in _appnet_state_text(e, None)               # 没查过
    assert "未校验" in _appnet_state_text(e, {})                 # 快照里没有这一格
    assert "外部删除" in _appnet_state_text(e, {key: "missing"})
    assert "实际不拦" in _appnet_state_text(e, {key: "drift"})    # 动作被改成允许
    assert _appnet_state_text(e, {key: "on"}) == "已禁止联网"
    assert "不一致" in _appnet_state_text(e, {key: "off"})        # 台账说禁止、规则却停用
    assert _appnet_state_text(_entry(blocked=False), {key: "off"}) == "已放行"
    assert "不一致" in _appnet_state_text(_entry(blocked=False), {key: "on"})


def test_state_text_hits_even_when_ledger_spelling_differs():
    """快照按 norm_path 做键：台账里大小写/斜杠不同也不能退化成"查不到"。"""
    got = _appnet_state_text({"program": PROG.upper(), "blocked": True},
                             {appnet.norm_path(PROG): "on"})
    assert got == "已禁止联网"


# ---------------- 构造与门禁 ----------------

def test_construct_reads_ledger_without_touching_firewall(tab, rec, ledger):
    ledger["entries"] = [_entry()]
    tab._load_ledger()
    assert tab.tbl.rowCount() == 1
    assert tab._states is None
    assert tab._worker is None
    assert rec.texts == []                                        # 零命令
    assert "未校验" in tab.tbl.item(0, 2).text()


def test_non_admin_disables_mutating_buttons(qtbot, monkeypatch):
    monkeypatch.setattr(appnet.admin, "is_admin", lambda: False)
    t = AppNetTab()
    qtbot.addWidget(t)
    assert t.btn_add.isEnabled() is False
    assert t.btn_refresh.isEnabled() is True                      # 只读查询不需要管理员
    assert "非管理员" in t.lbl_info.text()


# ---------------- 刷新 ----------------

def test_refresh_populates_table_from_firewall(qtbot, tab, rec, ledger):
    ledger["entries"] = [_entry()]
    _seed_firewall(rec)
    tab.refresh()
    _wait(qtbot, tab)
    assert tab.tbl.rowCount() == 1
    assert tab.tbl.item(0, 2).text() == "已禁止联网"
    assert tab.tbl.item(0, 0).data(Qt.UserRole) == PROG
    assert "✔" in tab.lbl_status.text()
    assert any("Get-NetFirewallRule" in t for t in rec.texts)


def test_refresh_shows_rule_deleted_outside_the_app(qtbot, tab, rec, ledger):
    ledger["entries"] = [_entry()]
    tab.refresh()                                                  # 防火墙里没有这条规则
    _wait(qtbot, tab)
    assert "外部删除" in tab.tbl.item(0, 2).text()
    assert "✔" in tab.lbl_status.text()                           # 查到了"没有"，不是查询失败


def test_refresh_degrades_honestly_when_powershell_unavailable(qtbot, tab, rec, ledger):
    ledger["entries"] = [_entry()]
    rec.fw.ps_available = False
    tab.refresh()
    _wait(qtbot, tab)
    assert "未校验" in tab.tbl.item(0, 2).text()                  # 查不动 ≠ 规则被删
    assert "查询失败" in tab.lbl_status.text()


# ---------------- 增 / 改 / 删 ----------------

def test_add_program_via_dialog_creates_block_rule(qtbot, tab, rec, ledger, monkeypatch):
    monkeypatch.setattr(QFileDialog, "getOpenFileName",
                        staticmethod(lambda *a, **k: (PROG, "程序 (*.exe)")))
    tab._pick_and_add()
    _wait(qtbot, tab)
    assert any("add rule" in t and "dir=out" in t and "action=block" in t
               for t in rec.texts)
    assert rec.fw.count(appnet.rule_name(PROG)) == 1              # 假防火墙里真的生效了
    assert tab.tbl.rowCount() == 1
    assert tab.tbl.item(0, 0).text() == os.path.basename(PROG)
    assert tab.tbl.item(0, 2).text() == "未校验（点「刷新状态」查询防火墙）"
    assert ledger["entries"][0]["blocked"] is True


def test_add_program_reports_validation_error(qtbot, tab, rec, ledger, monkeypatch):
    monkeypatch.setattr(appnet.os.path, "isfile", lambda p: False)
    monkeypatch.setattr(QFileDialog, "getOpenFileName",
                        staticmethod(lambda *a, **k: (r"C:\nope\x.exe", "")))
    tab._pick_and_add()
    _wait(qtbot, tab)
    assert rec.texts == []                                        # 校验不过就不该发命令
    assert "✘" in tab.lbl_status.text() and "不存在" in tab.lbl_status.text()
    assert ledger["entries"] == []


def test_cancelling_dialog_sends_nothing(tab, rec, monkeypatch):
    monkeypatch.setattr(QFileDialog, "getOpenFileName",
                        staticmethod(lambda *a, **k: ("", "")))
    tab._pick_and_add()
    assert tab._worker is None
    assert rec.texts == []


def test_allow_button_disables_existing_rule(qtbot, tab, rec, ledger):
    ledger["entries"] = [_entry()]
    _seed_firewall(rec)
    tab._load_ledger()
    tab.tbl.selectRow(0)
    assert tab.btn_allow.isEnabled() and tab.btn_del.isEnabled()
    tab.btn_allow.click()
    _wait(qtbot, tab)
    assert any("set rule" in t and "enable=no" in t for t in rec.texts)
    assert rec.fw.of(appnet.rule_name(PROG))["enabled"] is False
    assert rec.fw.count(appnet.rule_name(PROG)) == 1              # 只停用，不重复新建
    assert ledger["entries"][0]["blocked"] is False


def test_block_button_reenables_rule_and_keeps_one_rule(qtbot, tab, rec, ledger):
    """「禁止联网」按钮此前从未被点过——它才是最核心的那个动作。"""
    ledger["entries"] = [_entry(blocked=False)]
    _seed_firewall(rec, enabled=False)
    tab.refresh()
    _wait(qtbot, tab)
    assert tab.tbl.item(0, 2).text() == "已放行"
    tab.tbl.selectRow(0)
    tab.btn_block.click()
    _wait(qtbot, tab)
    assert any("set rule" in t and "enable=yes" in t for t in rec.texts)
    assert rec.fw.count(appnet.rule_name(PROG)) == 1
    assert "✔ 已禁止联网" in tab.lbl_status.text()
    assert ledger["entries"][0]["blocked"] is True


def test_successful_mutation_drops_only_that_rows_stale_state(qtbot, tab, rec, ledger):
    """回归：写操作后不作废快照，状态列就会拿改动前的实际状态比改动后的台账。

    曾表现为：状态栏 ✔ 已允许联网，同一行第三列却写「已禁止联网」甚至「不一致」。
    """
    ledger["entries"] = [_entry(PROG), _entry(PROG2, blocked=False)]
    _seed_firewall(rec)
    _seed_firewall(rec, program=PROG2, enabled=False)
    tab.refresh()
    _wait(qtbot, tab)
    assert tab.tbl.item(0, 2).text() == "已禁止联网"
    assert tab.tbl.item(1, 2).text() == "已放行"
    tab.tbl.selectRow(0)
    tab.btn_allow.click()
    _wait(qtbot, tab)
    assert tab.tbl.item(0, 2).text() == "未校验（点「刷新状态」查询防火墙）"
    assert tab.tbl.item(1, 2).text() == "已放行"                  # 没被动过的行信息保留
    assert set(tab._states) == {appnet.norm_path(PROG2)}


def test_failed_mutation_keeps_verified_state(qtbot, tab, rec, ledger):
    """操作失败时防火墙没变，快照仍然有效，不该被无端作废。"""
    ledger["entries"] = [_entry()]
    _seed_firewall(rec)
    tab.refresh()
    _wait(qtbot, tab)
    rec.fw.fail("set")
    tab.tbl.selectRow(0)
    tab.btn_allow.click()
    _wait(qtbot, tab)
    assert "✘" in tab.lbl_status.text()
    assert tab.tbl.item(0, 2).text() == "已禁止联网"


def test_drifted_rule_is_reported_as_not_blocking(qtbot, tab, rec, ledger):
    """用户在 wf.msc 把动作改成「允许」：Enabled 仍 True，但绝不能显示「已禁止联网」。"""
    ledger["entries"] = [_entry()]
    _seed_firewall(rec, action="Allow")
    tab.refresh()
    _wait(qtbot, tab)
    assert "实际不拦" in tab.tbl.item(0, 2).text()


def test_remove_deletes_rule_and_row(qtbot, tab, rec, ledger):
    ledger["entries"] = [_entry()]
    _seed_firewall(rec)
    tab._load_ledger()
    tab.tbl.selectRow(0)
    tab.btn_del.click()
    _wait(qtbot, tab)
    assert any("delete rule" in t for t in rec.texts)
    assert rec.fw.count(appnet.rule_name(PROG)) == 0
    assert tab.tbl.rowCount() == 0
    assert ledger["entries"] == []


def test_removing_middle_row_does_not_leave_buttons_on_the_neighbour(qtbot, tab, rec, ledger):
    """setRowCount 只裁末行：中间行被 pop 后 currentRow 会原地落到隔壁程序身上。

    不修的话，紧接着再点「禁止联网」作用在**另一个 exe** 上——比错显更糟。
    """
    ledger["entries"] = [_entry(PROG), _entry(PROG2),
                         _entry(r"C:\Tools\Third\third.exe")]
    tab._load_ledger()
    tab.tbl.setCurrentCell(1, 0)
    assert tab._selected_program() == PROG2
    _seed_firewall(rec, program=PROG2)
    tab.btn_del.click()
    _wait(qtbot, tab)
    assert tab.tbl.rowCount() == 2
    assert tab._selected_program() == ""                           # 不许指到 third.exe
    for b in (tab.btn_block, tab.btn_allow, tab.btn_del):
        assert b.isEnabled() is False


# ---------------- 后台线程的失败路径 ----------------

def test_worker_exception_still_reports_and_releases_buttons(qtbot, tab, rec, ledger,
                                                             monkeypatch):
    """job 抛异常时必须照发 done：否则 _worker 永不复位、四个按钮永久禁用。"""
    ledger["entries"] = [_entry()]
    tab._load_ledger()
    tab.tbl.selectRow(0)
    assert tab.btn_allow.isEnabled()

    def boom(*a, **k):
        raise RuntimeError("防火墙服务未响应")

    monkeypatch.setattr(appnet, "set_program_blocked", boom)
    tab.btn_allow.click()
    _wait(qtbot, tab)
    assert "✘" in tab.lbl_status.text() and "防火墙服务未响应" in tab.lbl_status.text()
    assert tab.btn_allow.isEnabled() is True        # 复位后才能再点


def test_second_request_is_ignored_while_worker_running(tab, rec, monkeypatch):
    """忙时再点「刷新」不得起第二个线程：两条命令交错会让回包与表格状态错位。"""
    sentinel = object()
    tab._worker = sentinel
    monkeypatch.setattr(appnet, "load_entries",
                        lambda: (_ for _ in ()).throw(AssertionError("不该被调用")))
    tab.refresh()
    tab._apply(True)
    tab._remove()
    assert tab._worker is sentinel
    assert rec.texts == []


# ---------------- 页面接线 ----------------

def test_toolbox_page_hosts_appnet_tab_last(qtbot):
    page = ToolboxPage()
    qtbot.addWidget(page)
    tabs = page.tabs
    assert tabs.tabText(tabs.count() - 1) == "应用联网"
    assert isinstance(tabs.widget(tabs.count() - 1), AppNetTab)
