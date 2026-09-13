"""UI 大页面（main_window / toolbox_page / wifi_page）分支补测。

定位：tests/test_ui_smoke.py 已覆盖构造与主干 happy-path；本文件针对 scoped
coverage 报告的未覆盖分支补齐（目标：三文件各自 >=82%）：

- main_window.py  窗口状态记忆 / 代理与 VPN 面板 / 诊断非静默路径与建议按钮去重 /
                  复制报告 / 双击修复 / 修复与组合修复全路径（ConfirmDialog、
                  QMessageBox.exec 全 mock）/ 设置保存（管理员自启 + 深色切换）/
                  日志读取 / 监控回调（status/action/suggest/提示音异常）/ 退出。
                  注：PySide6 离屏 + 多窗口 + 线程会话存在固有原生崩溃（smoke
                  文件头已知坑），故与 smoke 每进程约 6 个 MainWindow 的规模对齐，
                  把分支断言按 4 个窗口分组，而非每断言一个窗口。
- toolbox_page.py SpeedTestWorker 真实 run()（urlopen 假下载流 + 全失败）/
                  _LanScanWorker 无网段与 arp 解析 / _TracertWorker 停止与解析 /
                  _PortWorker 异常分支 / PortTab 选中保持与结束进程全分支
- wifi_page.py    懒扫描语义（首次 showEvent 才扫、hideEvent 停、恢复自动扫）/
                  scan_networks / profile_password 解析 / ScanWorker /
                  二维码失败与保存 / 已存密码读取与复制

安全（叠加 conftest 哨兵）：复用 test_ui_smoke 的 ui_offline_mocks（proc/psutil/
icmp/winsound/startfile/QMessageBox 静态方法离线化）；扫描 / 下载 / ping / arp /
tracert / 子进程 / urlopen / 注册表写 / 持久化 全部显式 monkeypatch 或走隔离目录，
绝不触网、不碰真实系统状态。
"""
import os
import sys
import types

import pytest

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from PySide6.QtCore import Qt, QThread, Signal
from PySide6.QtWidgets import (QApplication, QDialog, QFileDialog, QLabel,
                               QMessageBox, QSystemTrayIcon, QTreeWidgetItem)

# 与 smoke 相同护栏：离屏下托盘气泡统一 no-op（幂等，防原生崩溃）
QSystemTrayIcon.showMessage = lambda self, *a, **k: None

from PySide6.QtCore import qInstallMessageHandler


def _qt_message_silent(msg_type, context, message):
    """纯 no-op 的 Qt 消息处理器（覆盖 smoke 的写文件版）。

    解释器退出阶段 QApplication 析构遗留窗口时会发 Qt 消息；若此刻消息
    处理器还要做 Python 文件 I/O，极易在模块已部分卸载时触发原生
    fail-fast(0xC0000409)。静默处理器彻底消除这条路径。
    """


qInstallMessageHandler(_qt_message_silent)

# 复用 smoke 的离线 autouse 夹具与 MainWindow 工厂夹具（含后台线程回收）
from test_ui_smoke import FAKE_APS, main_win, make_main_window, ui_offline_mocks  # noqa: E402,F401

_KEEP = []   # 保活本文件创建的后台 worker 对象，防 QThread 提前析构


class _FakeDiagnosisWorker(QThread):
    """run_diagnosis 用的假线程：start() 可选地同步执行 run()，不起原生线程。

    经验：一次进程内真实启动的 QThread 越多，PySide6 离屏收尾 fail-fast
    (0xC0000409) 概率越高；用同步假 worker 保证分支覆盖的同时压低该概率。
    """

    progress = Signal(object)
    finished_ok = Signal(list)
    auto_run = False                                  # True: start() 即同步跑完

    def __init__(self, quick=False, parent=None):
        super().__init__(parent)
        self.quick = quick
        _KEEP.append(self)

    def start(self):
        if self.auto_run:
            self.run()                                # 同步完成，不起原生线程

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
        self.run()                                    # 同步执行：完成即发 got

    def run(self):
        from syskit.snapshot import take
        self.got.emit(take())


def _patch_no_thread_workers(monkeypatch):
    """把 main_window 的诊断/快照 worker 换成同步假实现（不启动原生线程）。"""
    from syskit.ui import main_window as mw
    monkeypatch.setattr(mw, "DiagnosisWorker", _FakeDiagnosisWorker)
    monkeypatch.setattr(mw, "SnapshotWorker", _FakeSnapshotWorker)


@pytest.fixture(autouse=True)
def _keepalive_cleanup():
    """测试结束、QApplication 仍存活时立即销毁 _KEEP 中的 QThread。

    若把这些（从未 start() 的）QThread 留到解释器退出阶段，QApplication 先亡，
    届时批量析构会触发 PySide6 原生 fail-fast(0xC0000409)。
    （注意：不主动冲刷 qtbot 挂起的窗口 deleteLater —— 与 smoke 一致，让窗口
    随进程收尾统一销毁，避免中途析构与下一个窗口构造产生竞态。）
    """
    yield
    _KEEP.clear()


# ============================================================
# syskit/ui/main_window.py
# ============================================================


def test_mw_guards_helpers_proxy_vpn_and_window_state(main_win, monkeypatch):
    """分组一（单窗口）：早退守卫 / 小工具 / 代理面板 / VPN 排查 / 窗口状态记忆。"""
    win = main_win
    from syskit import admin, proxy, repairs, settings as settings_mod, vpn
    from syskit.repairs import RepairResult

    # ---- _admin_banner：已是管理员 → 直接返回，不再追加控件（211）
    monkeypatch.setattr(admin, "is_admin", lambda: True)
    labels_before = win.statusBar().findChildren(QLabel)
    win._admin_banner()
    assert win.statusBar().findChildren(QLabel) == labels_before
    monkeypatch.setattr(admin, "is_admin", lambda: False)

    # ---- 忙碌守卫：诊断 / 修复 / 组合全部早退（424 / 783-784 / 818）
    win._busy = True
    win.run_diagnosis()
    win.run_repair("flush_dns")
    win.run_combo("fast")
    assert win._diag_worker is None and win._repair_worker is None
    assert win._combo_worker is None
    win._busy = False

    # ---- 快照刷新守卫：已有 worker 时不重复启动（355）
    win._snap_worker = object()
    win.refresh_adapters()
    assert not isinstance(win._snap_worker, QThread)
    win._snap_worker = None

    # ---- 忙碌文案上状态栏（206）
    win._set_busy(True, "忙碌中…")
    assert win.statusBar().currentMessage() == "忙碌中…"
    win._set_busy(False)

    # ---- 配置文件下拉 + MAC 状态 + 参数（612 / 619-621）
    monkeypatch.setattr(repairs, "wlan_profiles", lambda: ["HomeWiFi"])
    monkeypatch.setattr(repairs, "mac_random_state", lambda p: False)
    win._refresh_profiles()
    assert win.cmb_profile.currentText() == "HomeWiFi"
    assert "关闭" in win.lbl_mac.text()
    assert win._profile_param() == {"profile": "HomeWiFi"}

    # ---- 非日志页时 _reload_log 早退（768）
    win.switch_page(0)
    win._reload_log()

    # ---- 修复记录上限 60 条 + 未知动作标题回退（624-630）
    for _ in range(61):
        win._add_repair_record("flush_dns", RepairResult(True, "ok"))
    assert win.list_repair_log.count() == 60
    win._add_repair_record("ghost_x", RepairResult(False, "未知"))
    assert win.list_repair_log.count() == 60
    assert "ghost_x" in win.list_repair_log.item(0).text()

    # ---- 代理状态标签两分支 + 开关两分支（332-351）
    status = {"enabled": True, "server": "127.0.0.1:7890",
              "autoconfig": False, "pac_url": ""}
    monkeypatch.setattr(proxy, "get_proxy_status", lambda: dict(status))

    win._refresh_proxy_label()                       # 启用分支
    assert "127.0.0.1:7890" in win.lbl_proxy.text()
    assert win.btn_proxy.text() == "关闭代理"
    win._toggle_proxy()                              # 启用 → run_repair（非管理员被拒，已 mock）
    status.update(enabled=False, server="")
    win._refresh_proxy_label()                       # 未启用分支（338-341）
    assert "未启用" in win.lbl_proxy.text()
    assert win.btn_proxy.text() == "代理设置…"
    win._toggle_proxy()                              # 未启用 → os.startfile（已 mock no-op）

    # ---- VPN 排查：成功 → 汇总弹窗（exec 已 mock）；异常 → warning 早退（307-327）
    steps = [{"level": "ok", "name": "系统代理", "detail": "未启用"},
             {"level": "warn", "name": "隧道网卡", "detail": "检测到 TUN"},
             {"level": "fail", "name": "持久路由", "detail": "存在 hijack 路由"},
             {"level": "info", "name": "默认出口", "detail": "192.168.1.1"},
             {"level": "weird", "name": "未知级别", "detail": "?"}]
    monkeypatch.setattr(vpn, "vpn_checklist", lambda snap=None: steps)
    monkeypatch.setattr(vpn, "summarize", lambda checklist: "总体：注意")
    monkeypatch.setattr(QMessageBox, "exec",
                        lambda self, *a, **k: QMessageBox.StandardButton.Ok)
    win._vpn_checklist()

    def _vpn_boom(snap=None):
        raise RuntimeError("vpn 排查炸了")
    monkeypatch.setattr(vpn, "vpn_checklist", _vpn_boom)
    win._vpn_checklist()

    # ---- 窗口状态记忆（158-159 / 163-169）
    # 回归：此前 bytes(QByteArray).toBase64() 误用使保存静默失败、状态从不落盘（已修复）。
    win.switch_page(5)
    win._save_window_state()
    saved = settings_mod.load()
    assert saved.get("last_page") == 5
    assert saved.get("win_geometry")                     # base64 串非空
    win._restore_window_state()                      # 有 win_geometry → restoreGeometry
    assert win.stack.currentIndex() == 5
    assert win.nav.currentRow() == 7                 # 「⚡ 工具箱」所在行


def test_mw_diagnosis_visible_direct_done_report_and_item_fix(main_win, monkeypatch, qtbot):
    """分组二（单窗口）：非静默诊断全链路 / _diag_done 直调（去重与未知动作）/ 复制报告 / 双击修复。"""
    win = main_win
    _patch_no_thread_workers(monkeypatch)
    from syskit.netcheck import CheckResult

    # ---- 非静默诊断：跳诊断页 / 忙碌态 / 进度条（426-430 / 442-444）
    win.run_diagnosis(quick=False, goto_diag=True)
    assert win.stack.currentIndex() == 3
    assert win._busy is True
    assert win.btn_diag.text() == "诊断中…"
    assert not win.pb_diag.isHidden()
    assert win.tree_diag.topLevelItemCount() == 0     # 旧结果已清
    assert win.fix_btn_row.count() == 0               # 旧建议按钮已清空（442-444）

    # ---- 手动同步完成诊断（等效线程回报），验证完成态（489）
    win._diag_worker.run()
    assert win._diag_worker is None and win._snap_worker is None
    assert win._busy is False
    assert win.btn_diag.text() == "开始完整诊断"
    assert win.pb_diag.isHidden()
    assert win.statusBar().currentMessage().startswith("诊断完成")

    # ---- 直调 _diag_done：建议按钮去重（476）/ 未知动作跳过（479）
    results = [
        CheckResult("dns_cfg", "静态 DNS 残留", "fail", "8.8.8.8",
                    fix_ids=["dns_auto", "dns_auto"]),      # 重复 fix_id
        CheckResult("nla2", "NlaSvc", "warn", "手动启动",
                    fix_ids=["ghost_action"]),              # 未知动作
        CheckResult("real_conn", "真实连通性探测", "pass", "204探测:OK"),
    ]
    win._diag_done(results, silent=False)
    qtbot.wait_until(lambda: win._snap_worker is None, timeout=20000)
    titles = [win.fix_btn_row.itemAt(i).widget().text()
              for i in range(win.fix_btn_row.count())
              if win.fix_btn_row.itemAt(i).widget()]
    assert titles.count("DNS 恢复为 DHCP 自动获取") == 1    # 去重后仅一个
    assert not any("ghost" in t for t in titles)
    assert win.statusBar().currentMessage().startswith("诊断完成")
    assert win.pb_diag.isHidden()
    assert "204探测:OK" in win.lbl_probe.text()

    # ---- 复制报告（493-501）
    win._diag_progress(CheckResult("dns_cfg", "静态 DNS 残留", "fail",
                                   "8.8.8.8", fix_ids=["dns_auto"]))
    win._copy_report()
    text = QApplication.clipboard().text()
    assert "SysKit 诊断报告" in text and "静态 DNS 残留" in text and "结论" in text

    # ---- 双击条目修复守卫（504-506）
    win.tree_diag.clear()
    bare = CheckResult("x", "无修复项", "pass", "-")
    it = QTreeWidgetItem(["√", bare.name, bare.detail])
    it.setData(0, Qt.UserRole, bare)
    win._diag_item_fix(it, 0)                       # 无 fix_ids → 不触发
    assert win._repair_worker is None
    bad = CheckResult("y", "需修复", "fail", "x", fix_ids=["dns_auto"])
    it2 = QTreeWidgetItem(["×", bad.name, bad.detail])
    it2.setData(0, Qt.UserRole, bad)
    win._diag_item_fix(it2, 0)                      # 非管理员 + 改配置类 → 提示早退（已 mock）
    assert win._repair_worker is None


def test_mw_repair_and_combo_full_flows(main_win, monkeypatch, qtbot):
    """分组三（单窗口）：修复执行全路径（782-814）与组合修复全路径（817-873）。

    RepairWorker / ComboWorker 换成 start() 即同步发完信号的假线程，不真跑系统动作。
    """
    win = main_win
    _patch_no_thread_workers(monkeypatch)
    monkeypatch.setattr(_FakeDiagnosisWorker, "auto_run", True)
    from syskit import admin, repairs
    from syskit.repairs import RepairResult
    from syskit.ui import main_window as mw
    from syskit.ui.confirm_dialog import ConfirmDialog

    class _FakeRepairWorker(QThread):
        done = Signal(object)

        def __init__(self, aid, param=None, parent=None):
            super().__init__(parent)
            self.aid, self.param = aid, param or {}
            _KEEP.append(self)

        def start(self):
            self.done.emit((self.aid, RepairResult(True, f"已执行 {self.aid}",
                                                   need_reboot=True)))

    class _FakeComboWorker(QThread):
        item_done = Signal(object)
        all_done = Signal()

        def __init__(self, action_ids, parent=None):
            super().__init__(parent)
            self.action_ids = list(action_ids)
            _KEEP.append(self)

        def start(self):
            for aid in self.action_ids:
                self.item_done.emit((aid, RepairResult(True, f"完成 {aid}")))
            self.all_done.emit()

    def _wait_drained():
        qtbot.wait_until(lambda: win._diag_worker is None and win._snap_worker is None,
                         timeout=20000)

    # ---- run_repair 守卫（782-798）
    win.run_repair("ghost_x")                       # 未知动作早退
    assert win._repair_worker is None
    win._repair_worker = object()
    win.run_repair("flush_dns")                     # 后台占用早退
    assert not isinstance(win._repair_worker, QThread)
    win._repair_worker = None
    monkeypatch.setattr(admin, "is_admin", lambda: False)
    win.run_repair("dns_auto")                      # 非管理员 + 改配置类 → warning 早退
    assert win._repair_worker is None

    # ---- 管理员 + 确认对话框 取消 / 接受
    monkeypatch.setattr(mw, "RepairWorker", _FakeRepairWorker)
    monkeypatch.setattr(admin, "is_admin", lambda: True)
    monkeypatch.setattr(ConfirmDialog, "exec",
                        lambda self, *a, **k: QDialog.DialogCode.Rejected)
    win.run_repair("dns_auto")                      # 取消 → 不执行
    assert win._repair_worker is None and win._busy is False
    monkeypatch.setattr(ConfirmDialog, "exec",
                        lambda self, *a, **k: QDialog.DialogCode.Accepted)
    win.run_repair("dns_auto", {"adapter": "WLAN"})  # 接受 → fake worker 同步完成
    assert win._repair_worker is None and win._busy is False

    # ---- 安全动作免确认（flush_dns）：记录 + 自动复诊（801-814）
    monkeypatch.setattr(admin, "is_admin", lambda: False)
    win.run_repair("flush_dns")
    _wait_drained()
    assert win._repair_worker is None and win._busy is False
    assert win.list_repair_log.count() >= 2
    assert "√" in win.list_repair_log.item(0).text()
    assert "已执行 flush_dns" in win.list_repair_log.item(0).text()

    # ---- run_combo：忙碌守卫（818）
    win._busy = True
    win.run_combo("fast")
    assert win._combo_worker is None
    win._busy = False

    # ---- stack 无管理员 → warning 早退（820-822）
    monkeypatch.setattr(admin, "is_admin", lambda: False)
    win.run_combo("stack")
    assert win._combo_worker is None

    # ---- 全部受限且无管理员 → 提示「未执行任何动作」（839-844）
    monkeypatch.setattr(repairs, "COMBOS",
                        dict(repairs.COMBOS, onlycfg=("仅配置", ["dns_auto", "power_off"])))
    win.run_combo("onlycfg")
    assert win._combo_worker is None and win._busy is False

    # ---- 管理员 + 确认接受，但最终组合确认弹窗取消 → 不执行（845-852）
    monkeypatch.setattr(admin, "is_admin", lambda: True)
    monkeypatch.setattr(ConfirmDialog, "exec",
                        lambda self, *a, **k: QDialog.DialogCode.Accepted)
    monkeypatch.setattr(QMessageBox, "exec",
                        lambda self, *a, **k: QMessageBox.StandardButton.Cancel)
    win.run_combo("full")
    assert win._combo_worker is None and win._busy is False

    # ---- 最终确认 Ok + 未知动作跳过 + fake ComboWorker 逐项完成（853-873）
    monkeypatch.setattr(repairs, "COMBOS",
                        dict(repairs.COMBOS, mix=("混合修复", ["flush_dns", "ghost_x", "dns_auto"])))
    monkeypatch.setattr(QMessageBox, "exec",
                        lambda self, *a, **k: QMessageBox.StandardButton.Ok)
    monkeypatch.setattr(mw, "ComboWorker", _FakeComboWorker)
    win.run_combo("mix")
    _wait_drained()
    assert win._combo_worker is None and win._busy is False
    assert win.list_repair_log.count() >= 4         # 2 条修复 + 2 条组合记录


def test_mw_monitor_settings_logs_then_quit(main_win, monkeypatch, qtbot):
    """分组四（单窗口，quit 收尾）：监控回调 / 提示音异常 / 设置保存 / 日志读取 / 退出路径。"""
    win = main_win
    import winsound

    from syskit import admin, autostart, proxy, settings as settings_mod
    from syskit.repairs import RepairResult
    from syskit.ui import theme

    # ---- 监控回调：状态切换/事件时间线/托盘通知（891）/ 自动修复记录（894-898）/ 建议（911-914）
    def _boom(*a, **k):
        raise RuntimeError("无声卡")

    monkeypatch.setattr(winsound, "PlaySound", _boom)

    win._on_monitor_status("ok", "已恢复联网")        # 首次状态 + ok+message → 托盘通知
    assert win._last_state == "ok"
    assert win.eventstore.items()[0]["state"] == "ok"
    win._on_monitor_status("no_net", "探测失败 ×2")   # 状态切换 → 记录 + 提示音（异常被吞 907-908）
    assert win.eventstore.items()[0]["state"] == "no_net"
    assert win.tray._state == "no_net"
    win._play_alert("ok")                             # 直接调用亦不抛

    win._on_monitor_action(("flush_dns", RepairResult(True, "已刷新")))
    win._on_monitor_action(("ghost_x", RepairResult(False, "失败")))
    assert "ghost_x" in win.list_repair_log.item(0).text()
    win._on_monitor_suggest(("flush_dns", "频繁断网"))
    win._on_monitor_suggest(("ghost_x", "未知"))      # 未知动作 → 静默

    # ---- 设置保存：管理员自启开关两分支（722-725）+ 深色即时切换（729-736）
    monkeypatch.setattr(admin, "is_admin", lambda: True)
    monkeypatch.setattr(proxy, "get_proxy_status",
                        lambda: {"enabled": False, "server": "",
                                 "autoconfig": False, "pac_url": ""})
    state = {"enabled": False}
    calls = []
    monkeypatch.setattr(autostart, "enabled", lambda: state["enabled"])

    def _enable():
        calls.append("enable")
        state["enabled"] = True
        return True

    def _disable():
        calls.append("disable")
        state["enabled"] = False
        return True

    monkeypatch.setattr(autostart, "enable", _enable)
    monkeypatch.setattr(autostart, "disable", _disable)

    win.chk_start.setChecked(True)
    win._save_settings()                              # 勾选且未启用 → enable
    assert calls == ["enable"]
    win.chk_start.setChecked(False)
    win._save_settings()                              # 取消勾选且已启用 → disable
    assert calls == ["enable", "disable"]

    try:
        assert theme.is_dark() is False
        win.chk_dark.setChecked(True)
        win._save_settings()                          # 主题切换分支 + 复诊结论重渲染
        assert theme.is_dark() is True
        assert settings_mod.load()["dark_mode"] is True
        assert settings_mod.load()["poll_interval_sec"] == win.spin_interval.value()
        win.chk_dark.setChecked(False)
        win._save_settings()                          # 切回浅色，避免主题状态泄漏
        assert theme.is_dark() is False
    finally:
        if theme.is_dark():
            theme.apply(QApplication.instance(), dark=False)

    # ---- 日志页读取隔离目录中的 syskit.log（775-776），error.log 缺失 → 占位文案
    from syskit import config

    os.makedirs(config.LOG_DIR, exist_ok=True)
    with open(os.path.join(config.LOG_DIR, "syskit.log"), "w", encoding="utf-8") as f:
        f.write("2026-09-11 INFO SysKit 启动\n" * 3)
    win.switch_page(9)                            # 日志页（守卫按对象比较；曾因旧索引 8 假绿）
    win._reload_log()
    assert "SysKit 启动" in win.txt_log.toPlainText()
    win.cmb_log_src.setCurrentIndex(1)                # error.log 尚无内容
    assert "error.log 尚无内容" in win.txt_log.toPlainText()

    # ---- minimize_to_tray=False → close 走退出分支：停线程/隐藏托盘（923-938）
    settings_mod.save({"minimize_to_tray": False})
    win.show()
    qtbot.wait(20)
    win.close()
    QApplication.processEvents()
    assert not win.isVisible()
    # close 触发保存窗口状态（回归：此前保存静默失败从不落盘，已修复）
    assert settings_mod.load().get("win_geometry")
    assert not win.tele.isRunning()                   # 遥测线程已停
    assert not win.tray.isVisible()


# ============================================================
# syskit/ui/toolbox_page.py
# ============================================================


def test_speed_worker_download_and_all_fail(qtbot, monkeypatch):
    """SpeedTestWorker.run()：假下载流（进度回报 + 成功均速）与全部源不可达（36-78）。"""
    import time as time_mod
    import urllib.request

    import syskit.ui.toolbox_page as tp

    class _FakeResp:
        def __init__(self, chunks, delay=0.0):
            self._chunks = list(chunks)
            self._delay = delay

        def read(self, _n=-1):
            if self._delay:
                time_mod.sleep(self._delay)
                self._delay = 0.0          # 仅首个读阻塞一次，让测量线程先回报
            return self._chunks.pop(0) if self._chunks else b""

        def close(self):
            pass

    monkeypatch.setattr(urllib.request, "urlopen",
                        lambda req, timeout=10: _FakeResp([b"x" * 700_000], delay=1.1))
    w = tp.SpeedTestWorker(seconds=1)
    _KEEP.append(w)
    done, progress = [], []
    w.done.connect(lambda ok, bps: done.append((ok, bps)))
    w.progress.connect(lambda b, mb: progress.append((b, mb)))
    w.run()
    assert done and done[0][0] is True and done[0][1] > 0
    qtbot.wait_until(lambda: len(progress) >= 1, timeout=5000)   # 测量线程至少回报一次

    def _refuse(req, timeout=10):
        raise OSError("refused")

    monkeypatch.setattr(urllib.request, "urlopen", _refuse)
    w2 = tp.SpeedTestWorker()
    _KEEP.append(w2)
    done2 = []
    w2.done.connect(lambda ok, bps: done2.append((ok, bps)))
    w2.run()
    assert done2 == [(False, 0.0)]


def test_speed_tab_start_and_busy_guard(qtbot, monkeypatch):
    """SpeedTestTab.start()：按钮/进度条/文案状态 + 忙碌守卫（116-124）。"""
    import syskit.ui.toolbox_page as tp

    class _FakeSpeedWorker(QThread):
        progress = Signal(float, float)
        done = Signal(bool, float)

        def start(self):
            pass

    monkeypatch.setattr(tp, "SpeedTestWorker", _FakeSpeedWorker)
    tab = tp.SpeedTestTab()
    qtbot.addWidget(tab)
    assert tab._worker is None
    tab.start()
    assert isinstance(tab._worker, _FakeSpeedWorker)
    assert not tab.btn_start.isEnabled()
    assert not tab.pb.isHidden()
    assert "测速中" in tab.lbl_status.text()
    tab.start()                                       # 忙碌守卫：不重复启动
    tab._on_done(True, 2_000_000.0)
    assert tab._worker is None and tab.btn_start.isEnabled()
    assert tab.pb.isHidden()


def test_lan_worker_no_subnet_arp_parse_and_tab_start(qtbot, monkeypatch):
    """_LanScanWorker：无网段 → 空结果；有网段 → ping 全通 + arp MAC 解析；LanTab.start（160-215）。"""
    import socket as socket_mod
    import subprocess

    import psutil
    from syskit import icmp

    import syskit.ui.toolbox_page as tp

    monkeypatch.setattr(psutil, "net_if_addrs", lambda: {})
    w0 = tp._LanScanWorker()
    _KEEP.append(w0)
    out0 = []
    w0.done.connect(out0.append)
    w0.run()
    assert out0 == [[]]

    monkeypatch.setattr(psutil, "net_if_addrs", lambda: {
        "Eth": [types.SimpleNamespace(family=socket_mod.AF_INET, address="127.0.0.1"),
                types.SimpleNamespace(family=socket_mod.AF_INET6, address="::1"),
                types.SimpleNamespace(family=socket_mod.AF_INET, address="169.254.1.2"),
                types.SimpleNamespace(family=socket_mod.AF_INET, address="192.168.1.5")]})
    monkeypatch.setattr(icmp, "ping", lambda ip, timeout_ms=400: 3.0)
    monkeypatch.setattr(subprocess, "run", lambda *a, **k: types.SimpleNamespace(
        stdout=b"  192.168.1.2          aa-bb-cc-dd-ee-ff     dynamic\n"))
    w1 = tp._LanScanWorker()
    _KEEP.append(w1)
    out1 = []
    w1.done.connect(out1.append)
    w1.run()
    rows = out1[0]
    assert len(rows) == 254 and rows[0][0] == "192.168.1.1"      # 数值序
    row2 = next(r for r in rows if r[0] == "192.168.1.2")
    assert row2 == ("192.168.1.2", "aa-bb-cc-dd-ee-ff", "3")     # MAC 已解析

    class _FakeLanWorker(QThread):
        done = Signal(list)

        def start(self):
            pass

    monkeypatch.setattr(tp, "_LanScanWorker", _FakeLanWorker)
    tab = tp.LanTab()
    qtbot.addWidget(tab)
    tab.start()
    assert isinstance(tab._worker, _FakeLanWorker)
    assert not tab.btn_scan.isEnabled() and "扫描中" in tab.lbl_status.text()
    tab.start()                                       # 忙碌守卫
    tab._done([("192.168.1.2", "aa:bb:cc:dd:ee:ff", "3")])
    assert tab.btn_scan.isEnabled() and tab.tbl.rowCount() == 1


def test_tracert_worker_stop_parse_error_and_tab_guards(qtbot, monkeypatch):
    """_TracertWorker.stop/解析/异常静默 + PingRouteTab 空目标与忙碌守卫（334-358 / 408 / 415）。"""
    import subprocess

    import syskit.ui.toolbox_page as tp

    class _FakeProc:
        def __init__(self, lines):
            self.stdout = iter(lines)
            self.killed = False

        def kill(self):
            self.killed = True

    early = _FakeProc([b"  1    1 ms    10.0.0.1"])
    monkeypatch.setattr(subprocess, "Popen", lambda *a, **k: early)
    w_stop = tp._TracertWorker("1.2.3.4")
    _KEEP.append(w_stop)
    w_stop.stop()                                     # 预先停止
    w_stop.run()
    assert early.killed                               # 首行即 kill 并返回

    fake = _FakeProc([b"  1    2 ms   10.0.0.1",
                      b"  request timeout",
                      b"  2    *    *"])
    monkeypatch.setattr(subprocess, "Popen", lambda *a, **k: fake)
    w = tp._TracertWorker("223.5.5.5")
    _KEEP.append(w)
    lines = []
    w.line.connect(lines.append)
    w.run()
    assert lines == [{"hop": 1, "ms": [2], "ip": "10.0.0.1"},
                     {"hop": 2, "ms": [], "ip": "*"}]

    def _boom(*a, **k):
        raise RuntimeError("no tracert")

    monkeypatch.setattr(subprocess, "Popen", _boom)
    w_err = tp._TracertWorker("x")
    _KEEP.append(w_err)
    w_err.run()                                       # 异常被吞，不抛出

    tab = tp.PingRouteTab()
    qtbot.addWidget(tab)
    tab.ed_target.clear()
    tab._ping_once()                                  # 空目标早退
    tab.tracert()                                     # 空目标早退
    class _Running:
        def stop(self):
            self.stopped = True
    tab._tracert = _Running()                          # 运行中 → 再点变中止
    tab.ed_target.setText("223.5.5.5")
    tab.tracert()
    assert tab._tracert.stopped is True
    assert tab.btn_route.text() == "路由追踪"
    tab._tracert = None


def test_port_worker_access_denied_and_error_rows(monkeypatch):
    """_PortWorker：AccessDenied 整体早退 / 单条连接异常跳过（445-467）。"""
    import psutil

    import syskit.ui.toolbox_page as tp

    monkeypatch.setattr(psutil, "net_connections",
                        lambda kind="inet": (_ for _ in ()).throw(psutil.AccessDenied("x")))
    w0 = tp._PortWorker()
    _KEEP.append(w0)
    out0 = []
    w0.done.connect(out0.append)
    w0.run()
    assert out0 == [[]]

    def _raise_no_such(pid):
        raise psutil.NoSuchProcess(pid)

    class _BadName:
        def name(self):
            raise RuntimeError("boom")

    conns = [types.SimpleNamespace(pid=5, laddr=None, raddr=None, type=1, status="NONE"),
             types.SimpleNamespace(pid=6, laddr=None, raddr=None, type=2, status="NONE")]
    monkeypatch.setattr(psutil, "net_connections", lambda kind="inet": conns)
    # pid=5 → NoSuchProcess；pid=6 → name() 抛通用异常：两行都被跳过
    monkeypatch.setattr(psutil, "Process",
                        lambda pid: _raise_no_such(pid) if pid == 5 else _BadName())
    w1 = tp._PortWorker()
    _KEEP.append(w1)
    out1 = []
    w1.done.connect(out1.append)
    w1.run()
    assert out1 == [[]]


def test_port_tab_refresh_guard_selection_restore(qtbot, monkeypatch):
    """PortTab：刷新守卫（520）/ 选中 PID 跨刷新保持（533-536 / 542-546）。"""
    import syskit.ui.toolbox_page as tp

    class _FakePortWorker(QThread):
        done = Signal(list)

        def start(self):
            pass

    monkeypatch.setattr(tp, "_PortWorker", _FakePortWorker)
    tab = tp.PortTab()                                # 懒加载：构造期不扫描
    qtbot.addWidget(tab)
    tab.refresh()                                     # 显式触发（走假 worker，不启动线程）
    assert isinstance(tab._worker, _FakePortWorker)
    tab._done([])                                     # 手动完成刷新
    assert "共 0 条" in tab.lbl_status.text()

    tab._worker = object()
    tab.refresh()                                     # 忙碌守卫
    assert not isinstance(tab._worker, QThread)
    tab._worker = None

    rows = [
        {"proto": "TCP", "tcp": True, "laddr": "127.0.0.1:8443", "raddr": "0.0.0.0:0",
         "status": "LISTEN", "pid": 4321, "name": "websvc.exe", "exe": "C:\\a.exe"},
        {"proto": "UDP", "tcp": False, "laddr": "0.0.0.0:5353", "raddr": "-",
         "status": "-", "pid": 0, "name": "System", "exe": ""},
    ]
    tab._done(rows)
    assert tab.tbl.rowCount() == 2

    tab.tbl.setCurrentCell(0, 0)
    tab._done(rows)                                   # TCP 行选中 → 刷新后保持
    assert tab.tbl.currentRow() == 0
    assert tab.tbl.item(0, 4).text() == "4321"

    tab.tbl.setCurrentCell(1, 0)
    tab._done(rows)                                   # pid 列为 "-" → ValueError → 放弃恢复
    assert tab._rows is rows


def test_port_tab_kill_selected_all_branches(qtbot, monkeypatch):
    """结束进程全分支：未选中/隐藏行/系统级/取消/成功/三类异常（572-598）。"""
    import psutil

    import syskit.ui.toolbox_page as tp

    infos = []
    warns = []

    def _info(*a, **k):
        infos.append(a[2])
        return QMessageBox.StandardButton.Ok

    def _warn(*a, **k):
        warns.append(a[2])
        return QMessageBox.StandardButton.Ok

    monkeypatch.setattr(QMessageBox, "information", staticmethod(_info))
    monkeypatch.setattr(QMessageBox, "warning", staticmethod(_warn))

    class _FakePortWorker(QThread):
        done = Signal(list)

        def start(self):
            pass

    monkeypatch.setattr(tp, "_PortWorker", _FakePortWorker)
    tab = tp.PortTab()
    qtbot.addWidget(tab)
    tab._worker = None                                # 懒加载：无构造期 worker，直接就绪
    rows = [
        {"proto": "TCP", "tcp": True, "laddr": "127.0.0.1:8443", "raddr": "0.0.0.0:0",
         "status": "LISTEN", "pid": 4321, "name": "websvc.exe", "exe": "C:\\a.exe"},
        {"proto": "UDP", "tcp": False, "laddr": "0.0.0.0:5353", "raddr": "-",
         "status": "-", "pid": 0, "name": "System", "exe": ""},
    ]
    tab._done(rows)

    tab.tbl.setCurrentCell(-1, -1)
    tab._kill_selected()                              # 未选中
    assert "选中" in infos[-1]

    tab.tbl.setCurrentCell(0, 0)
    tab.tbl.setRowHidden(0, True)
    tab._kill_selected()                              # 隐藏行视为未选中
    assert "选中" in infos[-1]
    tab.tbl.setRowHidden(0, False)

    tab.tbl.setCurrentCell(1, 0)
    tab._kill_selected()                              # 系统级（pid=0）
    assert "系统级" in infos[-1]

    tab.tbl.setCurrentCell(0, 0)
    questions = []

    def _question(*a, **k):
        questions.append(a[2])
        return QMessageBox.StandardButton.No

    monkeypatch.setattr(QMessageBox, "question", staticmethod(_question))
    tab._kill_selected()                              # 用户取消 → 不执行
    assert questions and questions[-1].startswith("确定结束进程")

    killed = []

    def _yes(*a, **k):
        questions.append(a[2])
        return QMessageBox.StandardButton.Yes

    monkeypatch.setattr(QMessageBox, "question", staticmethod(_yes))
    monkeypatch.setattr(psutil, "Process",
                        lambda pid: types.SimpleNamespace(kill=lambda: killed.append(pid)))
    tab._kill_selected()                              # 成功
    assert killed == [4321]

    def _raiser(exc):
        def _p(pid):
            raise exc
        return _p

    monkeypatch.setattr(psutil, "Process", _raiser(psutil.AccessDenied("x")))
    tab._kill_selected()
    assert "权限不足" in warns[-1]                    # AccessDenied → warning
    monkeypatch.setattr(psutil, "Process", _raiser(psutil.NoSuchProcess(4321)))
    tab._kill_selected()
    assert "自行退出" in infos[-1]                    # NoSuchProcess → information
    monkeypatch.setattr(psutil, "Process", _raiser(RuntimeError("boom")))
    tab._kill_selected()
    assert "结束进程失败" in warns[-1]                # 其它异常 → warning


# ============================================================
# syskit/ui/wifi_page.py
# ============================================================

NETSH_TEXT = """接口名称: WLAN
There are 3 networks currently visible.

SSID 1 : HomeWiFi
    身份验证: WPA2-PSK

SSID: HomeWiFi
    BSSID 1: aa:bb:cc:dd:ee:01
        信号: 82%
        信道: 6
        无线电类型: 802.11ax

SSID:
    身份验证: Open
    BSSID 1: aa:bb:cc:dd:ee:02
        信号: 40%

SSID: Office5G
    BSSID 1: aa:bb:cc:dd:ee:03
        信号: 55%
        信道: 36
        身份验证: WPA3-SAE
"""


def test_wifi_signal_color_and_scan_networks_parsing(monkeypatch):
    """_signal_color 兜底 + scan_networks 全量解析：SSID 跳过/隐藏网络/auth 回填/频段/排序（24 / 32-74）。"""
    from syskit import proc

    import syskit.ui.wifi_page as wp

    assert wp._signal_color(-5) == "#c62828"          # 兜底分支
    assert wp._signal_color(60) == "#2e9e44"

    monkeypatch.setattr(proc, "run",
                        lambda cmd, timeout=20: proc.CmdResult(code=0, stdout=NETSH_TEXT, stderr=""))
    aps = wp.scan_networks()
    assert [a["ssid"] for a in aps] == ["HomeWiFi", "Office5G", "(隐藏)"]   # 按信号降序
    assert aps[0]["signal"] == 82 and aps[0]["channel"] == 6
    assert aps[0]["band"] == "2.4G" and aps[0]["radio"] == "802.11ax"
    assert aps[1]["band"] == "5G"
    assert aps[1]["auth"] == "WPA3-SAE"               # BSSID 之后才出现的认证 → 回填
    assert aps[2]["auth"] == "Open" and aps[2]["band"] == "?"

    monkeypatch.setattr(proc, "run",
                        lambda cmd, timeout=20: proc.CmdResult(
                            code=5, stdout="需要开启「位置」服务 (Location)", stderr=""))
    with pytest.raises(RuntimeError, match="位置"):
        wp.scan_networks()


def test_wifi_profile_password_parse(monkeypatch):
    """profile_password：认证与密码解析 + 空输出（79-89）。"""
    from syskit import proc

    import syskit.ui.wifi_page as wp

    out = "    身份验证: WPA2-PSK\n    关键内容: my_secret\n"
    monkeypatch.setattr(proc, "run",
                        lambda cmd, timeout=15: proc.CmdResult(code=0, stdout=out, stderr=""))
    assert wp.profile_password("HomeWiFi") == ("WPA2-PSK", "my_secret")

    monkeypatch.setattr(proc, "run",
                        lambda cmd, timeout=15: proc.CmdResult(code=1, stdout="", stderr=""))
    assert wp.profile_password("Cafe") == ("", "")


def test_wifi_scan_worker_done_and_failed(monkeypatch):
    """ScanWorker.run()：成功发 done、异常发 failed（97-100）。"""
    import syskit.ui.wifi_page as wp

    w = wp.ScanWorker()
    _KEEP.append(w)
    out, errs = [], []
    w.done.connect(out.append)
    w.failed.connect(errs.append)
    monkeypatch.setattr(wp, "scan_networks", lambda: FAKE_APS)
    w.run()
    assert out == [FAKE_APS] and errs == []

    def _boom():
        raise RuntimeError("需要位置服务")

    monkeypatch.setattr(wp, "scan_networks", _boom)
    w2 = wp.ScanWorker()
    _KEEP.append(w2)
    out2, errs2 = [], []
    w2.done.connect(out2.append)
    w2.failed.connect(errs2.append)
    w2.run()
    assert out2 == [] and errs2 == ["需要位置服务"]


def test_wifi_lazy_scan_lifecycle(qtbot, monkeypatch):
    """懒扫描语义：构造不扫 / 首次 showEvent 扫一次 / 再次 show 不重扫 / 自动刷新随可见性启停（123-148 / 183）。"""
    import syskit.ui.wifi_page as wp

    starts = []

    class _FakeScanWorker(QThread):
        done = Signal(list)
        failed = Signal(str)

        def start(self):
            starts.append(1)
            self.done.emit([])                        # 同步完成，_scan_worker 复位

    monkeypatch.setattr(wp, "ScanWorker", _FakeScanWorker)
    page = wp.WifiPage()
    qtbot.addWidget(page)

    assert starts == [] and page._initial_scanned is False      # 构造不扫描
    page.show()
    assert starts == [1] and page._initial_scanned is True      # 首次可见才扫
    page.hide()
    page.show()
    assert starts == [1]                                        # 已扫过 → 不重扫

    page.chk_auto.setChecked(True)                              # 可见时开启 → 定时器启动
    assert page._scan_timer.isActive()
    page.hide()                                                 # 隐藏 → 停止并记忆
    assert not page._scan_timer.isActive() and page._resume_auto is True
    page.show()                                                 # 恢复自动扫描
    assert page._scan_timer.isActive() and page._resume_auto is False
    assert starts == [1]

    page._auto_scan_tick()                                      # 可见 → 自动扫一次
    assert starts == [1, 1]
    page.hide()
    page._auto_scan_tick()                                      # 不可见 → 跳过
    assert starts == [1, 1]

    page.chk_auto.setChecked(False)                             # 关闭 → 停定时器
    assert not page._scan_timer.isActive()

    page._scan_worker = object()
    page.scan()                                                 # worker 占用 → 守卫早退
    assert starts == [1, 1]
    page._scan_worker = None


def test_wifi_scan_failed_and_channel_chart_skip(qtbot):
    """扫描失败文案（210-212）+ 信道统计跳过无信道 AP（231）。"""
    import syskit.ui.wifi_page as wp

    page = wp.WifiPage()
    qtbot.addWidget(page)
    page._scan_failed("位置服务未开启")
    assert "扫描失败" in page.lbl_scan.text() and "位置服务未开启" in page.lbl_scan.text()
    assert page.btn_scan.isEnabled() and page._scan_worker is None

    aps = [dict(a) for a in FAKE_APS] + [
        {"ssid": "NoCh", "bssid": "AA:BB:CC:DD:EE:09", "signal": 30,
         "channel": 0, "radio": "", "auth": "", "band": "?"}]
    page._aps = aps
    page._update_channel_charts()
    assert "推荐" in page.lbl_advice.text()


def test_wifi_gen_qr_empty_failure_and_save(qtbot, monkeypatch, tmp_path):
    """二维码：空配置早退（287）/ 生成失败提示（313-314）/ 保存守卫与落盘（317-323）。"""
    import syskit.ui.wifi_page as wp

    page = wp.WifiPage()
    qtbot.addWidget(page)
    page._gen_qr()                                    # 空配置文件 → 早退
    assert not page.btn_save_qr.isEnabled()

    fake_qr = types.ModuleType("qrcode")

    def _boom(*a, **k):
        raise RuntimeError("qrcode 不可用")

    fake_qr.make = _boom
    monkeypatch.setitem(sys.modules, "qrcode", fake_qr)
    monkeypatch.setattr(wp, "profile_password", lambda name: ("WPA2-PSK", "pw"))
    page.cmb_share.addItem("HomeWiFi")
    page.cmb_share.setCurrentText("HomeWiFi")
    page._gen_qr()                                    # 生成失败 → warning（已 mock）
    assert not page.btn_save_qr.isEnabled()

    if hasattr(page, "_qr_png"):
        del page._qr_png
    page._save_qr()                                   # 无二维码 → 早退
    page._qr_png = b"PNGDATA"

    target = {"path": ""}
    monkeypatch.setattr(QFileDialog, "getSaveFileName",
                        staticmethod(lambda *a, **k: (target["path"], "PNG (*.png)")))
    page._save_qr()                                   # 未选择路径 → 不写文件

    out = tmp_path / "wifi_qr.png"
    target["path"] = str(out)
    page._save_qr()
    assert out.read_bytes() == b"PNGDATA"


def test_wifi_passwords_load_copy_and_worker(qtbot, monkeypatch):
    """已存密码：读取守卫/无管理员拒绝/管理员启动（fake）/复制到剪贴板/_PasswordWorker.run（353-394）。"""
    from syskit import admin, repairs

    import syskit.ui.wifi_page as wp

    page = wp.WifiPage()
    qtbot.addWidget(page)
    real_pw_worker = wp._PasswordWorker               # 先留真类，后面会被 patch 成假类

    page._pw_worker = object()
    page._load_passwords()                            # worker 占用 → 早退
    page._pw_worker = None

    monkeypatch.setattr(admin, "is_admin", lambda: False)
    page._load_passwords()                            # 非管理员 → warning 早退
    assert page._pw_worker is None

    class _FakePwWorker(QThread):
        done = Signal(list)

        def start(self):
            pass

    monkeypatch.setattr(wp, "_PasswordWorker", _FakePwWorker)
    monkeypatch.setattr(admin, "is_admin", lambda: True)
    page._load_passwords()
    assert isinstance(page._pw_worker, _FakePwWorker)
    assert not page.btn_pw.isEnabled() and "读取中" in page.lbl_pw.text()

    page._pw_done([("HomeWiFi", "WPA2-PSK", "pw123"), ("Cafe", "?", None)])
    assert page.btn_pw.isEnabled()
    assert page.tbl_pw.item(1, 2).text() == "••••••••"          # 默认掩码
    page.chk_show_pw.setChecked(True)                           # 显示 → 明文
    assert page.tbl_pw.item(1, 2).text() == "(开放网络)"        # 空密码占位

    page.tbl_pw.setCurrentCell(-1, -1)
    page._copy_password()                             # 无选中 → 早退
    page.tbl_pw.selectRow(0)
    page._copy_password()
    assert QApplication.clipboard().text() == "pw123"
    assert "已复制" in page.lbl_pw.text()

    # _PasswordWorker.run：正常解析 + 单个失败回退（386-394）
    monkeypatch.setattr(repairs, "wlan_profiles", lambda: ["A", "B"])

    def _pp(name):
        if name == "A":
            return ("WPA2-PSK", "pw1")
        raise OSError("读取失败")

    monkeypatch.setattr(wp, "profile_password", _pp)
    w = real_pw_worker()
    _KEEP.append(w)
    rows = []
    w.done.connect(rows.append)
    w.run()
    assert rows == [[("A", "WPA2-PSK", "pw1"), ("B", "?", "?")]]
