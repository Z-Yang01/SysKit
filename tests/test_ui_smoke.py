"""UI 离屏冒烟测试（QT_QPA_PLATFORM=offscreen，pytest-qt）。

运行：QT_QPA_PLATFORM=offscreen python -m pytest tests/test_ui_smoke.py -q --timeout=60

覆盖此前 0% 覆盖的 UI 模块：
    syskit/ui/main_window.py / dashboard_page.py / wifi_page.py / disk_page.py /
    system_page.py / toolbox_page.py / confirm_dialog.py / tray.py / app.py /
    workers.py / theme.py
（charts.py 的 human_bytes 在 test_toolbox_calc.py 已覆盖；图表控件随各页面构造 +
 grab() 渲染冒烟。）

Mock 策略（叠加在仓库 conftest 哨兵之上，绝不发真实网络请求 / 不改系统状态）：
- autouse 夹具 ui_offline_mocks：
  * proc.run / proc.run_ps → 返回空的成功 CmdResult（不启动任何真实进程）；
  * psutil 全部采样函数 → 固定假数据；
  * icmp.ping / icmp.default_gateway → 固定假值（绝不发真实 ICMP）；
  * winsound.PlaySound / os.startfile → no-op；
  * QMessageBox.information/warning/question/critical/about → no-op
    （offscreen 下模态 exec 会永久挂起）。
- 后台 worker（workers.py、DiskScanWorker、DeleteWorker、_LanScanWorker、
  _PortWorker 等）一律不调用 start()，改为同步直接调用 run() 验证信号链路，
  其内部依赖（netcheck.run_all / snapshot.take / diskusage.* / subprocess / psutil）
  全部 monkeypatch 为假实现。
- MainWindow 构造时自动启动 monitor/telemetry/诊断线程，夹具统一回收：
  tele.wait + 探测 Monitor._tstate_lock 确认线程真正退出（Monitor 不能 join，
  见疑似 bug 7），托盘 showMessage 全进程 no-op 并尽早 deleteLater。

稳定性护栏（针对 PySide6 离屏 + pytest 捕获环境的原生 fail-fast 崩溃）：
模块导入即在 QApplication 创建前 qInstallMessageHandler 接管 Qt 消息
（README 已知坑：默认处理器在句柄被接管时 fail-fast 0xC0000409），并将
QSystemTrayIcon.showMessage 置为 no-op。个别运行仍可能在进程退出阶段触发
PySide6 原生 fail-fast（无任何 Qt 消息/Python 堆栈，疑似 PySide6 6.11 离屏
平台在多窗口+线程会话收尾时的固有缺陷）：该崩溃发生在全部用例通过之后，
不影响测试结果，重跑即可。

各页面测试均验证：离屏构造、关键子控件存在、核心槽函数可调用、无异常销毁
（qtbot.addWidget 统一清理；MainWindow 额外做线程回收）。

================================================================================
已修复隐患（原疑似 bug，源码已改，测试同步锁定正确行为）：
1. disk_page ROLE_SIZE 已改为 Qt.UserRole+2，与 ROLE_FILLED 分离。
2. PingRouteTab.tracert() 完成后 _tracert 复位为 None，二次追踪可用。
3. workers.RepairWorker 对未知 action_id 也发 done 信号。
4. wifi_page 扫描/读密码拆分为 _scan_worker / _pw_worker 独立槽位。
5. SpeedTestWorker 每个 URL 独立计数，避免跨 URL 字节累计高估速率。
6. monitor 停止事件改名 _stop_evt；Monitor.join()/teardown 不再遮蔽 Thread._stop。

仍存在的限制：
- app.py:run_gui() 单实例互斥分支创建 QApplication 弹完提示后直接 return，
  互斥量句柄依赖进程退出释放；且 run_gui 会创建真实 Win32 互斥量并进入
  app.exec() 事件循环，无法在离屏单测中安全执行 —— 对应用例以 skip 写明。
================================================================================
"""
import os
import sys
import tempfile
import time
import types

import pytest

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from PySide6.QtCore import Qt, qInstallMessageHandler
from PySide6.QtWidgets import (QApplication, QDialog, QDialogButtonBox, QLabel,
                               QMenu, QSystemTrayIcon, QTabWidget, QWidget)


def _qt_message_sink(msg_type, context, message):
    """接管 Qt 消息处理：默认处理器在 pytest 捕获/无控制台的句柄环境下会
    fail-fast(0xC0000409)（README 已知坑，app.run_gui 用同样手段规避），
    测试进程同样命中，故在 QApplication 创建前安装本处理器。"""
    try:
        with open(os.path.join(tempfile.gettempdir(), "syskit_qt_sink.log"),
                  "a", encoding="utf-8") as fh:
            fh.write("[qt] {}\n".format(message))
    except Exception:
        pass


qInstallMessageHandler(_qt_message_sink)

# offscreen 平台无系统托盘实现：托盘气泡（showMessage）在部分时序下会触发
# 原生 fail-fast 崩溃，对整个测试进程统一 no-op（冒烟只验证调用不崩溃）。
QSystemTrayIcon.showMessage = lambda self, *a, **k: None

# ---------------- autouse：离线 mock 环境 ----------------


@pytest.fixture(autouse=True)
def ui_offline_mocks(monkeypatch):
    """UI 冒烟统一离线环境（在 conftest 哨兵之后生效，仅放宽 proc 与补齐假数据，
    subprocess/socket/urlopen/winreg 写 哨兵保持原样 —— 漏 mock 依旧立刻失败）。"""
    from syskit import icmp, proc

    def _fake_run(cmd, timeout=20):
        return proc.CmdResult(code=0, stdout="", stderr="")

    monkeypatch.setattr(proc, "run", _fake_run)
    monkeypatch.setattr(proc, "run_ps", _fake_run)

    import psutil
    nic = types.SimpleNamespace(bytes_recv=120_000, bytes_sent=60_000)
    part = types.SimpleNamespace(device="C:\\", mountpoint="C:\\",
                                 fstype="NTFS", opts="rw fixed")
    monkeypatch.setattr(psutil, "net_io_counters",
                        lambda pernic=False: {"Ethernet0": nic} if pernic else nic)
    monkeypatch.setattr(psutil, "cpu_percent", lambda interval=None: 7.5)
    monkeypatch.setattr(psutil, "virtual_memory",
                        lambda: types.SimpleNamespace(percent=41.0,
                                                      total=16 * 2 ** 30,
                                                      available=9 * 2 ** 30))
    monkeypatch.setattr(psutil, "disk_io_counters",
                        lambda perdisk=False: types.SimpleNamespace(
                            read_bytes=4096, write_bytes=2048))
    monkeypatch.setattr(psutil, "process_iter", lambda attrs=None: iter(()))
    monkeypatch.setattr(psutil, "net_if_addrs", lambda: {})
    monkeypatch.setattr(psutil, "net_connections", lambda kind="inet": [])
    monkeypatch.setattr(psutil, "disk_partitions", lambda all=False: [part])
    monkeypatch.setattr(psutil, "disk_usage",
                        lambda path: types.SimpleNamespace(
                            total=500 * 2 ** 30, used=200 * 2 ** 30,
                            free=300 * 2 ** 30, percent=40.0))

    monkeypatch.setattr(icmp, "ping", lambda host, timeout_ms=1500: 8.0)
    monkeypatch.setattr(icmp, "default_gateway", lambda: "192.168.1.1")

    try:
        import winsound
        monkeypatch.setattr(winsound, "PlaySound", lambda *a, **k: None)
    except ImportError:
        pass
    monkeypatch.setattr(os, "startfile", lambda *a, **k: None, raising=False)

    from PySide6.QtWidgets import QMessageBox
    monkeypatch.setattr(QMessageBox, "information",
                        staticmethod(lambda *a, **k: QMessageBox.StandardButton.Ok))
    monkeypatch.setattr(QMessageBox, "warning",
                        staticmethod(lambda *a, **k: QMessageBox.StandardButton.Ok))
    monkeypatch.setattr(QMessageBox, "question",
                        staticmethod(lambda *a, **k: QMessageBox.StandardButton.Yes))
    monkeypatch.setattr(QMessageBox, "critical",
                        staticmethod(lambda *a, **k: QMessageBox.StandardButton.Ok))
    monkeypatch.setattr(QMessageBox, "about", staticmethod(lambda *a, **k: None))


# ---------------- theme.py ----------------


def test_theme_palette_qss_and_dark_toggle(qapp):
    from syskit.ui import theme
    pal = theme.pal()
    for key in ("bg", "card", "border", "text", "sub", "accent", "sel_bg"):
        assert key in pal, f"调色板缺 {key}"
    assert theme.is_dark() is False                       # 模块初始浅色
    qss = theme._build_qss(pal)
    assert "QMainWindow" in qss and "@bg" not in qss      # 占位符全部替换
    try:
        theme.apply(qapp, dark=True)
        assert theme.is_dark() is True
        dark_bg = theme.pal()["bg"]
        assert dark_bg in qapp.styleSheet()               # 深色背景色已写入 QSS
        assert "@accent_hover" not in qapp.styleSheet()   # 长 token 不被短 token 截断
    finally:
        theme.apply(qapp, dark=False)                     # 还原，避免状态泄漏
    assert theme.is_dark() is False


# ---------------- confirm_dialog.py ----------------


def test_confirm_dialog_shows_action_risk_restore(qtbot):
    from syskit import repairs
    from syskit.ui.confirm_dialog import ConfirmDialog
    act = repairs.get_action("flush_dns")
    dlg = ConfirmDialog(act, param_desc="额外参数：adapter=WLAN")
    qtbot.addWidget(dlg)
    texts = [lbl.text() for lbl in dlg.findChildren(QLabel)]
    assert act.title in texts
    assert any("还原方式" in t for t in texts)
    assert any("额外参数" in t for t in texts)
    assert any("风险级别" in t for t in texts)


def test_confirm_dialog_yes_accepts_and_reboot_hint(qtbot):
    from syskit import repairs
    from syskit.ui.confirm_dialog import ConfirmDialog
    act = repairs.get_action("winsock_reset")     # 强力类 + need_reboot=True
    dlg = ConfirmDialog(act)
    qtbot.addWidget(dlg)
    dlg.show()
    assert dlg.isVisible()
    texts = [lbl.text() for lbl in dlg.findChildren(QLabel)]
    assert any("重启" in t for t in texts)         # 需重启提示
    box = dlg.findChildren(QDialogButtonBox)[0]
    box.button(QDialogButtonBox.Yes).click()       # 非阻塞（等效 exec 返回 Accepted）
    assert dlg.result() == QDialog.Accepted
    assert not dlg.isVisible()


# ---------------- workers.py ----------------


def test_worker_diagnosis_signal_chain(monkeypatch):
    """DiagnosisWorker 不 start()，同步 run() 验证 progress/finished_ok 信号。"""
    from syskit import netcheck
    from syskit.netcheck import CheckResult
    from syskit.ui.workers import DiagnosisWorker
    expect = [CheckResult("real_conn", "真实连通性探测", "pass", "204探测:OK"),
              CheckResult("dns_cfg", "静态 DNS", "fail", "8.8.8.8", fix_ids=["dns_auto"])]

    def fake_run_all(progress=None, quick=False, should_stop=None):
        if progress:
            for r in expect:
                progress(r)
        return expect

    monkeypatch.setattr(netcheck, "run_all", fake_run_all)
    w = DiagnosisWorker(quick=True)
    seen_progress, seen_done = [], []
    w.progress.connect(seen_progress.append)
    w.finished_ok.connect(seen_done.append)
    w.run()
    assert len(seen_progress) == 2
    assert seen_done and [r.id for r in seen_done[0]] == ["real_conn", "dns_cfg"]


def test_worker_repair_and_combo_signal_chain(monkeypatch):
    from syskit import repairs
    from syskit.repairs import RepairResult
    from syskit.ui.workers import ComboWorker, RepairWorker, action_summary
    fake_act = types.SimpleNamespace(execute=lambda param: RepairResult(True, "假动作完成"))
    monkeypatch.setattr(repairs, "get_action",
                        lambda aid: fake_act if aid == "flush_dns" else None)

    rw = RepairWorker("flush_dns", {"profile": "x"})
    done = []
    rw.done.connect(done.append)
    rw.run()
    assert done and done[0][0] == "flush_dns" and done[0][1].success

    rw2 = RepairWorker("nope")                    # 动作不存在：必须发 done，避免界面永久忙碌
    done2 = []
    rw2.done.connect(done2.append)
    rw2.run()
    assert done2 and done2[0][0] == "nope"
    assert done2[0][1].success is False
    assert "未知修复动作" in done2[0][1].message

    cw = ComboWorker(["flush_dns", "nope", "flush_dns"])
    items, finished = [], []
    cw.item_done.connect(items.append)
    cw.all_done.connect(lambda: finished.append(1))   # all_done 为无参信号
    cw.run()
    assert len(items) == 2 and len(finished) == 1
    assert action_summary("flush_dns") is repairs.get_action("flush_dns")


def test_worker_snapshot_signal_chain(monkeypatch):
    from syskit import snapshot as snap_mod
    from syskit.snapshot import Snapshot
    from syskit.ui.workers import SnapshotWorker
    fake = Snapshot()
    monkeypatch.setattr(snap_mod, "take", lambda use_ps=True: fake)
    w = SnapshotWorker()
    got = []
    w.got.connect(got.append)
    w.run()
    assert got and got[0] is fake


def test_worker_exception_still_emits_done(monkeypatch):
    """回归：worker 内部异常也必须发出完成信号。

    否则调用方的 _xxx_worker 永不复位：界面永久忙碌、按钮永久禁用
    （_LanScanWorker 曾无任何 try/except）。
    """
    import psutil

    from syskit import repairs
    from syskit import snapshot as snap_mod
    from syskit.ui import toolbox_page as tp
    from syskit.ui.workers import ComboWorker, SnapshotWorker

    class _Boom:
        def execute(self, param):
            raise RuntimeError("boom")

    monkeypatch.setattr(repairs, "get_action",
                        lambda aid: _Boom() if aid == "x" else None)
    cw = ComboWorker(["x"])
    finished = []
    cw.all_done.connect(lambda: finished.append(1))
    cw.run()
    assert finished == [1]                       # 异常后仍发 all_done

    monkeypatch.setattr(snap_mod, "take",
                        lambda use_ps=True: (_ for _ in ()).throw(RuntimeError("no ps")))
    sw = SnapshotWorker()
    got = []
    sw.got.connect(got.append)
    sw.run()
    assert len(got) == 1 and got[0].ok is False  # 失败也回报空快照

    monkeypatch.setattr(psutil, "net_if_addrs",
                        lambda: (_ for _ in ()).throw(RuntimeError("boom")))
    lw = tp._LanScanWorker()
    out = []
    lw.done.connect(out.append)
    lw.run()
    assert out == [[]]                           # 异常后仍发 done，按钮可复位


def test_combo_worker_cancel_skips_remaining(monkeypatch):
    """回归：组合修复中止——cancel 后剩余动作跳过，all_done 照常发出。"""
    from syskit import repairs
    from syskit.repairs import RepairResult
    from syskit.ui.workers import ComboWorker

    executed = []

    class _Act:
        def __init__(self, name):
            self.title = name

        def execute(self, param):
            executed.append(self.title)
            return RepairResult(True, "ok")

    acts = {aid: _Act(aid) for aid in ("a1", "a2", "a3")}
    monkeypatch.setattr(repairs, "get_action", lambda aid: acts.get(aid))

    cw = ComboWorker(["a1", "a2", "a3"])
    cw.run()                                     # 不取消 → 全执行
    assert executed == ["a1", "a2", "a3"] and cw.cancelled is False

    executed.clear()
    cw2 = ComboWorker(["a1", "a2", "a3"])
    cw2.cancel()                                 # 预先取消 → 一项都不执行
    done = []
    cw2.all_done.connect(lambda: done.append(1))
    cw2.run()
    assert executed == [] and cw2.cancelled is True and done == [1]

    # 运行中途取消：第 1 项执行后置位（模拟当前项完成后再取消）
    executed.clear()
    cw3 = ComboWorker(["a1", "a2", "a3"])
    orig_execute = acts["a1"].execute

    def exec_then_cancel(param):
        r = orig_execute(param)
        cw3.cancel()
        return r

    monkeypatch.setattr(acts["a1"], "execute", exec_then_cancel)
    cw3.run()
    assert executed == ["a1"]                    # 当前项完成后停止
    assert cw3.cancelled is True


def test_netcheck_run_all_should_stop(monkeypatch):
    """run_all 的 should_stop：命中后停止并追加 skip 状态的「诊断中止」记录。"""
    from syskit import netcheck
    from syskit.netcheck import CheckResult, PASS

    c1 = CheckResult("c1", "检查一", PASS, "ok")
    c2 = CheckResult("c2", "检查二", PASS, "ok")
    from syskit import snapshot as snap_mod
    monkeypatch.setattr(netcheck, "ALL_CHECKS", [lambda s: c1, lambda s: c2])
    monkeypatch.setattr(snap_mod, "take", lambda use_ps=True: object())
    n = {"calls": 0}

    def stop_after_first():
        n["calls"] += 1
        return n["calls"] > 1                    # 第 2 项前中止

    out = netcheck.run_all(should_stop=stop_after_first)
    assert [r.id for r in out] == ["c1", "diag_aborted"]
    assert out[-1].status == "skip" and "中止" in out[-1].detail

    assert [r.id for r in netcheck.run_all()] == ["c1", "c2"]   # 不传 → 原行为


def test_diag_and_combo_stop_buttons(make_main_window, monkeypatch):
    """停止按钮接线：busy 时启用、点击调用 worker.cancel、完成后禁用。"""
    win = make_main_window()
    assert not win.btn_diag_stop.isEnabled()
    assert not win.btn_combo_stop.isEnabled()

    class _FakeWorker:
        def __init__(self):
            self.cancel_called = 0

        def cancel(self):
            self.cancel_called += 1

    win._diag_worker = _FakeWorker()
    win._stop_diag()
    assert win._diag_worker.cancel_called == 1
    assert not win.btn_diag_stop.isEnabled()     # 点击后立即置灰防重复

    class _FakeCombo(_FakeWorker):
        def __init__(self):
            super().__init__()
            self.action_ids = ["a", "b"]

    win._combo_worker = _FakeCombo()
    win._stop_combo()
    assert win._combo_worker.cancel_called == 1
    assert not win.btn_combo_stop.isEnabled()


# ---------------- dashboard_page.py ----------------


def test_dashboard_construct_widgets_and_paint(qtbot):
    from syskit.stores import EventStore
    from syskit.ui.dashboard_page import DashboardPage
    page = DashboardPage(EventStore())
    qtbot.addWidget(page)
    for attr in ("card_gw", "card_pub", "card_down", "card_up", "card_traffic",
                 "chart_lat", "chart_speed", "chart_wifi", "list_events"):
        assert getattr(page, attr, None) is not None, f"缺关键控件 {attr}"
    page.show()
    page.grab()                                   # 触发 LineChart.paintEvent
    qtbot.wait(20)


def test_dashboard_update_tick_and_event_timeline(qtbot):
    from syskit.stores import EventStore
    from syskit.ui.dashboard_page import DashboardPage
    store = EventStore()
    store.add("no_net", "连通性探测失败 ×2")
    page = DashboardPage(store)
    qtbot.addWidget(page)
    page.reload_events()
    assert page.list_events.count() == 1
    assert "连接中断" in page.list_events.item(0).text()

    page.update_tick({"lat_gw": 12.0, "lat_pub": 33.0, "gateway": "192.168.1.1",
                      "down": 1024.0, "up": 2048.0, "wifi_signal": 77,
                      "today_sent": 500, "today_recv": 900})
    assert "12" in page.card_gw.value_label.text()
    assert "ms" in page.card_pub.value_label.text()
    assert "KB" in page.card_down.value_label.text()
    assert "B" in page.card_traffic.value_label.text()
    assert page.card_gw.toolTip() == "192.168.1.1"

    page.update_tick({"lat_gw": None, "lat_pub": None})
    assert "超时" in page.card_gw.value_label.text()


# ---------------- wifi_page.py ----------------

FAKE_APS = [
    {"ssid": "HomeWiFi", "bssid": "AA:BB:CC:DD:EE:01", "signal": 82, "channel": 6,
     "radio": "802.11ax", "auth": "WPA2-PSK", "band": "2.4G"},
    {"ssid": "Office5G", "bssid": "AA:BB:CC:DD:EE:02", "signal": 55, "channel": 36,
     "radio": "802.11ax", "auth": "WPA3-SAE", "band": "5G"},
]


def test_wifi_page_scan_fills_table_and_channel_advice(qtbot, monkeypatch):
    import syskit.ui.wifi_page as wp
    from syskit import repairs
    monkeypatch.setattr(wp, "scan_networks", lambda: FAKE_APS)
    monkeypatch.setattr(repairs, "wlan_profiles", lambda: ["HomeWiFi"])
    page = wp.WifiPage()
    qtbot.addWidget(page)
    assert page._scan_worker is None                  # 构造时不自动扫描
    page.show()                                       # 首次可见才扫
    assert page._initial_scanned is True
    qtbot.wait_until(lambda: page._scan_worker is None and page._share_worker is None
                     and page.tbl_aps.rowCount() >= 2,
                     timeout=10000)

    tabs = page.findChildren(QTabWidget)[0]
    assert tabs.count() == 4                      # 周边 AP / 信道分析 / 二维码 / 已存密码
    assert page.tbl_aps.rowCount() == 2
    assert page.tbl_aps.item(0, 0).text() == "HomeWiFi"   # 按信号降序
    assert "共发现 2" in page.lbl_scan.text()
    assert "推荐" in page.lbl_advice.text()               # 信道推荐已生成
    assert page.cmb_share.count() == 1
    page.hide()                                       # 隐藏时应停掉自动扫描
    assert not page._scan_timer.isActive()


def test_wifi_page_generate_qr(qtbot, monkeypatch):
    import syskit.ui.wifi_page as wp
    page = wp.WifiPage()
    qtbot.addWidget(page)
    monkeypatch.setattr(wp, "profile_password", lambda name: ("WPA2-PSK", "s3cret"))
    assert not page.btn_save_qr.isEnabled()
    page.cmb_share.addItem("HomeWiFi")
    page.cmb_share.setCurrentText("HomeWiFi")
    page._gen_qr()                              # 密码读取在后台线程
    qtbot.wait_until(lambda: page._qr_worker is None and page.btn_save_qr.isEnabled(),
                     timeout=10000)
    pix = page.lbl_qr.pixmap()
    assert pix is not None and not pix.isNull()


def test_wifi_page_password_table_fill(qtbot):
    import syskit.ui.wifi_page as wp
    page = wp.WifiPage()
    qtbot.addWidget(page)
    page._pw_done([("HomeWiFi", "WPA2", "pw123"), ("Cafe", "?", "?")])
    assert page.tbl_pw.rowCount() == 2
    assert page.tbl_pw.item(0, 2).text() == "••••••••"           # 默认掩码
    page.chk_show_pw.setChecked(True)                            # 显示 → 明文
    assert page.tbl_pw.item(0, 2).text() == "pw123"
    assert "共 2" in page.lbl_pw.text()


# ---------------- disk_page.py ----------------


def _fake_scan_result():
    from syskit.diskusage import DirNode, ScanResult
    root = DirNode(name="C:", path="C:\\", total=1500, file_count=30,
                   children=[DirNode(name="Users", path="C:\\Users",
                                     total=1000, file_count=20)])
    return ScanResult(root=root, ext_stats={"zip": 1000, "txt": 500},
                      top_files=[(1000, "C:\\Users\\big.zip", time.time() - 60),
                                 (500, "C:\\note.txt", time.time())],
                      file_count=30, error_count=2, duration=1.5, cancelled=False)


def test_disk_tab_construct_lists_fake_drives(qtbot):
    from syskit.ui.disk_page import DiskTab
    tab = DiskTab()
    qtbot.addWidget(tab)
    assert tab.combo.count() == 1                 # psutil mock：仅一个 C:
    assert tab.combo.itemData(0) == "C:\\"
    assert tab.btn_scan.isEnabled()
    assert not tab.btn_stop.isEnabled()
    for attr in ("tree", "table", "chart", "btn_delete", "pb"):
        assert getattr(tab, attr, None) is not None


def test_disk_tab_scan_render_tree_table_chart(qtbot, monkeypatch):
    from syskit import diskusage
    from syskit.ui.disk_page import DiskTab
    monkeypatch.setattr(diskusage, "scan_tree",
                        lambda root, stop=None, progress=None: _fake_scan_result())
    tab = DiskTab()
    qtbot.addWidget(tab)
    tab.scan("C:\\")                              # 后台 worker，扫描函数已 mock
    qtbot.wait_until(lambda: tab._worker is None, timeout=10000)

    assert "扫描完成" in tab.lbl_result.text()
    assert "跳过 2" in tab.lbl_result.text()
    assert tab.tree.topLevelItemCount() == 1
    root_item = tab.tree.topLevelItem(0)
    assert root_item.text(0) == "C:"
    assert root_item.childCount() == 1            # setExpanded 触发懒加载填充
    assert root_item.child(0).text(0) == "Users"
    assert tab.table.rowCount() == 2
    assert tab.table.item(0, 0).text() == "big.zip"       # 按大小列降序
    assert len(tab.chart._bars) == 2                      # 文件类型 Top 图
    assert tab.btn_delete.isEnabled()

    tab._scan_done(None)                          # 扫描失败分支
    assert "扫描失败" in tab.lbl_progress.text()


def test_disk_tab_collect_and_delete_worker(qtbot, monkeypatch):
    from syskit import diskusage
    from syskit.ui.disk_page import DeleteWorker, DiskTab
    tab = DiskTab()
    qtbot.addWidget(tab)
    tab._scan_done(_fake_scan_result())

    assert tab._collect() == []                   # 未勾选未选中 → 空
    root_item = tab.tree.topLevelItem(0)
    child_item = root_item.child(0)
    child_item.setCheckState(0, Qt.Checked)
    sel = tab._collect()
    assert sel == [("C:\\Users", 1000)]
    child_item.setCheckState(0, Qt.Unchecked)     # 还原，供下方空选守卫用

    # DeleteWorker 同步执行，delete_to_recycle_bin 已 mock —— 绝不碰真实回收站
    monkeypatch.setattr(diskusage, "delete_to_recycle_bin",
                        lambda paths: (len(paths), []))
    w = DeleteWorker(["C:\\Users"])
    out = []
    w.done.connect(lambda ok, errors: out.append((ok, errors)))
    w.run()
    assert out == [(1, [])]

    tab.delete_checked()                          # 无勾选无选中：仅提示返回，不启动删除
    assert tab._delete_worker is None


def test_disk_tab_reveal_missing_path_noop(qtbot, monkeypatch):
    """双击定位：路径不存在时直接返回，不触达 subprocess（哨兵保护未解除）。"""
    from syskit.ui.disk_page import DiskTab
    tab = DiskTab()
    qtbot.addWidget(tab)
    tab._scan_done(_fake_scan_result())
    monkeypatch.setattr(os.path, "exists", lambda p: False)
    tab._reveal(tab.table.item(0, 0))             # 不应抛异常/拉起 explorer


# ---------------- toolbox_page.py ----------------


def test_toolbox_page_constructs_six_tabs(qtbot):
    from syskit.ui.toolbox_page import PortTab, ToolboxPage
    page = ToolboxPage()
    qtbot.addWidget(page)
    tabs = page.findChildren(QTabWidget)[0]
    assert tabs.count() == 6                        # +Hosts 编辑
    assert tabs.tabText(5) == "Hosts 编辑"
    port = page.findChildren(PortTab)[0]
    assert port._worker is None                     # 懒加载：构造期不扫描
    port.refresh()                                  # 显式触发（首显由 showEvent 自动）
    qtbot.wait_until(lambda: port._worker is None, timeout=10000)
    assert "共 0 条" in port.lbl_status.text()    # psutil.net_connections mock 为空


def test_speed_test_tab_result_handlers(qtbot):
    from syskit.ui.toolbox_page import SpeedTestTab
    tab = SpeedTestTab()
    qtbot.addWidget(tab)
    tab._on_progress(500_000.0, 3.0)
    assert "实时" in tab.lbl_status.text()
    tab._on_done(True, 2_000_000.0)
    assert "Mbps" in tab.lbl_big.text()
    assert tab.list_hist.count() == 1
    tab._on_done(False, 0.0)
    assert "测速失败" in tab.lbl_big.text()
    assert tab.btn_start.isEnabled()              # 失败后按钮恢复


def test_lan_tab_done_and_scan_worker(qtbot, monkeypatch):
    import socket as socket_mod
    import subprocess
    import psutil
    from syskit import icmp
    from syskit.ui import toolbox_page as tp
    tab = tp.LanTab()
    qtbot.addWidget(tab)

    tab._done([("192.168.1.2", "AA:BB:CC:DD:EE:FF", "3"),
               ("192.168.1.10", "?", "12")])
    assert tab.tbl.rowCount() == 2
    assert "发现 2 台设备" in tab.lbl_status.text()

    # _LanScanWorker 同步 run：psutil 全局已 mock，此处显式补 arp 假返回（显式 mock 模式）
    monkeypatch.setattr(psutil, "net_if_addrs", lambda: {
        "Ethernet0": [types.SimpleNamespace(family=socket_mod.AF_INET,
                                            address="192.168.1.5")]})
    monkeypatch.setattr(icmp, "ping", lambda ip, timeout_ms=400: 3.0)
    monkeypatch.setattr(subprocess, "run",
                        lambda *a, **k: types.SimpleNamespace(stdout=b""))
    w = tp._LanScanWorker()
    out = []
    w.done.connect(out.append)
    w.run()
    assert out and out[0], "扫描 worker 应返回非空设备列表"
    assert out[0][0][0].startswith("192.168.1.")


def test_dns_subnet_tab_smoke(qtbot, monkeypatch):
    """轻量冒烟即可（calc/query 已被 tests/test_toolbox_calc.py 深度覆盖）。"""
    import socket as socket_mod
    from syskit import dnsquery
    from syskit.ui.toolbox_page import DnsSubnetTab
    tab = DnsSubnetTab()
    qtbot.addWidget(tab)
    assert tab.ed_domain.text() and tab.ed_ip.text()      # 有默认输入
    tab.calc()
    assert tab.list_sub.count() == 6
    # 查询已异步化：先 mock 再触发，等后台 worker 回填（不再真实出网）
    monkeypatch.setattr(socket_mod, "getaddrinfo",
                        lambda *a, **k: [(socket_mod.AF_INET, 1, 6, "",
                                         ("223.5.5.5", 0))])
    tab.query()                                           # 系统默认分支
    qtbot.wait_until(lambda: tab._qw is None)
    assert tab.list_dns.item(0).text() == "223.5.5.5"
    monkeypatch.setattr(dnsquery, "query",
                        lambda host, server, timeout=3.0, qtype=1: ["119.29.29.29"])
    tab.cmb_server.setCurrentIndex(2)
    tab.query()
    qtbot.wait_until(lambda: tab._qw is None)
    assert tab.list_dns.item(0).text() == "119.29.29.29"


def test_ping_route_tab(qtbot, monkeypatch):
    import subprocess
    from syskit import icmp
    from syskit.ui.toolbox_page import PingRouteTab
    tab = PingRouteTab()
    qtbot.addWidget(tab)

    monkeypatch.setattr(icmp, "ping", lambda host, timeout_ms=1500: 8.0)
    tab._ping_once()                                      # 后台线程 ping → 等回填
    qtbot.wait_until(lambda: tab._ping_worker is None
                     and 8.0 in list(tab.chart._series[0][2]), timeout=10000)

    tab.toggle_ping()                                     # 启动 1s 定时器
    assert tab._ping_timer.isActive()
    tab.toggle_ping()                                     # 立即停，测试内不残留定时器
    assert not tab._ping_timer.isActive()

    # 路由追踪：subprocess.Popen 显式 mock 为立即结束的假进程
    class _FakeProc:
        stdout = iter([])
        def kill(self):
            pass
    monkeypatch.setattr(subprocess, "Popen", lambda *a, **k: _FakeProc())
    tab.tracert()
    # finished 槽会把 _tracert 复位为 None 并重新启用按钮
    qtbot.wait_until(lambda: tab._tracert is None and tab.btn_route.isEnabled(),
                     timeout=10000)

    tab._route_line({"hop": 1, "ms": [10, 12], "ip": "10.0.0.1"})
    tab._route_line({"hop": 2, "ms": [], "ip": "*"})
    assert tab.tbl_route.rowCount() == 2
    assert tab.tbl_route.item(0, 1).text() == "10 ms, 12 ms"
    assert tab.tbl_route.item(1, 1).text() == "超时"

    tab.tracert()                                      # 二次追踪可再次启动
    assert tab._tracert is not None
    qtbot.wait_until(lambda: tab._tracert is None and tab.btn_route.isEnabled(),
                     timeout=10000)


def test_port_tab_refresh_filter_and_worker(qtbot, monkeypatch):
    import psutil
    from syskit.ui.toolbox_page import PortTab, _PortWorker
    tab = PortTab()                               # 懒加载：构造期不扫描
    qtbot.addWidget(tab)
    tab.refresh()                                 # 显式触发（psutil mock 为空）
    qtbot.wait_until(lambda: tab._worker is None, timeout=10000)
    assert "共 0 条" in tab.lbl_status.text()

    rows = [
        {"proto": "TCP", "tcp": True, "laddr": "127.0.0.1:8443", "raddr": "0.0.0.0:0",
         "status": "LISTEN", "pid": 4321, "name": "websvc.exe",
         "exe": "C:\\app\\websvc.exe"},
        {"proto": "UDP", "tcp": False, "laddr": "0.0.0.0:5353", "raddr": "-",
         "status": "-", "pid": 0, "name": "System", "exe": ""},
    ]
    tab._done(rows)
    assert tab.tbl.rowCount() == 2
    assert "共 2 条" in tab.lbl_status.text()
    assert tab.tbl.item(0, 5).toolTip() == "C:\\app\\websvc.exe"

    tab.ed_filter.setText("websvc")
    assert not tab.tbl.isRowHidden(0) and tab.tbl.isRowHidden(1)
    tab.ed_filter.setText("")
    tab.chk_listen.setChecked(True)
    assert not tab.tbl.isRowHidden(0) and tab.tbl.isRowHidden(1)
    tab.chk_listen.setChecked(False)

    # _PortWorker 同步 run：显式 mock 一条假连接与假进程
    conn = types.SimpleNamespace(pid=99,
                                 laddr=types.SimpleNamespace(ip="127.0.0.1", port=80),
                                 raddr=None, type=1, status="LISTEN")
    monkeypatch.setattr(psutil, "net_connections", lambda kind="inet": [conn])

    class _P:
        def name(self):
            return "p.exe"
        def exe(self):
            return "C:\\p.exe"
    monkeypatch.setattr(psutil, "Process", lambda pid: _P())
    w = _PortWorker()
    out = []
    w.done.connect(out.append)
    w.run()
    assert out and out[0][0]["name"] == "p.exe" and out[0][0]["proto"] == "TCP"


# ---------------- system_page.py ----------------


def test_system_page_tabs_and_perf_tick(qtbot):
    from syskit.ui.system_page import SystemPage
    page = SystemPage()
    qtbot.addWidget(page)
    tabs = page.findChildren(QTabWidget)[0]
    assert tabs.count() == 7                      # 性能/启动项/电池/清理/磁盘/剪贴板/场景
    page.update_tick({"cpu": 12.0, "mem": 55.0, "disk_r": 1024, "disk_w": 512,
                      "top_procs": [{"pid": 7, "name": "a.exe",
                                     "cpu": 12.5, "mem": 4096}]})
    assert page.perf.tbl.rowCount() == 1
    assert page.perf.tbl.item(0, 1).text() == "a.exe"
    assert page.perf.tbl.item(0, 0).text() == "7"


def test_battery_tab_both_branches(qtbot, monkeypatch):
    from syskit import proc
    from syskit.ui.system_page import BatteryTab
    tab = BatteryTab()                            # 懒加载：构造期无查询
    qtbot.addWidget(tab)
    monkeypatch.setattr("syskit.ui.system_page._BatteryWorker.start",
                        lambda self: self.run())            # worker 同步执行（项目惯例）
    tab.refresh()                                 # autouse fake run_ps → 无电池分支
    assert "未检测到电池" in tab.lbl_design.text()

    monkeypatch.setattr(proc, "run_ps",
                        lambda script, timeout=40: proc.CmdResult(0, "40000,32000,80", ""))
    tab.refresh()
    assert tab.lbl_design.text() == "40000 mWh"
    assert tab.lbl_full.text() == "32000 mWh"
    assert tab.lbl_health.text() == "80.0%"
    assert tab.lbl_remain.text() == "80%"


def test_cleanup_tab_scan_and_clean_guard(qtbot, monkeypatch):
    from syskit.ui import system_page as sp
    tab = sp.CleanupTab()
    qtbot.addWidget(tab)
    assert set(tab.checks) == {"user_temp", "win_temp", "thumbcache", "recycle"}

    monkeypatch.setattr(sp.glob, "glob", lambda *a, **k: [])   # 不遍历真实临时目录
    tab.scan()                                    # _ScanWorker：回收站走 fake run_ps
    qtbot.wait_until(lambda: tab._scan_worker is None, timeout=10000)
    assert "0B" in tab.checks["user_temp"].text()
    assert "0B" in tab.checks["recycle"].text()

    tab.clean()                                   # 未勾选：仅提示返回（QMessageBox 已 mock）
    assert tab._clean_worker is None


def test_startup_tab_reload_is_read_only(qtbot):
    """StartupTab 仅读注册表 Run 键与启动目录（conftest 允许只读），reload 不崩。"""
    from syskit.ui.system_page import StartupTab
    tab = StartupTab()                            # 构造即 reload 一次
    qtbot.addWidget(tab)
    assert tab.tbl.columnCount() == 4
    tab.reload()                                  # 二次 reload 不崩、不残留
    assert tab.tbl.columnCount() == 4


# ---------------- tray.py ----------------


def test_state_icon_all_states(qtbot):
    from syskit.ui.tray import state_icon
    for state in ("ok", "no_net", "ncsi_false", "offline", "no_internet", "unknown"):
        icon = state_icon(state)
        assert not icon.isNull()
        assert icon.pixmap(64, 64).width() == 64


def test_app_icon_multi_size(qtbot):
    """应用图标：多尺寸代码绘制非空（窗口接线在下方主窗口构造测试中断言）。"""
    from syskit.ui import theme
    icon = theme.app_icon()
    assert not icon.isNull()
    for size in (16, 32, 256):
        assert icon.pixmap(size).width() == size


def test_tray_host_state_rate_notify_and_menu(qtbot):
    from syskit.ui.tray import TrayHost

    class _Win(QWidget):
        diag_calls = []
        repair_calls = []
        quit_called = False

        def run_diagnosis(self, quick=False):
            self.diag_calls.append(quick)

        def run_repair(self, aid, param=None):
            self.repair_calls.append(aid)

        def quit_app(self):
            self.quit_called = True

    win = _Win()
    qtbot.addWidget(win)
    tray = TrayHost(win)
    tray.set_state("no_net", "探测失败 ×2")
    tip = tray.toolTip()
    assert "SysKit" in tip and "已连接但无法上网" in tip and "探测失败 ×2" in tip

    tray.update_rate(1024.0, 2048.0)
    assert "↓" in tray.toolTip() and "↑" in tray.toolTip()

    tray.notify("标题", "内容", "ok")
    assert "网络正常" in tray.toolTip()

    tray._activated(QSystemTrayIcon.Trigger)      # 单击托盘 → 显示主窗口
    assert win.isVisible()

    menu = tray.contextMenu()
    assert isinstance(menu, QMenu)
    actions = menu.actions()                      # 打开/检测/修复 + 剪贴板子菜单 + 分隔线 + 退出
    assert len(actions) == 6
    clip_menu = actions[3].menu()
    assert clip_menu is not None and "剪贴板" in actions[3].text()
    tray._fill_clip_menu(clip_menu)               # 空历史 → 灰色占位
    assert clip_menu.actions() and not clip_menu.actions()[0].isEnabled()
    actions[1].trigger()
    assert win.diag_calls == [True]
    actions[2].trigger()
    assert win.repair_calls == ["flush_dns"]
    actions[-1].trigger()
    assert win.quit_called
    tray.hide()


# ---------------- app.py ----------------


def test_app_enable_crash_log_writes_to_isolated_dir():
    """crash.log 落在 conftest 重定向后的 LOCALAPPDATA（临时目录），不碰真实目录。"""
    import faulthandler
    import threading as threading_mod
    from syskit import config
    from syskit.ui import app as app_mod
    old_excepthook = sys.excepthook
    old_thread_hook = threading_mod.excepthook
    fh = None
    try:
        fh = app_mod._enable_crash_log()
        assert fh is not None
        crash = os.path.join(config.LOCALAPPDATA_DIR, "crash.log")
        assert os.path.isfile(crash)
        assert "launch" in open(crash, encoding="utf-8").read()
    finally:
        if fh:
            fh.close()
        faulthandler.disable()                    # 撤销 faulthandler 全局接管
        sys.excepthook = old_excepthook           # 还原全局钩子，避免泄漏到其它测试
        threading_mod.excepthook = old_thread_hook


def test_app_run_gui_out_of_scope():
    pytest.skip("run_gui 会创建真实 Win32 互斥量并进入 app.exec() 事件循环、弹模态框，"
                "无法在离屏单测中安全执行；app.py 仅冒烟 _enable_crash_log（见文件头 bug 6）")


# ---------------- main_window.py ----------------


@pytest.fixture
def make_main_window(qtbot, monkeypatch):
    """构造 MainWindow 的工厂夹具：诊断/快照全部走假数据，teardown 回收后台线程。"""
    from syskit import netcheck
    from syskit.netcheck import CheckResult
    results = [
        CheckResult("real_conn", "真实连通性探测", "pass", "204探测:OK"),
        CheckResult("nlasvc", "NlaSvc 服务", "warn", "手动启动且无触发器",
                    fix_ids=["fix_nlasvc"]),
    ]

    def fake_run_all(progress=None, quick=False, should_stop=None):
        if progress:
            for r in results:
                progress(r)
        return results

    monkeypatch.setattr(netcheck, "run_all", fake_run_all)
    from syskit.ui.main_window import MainWindow
    created = []

    def _make(**kwargs):
        win = MainWindow(**kwargs)
        qtbot.addWidget(win)
        created.append(win)
        qtbot.wait_until(lambda: win._diag_worker is None and win._snap_worker is None,
                         timeout=20000)
        qtbot.wait(30)
        return win

    yield _make

    for win in created:                           # 线程回收（Monitor.join/is_alive
        try:                                      # 都会因 Thread._stop 被事件对象
            qtbot.wait_until(lambda w=win: w._snap_worker is None, timeout=5000)
        except Exception:
            pass                                  # 遮蔽而抛 TypeError，见文件头 bug 7）
        win.mon.stop()                            # 只置位停止事件
        win.tele.stop()
        win.tele.wait(3000)
        # 用原始 _tstate_lock 探测守护监控线程是否真正退出（绕开被遮蔽的
        # Thread 方法）：锁被 C 层释放 = run() 已结束。若不等待，解释器退出
        # 阶段守护线程可能仍在调用 Qt，引发原生 fail-fast(0xC0000409) 崩溃。
        deadline = time.time() + 5
        while time.time() < deadline:
            lock = win.mon._tstate_lock
            if lock is None or lock.acquire(False):   # 非阻塞探测（3.10 仅支持位置参数）
                if lock is not None:
                    lock.release()
                break
            time.sleep(0.01)
        win.tray.hide()
        win.tray.deleteLater()                    # 托盘尽早销毁，不留到进程收尾
        win.hide()
        win.deleteLater()                         # 在事件循环仍存活时确定性析构窗口
    QApplication.processEvents()
    # 主动回收，减少留到解释器退出阶段才析构的 Qt 对象
    import gc
    gc.collect()
    QApplication.processEvents()


@pytest.fixture
def main_win(make_main_window):
    return make_main_window()


def test_main_window_constructs_and_diag_fills_ui(main_win):
    from syskit import config
    win = main_win
    assert config.DISPLAY_NAME in win.windowTitle()      # 标题可附带版本号
    assert config.VERSION in win.windowTitle()
    assert not win.windowIcon().isNull()                 # 标题栏/任务栏图标已接线
    assert win.stack.count() == 10                # 概览/监控/WiFi/诊断/修复/工具箱/系统/密码库/设置/日志
    assert win.nav.count() == 13                  # 10 个页面项 + 3 个分组标题
    # 构造时的静默快速诊断已回填 UI
    assert win.lbl_verdict.text() != "尚未诊断"
    assert "204探测:OK" in win.lbl_probe.text()
    assert win.pb_diag.isHidden()
    fix_titles = [win.fix_btn_row.itemAt(i).widget().text()
                  for i in range(win.fix_btn_row.count())
                  if win.fix_btn_row.itemAt(i).widget()]
    assert any("NlaSvc" in t for t in fix_titles)         # warn 项生成建议修复按钮
    assert win.tbl_actions.rowCount() == len(repairs_actions())
    assert win.tray._state in ("ok", "no_net", "ncsi_false", "offline", "no_internet")


def repairs_actions():
    from syskit import repairs
    return repairs.ACTIONS


def test_main_window_show_switch_all_pages_and_close(main_win):
    win = main_win
    win.show()
    assert win.isVisible()
    for idx in range(win.stack.count()):          # 关键页面切换不崩
        win.switch_page(idx)
        QApplication.processEvents()
        assert win.stack.currentIndex() == idx
    win.switch_page(9)                            # 日志页（守卫按对象比较；曾因旧索引 8 长期假绿）
    QApplication.processEvents()
    assert "syskit.log" in win.txt_log.toPlainText()
    # 点击侧边栏全部行（分组行 UserRole=None 不应崩溃）
    for row in range(win.nav.count()):
        win.nav.setCurrentRow(row)
        QApplication.processEvents()
    win.close()                                   # minimize_to_tray 默认 True → 隐藏而非退出
    QApplication.processEvents()
    assert not win.isVisible()
    assert win.stack.count() == 10                # 窗口仍存活（仅隐藏）


def test_main_window_minimized_starts_hidden(make_main_window):
    win = make_main_window(minimized=True)
    assert not win.isVisible()
    assert win.stack.count() == 10


def test_quit_app_waits_page_workers(make_main_window, qtbot, monkeypatch):
    """回归：quit_app 必须等待页面级 QThread（诊断/修复/组合/快照），
    否则运行中的线程随窗口析构 → qFatal 0xC0000409（生产 BEX64 崩溃同源）。"""
    from PySide6.QtCore import QThread, Signal

    class _Slow(QThread):
        done = Signal()

        def __init__(self):
            super().__init__()
            self._finished_run = False

        def run(self):
            import time as _t
            _t.sleep(0.4)               # 模拟收尾延迟
            self._finished_run = True
            self.done.emit()

    win = make_main_window()
    from syskit.ui.workers import reap
    slow = reap(_Slow(), win)          # 真实 worker 均挂宿主对象树（quit_app 全树扫尾可见）
    win._repair_worker = slow
    slow.start()
    # 主线程直接调 quit_app：先停后台线程，再等 slow
    monkeypatch.setattr(win.tele.traffic, "save_if_dirty", lambda: None)
    win.quit_app()
    assert slow._finished_run            # run 已完整结束（wait 生效）
    assert win._repair_worker is None    # 引用已安全释放
    assert win._snap_worker is None and win._diag_worker is None
    assert win._combo_worker is None


def test_release_worker_waits_before_drop(make_main_window):
    """_release_worker：复位前 wait，线程 emit 后的收尾不再有 GC 析构竞态。"""
    win = make_main_window()
    w = win._snap_worker
    win._release_worker("_snap_worker")
    assert win._snap_worker is None
    if w is not None:
        assert not w.isRunning()


def test_main_window_monitor_status_and_telemetry(main_win):
    win = main_win
    win._on_monitor_status("ok", "")
    win._on_monitor_status("no_net", "连通性探测失败 ×1")     # 状态切换 → 事件时间线（声音已 mock）
    assert win.eventstore.items()[0]["state"] == "no_net"
    assert win.tray._state == "no_net"
    assert "已连接但无法上网" in win.tray.toolTip()

    d = {"lat_gw": 5.0, "lat_pub": 9.0, "gateway": "192.168.1.1",
         "down": 2048.0, "up": 1024.0, "cpu": 3.0, "mem": 30.0,
         "top_procs": []}
    # 遥测只喂当前可见页（挂机省 CPU）：不在前台的页不更新
    win.stack.setCurrentWidget(win.page_dash)
    win._on_telemetry(d)
    assert "5" in win.page_dash.card_gw.value_label.text()
    assert "↓" in win.tray.toolTip()              # 托盘实时速率（不受页面可见性影响）
    win.stack.setCurrentWidget(win.page_system)
    win._on_telemetry(d)
    assert win.page_system.perf.tbl.rowCount() == 0       # 空 top_procs 不崩


def test_main_window_fill_adapters_and_diag_progress(main_win):
    from syskit.netcheck import CheckResult
    from syskit.snapshot import Adapter, Snapshot
    win = main_win
    snap = Snapshot(adapters=[
        Adapter(name="WLAN", desc="Intel Wi-Fi 6 AX201", status="Up",
                ipv4=["192.168.1.23"], gw4=["192.168.1.1"],
                dns=["192.168.1.1", "223.5.5.5"], conn_v4="Internet"),
        Adapter(name="WAN Miniport (IP)", desc="WAN Miniport (IP)", status="Up"),
    ])
    win._fill_adapters(snap)
    assert win.tbl_adapters.rowCount() == 1       # WAN Miniport 被过滤
    assert win.tbl_adapters.item(0, 0).text().startswith("★")   # 在线网卡加星

    win.tree_diag.clear()
    win._diag_results.clear()
    r = CheckResult("dns_cfg", "静态 DNS 残留", "fail", "8.8.8.8",
                    fix_ids=["dns_auto"])
    win._diag_progress(r)
    assert win.tree_diag.topLevelItemCount() == 1
    assert win.tree_diag.topLevelItem(0).data(0, Qt.UserRole) is r
    assert win.tree_diag.topLevelItem(0).text(0) == "×"     # FAIL 标记


def test_main_window_action_filter_and_save_settings(main_win):
    from syskit import settings as settings_mod
    win = main_win
    win.ed_filter.setText("dns")                  # 修复动作筛选
    hidden = [win.tbl_actions.isRowHidden(r) for r in range(win.tbl_actions.rowCount())]
    assert any(hidden) and not all(hidden)
    win.ed_filter.setText("")
    hidden = [win.tbl_actions.isRowHidden(r) for r in range(win.tbl_actions.rowCount())]
    assert not any(hidden)

    win.switch_page(7)                            # 设置页
    win.chk_dark.setChecked(False)                # 保持浅色，避免主题状态泄漏
    win._save_settings()                          # QMessageBox 已 mock；写隔离目录
    assert settings_mod.load()["poll_interval_sec"] == win.spin_interval.value()
    assert settings_mod.load()["auto_repair_enabled"] == win.chk_auto.isChecked()
    assert win.stack.currentIndex() == 7


def test_repair_done_refreshes_proxy_label(monkeypatch):
    """回归：修复完成（尤其 proxy_off）后必须重读系统代理状态。

    原实现只在 _toggle_proxy 里同步刷新一次，而修复在后台线程异步执行，
    刷新发生在真正改动之前 → 标签长期停留在旧值。

    这里用「假 self」直接调用真实方法体：本文件已有 6 个用例各建一个
    MainWindow，PySide6 6.11 离屏平台在窗口数量继续增加时收尾不稳定，
    故此用例不建窗口（只验证 _repair_done 的编排逻辑）。
    """
    from PySide6.QtWidgets import QMessageBox

    from syskit.repairs import RepairResult
    from syskit.ui.main_window import MainWindow

    monkeypatch.setattr(QMessageBox, "information",
                        staticmethod(lambda *a, **k: QMessageBox.Ok))

    class _FakeWin:
        def __init__(self):
            self._repair_worker = None
            self.busy = []
            self.records = []
            self.mac_checked = 0
            self.proxy_refreshed = 0
            self.status_msgs = []
            self.diag_calls = []

        def _release_worker(self, attr):
            setattr(self, attr, None)

        def _set_busy(self, busy, text=""):
            self.busy.append(busy)

        def statusBar(self):
            return types.SimpleNamespace(showMessage=lambda *a: self.status_msgs.append(a))

        def _add_repair_record(self, aid, result):
            self.records.append((aid, result.success))

        def _update_mac_state(self):
            self.mac_checked += 1

        def _refresh_proxy_label(self):
            self.proxy_refreshed += 1

        def run_diagnosis(self, **kw):
            self.diag_calls.append(kw)

    obj = _FakeWin()
    MainWindow._repair_done(obj, ("flush_dns", RepairResult(True, "已刷新 DNS 缓存")))

    assert obj.proxy_refreshed == 1                  # 修复后重读代理状态
    assert obj._repair_worker is None                # 忙碌标志已复位
    assert obj.records == [("flush_dns", True)]
    assert obj.mac_checked == 1
    assert obj.busy and obj.busy[-1] is False
    assert obj.diag_calls and obj.diag_calls[0].get("quick") is True
