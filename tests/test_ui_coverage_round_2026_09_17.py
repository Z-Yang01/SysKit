"""UI 层缺口收口第二轮（2026-09-17，Task #001：Qt UI 覆盖率 >=99%）。

针对 scoped 报告残余缺口，只补分支不改业务代码：
- main_window.py  toggle_palette 三岔 / run_palette_command 全路径（空值/URL/
                  确认取消/执行成功/执行异常）/ _save_window_state 异常吞并 /
                  _diag_done 中止分支 / _save_settings 四条错误路径（热键非法、
                  开机自启异常、设置写盘失败、热键被占回退与原键告警）/
                  _reload_log 无 txt_log 守卫 / _repair_done 代理刷新异常 /
                  run_combo 单项确认拒绝 continue / _combo_done 中止分支 /
                  quit_app 三道扫尾（mon.join RuntimeError、可 stop 线程、
                  四 worker 显式 wait）。
- vault_page.py   自动锁 0 分钟、网站行同步后补选、账号表陈旧代理行 continue、
                  补选恢复、单元格点击守卫与标签复制、_add_account 单次 exec
                  （疑似 bug #11 修复回归）、_require_master_verified 全分支、
                  TOTP 弹窗三守卫 +
                  qrcode 缺失降级、锁定态备注读取、显示密码未验证早退。
- toolbox_page.py SpeedTestWorker 流提前结束 / 读取异常两条失败路、
                  _LanScanWorker arp 失败、PingRouteTab 显隐恢复持续 ping、
                  PortTab 显隐自动刷新与缺单元格早退、HostsTab 增删改存全流程
                  （注入临时 hosts 文件，写盘只落 pytest 临时目录）。
- system_page.py  _BatteryWorker.run_ps 异常兜底、BatteryTab 查询防重入、
                  剪贴板过期清理/刷盘异常吞并、未监听时读剪贴板异常。
- charts.py:16   论证为不可达死代码（见文件尾注释），不强凑。

安全（叠加 conftest 哨兵与 test_ui_smoke.ui_offline_mocks）：进程/注册表写/
socket/urlopen 全部显式 mock；QMessageBox 静态弹窗 no-op；QDialog.exec /
QInputDialog.getText 注入假件；后台 worker 一律同步 run() 或注入同步假实现，
唯一真实线程是 quit_app 用例中带 stop() 的短命 _Stoppy（测试内 wait 收尾）。
MainWindow 数量控制在 3 个（离屏多窗口收尾已知坑，与 smoke/units 规模对齐）。

已知不可达（防御分支不强凑，论证代替覆盖）：
- charts.py:16  human_bytes 循环对 unit=="TB" 恒返回（`or unit == "TB"` 恒真），
  其后的 `return f"{v:.1f}TB"` 永不可达。
- vault_page.py:339  _sync_acct_table 的 `if row is None: continue`：代理行
  UserRole 直接取自同一份 acct_model._rows，二者自洽，正常运行无陈旧行可注入。
"""
import base64
import os
import sys
import time
import types

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

import pytest
from PySide6.QtCore import Qt, QThread, Signal, qInstallMessageHandler
from PySide6.QtGui import QDesktopServices
from PySide6.QtWidgets import (QApplication, QDialog, QListWidgetItem, QMessageBox,
                               QSystemTrayIcon, QTableWidgetItem)


def _qt_message_silent(msg_type, context, message):
    """纯 no-op 的 Qt 消息处理器（解释器收尾阶段不再做文件 I/O，防原生 fail-fast）。"""


qInstallMessageHandler(_qt_message_silent)
# offscreen 无托盘实现：气泡 showMessage 统一 no-op（幂等，smoke 已设同款）
QSystemTrayIcon.showMessage = lambda self, *a, **k: None

# 复用 smoke 的离线 autouse 夹具与 MainWindow 工厂夹具（含后台线程回收）
from test_ui_smoke import main_win, make_main_window, ui_offline_mocks  # noqa: E402,F401
# 复用 vault 缺口文件的夹具与假件（seeded 系列走隔离临时目录 + 小 KDF 迭代）
from test_vault_page_gaps import (  # noqa: E402,F401
    _FakeQClipboard, _page, _patch_ask, _select, fast_kdf, seeded, seeded_master)

RFC_SECRET = base64.b32encode(b"12345678901234567890").decode("ascii")

_KEEP = []   # 保活本文件创建的 QThread，防提前析构（与 test_ui_pages_units 同款）


@pytest.fixture(autouse=True)
def _keepalive_cleanup():
    yield
    _KEEP.clear()


@pytest.fixture(autouse=True)
def msgbox_recorder(msgbox_noop, ui_offline_mocks, monkeypatch):
    """弹窗记录器：显式依赖 conftest 的 msgbox_noop 与 smoke 的 ui_offline_mocks
    两个 autouse 夹具，保证本记录器最后生效（前两者会把弹窗方法覆盖成无记录
    版本，覆盖顺序在不同收集顺序下不稳定）。返回 calls 供断言；question 默认
    Yes（与 conftest 语义一致）。"""
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


# ---------------- 同步假 worker（不启动原生线程） ----------------


class _FakeDiagnosisWorker(QThread):
    progress = Signal(object)
    finished_ok = Signal(list)
    auto_run = False                       # True: start() 即同步跑完

    def __init__(self, quick=False, parent=None):
        super().__init__(parent)
        self.quick = quick
        _KEEP.append(self)

    def start(self):
        if self.auto_run:
            self.run()

    def run(self):
        from syskit import netcheck
        try:
            results = netcheck.run_all(
                progress=lambda r: self.progress.emit(r), quick=self.quick)
            self.finished_ok.emit(results)
        except Exception:
            self.finished_ok.emit([])


class _FakeSnapshotWorker(QThread):
    got = Signal(object)

    def __init__(self, parent=None):
        super().__init__(parent)
        _KEEP.append(self)

    def start(self):
        self.run()

    def run(self):
        from syskit.snapshot import take
        self.got.emit(take())


def _patch_no_thread_workers(monkeypatch):
    from syskit.ui import main_window as mw
    monkeypatch.setattr(mw, "DiagnosisWorker", _FakeDiagnosisWorker)
    monkeypatch.setattr(mw, "SnapshotWorker", _FakeSnapshotWorker)


# ============================================================
# syskit/ui/main_window.py
# ============================================================


def test_mw_palette_toggle_and_run_command(main_win, qtbot, monkeypatch, msgbox_recorder):
    """命令面板：创建/toggle/销毁后吞 RuntimeError；面板命令五条路径。"""
    win = main_win

    # ---- toggle_palette（179-184）
    assert win._palette is None
    win.toggle_palette()                       # 首次创建 + popup
    assert win._palette is not None
    win.toggle_palette()                       # 已可见 → hide
    monkeypatch.setattr(win._palette, "toggle",
                        lambda: (_ for _ in ()).throw(RuntimeError("destroyed")))
    win.toggle_palette()                       # RuntimeError 被吞

    # ---- run_palette_command（188-205）
    opened = []
    monkeypatch.setattr(QDesktopServices, "openUrl",
                        lambda url: opened.append(url.toString()))
    win.run_palette_command("   ")             # 空值 → 早退
    assert opened == []

    win.run_palette_command("https://example.com/qr")   # URL 直开，不弹确认
    assert opened == ["https://example.com/qr"]

    ret = {"v": QMessageBox.StandardButton.No}
    questions = []

    def _question(*a, **k):
        questions.append(a[2] if len(a) > 2 else "")   # 本桩覆盖记录器，须自记
        return ret["v"]

    monkeypatch.setattr(QMessageBox, "question", staticmethod(_question))
    from syskit import proc
    win.run_palette_command("echo hi")         # 确认取消 → 不执行
    assert opened == ["https://example.com/qr"]

    ret["v"] = QMessageBox.StandardButton.Yes
    monkeypatch.setattr(proc, "run",
                        lambda cmd, timeout=30: proc.CmdResult(code=0, stdout="  ok-out ", stderr=""))
    win.run_palette_command("echo hi")         # 执行成功 → 托盘气泡「完成: …」（后台线程）
    qtbot.wait_until(lambda: win._cmd_worker is None)
    assert questions and "echo hi" in questions[-1]  # question 弹窗内容含命令

    def _boom_run(cmd, timeout=30):
        raise RuntimeError("no proc")

    monkeypatch.setattr(proc, "run", _boom_run)
    win.run_palette_command("echo hi")         # 执行异常 → 失败气泡，不抛
    qtbot.wait_until(lambda: win._cmd_worker is None)


def test_mw_diag_aborted_and_save_window_state_error(main_win, monkeypatch):
    """诊断中止分支 + 窗口状态保存异常吞并。"""
    from syskit import settings as settings_mod
    from syskit.netcheck import CheckResult

    win = main_win
    win._release_worker("_diag_worker")
    win._set_busy(True, "诊断中…")             # 模拟完整诊断进行中
    aborted = [CheckResult("diag_aborted", "诊断中止", "skip", "用户中止")]
    win._diag_done(aborted, silent=False)      # 中止结果不得用于判定网络状态
    assert "诊断已中止" in win.lbl_verdict.text()
    assert win.statusBar().currentMessage().startswith("诊断已中止")
    assert win.pb_diag.isHidden()
    assert win._busy is False                  # 中止早退也必须复位忙碌态（否则全按钮闸死）
    assert win.btn_full.isEnabled() and win.btn_diag.isEnabled()

    def _boom_save(patch):
        raise OSError("disk full")

    monkeypatch.setattr(settings_mod, "save", _boom_save)
    win._save_window_state()                   # 异常被吞（262-263），不抛


def test_mw_save_settings_guard_paths(make_main_window, monkeypatch, msgbox_recorder):
    """_save_settings 四条错误路径：热键非法 / 自启异常 / 写盘失败 / 热键被占。"""
    from syskit import admin, autostart, hotkey, settings as settings_mod
    from syskit.ui import palette as palette_mod

    win = make_main_window()
    fallback = settings_mod.load().get("palette_hotkey", "Ctrl+Alt+K")

    # (a) 热键无法解析 → 警告 + 本次保存回退已存热键（890-893）
    win.cmb_hotkey.setCurrentText("不是热键")
    n0 = len(msgbox_recorder)
    win._save_settings()
    assert any("无法识别" in a[2] for _k, a in msgbox_recorder[n0:])
    assert settings_mod.load()["palette_hotkey"] == fallback   # 未被垃圾值污染

    # (b) 管理员 + autostart.enabled 抛异常 → 警告，其余设置继续保存（918-920）
    monkeypatch.setattr(admin, "is_admin", lambda: True)
    monkeypatch.setattr(autostart, "enabled",
                        lambda: (_ for _ in ()).throw(RuntimeError("wmi gone")))
    win.chk_start.setChecked(True)
    n1 = len(msgbox_recorder)
    win._save_settings()
    assert any("开机自启设置失败" in a[2] for _k, a in msgbox_recorder[n1:])

    # (c) settings.save 抛 OSError → 警告 + return（923-926）
    monkeypatch.setattr(admin, "is_admin", lambda: False)
    win.cmb_hotkey.setCurrentText(fallback)    # 恢复合法热键：本步只有写盘失败一条弹窗
    with monkeypatch.context() as m:
        m.setattr(settings_mod, "save",
                  lambda patch: (_ for _ in ()).throw(OSError("disk full")))
        n2 = len(msgbox_recorder)
        win._save_settings()
        assert any("设置写入失败" in a[2] for _k, a in msgbox_recorder[n2:])
        assert len(msgbox_recorder) == n2 + 1      # return 早退：无「设置已保存」提示

    # (d) 新热键被占用 → 回退旧键并提示（932-938）
    new_key = "Ctrl+Alt+Space"
    assert hotkey.parse_hotkey(new_key) is not None
    monkeypatch.setattr(palette_mod, "setup_global_hotkey", lambda w, hotkey_id=1: False)
    monkeypatch.setattr(palette_mod, "teardown_global_hotkey", lambda w, hotkey_id=1: None)
    win.cmb_hotkey.setCurrentText(new_key)
    n3 = len(msgbox_recorder)
    win._save_settings()
    assert win.cmb_hotkey.currentText() == fallback    # 下拉框已回退旧键
    assert settings_mod.load()["palette_hotkey"] == fallback
    assert any("已被其他程序占用" in a[2] for _k, a in msgbox_recorder[n3:])

    # (e) 原键注册失败（新旧相同）→ 明确警告不静默（939-940）
    win.cmb_hotkey.setCurrentText(fallback)
    n4 = len(msgbox_recorder)
    win._save_settings()
    assert any("注册失败" in a[2] for _k, a in msgbox_recorder[n4:])


def test_mw_combo_confirm_continue_and_cancelled_done(make_main_window, monkeypatch,
                                                      qtbot, msgbox_recorder):
    """run_combo 单项确认拒绝 → continue；_combo_done 中止分支文案。"""
    from syskit import admin, repairs
    from syskit.repairs import RepairResult
    from syskit.ui import main_window as mw
    from syskit.ui.confirm_dialog import ConfirmDialog

    win = make_main_window()
    _patch_no_thread_workers(monkeypatch)
    monkeypatch.setattr(_FakeDiagnosisWorker, "auto_run", True)
    monkeypatch.setattr(admin, "is_admin", lambda: True)

    # ---- 单项确认拒绝 → continue（1059）：拒绝 1 项、接受 1 项
    monkeypatch.setattr(repairs, "COMBOS",
                        dict(repairs.COMBOS, mix2=("混合2", ["dns_auto", "dns_auto"])))
    retcodes = iter([0, 1])                    # Rejected, Accepted
    monkeypatch.setattr(ConfirmDialog, "exec", lambda self, *a, **k: next(retcodes))
    monkeypatch.setattr(QMessageBox, "exec",
                        lambda self, *a, **k: QMessageBox.StandardButton.Cancel)
    win.run_combo("mix2")                      # 最终确认取消 → 不执行
    assert win._combo_worker is None and win._busy is False

    # ---- _combo_done：worker.cancelled → 「组合修复已中止」文案（1099-1105）
    win._combo_results = [("flush_dns", RepairResult(True, "已刷新"))]
    win._combo_worker = types.SimpleNamespace(
        cancelled=True, action_ids=["flush_dns", "dns_auto"],
        wait=lambda ms: None)
    win._combo_done()
    assert win._combo_worker is None
    assert msgbox_recorder[-1][1][1] == "组合修复已中止"
    assert "剩余约 1 项未执行" in msgbox_recorder[-1][1][2]


def test_mw_repair_done_proxy_refresh_failure(monkeypatch, msgbox_recorder):
    """_repair_done：修复后刷新代理状态抛异常 → 记日志不中断（1032-1036）。

    用「假 self」直调真实方法体，不再新造 MainWindow（离屏收尾已知坑）。
    """
    from syskit.repairs import RepairResult
    from syskit.ui.main_window import MainWindow

    class _FakeWin:
        def __init__(self):
            self._repair_worker = None
            self.busy = []
            self.records = []
            self.status_msgs = []
            self.diag_calls = []
            self.proxy_calls = 0

        def _release_worker(self, attr):
            setattr(self, attr, None)

        def _set_busy(self, busy, text=""):
            self.busy.append(busy)

        def statusBar(self):
            return types.SimpleNamespace(
                showMessage=lambda *a: self.status_msgs.append(a))

        def _add_repair_record(self, aid, result):
            self.records.append((aid, result.success))

        def _update_mac_state(self):
            pass

        def _refresh_proxy_label(self):
            self.proxy_calls += 1
            raise RuntimeError("proxy registry gone")

        def run_diagnosis(self, **kw):
            self.diag_calls.append(kw)

    obj = _FakeWin()
    MainWindow._repair_done(obj, ("flush_dns", RepairResult(True, "已刷新 DNS 缓存")))
    assert obj.records == [("flush_dns", True)]
    assert obj.proxy_calls == 1                # 刷新已被调用
    assert obj.busy and obj.busy[-1] is False  # 异常后仍恢复忙碌态
    assert obj.diag_calls and obj.diag_calls[0].get("quick") is True  # 自动复诊照常


def test_mw_reload_log_txt_log_guard():
    """_reload_log 无 txt_log 属性时守卫早退（984，构造期防御分支）。"""
    from syskit.ui.main_window import MainWindow
    MainWindow._reload_log(types.SimpleNamespace())    # 不抛即通过


def test_mw_quit_app_full_sweep(make_main_window, monkeypatch, qtbot):
    """quit_app 三道扫尾：mon.join RuntimeError 吞并 / 可 stop 线程先停再等 /
    四 worker 显式 wait 复位。本文件最后一个窗口用例（退出后不再建窗）。"""
    from syskit.ui.main_window import MainWindow

    win = make_main_window()

    class _Stoppy(QThread):
        """带 callable stop 的短命线程：模拟 tracert 类可中止 worker。"""

        def __init__(self, parent=None):
            super().__init__(parent)
            self.stopped = 0

        def stop(self):
            self.stopped += 1

        def run(self):
            for _ in range(100):
                if self.stopped:
                    break
                time.sleep(0.02)

    stoppy = _Stoppy(win)                      # 挂到主窗口：全树扫尾可枚举
    stoppy.start()
    assert stoppy.isRunning()

    class _FakeWorker:                         # 非 QThread：树扫尾扫不到 → 走双保险
        def isRunning(self):
            return True

        def wait(self, ms):
            return True

    win._snap_worker = _FakeWorker()

    def _boom_join(*a, **k):
        raise RuntimeError("Thread._stop shadowed")

    monkeypatch.setattr(win.mon, "join", _boom_join)
    monkeypatch.setattr(win.tele.traffic, "save_if_dirty", lambda: None)
    win.quit_app()

    assert stoppy.stopped >= 1                 # 树扫尾已调用 stop()
    stoppy.wait(3000)
    assert not stoppy.isRunning()
    assert win._snap_worker is None            # 四 worker 双保险 wait 后复位


# ============================================================
# syskit/ui/vault_page.py
# ============================================================


def test_vault_autolock_zero_and_site_row_recovery(qtbot, seeded, monkeypatch):
    """自动锁 0 分钟不启动；网站列表同步后仍无选中 → 补选第 0 行（297-298）。"""
    from syskit import settings as settings_mod

    page = _page(qtbot)
    assert page.crypto.is_unlocked
    settings_mod.save({"vault_autolock_min": 0})
    page._restart_autolock()
    assert not page._autolock.isActive()       # 0 = 永不自动锁
    settings_mod.save({"vault_autolock_min": 10})

    # 注入「只填不选」的同步实现：模拟列表有行但无选中的异常状态
    def fill_only(lst, proxy, keep_id=None):
        lst.blockSignals(True)
        lst.clear()
        for i in range(proxy.rowCount()):
            idx = proxy.index(i, 0)
            it = QListWidgetItem(idx.data(Qt.DisplayRole))
            it.setData(Qt.UserRole, idx.data(Qt.UserRole))
            lst.addItem(it)
        lst.blockSignals(False)

    monkeypatch.setattr(page, "_sync_list", fill_only)
    page.lst_sites.setCurrentRow(-1)
    page.refresh_sites()
    assert page.lst_sites.count() == 2
    assert page.lst_sites.currentRow() == 0    # 兜底补选已生效


def test_vault_acct_table_stale_proxy_row_and_selection_recovery(qtbot, seeded,
                                                                 monkeypatch):
    """账号表：陈旧代理行 continue 跳过；同步后无选中 → 补选第 0 行（413-414）。"""
    page = _page(qtbot)
    _select(page, page.lst_sites, "GitHub")
    page.refresh_accounts()
    assert page.tbl_accts.rowCount() == 2              # alice / bob

    # 谎报代理行数 +1：多出的行是无效索引 → row=None → continue（339 注入验证）
    orig_rows = page.acct_proxy.rowCount
    monkeypatch.setattr(page.acct_proxy, "rowCount",
                        lambda parent=None: orig_rows() + 1)
    page.refresh_accounts()
    assert page.tbl_accts.rowCount() == 2              # 无效行被跳过、正常行不受影响
    monkeypatch.setattr(page.acct_proxy, "rowCount", orig_rows)   # 还原原绑定方法

    # 注入「只填不选」的表格同步：模拟同步后无选中 → refresh_accounts 兜底补选
    def fill_table_only(keep_id=None):
        t = page.tbl_accts
        t.blockSignals(True)
        t.setRowCount(0)
        for row in page.acct_model._rows:
            r = t.rowCount()
            t.insertRow(r)
            c0 = QTableWidgetItem(row.get("username") or row.get("label") or "?")
            c0.setData(Qt.UserRole, row["id"])
            t.setItem(r, 0, c0)
        t.blockSignals(False)

    monkeypatch.setattr(page, "_sync_acct_table", fill_table_only)
    page.tbl_accts.setCurrentCell(-1, -1)
    page.refresh_accounts()
    assert page.tbl_accts.rowCount() == 2
    assert page.tbl_accts.currentRow() == 0    # 兜底补选已生效（414）


def test_vault_cell_click_guards_and_label_copy(qtbot, seeded, monkeypatch):
    """单元格点击：行不匹配 / 无账号 id 早退；标签列复制（含无标签不复制）。"""
    page = _page(qtbot)
    _select(page, page.lst_sites, "GitHub")
    page.refresh_accounts()
    fake = _FakeQClipboard()
    monkeypatch.setattr(page, "_clip", lambda: fake)

    page.tbl_accts.setCurrentCell(0, 0)
    page._on_acct_cell_clicked(5, 0)           # 行号与当前行不符 → 早退
    assert fake.text_val == ""

    page.tbl_accts.setCurrentCell(-1, -1)
    page._on_acct_cell_clicked(-1, 0)          # 匹配但无有效账号 id → 早退
    assert fake.text_val == ""

    row = _select(page, page.tbl_accts, "alice")
    page._on_acct_cell_clicked(row, 2)         # 标签列：work → 复制
    assert fake.text_val == "work"
    assert "标签已复制" in page.lbl_detail.text()

    bob_row = _select(page, page.tbl_accts, "bob")
    page._on_acct_cell_clicked(bob_row, 2)     # bob 无标签 → 不复制
    assert fake.text_val == "work"


def test_vault_add_account_single_exec_accepted(qtbot, seeded, monkeypatch):
    """FIX（疑似 bug #11）：_add_account 只允许一次模态 exec（557-567）。

    修复前 565-567 行对同一对话框二次 exec()：首次接受保存后对话框再次弹出，
    二次取消会静默丢弃用户输入。现断言仅发起一次 exec 且接受后正常入库。
    """
    import syskit.ui.vault_page as vp_mod
    from syskit import vault

    page = _page(qtbot)
    _select(page, page.lst_sites, "GitHub")
    gh = vault.search_sites("GitHub")[0]
    before = vault.list_accounts(gh["id"])

    real = vp_mod._AccountDialog
    calls = {"n": 0}

    def factory(parent=None, edit=False, account=None):
        d = real(parent, edit=edit, account=account)

        def exec2():
            calls["n"] += 1
            return 1  # 用户点「保存」接受
        monkeypatch.setattr(d, "exec", exec2)
        monkeypatch.setattr(d, "fields", lambda: {"label": "new", "username": "u",
                                                  "password": "p@ss", "totp_secret": "",
                                                  "notes": ""})
        return d

    monkeypatch.setattr(vp_mod, "_AccountDialog", factory)
    page._add_account()
    assert calls["n"] == 1                             # 只弹一次（修复前为 2）
    assert len(vault.list_accounts(gh["id"])) == len(before) + 1  # 接受 → 入库


def test_vault_require_master_verified_branches(qtbot, seeded_master, monkeypatch,
                                                msgbox_recorder):
    """主密码安全门全分支：窗口内重解锁 / 解锁错误与成功 / 验证取消。"""
    page = _page(qtbot)
    assert page.crypto.has_master_password() and not page.crypto.is_unlocked

    # ---- 窗口内但已锁定 → 静默重解锁（654-655）
    page._master_until = time.time() + 600
    asked = _patch_ask(monkeypatch, [("pw-master", True)])
    assert page._require_master_verified() is True
    assert page.crypto.is_unlocked
    assert asked == ["解锁密码库"]

    # ---- 窗口外 + 锁定：错误密码警告 / 正确密码开信任窗（662-667）
    page.crypto.lock()
    page._master_until = 0.0
    _patch_ask(monkeypatch, [("bad", True)])
    assert page._require_master_verified() is False
    assert "主密码错误" in msgbox_recorder[-1][1][2]

    _patch_ask(monkeypatch, [("pw-master", True)])
    assert page._require_master_verified() is True
    assert page._master_until > time.time()    # _grant_trust 已开免验证窗

    # ---- 窗口外 + 已解锁：验证弹窗取消 → False（668-670）
    page._master_until = 0.0
    _patch_ask(monkeypatch, [("", False)])
    assert page._require_master_verified() is False


def test_vault_current_secrets_blank_when_locked(qtbot, seeded_master):
    """锁定态读取 TOTP/备注 → 解密抛 VaultLockedError → 空串兜底（537-546）。

    notes 必须非空（NULL 在 get_secret 内提前返回空串，走不到解密）。"""
    from syskit import vault

    crypto = seeded_master
    assert crypto.unlock("pw-master")
    gh = vault.search_sites("GitHub")[0]["id"]
    aid = vault.add_account(gh, "pw-note", crypto, username="noter",
                            totp_secret=RFC_SECRET, notes="加密备注")
    crypto.lock()
    page = _page(qtbot)                        # 同一 key 文件：构造即锁定
    assert not page.crypto.is_unlocked
    assert page._current_totp_or_blank(aid) == ""
    assert page._current_notes_or_blank(aid) == ""


def test_vault_show_totp_guards_and_qrcode_missing(qtbot, seeded_master, monkeypatch,
                                                   msgbox_recorder):
    """TOTP 弹窗：无选中 / 锁定异常 / 无密钥三守卫 + qrcode 缺失降级。"""
    import syskit.ui.vault_page as vp_mod
    from syskit import vault
    from syskit.vault_crypto import VaultLockedError

    monkeypatch.setattr(QDialog, "exec", lambda self, *a, **k: 0)   # 防模态阻塞
    page = _page(qtbot)
    assert not page.crypto.is_unlocked

    page.lst_sites.setCurrentRow(-1)           # 无选中 → 早退（719）
    page._show_totp_dialog()

    _select(page, page.lst_sites, "GitHub")
    page.refresh_accounts()
    _select(page, page.tbl_accts, "alice")     # alice 含 TOTP

    _patch_ask(monkeypatch, [("pw-master", True)])   # _ensure_unlocked 走真解锁
    with monkeypatch.context() as m:
        def boom(*a, **k):
            raise VaultLockedError("locked")

        m.setattr(vault, "get_secret", boom)
        page._show_totp_dialog()               # 解密被锁 → 静默返回（722-723）

    _select(page, page.tbl_accts, "bob")       # bob 未配 TOTP
    page._show_totp_dialog()
    assert "未配置 TOTP" in msgbox_recorder[-1][1][2]   # 726-727

    monkeypatch.setitem(sys.modules, "qrcode", None)     # import qrcode → ImportError
    _select(page, page.tbl_accts, "alice")
    page._show_totp_dialog()                   # 二维码降级为失败标签（750-751）
    assert page.findChildren(QDialog)          # 对话框已构造（exec 已短路防阻塞）


def test_vault_toggle_show_password_requires_master(qtbot, seeded_master, monkeypatch):
    """锁定主密码库点「显示」：验证取消 → 不进入明文态（774-775）。"""
    page = _page(qtbot)
    _select(page, page.lst_sites, "GitHub")
    page.refresh_accounts()
    _select(page, page.tbl_accts, "alice")
    _patch_ask(monkeypatch, [("", False)])     # 验证弹窗取消
    page._toggle_show_password()
    assert page._showing_pw is False
    assert page.btn_show_pw.text() == "显示"


def test_vault_change_master_verifies_current_password(qtbot, seeded_master,
                                                       monkeypatch, msgbox_recorder):
    """信任窗口内更改主密码必须先验证旧密码（曾免验 → 接触键盘即可接管库）；
    解锁成功同时授予信任窗口（与复制路径同语义，曾只解锁不授信）。"""
    page = _page(qtbot)
    _select(page, page.lst_sites, "GitHub")
    _patch_ask(monkeypatch, [("pw-master", True)])
    assert page._ensure_unlocked() is True
    assert page._master_until > time.time()            # 解锁即授信

    _patch_ask(monkeypatch, [("wrong", True)])         # 旧密码错误 → 拒绝
    page._set_master()
    assert any("当前主密码错误" in a[2] for _k, a in msgbox_recorder)

    _patch_ask(monkeypatch, [("pw-master", True), ("new-pw", True)])
    page._set_master()                                 # 旧密码正确 → 设新主密码
    assert page.crypto.verify_password("new-pw") is True


def test_vault_dpapi_unlock_failure_guides_not_password_loop(qtbot, seeded,
                                                              monkeypatch,
                                                              msgbox_recorder):
    """DPAPI 库解封失败：给引导提示并保持未解锁，不再进入「主密码错误」循环。"""
    page = _page(qtbot)
    assert page.crypto.has_master_password() is False
    page.crypto.lock()                                   # seeded 为解锁态：先锁定
    assert page.crypto.is_unlocked is False
    monkeypatch.setattr(page.crypto, "unlock", lambda pw=None: False)
    asked = _patch_ask(monkeypatch, [("whatever", True)])   # 若被弹出即失败证据
    assert page._ensure_unlocked() is False
    assert asked == []                               # 不索要主密码（本库没有）
    assert any("DPAPI" in a[2] for _k, a in msgbox_recorder)


# ============================================================
# syskit/ui/toolbox_page.py
# ============================================================


def test_speed_worker_stream_end_and_read_error(qtbot, monkeypatch):
    """SpeedTestWorker：流提前结束（break）与读取异常两条失败路径 → done(False, 0)。"""
    import urllib.request

    import syskit.ui.toolbox_page as tp

    class _EmptyResp:
        def read(self, _n=-1):
            return b""                         # 立即 EOF → break（69）

        def close(self):
            pass

    monkeypatch.setattr(urllib.request, "urlopen",
                        lambda req, timeout=10: _EmptyResp())
    w = tp.SpeedTestWorker(seconds=1)
    _KEEP.append(w)
    done = []
    w.done.connect(lambda ok, bps: done.append((ok, bps)))
    w.run()
    assert done == [(False, 0.0)]              # 不足 512KB → 全部源视为不可用
    qtbot.wait(1400)                           # 等 measure 守护线程退出再销毁 worker

    class _BoomResp:
        def read(self, _n=-1):
            raise OSError("connection reset")  # 读取异常 → except pass（71-72）

        def close(self):
            pass

    monkeypatch.setattr(urllib.request, "urlopen",
                        lambda req, timeout=10: _BoomResp())
    w2 = tp.SpeedTestWorker(seconds=1)
    _KEEP.append(w2)
    done2 = []
    w2.done.connect(lambda ok, bps: done2.append((ok, bps)))
    w2.run()
    assert done2 == [(False, 0.0)]
    qtbot.wait(1400)                           # 同上：守护线程收尾后再进入清理


def test_lan_worker_arp_failure_keeps_rows(qapp, monkeypatch):
    """arp -a 抛异常 → MAC 列留空，存活设备照常展示（190-192）。"""
    import socket
    import subprocess
    import types

    import psutil
    from syskit import icmp
    import syskit.ui.toolbox_page as tp

    monkeypatch.setattr(psutil, "net_if_addrs", lambda: {
        "Eth": [types.SimpleNamespace(family=socket.AF_INET, address="192.168.1.5")]})
    monkeypatch.setattr(icmp, "ping",
                        lambda ip, timeout_ms=400: 2.0 if ip.endswith(".1") else None)
    monkeypatch.setattr(subprocess, "run",
                        lambda *a, **k: (_ for _ in ()).throw(RuntimeError("arp gone")))
    w = tp._LanScanWorker()
    _KEEP.append(w)
    out = []
    w.done.connect(out.append)
    w.run()
    rows = out[0]
    assert rows and rows[0][0] == "192.168.1.1"
    assert rows[0][1] == "?"                   # MAC 列留空
    assert rows[0][2] == "2"


def test_pingroute_show_hide_resume(qtbot, monkeypatch):
    """PingRouteTab：隐藏停持续 ping 并记忆，重新显示恢复（409-421）。"""
    from syskit import icmp
    import syskit.ui.toolbox_page as tp

    monkeypatch.setattr(icmp, "ping", lambda host, timeout_ms=1500: 5.0)
    tab = tp.PingRouteTab()
    qtbot.addWidget(tab)

    tab.show()                                 # 未开启 ping → showEvent 不恢复
    assert not tab._ping_timer.isActive()

    tab.toggle_ping()                          # 启动（icmp 已 mock，后台线程回报一次）
    assert tab._ping_timer.isActive()
    # 持续 ping 现为真实 QThread：等在飞 worker 收尾再隐藏，避免 tab 回收时
    # 线程仍运行触发 qFatal（生产由 quit_app 全树等待兜底）
    qtbot.wait_until(lambda: tab._ping_worker is None, timeout=10000)
    tab.hide()                                 # 隐藏 → 停表 + 记忆（418-421）
    assert not tab._ping_timer.isActive()
    assert tab._ping_was_active is True
    assert tab.btn_ping.text() == "开始持续 ping"

    tab.show()                                 # 重新显示 → 恢复持续 ping（411-413）
    assert tab._ping_timer.isActive()
    assert tab._ping_was_active is False
    qtbot.wait_until(lambda: tab._ping_worker is None, timeout=10000)
    tab.toggle_ping()                          # 收尾停表，测试内不残留定时器
    assert not tab._ping_timer.isActive()
    qtbot.wait_until(lambda: tab._ping_worker is None, timeout=10000)


def test_port_tab_show_hide_auto_refresh(qtbot, monkeypatch):
    """PortTab：首显懒扫描 / 隐藏停自动刷新 / 重显恢复（552-566）。"""
    import syskit.ui.toolbox_page as tp

    class _FakePortWorker(QThread):
        done = Signal(list)

        def start(self):
            pass

    monkeypatch.setattr(tp, "_PortWorker", _FakePortWorker)
    tab = tp.PortTab()
    qtbot.addWidget(tab)
    _KEEP.append(tab._worker)

    tab.show()                                 # 首显 → 懒扫描一次（554-556）
    assert tab._initial_refreshed is True
    assert isinstance(tab._worker, _FakePortWorker)

    tab.chk_auto.setChecked(True)              # 开启自动刷新 → 定时器启动
    assert tab._timer.isActive()
    tab.hide()                                 # 隐藏 → 停表 + 记忆（564-566）
    assert not tab._timer.isActive()
    assert tab._auto_paused is True

    tab.show()                                 # 重显 → 恢复自动刷新（557-560）
    assert tab._timer.isActive()
    assert tab._auto_paused is False
    tab.chk_auto.setChecked(False)             # 收尾停表
    assert not tab._timer.isActive()


def test_port_tab_kill_selected_missing_cells(qtbot, msgbox_recorder):
    """结束进程：选中行缺 PID/进程单元格 → 直接返回（627-630）。"""
    import syskit.ui.toolbox_page as tp

    tab = tp.PortTab()
    qtbot.addWidget(tab)
    tab.tbl.setRowCount(1)                     # 空行：无任何单元格
    tab.tbl.setCurrentCell(0, 0)
    tab._kill_selected()
    assert tab._worker is None                 # 早退且未触发刷新


def test_hosts_tab_crud_and_save_paths(tmp_path, qtbot, monkeypatch, msgbox_recorder):
    """HostsTab 全流程（注入临时 hosts 文件，写盘只落 pytest 临时目录）：
    加载三态行、_add 四分支、_toggle 注释行与条目行、_remove、_save 四分支。"""
    from syskit import admin, hostsed
    import syskit.ui.toolbox_page as tp

    hosts = tmp_path / "hosts"
    hosts.write_text("127.0.0.1 localhost\n# comment line\n"
                     "# 10.0.0.1 old.local\n", encoding="utf-8")
    tab = tp.HostsTab(path=str(hosts))
    qtbot.addWidget(tab)

    assert tab.tbl.rowCount() == 3
    assert tab.tbl.item(0, 0).text() == "✔ 启用"       # 启用条目（737）
    assert tab.tbl.item(1, 0).text() == "—"            # 注释行
    assert tab.tbl.item(2, 0).text() == "✘ 禁用"       # 被注释的历史条目
    assert not tab.btn_save.isEnabled()                # 非管理员只读

    # ---- _selected_index：行存在但状态列缺失 → -1（757-759）
    tab.tbl.setCurrentCell(0, 0)
    tab.tbl.takeItem(0, 0)
    assert tab._selected_index() == -1

    # ---- _add：空输入 / IP 非法 / 重复 / 成功（777-795）
    tab.edt_ip.setText("")
    tab.edt_host.setText("")
    tab._add()                                 # 空输入 → 提示
    assert "请填写 IP 和主机名" in msgbox_recorder[-1][1][2]

    tab.edt_ip.setText("not-an-ip")
    tab.edt_host.setText("h.local")
    tab._add()                                 # IP 非法 → 警告
    assert "IP 格式不合法" in msgbox_recorder[-1][1][2]

    tab.edt_ip.setText("127.0.0.1")
    tab.edt_host.setText("LOCALHOST")
    tab._add()                                 # 重复（大小写不敏感）→ 警告
    assert "已存在" in msgbox_recorder[-1][1][2]

    n = len(tab._entries)
    tab.edt_ip.setText("10.1.1.1")
    tab.edt_host.setText("new.local")
    tab.edt_cmt.setText("测试备注")
    tab._add()                                 # 成功 → 追加 + 脏标记 + 输入清空
    assert len(tab._entries) == n + 1
    assert tab._dirty is True and tab.lbl_dirty.text() == "● 未保存"
    assert tab.btn_reload.isEnabled()
    assert tab.edt_ip.text() == "" and tab.edt_host.text() == ""
    assert tab.tbl.item(tab.tbl.rowCount() - 1, 0).text() == "✔ 启用"

    # ---- _toggle：无选中早退（798-800）/ 注释行无效果 / 条目行启用↔禁用（797-805）
    tab.tbl.setCurrentCell(-1, -1)
    tab._toggle()                              # 无选中 → 直接返回，无副作用
    comment_row = next(r for r in range(tab.tbl.rowCount())
                       if tab.tbl.item(r, 0).text() == "—")
    tab.tbl.selectRow(comment_row)
    tab._toggle()
    assert tab.tbl.item(comment_row, 0).text() == "—"

    tab.tbl.selectRow(0)
    tab._toggle()                              # 启用 → 禁用
    assert tab.tbl.item(0, 0).text() == "✘ 禁用"
    tab._toggle()                              # 禁用 → 启用
    assert tab.tbl.item(0, 0).text() == "✔ 启用"

    # ---- _remove：无选中 no-op / 选中删除（807-812）
    tab.tbl.setCurrentCell(-1, -1)
    tab._remove()
    assert len(tab._entries) == n + 1
    tab.tbl.selectRow(tab.tbl.rowCount() - 1)  # new.local
    tab._remove()
    assert len(tab._entries) == n
    assert tab._dirty is True

    # ---- _save：未脏 / 非管理员 / 写入失败 / 成功（814-829）
    tab._dirty = False
    tab._save()                                # 未脏 → 直接返回
    monkeypatch.setattr(admin, "is_admin", lambda: False)
    tab._dirty = True
    tab._save()                                # 非管理员 → 警告
    assert "管理员" in msgbox_recorder[-1][1][2]
    assert tab._dirty is True                  # 内容未丢

    monkeypatch.setattr(admin, "is_admin", lambda: True)
    with monkeypatch.context() as m:
        def boom(entries, path, backup=True):
            raise OSError("access denied")

        m.setattr(hostsed, "write_entries", boom)
        tab._save()                            # 写入失败 → 警告
        assert "保存失败" in msgbox_recorder[-1][1][2]

    tab._save()                                # 成功 → 落临时文件 + 提示
    assert tab._dirty is False
    assert "已保存" in msgbox_recorder[-1][1][2]
    assert "10.1.1.1" not in hosts.read_text(encoding="utf-8")   # 已被删除的项不回写
    assert "localhost" in hosts.read_text(encoding="utf-8")


# ============================================================
# syskit/ui/system_page.py
# ============================================================


def test_battery_worker_runps_exception_and_refresh_guard(qtbot, monkeypatch):
    """_BatteryWorker：run_ps 抛异常 → 发空串兜底（241-242）；查询中防重入（279）。"""
    from syskit import proc
    from syskit.ui import system_page as sp

    def boom(script, timeout=30):
        raise RuntimeError("powershell gone")

    monkeypatch.setattr(proc, "run_ps", boom)
    w = sp._BatteryWorker()
    got = []
    w.got.connect(got.append)
    w.run()
    assert got == [""]                         # 异常也发信号，UI 不会永久「查询中」

    tab = sp.BatteryTab()                      # 懒加载：构造期无查询
    qtbot.addWidget(tab)
    tab._bw = object()
    tab.refresh()                              # 已有 worker → 直接返回
    assert tab.lbl_note.text() == " "          # 未进入「查询中…」状态


def test_clipboard_purge_flush_swallows_and_unwatched_error(qtbot, monkeypatch):
    """剪贴板：过期清理 / 刷盘异常吞并（595-596、601-602）；未监听时读剪贴板
    异常吞并并保持不补录语义（636-637）。"""
    from syskit.ui.system_page import ClipboardTab

    tab = ClipboardTab()
    qtbot.addWidget(tab)
    tab.hist.clear()

    def boom(*a, **k):
        raise RuntimeError("disk gone")

    monkeypatch.setattr(tab.hist, "purge_older_than", boom)
    tab._purge_expired()                       # 异常被吞，不抛

    monkeypatch.setattr(tab.hist, "flush_if_dirty", boom)
    tab.flush_disk()                           # 退出路径刷盘异常同样吞并

    class Boom:
        def text(self):
            raise RuntimeError("clipboard gone")

    tab._clip = Boom()
    tab.chk_watch.setChecked(False)
    tab._on_clip_change()                      # 未监听 + 读取异常 → 吞掉返回
    assert tab.hist.items() == []


def test_scan_and_clean_workers_swallow_exceptions(qapp, monkeypatch):
    """清理两 worker：扫描/清理中途异常 → done 信号照发，按钮不会永久禁用
    （330-332 / 391-393）。worker 同步 run()，不 start()。"""
    from syskit.ui import system_page as sp

    def boom_getter():
        raise RuntimeError("temp dir gone")

    # _ScanWorker：CLEAN_TARGETS getter 抛异常 → done({}) 照发
    targets = sp.CLEAN_TARGETS
    monkeypatch.setattr(sp, "CLEAN_TARGETS",
                        [("user_temp", "用户临时文件", boom_getter)])
    w = sp._ScanWorker()
    _KEEP.append(w)
    got = []
    w.done.connect(got.append)
    w.run()
    assert got == [{}]

    # _CleanWorker：取目录 getter 抛异常 → done(0, []) 照发
    monkeypatch.setattr(sp, "CLEAN_TARGETS", targets)   # 还原，仅改 getter 注入
    def boom_getter2():
        raise RuntimeError("target vanished")

    monkeypatch.setattr(sp, "CLEAN_TARGETS",
                        [("user_temp", "用户临时文件", boom_getter2)])
    w2 = sp._CleanWorker(["user_temp"])
    _KEEP.append(w2)
    done2 = []
    w2.done.connect(lambda freed, errors: done2.append((freed, errors)))
    w2.run()
    assert done2 == [(0, [])]
