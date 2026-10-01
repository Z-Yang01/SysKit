"""主窗口：侧边栏导航（概览/网络监控/诊断/修复/工具箱/系统助手/设置/日志）+ 托盘 + 监控。"""
import os
import time

from PySide6.QtCore import QEvent, Qt, QTimer, QUrl, Signal
from PySide6.QtGui import QColor, QFont, QDesktopServices, QKeySequence, QShortcut
from PySide6.QtWidgets import (QCheckBox, QComboBox, QFormLayout,
                               QFrame, QGroupBox, QHBoxLayout, QHeaderView, QLabel, QLineEdit,
                               QListWidget, QListWidgetItem, QMainWindow, QMessageBox,
                               QPlainTextEdit, QProgressBar, QPushButton, QSpinBox,
                               QStackedWidget, QSystemTrayIcon,
                               QTableWidgetItem, QTreeWidget, QTreeWidgetItem,
                               QVBoxLayout, QWidget)

from .. import admin, autostart, config, hotkey, monitor, netcheck, repairs, settings
from .. import logger as _logging
from ..logger import LOGGER
from ..stores import EventStore
from ..telemetry import GROUPS_DASH, GROUPS_SYSTEM, TelemetryWorker
from . import theme
from .confirm_dialog import ConfirmDialog
from .dashboard_page import DashboardPage
from .palette import (CommandPalette, setup_global_hotkey, setup_paste_hotkey,
                      setup_quick_paste_hotkey, teardown_global_hotkey,
                      teardown_paste_hotkey, teardown_quick_paste_hotkey)
from .system_page import SystemPage
from .toolbox_page import ToolboxPage
from .tray import TrayHost
from .vault_page import VaultPage
from .wifi_page import WifiPage
from .workers import (ComboWorker, DiagnosisWorker, PlainCmdWorker, RepairWorker,
                      SnapshotWorker, reap)

# 状态 → 语义角色：用色一律经 theme.pal() 现取（补57 深色修复——
# 旧字面 #2e9e44 系在暗卡上对比度 2~3:1，且切主题不刷新）
STATUS_ROLE = {
    "normal": "ok", "ncsi_false_alarm": "warn", "dns_broken": "warn",
    "no_internet": "err",
    # monitor 实时状态（_on_monitor_status / 托盘）
    "ok": "ok", "no_net": "warn", "ncsi_false": "warn", "offline": "sub",
}
STATUS_TEXT = {
    "normal": "网络正常",
    "ncsi_false_alarm": "实际有网，Windows 误报'无网络'",
    "dns_broken": "能连上但 DNS 异常/缓慢",
    "no_internet": "外网不可达",
}
CHECK_ROLE = {"pass": "ok", "fail": "err", "warn": "warn",
              "skip": "sub", "info": "info"}
TRAY_STATE = {"normal": "ok", "ncsi_false_alarm": "ncsi_false",
              "dns_broken": "no_net", "no_internet": "no_internet"}


def status_color(state: str) -> str:
    return theme.pal().get(STATUS_ROLE.get(state, "sub"), "")


def check_color(status: str) -> str:
    return theme.pal().get(CHECK_ROLE.get(status, "sub"), "")


class MainWindow(QMainWindow):
    sig_status = Signal(str, str)       # monitor 线程 → UI（跨线程信号自动排队）
    sig_action = Signal(object)
    sig_suggest = Signal(object)

    def __init__(self, minimized=False):
        super().__init__()
        # v2 热键迁移必须在任何设置读取方（设置页/热键注册）之前跑一次，
        # 否则组合框回显与实际注册键不一致
        self._migrate_hotkey_settings_v2()
        self.setWindowTitle(f"{config.DISPLAY_NAME}  v{config.VERSION}")
        self.setWindowIcon(theme.app_icon())   # 标题栏/任务栏/Alt+Tab 蓝色 S 标识
        self.resize(980, 680)
        self.tray = TrayHost(self)
        self.tray.show()

        self._diag_worker = None
        self._repair_worker = None
        self._combo_worker = None
        self._snap_worker = None
        self._cmd_worker = None
        self._busy = False
        self._diag_results = []
        self._last_state = None
        self._tray_rate = bool(settings.load().get("tray_rate_enabled", True))

        self.eventstore = EventStore()

        self.mon = monitor.Monitor(
            on_status=lambda s, m: self.sig_status.emit(s, m),
            on_action=lambda a, r: self.sig_action.emit((a, r)),
            on_suggest=lambda f, why: self.sig_suggest.emit((f, why)),
        )
        self.sig_status.connect(self._on_monitor_status)
        self.sig_action.connect(self._on_monitor_action)
        self.sig_suggest.connect(self._on_monitor_suggest)
        self.mon.start()

        self.tele = TelemetryWorker()
        self.tele.setParent(self)      # 挂对象树：quit_app 全树扫尾可枚举
        self.tele.tick.connect(self._on_telemetry)
        self.tele.start()

        # 侧边栏 + 页面栈
        self.stack = QStackedWidget()
        self.page_status = self._build_status_tab()
        self.page_dash = DashboardPage(self.eventstore, traffic=self.tele.traffic,
                                       rate_stats=self.tele.rate_stats)
        self.page_diag = self._build_diag_tab()
        self.page_repair = self._build_repair_tab()
        self.page_toolbox = ToolboxPage()
        self.page_system = SystemPage()
        self.page_vault = VaultPage()
        self.page_settings = self._build_settings_tab()
        self.page_logs = self._build_log_tab()
        pages = [("🏠 概览", self.page_status),
                 ("📈 网络监控", self.page_dash),
                 ("📶 WiFi 工具", WifiPage()),
                 ("🔍 诊断", self.page_diag),
                 ("🔧 修复", self.page_repair),
                 ("⚡ 工具箱", self.page_toolbox),
                 ("🧰 系统助手", self.page_system),
                 ("🔐 密码库", self.page_vault),
                 ("⚙ 设置", self.page_settings),
                 ("📋 日志", self.page_logs)]
        for w in pages:
            self.stack.addWidget(w[1])
        self._nav_defs = [("分组", "网络", None), ("item", "🏠 概览", 0),
                          ("item", "📈 网络监控", 1), ("item", "📶 WiFi 工具", 2),
                          ("item", "🔍 诊断", 3), ("item", "🔧 修复", 4),
                          ("分组", "工具", None), ("item", "⚡ 工具箱", 5),
                          ("item", "🧰 系统助手", 6), ("item", "🔐 密码库", 7),
                          ("分组", "其他", None), ("item", "⚙ 设置", 8),
                          ("item", "📋 日志", 9)]
        self.nav = QListWidget()
        self.nav.setObjectName("sidebar")
        for kind, label, idx in self._nav_defs:
            item = QListWidgetItem(label)
            if kind == "分组":
                item.setFlags(Qt.NoItemFlags)
                item.setData(Qt.UserRole, None)
            else:
                item.setData(Qt.UserRole, idx)
            tips = {
                "🏠 概览": "网卡一览、网络状态徽章与快捷修复",
                "📈 网络监控": "延迟 / 速率 / 流量 / 断网事件时间线",
                "📶 WiFi 工具": "扫描周边 AP、信道推荐、二维码与已存密码",
                "🔍 诊断": "15 项逐条检查，双击未通过项可直接修复",
                "🔧 修复": "一键组合与单项修复（按风险分级）",
                "⚡ 工具箱": "测速、局域网、端口/进程、DNS/子网、ping/路由",
                "🧰 系统助手": "性能、启动项、电池、清理、磁盘、剪贴板、场景模式",
                "🔐 密码库": "网站/账号两级密码管理，敏感剪贴板防记录",
                "⚙ 设置": "自动修复策略、外观、开机自启",
                "📋 日志": "运行日志与错误日志（7 天自动清理）",
            }
            if label in tips:
                item.setToolTip(tips[label])
            self.nav.addItem(item)
        self.nav.currentRowChanged.connect(self._nav_changed)

        # 左侧栏：品牌头 + 导航列表（导航行号语义保持不变）
        left_col = QWidget()
        left_lay = QVBoxLayout(left_col)
        left_lay.setContentsMargins(0, 0, 0, 0)
        left_lay.setSpacing(9)
        left_lay.addWidget(self._build_brand())
        left_lay.addWidget(self.nav, 1)

        body = QHBoxLayout()
        body.setContentsMargins(10, 10, 10, 4)
        body.setSpacing(10)
        body.addWidget(left_col)
        body.addWidget(self.stack, 1)
        central = QWidget()
        central.setLayout(body)
        self.setCentralWidget(central)

        self._log_timer = QTimer(self)
        self._log_timer.timeout.connect(self._reload_log)
        self._log_timer.start(5000)
        self.stack.currentChanged.connect(lambda _i: self._on_page_changed())

        self._admin_banner()
        self._restore_window_state()
        self._setup_palette()
        self.run_diagnosis(quick=True, silent=True)
        if minimized:
            self.hide()
        # 无论哪条启动路径都先算一次采样范围：minimized 自启（自启唯一形态）+
        # 停在首页时不发 currentChanged、从未 show 的窗口 hide() 也不发 hideEvent，
        # 不兜这一底 _need 会保持全量——托盘常驻期一直做全套昂贵采样（补55 B-1）
        self._refresh_telemetry_scope()

    # ---------------- 命令面板 ----------------
    def _migrate_hotkey_settings_v2(self):
        """v2 双热键一次性迁移（migrated_v2 幂等标志，硬约束⑪）。

        旧默认 clip_paste_hotkey=="Ctrl+Alt+V" → 面板键迁 Ctrl+Win+V、快速键
        Ctrl+Alt+V；自定义过面板键的用户不动。只跑一次：用户事后改回旧值
        也不会再迁移。写盘失败仅留痕（最坏情形：双键同串，面板键注册失败走
        回退链降级，不阻塞启动）。
        """
        s = settings.load()
        if s.get("migrated_v2"):
            return
        patch = {"migrated_v2": True}
        if str(s.get("clip_paste_hotkey", "") or "").strip() == "Ctrl+Alt+V":
            patch["clip_paste_hotkey"] = "Ctrl+Win+V"
            patch["clip_quick_hotkey"] = "Ctrl+Alt+V"
        try:
            settings.save(patch)
        except OSError:
            LOGGER.warning("v2 热键迁移写盘失败（不影响本次运行，重启后重试）", exc_info=True)

    def _setup_palette(self):
        """全局热键 + 应用内 Ctrl+K；注册失败静默降级（托盘菜单仍可呼出）。"""
        self._palette = None
        self._hotkey_filter = None
        self._clip_paste = None
        self._paste_hotkey_filter = None
        self._quick_hotkey_filter = None
        setup_global_hotkey(self)
        setup_quick_paste_hotkey(self)      # id=2 快速直贴
        setup_paste_hotkey(self)            # id=3 面板
        sc = QShortcut(QKeySequence("Ctrl+K"), self)
        sc.activated.connect(self.toggle_palette)

    def toggle_palette(self):
        try:
            if self._palette is None:
                self._palette = CommandPalette(self)
            self._palette.toggle()
        except RuntimeError:
            pass        # C++ 对象已被销毁（测试卸载路径）

    def toggle_clip_paste(self):
        """快捷粘贴面板：呼出**前**记录前台窗口，粘贴时把焦点还给它。"""
        try:
            from .. import paste as paste_mod
            from .clip_paste import ClipPastePanel
            if self._clip_paste is None:
                self._clip_paste = ClipPastePanel(self)
            # 显隐判断在 ClipPastePanel.toggle 内；面板可见时前台就是它自己，
            # 但 toggle 走隐藏分支、忽略该句柄，无副作用
            self._clip_paste.toggle(paste_mod.foreground_window())
        except RuntimeError:
            pass        # 与 toggle_palette 同款防御

    def _paste_notify(self, title: str, msg: str):
        tray = getattr(self, "tray", None)
        if tray is not None:
            try:
                tray.notify(title, msg)
                return
            except Exception:
                pass
        LOGGER.info("{}: {}", title, msg)

    def quick_paste_latest(self):
        """快速粘贴最新一条剪贴板历史（全局热键 Ctrl+Alt+V，不弹面板）。

        记录前台 → 写回历史最新条 → 焦点守卫注键（clip_paste.
        inject_ctrl_v_guarded 单源：修饰键释放/前台分派/计数断言全在内）。
        历史空/写回失败托盘降级提示，绝不静默丢内容。
        """
        try:
            from .. import clipboard as clipboard_mod
            from .. import paste as paste_mod
            from .clip_paste import (inject_ctrl_v_guarded, live_clip_history,
                                     write_back_entry)
            hwnd = paste_mod.foreground_window()
            # 活实例直读（权威源、零盘往返）；假窗/页面未建时回退盘读
            hist = live_clip_history(self)
            if hist is not None:
                items = hist.items()
            else:
                clipboard_mod.flush_pending()
                items = clipboard_mod.ClipboardHistory(
                    max_items=int(settings.load().get("clipboard_max", 50))).items()
            if not items:
                self._paste_notify("快速粘贴", "剪贴板历史为空，无可粘贴内容")
                return
            if not write_back_entry(items[0]):
                self._paste_notify("快速粘贴",
                                   "粘贴失败：内容不可用（图片文件可能已被清理）")
                return
            inject_ctrl_v_guarded(
                hwnd, items[0], lambda msg: self._paste_notify("快速粘贴", msg))
        except RuntimeError:
            pass        # 与 toggle_palette 同款防御

    def run_palette_command(self, value: str):
        """面板自定义命令：URL 直接打开；本地命令确认后执行并气泡反馈。"""
        v = (value or "").strip()
        if not v:
            return
        if v.lower().startswith(("http://", "https://")):
            QDesktopServices.openUrl(QUrl(v))
            return
        ret = QMessageBox.question(self, "运行命令", f"执行以下命令？\n\n{v}",
                                   QMessageBox.Yes | QMessageBox.No)
        if ret != QMessageBox.Yes:
            return
        if self._cmd_worker is not None:      # 防重入：一条命令未完成不再启动第二条
            self.tray.notify("命令面板", "上一条命令仍在执行，请稍候")
            return
        # 后台执行：str 命令走 shell，proc.run 最长可阻塞 30s，绝不能冻住 UI
        worker = reap(PlainCmdWorker(v, timeout=30), self)
        self._cmd_worker = worker

        def _cmd_done(ok, out):
            if self._cmd_worker is worker:    # 只清自己：防止完成回调误清新引用
                self._cmd_worker = None
            self.tray.notify("命令面板", ("完成: " if ok else "失败: ") + out[:120])

        worker.done.connect(_cmd_done)
        worker.start()

    # ---------------- 品牌头 ----------------
    def _build_brand(self) -> QWidget:
        """侧边栏顶部品牌区：代码绘制的标识 + 名称 + 版本徽章。

        宽度对齐导航列表（≤200），保持左栏视觉为一整列。
        """
        card = QFrame()
        card.setObjectName("brandCard")
        card.setMaximumWidth(200)
        outer = QHBoxLayout(card)
        outer.setContentsMargins(12, 10, 12, 10)
        outer.setSpacing(9)

        logo = QLabel()
        logo.setPixmap(theme.app_logo_pixmap(34))
        logo.setFixedSize(34, 34)
        logo.setToolTip("已以管理员身份运行，可执行全部修复动作" if admin.is_admin()
                        else "未以管理员运行：诊断可用，修复动作会被拒绝")
        outer.addWidget(logo, 0, Qt.AlignVCenter)

        col = QVBoxLayout()
        col.setContentsMargins(0, 0, 0, 0)
        col.setSpacing(1)
        title = QLabel(config.APP_NAME)
        title.setObjectName("brandTitle")
        sub = QLabel("系统工具箱")
        sub.setObjectName("brandSub")
        col.addWidget(title)
        col.addWidget(sub)
        outer.addLayout(col, 1)

        ver = QLabel(f"v{config.VERSION}")
        ver.setObjectName("badge" if admin.is_admin() else "badgeWarn")
        outer.addWidget(ver, 0, Qt.AlignTop)
        theme.add_shadow(card, blur=18, dy=3, alpha=26)
        return card

    # ---------------- 窗口状态记忆 ----------------
    def _restore_window_state(self):
        s = settings.load()
        geo = s.get("win_geometry")
        if geo:
            from PySide6.QtCore import QByteArray
            self.restoreGeometry(QByteArray.fromBase64(geo.encode()))
        self.switch_page(int(s.get("last_page", 0)) or 0)

    def _save_window_state(self):
        try:
            settings.save({
                # saveGeometry() 返回 QByteArray，须先 toBase64() 再转 bytes
                #（此前 bytes(...).toBase64() 对 bytes 调 toBase64 必抛 AttributeError
                #  且被 except 吞掉，窗口几何从不持久化）
                "win_geometry": bytes(self.saveGeometry().toBase64()).decode(),
                "last_page": self.stack.currentIndex(),
            })
        except Exception:
            pass

    # ---------------- 导航 ----------------
    def switch_page(self, idx: int):
        self.stack.setCurrentIndex(idx)
        for row in range(self.nav.count()):
            if self.nav.item(row).data(Qt.UserRole) == idx:
                self.nav.blockSignals(True)
                self.nav.setCurrentRow(row)
                self.nav.blockSignals(False)
                break

    def _nav_changed(self, row):
        idx = self.nav.item(row).data(Qt.UserRole) if 0 <= row < self.nav.count() else None
        if idx is not None:
            self.stack.setCurrentIndex(idx)
            if int(settings.load().get("last_page", -1)) != idx:
                settings.save({"last_page": idx})

    # ---------------- 遥测 ----------------
    def _refresh_telemetry_scope(self):
        """告诉遥测引擎「此刻的界面看得懂哪些分组」，没人看的昂贵采样就不做。

        省掉的是 netsh wlan 子进程（每 5 秒）、两轮 ICMP（每 3 秒）、进程表遍历
        （每 2 秒）：托盘驻留是进程生命期的绝大多数时间，那时只要速率一项。
        页面回前台由 showEvent / 切页重新打开，图表另有 reset_if_stale() 清断流。
        """
        if getattr(self, "tele", None) is None or getattr(self, "stack", None) is None:
            return                                  # __init__ 早期：tele/stack 尚未就位
        need = set()
        if self.isVisible() and not self.isMinimized():
            cur = self.stack.currentWidget()
            if cur is self.page_dash:
                need |= GROUPS_DASH
            elif cur is self.page_system:
                need |= GROUPS_SYSTEM
        self.tele.set_need(need)

    def showEvent(self, event):
        super().showEvent(event)
        self._refresh_telemetry_scope()

    def hideEvent(self, event):
        super().hideEvent(event)
        self._refresh_telemetry_scope()

    def changeEvent(self, event):
        super().changeEvent(event)
        # 从任务栏「最小化」不触发 hideEvent，光靠显隐两个钩子会把范围永久留在空集
        if event.type() == QEvent.WindowStateChange:
            self._refresh_telemetry_scope()

    def _on_telemetry(self, d: dict):
        # 窗口不可见/最小化时页面一概不喂：update_tick 会刷新页面的 _last_tick，
        # 击穿 reset_if_stale 的断流判定（补55 B-3），PerfTab 的重建也是白做；
        # 托盘速率不依赖页面，照常
        if not self.isVisible() or self.isMinimized():
            if self._tray_rate and "down" in d:
                self.tray.update_rate(d["down"], d["up"])
            return
        cur = self.stack.currentWidget()
        if cur is self.page_dash:
            self.page_dash.update_tick(d)
        if cur is self.page_system:
            self.page_system.update_tick(d)
        if self._tray_rate and "down" in d:
            self.tray.update_rate(d["down"], d["up"])

    # ---------------- 忙碌态 ----------------
    def _set_busy(self, busy, text=""):
        self._busy = busy
        for w in (self.btn_quick, self.btn_full, self.btn_refresh_adapters,
                  self.btn_diag, self.btn_copy_report, self.btn_export_report,
                  self.btn_combo_fast,
                  self.btn_combo_full, self.btn_combo_stack, self.btn_mac_on,
                  self.btn_mac_off, self.btn_reconnect, self.btn_profiles):
            if w:
                w.setEnabled(not busy)
        self.tbl_actions.setEnabled(not busy)
        if busy and text:
            self.statusBar().showMessage(text)

    # ---------------- 管理员横幅 ----------------
    def _admin_banner(self):
        if admin.is_admin():
            return
        lbl = QLabel("未以管理员身份运行：诊断可用，修复动作将被拒绝。")
        self._admin_banner_lbl = lbl
        self._style_admin_banner()
        btn = QPushButton("以管理员重启")
        btn.setFlat(True)
        btn.clicked.connect(self._elevate_restart)
        self.statusBar().addWidget(btn)
        self.statusBar().addWidget(lbl)

    def _style_admin_banner(self):
        lbl = getattr(self, "_admin_banner_lbl", None)
        if lbl is not None:
            lbl.setStyleSheet(f"color:{theme.pal()['warn']};")

    def _elevate_restart(self):
        """提权拉起新实例后本实例必须退出：单实例互斥体在旧进程手里，
        新实例会撞 ERROR_ALREADY_EXISTS 弹「已在运行」后自灭。"""
        if not admin.elevate_and_restart(["--minimized"]):
            self.statusBar().showMessage("提权未完成（UAC 被拒绝或失败），仍在以普通权限运行", 6000)
            return
        self.tray.hide()            # 气泡与图标随旧实例一起收场
        self.quit_app()

    # ---------------- 状态页 ----------------
    def _set_status(self, text, color, sub=""):
        """大状态徽章：彩点 + 文字（HTML 富文本）。"""
        sub_c = theme.pal()["sub"]
        html = (f'<span style="color:{color}; font-size:22pt;">●</span>'
                f'&nbsp;<span style="color:{color}; font-size:18pt; font-weight:bold;">{text}</span>')
        if sub:
            html += f'<br><span style="font-size:10pt; color:{sub_c};">{sub}</span>'
        self.lbl_big.setText(html)

    def _build_status_tab(self) -> QWidget:
        w = QWidget()
        lay = QVBoxLayout(w)
        lay.setContentsMargins(14, 14, 14, 10)
        self.lbl_big = QLabel("正在检测…")
        self.lbl_big.setFont(QFont("Microsoft YaHei UI", 14))
        self.lbl_big.setAlignment(Qt.AlignCenter)
        self.lbl_big.setObjectName("card")
        self.lbl_big.setMinimumHeight(96)
        theme.add_shadow(self.lbl_big, blur=24, dy=4, alpha=34)
        lay.addWidget(self.lbl_big)

        self.lbl_probe = QLabel(" ")
        self.lbl_probe.setObjectName("card")
        self.lbl_probe.setProperty("hint", True)
        self.lbl_probe.setAlignment(Qt.AlignCenter)
        self.lbl_probe.setWordWrap(True)
        lay.addWidget(self.lbl_probe)

        # 快捷操作面板
        grp_quick = QGroupBox("快捷操作")
        layq = QHBoxLayout(grp_quick)
        b_dns = QPushButton("刷新 DNS 缓存")
        b_dns.setToolTip("执行安全动作 flush_dns：清空系统 DNS 解析缓存，可反复执行")
        b_dns.clicked.connect(lambda: self.run_repair("flush_dns"))
        b_re = QPushButton("重连 WiFi")
        b_re.setToolTip("重启 WLAN 服务并重连当前配置文件（网络会短暂中断数秒）")
        b_re.clicked.connect(lambda: self.run_repair("restart_wlan"))
        b_net = QPushButton("打开网络设置")
        b_net.setToolTip("跳转 Windows 设置 · 网络和 Internet")
        b_net.clicked.connect(lambda: os.startfile("ms-settings:network-status"))
        b_vpn = QPushButton("VPN 排查")
        b_vpn.setToolTip("只读检查：隧道网卡 / 系统代理 / 持久路由 / 默认出口；不会改系统\n"
                         "VPN 或 Clash 异常时先点这里")
        b_vpn.clicked.connect(self._vpn_checklist)
        self.lbl_proxy = QLabel("系统代理：…")
        self.btn_proxy = QPushButton("关闭代理")
        self.btn_proxy.setToolTip("若系统启用了 HTTP 代理，一键关闭以免误拦探测\n"
                                  "注意：Clash 正常使用时可能依赖系统代理，VPN 场景请先用「VPN 排查」")
        self.btn_proxy.clicked.connect(self._toggle_proxy)
        layq.addWidget(b_dns)
        layq.addWidget(b_re)
        layq.addWidget(b_net)
        layq.addWidget(b_vpn)
        layq.addStretch(1)
        layq.addWidget(self.lbl_proxy)
        layq.addWidget(self.btn_proxy)
        lay.addWidget(grp_quick)
        self._refresh_proxy_label()

        btns = QHBoxLayout()
        self.btn_quick = QPushButton("快速检测")
        self.btn_quick.setToolTip("跳过慢速项，约 5–10 秒完成核心连通性检查")
        self.btn_quick.clicked.connect(lambda: self.run_diagnosis(quick=True))
        self.btn_full = QPushButton("一键体检")
        self.btn_full.setToolTip("完整 15 项检查，覆盖服务/路由/DNS/NCSI/省电等")
        self.btn_full.setProperty("primary", True)
        self.btn_full.clicked.connect(lambda: self.run_diagnosis(quick=False, goto_diag=True))
        self.btn_refresh_adapters = QPushButton("刷新网卡")
        self.btn_refresh_adapters.setToolTip("重新采集网卡/路由快照并刷新列表")
        self.btn_refresh_adapters.clicked.connect(self.refresh_adapters)
        btns.addWidget(self.btn_quick)
        btns.addWidget(self.btn_full)
        btns.addWidget(self.btn_refresh_adapters)
        btns.addStretch(1)
        lay.addLayout(btns)

        self.tbl_adapters = theme.make_table(["名称", "状态", "IPv4", "网关", "DNS", "NLM判定"])
        self.tbl_adapters.horizontalHeader().setSectionResizeMode(QHeaderView.ResizeToContents)
        # 最后一列吃掉剩余宽度：避免横向滚动条（NLM 判定文案最长）
        self.tbl_adapters.horizontalHeader().setStretchLastSection(True)
        lay.addWidget(self.tbl_adapters, 1)     # 拉伸填满页面剩余高度（否则下方留大片空白）
        self.refresh_adapters()
        return w

    # ---------------- 代理 ----------------
    def _vpn_checklist(self):
        from .. import vpn
        try:
            steps = vpn.vpn_checklist()
            head = vpn.summarize(steps)
        except Exception as e:
            QMessageBox.warning(self, config.DISPLAY_NAME, f"VPN 排查失败: {e}")
            return
        lines = []
        mark = {"ok": "√", "warn": "!", "fail": "×", "info": "·"}
        for s in steps:
            lines.append(f"[{mark.get(s.get('level'), '?')}] {s['name']}\n    {s['detail']}")
        box = QMessageBox(self)
        box.setWindowTitle("VPN 排查（只读）")
        box.setIcon(QMessageBox.Information)
        box.setText(head)
        box.setInformativeText("\n\n".join(lines))
        box.setDetailedText(
            "本检查不修改任何系统设置。\n"
            "若 VPN 用不了：先重启 VPN 客户端，再确认本页「系统代理/隧道网卡」是否匹配你的模式。\n"
            "SysKit 在检测到隧道在线时，自动修复不会做续租/重启 WLAN/关代理/删路由。")
        box.exec()

    def _refresh_proxy_label(self):
        from .. import proxy
        st = proxy.get_proxy_status()
        if st["enabled"]:
            self.lbl_proxy.setText(f"系统代理：{st['server'] or 'PAC 脚本'}")
            self.lbl_proxy.setStyleSheet(f"color:{theme.pal()['warn']}; font-weight:bold;")
            self.btn_proxy.setText("关闭代理")
            self.btn_proxy.setProperty("danger", True)
        else:
            self.lbl_proxy.setText("系统代理：未启用")
            self.lbl_proxy.setStyleSheet(f"color:{theme.pal()['sub']};")
            self.btn_proxy.setText("代理设置…")
            self.btn_proxy.setProperty("danger", False)
        self.btn_proxy.style().unpolish(self.btn_proxy)
        self.btn_proxy.style().polish(self.btn_proxy)

    def _toggle_proxy(self):
        from .. import proxy
        if proxy.get_proxy_status()["enabled"]:
            self.run_repair("proxy_off")
            # 不在此处刷新标签：proxy_off 是异步修复，此刻读到的是旧状态
            # （完成回调 _repair_done 已兜底刷新）
        else:
            os.startfile("ms-settings:network-proxy")
            self._refresh_proxy_label()

    def refresh_adapters(self):
        if self._snap_worker:
            return
        self._snap_worker = reap(SnapshotWorker(), self)
        self._snap_worker.got.connect(self._fill_adapters)
        self._snap_worker.start()

    def _release_worker(self, attr: str):
        """复位 worker 前先 wait：QThread 在 emit 后仍有微秒级收尾，
        此时 GC 析构会 qFatal（0xC0000409 / P9=7），即退出崩溃同源问题。"""
        w = getattr(self, attr, None)
        if w is not None:
            w.wait(3000)
        setattr(self, attr, None)

    def _fill_adapters(self, snap):
        self._release_worker("_snap_worker")
        self._last_snap = snap      # 主题切换时重填（背景/前景色固化在 item 里，QSS 不覆盖）
        rows = [a for a in snap.adapters if "WAN Miniport" not in a.desc]
        self.tbl_adapters.setRowCount(len(rows))
        # 在线行用柔和绿底：曾挪用选中色 sel_bg——高亮行与真实选中无法区分，
        # 且填充后颜色固化，切暗色主题后残留刺眼的浅蓝底
        online_bg = QColor(theme.pal()["ok_soft"])
        ok_color = QColor(theme.pal()["ok"])
        for i, a in enumerate(rows):
            online = bool(a.gw4) and a.conn_v4 == "Internet"
            dns_txt = ", ".join(a.dns[:3]) + ("…" if len(a.dns) > 3 else "")
            vals = [a.name, a.status, ", ".join(a.ipv4), ", ".join(a.gw4), dns_txt, a.conn_v4]
            for c, v in enumerate(vals):
                item = QTableWidgetItem(("★ " if c == 0 and online else "") + str(v))
                if a.status.lower() == "up":
                    item.setForeground(ok_color)
                if online:
                    item.setBackground(online_bg)
                self.tbl_adapters.setItem(i, c, item)

    # ---------------- 诊断页 ----------------
    def _build_diag_tab(self) -> QWidget:
        w = QWidget()
        lay = QVBoxLayout(w)
        lay.setContentsMargins(14, 14, 14, 10)
        btns = QHBoxLayout()
        self.btn_diag = QPushButton("开始完整诊断")
        self.btn_diag.clicked.connect(lambda: self.run_diagnosis(quick=False))
        btns.addWidget(self.btn_diag)
        self.btn_diag_stop = QPushButton("停止")
        self.btn_diag_stop.setProperty("danger", True)
        self.btn_diag_stop.setToolTip("中止诊断：当前检查完成后停止，已完成项保留")
        self.btn_diag_stop.setEnabled(False)
        self.btn_diag_stop.clicked.connect(self._stop_diag)
        btns.addWidget(self.btn_diag_stop)
        self.btn_copy_report = QPushButton("复制报告")
        self.btn_copy_report.clicked.connect(self._copy_report)
        btns.addWidget(self.btn_copy_report)
        self.btn_export_report = QPushButton("导出报告")
        self.btn_export_report.setToolTip(
            "把诊断结果与结论保存为文本文件（可存档或发给他人求助）。\n"
            "注意：内容含 WiFi 名 / 内网 DNS / 代理地址等局域网信息，外发前请自行斟酌。")
        self.btn_export_report.clicked.connect(self._export_report)
        btns.addWidget(self.btn_export_report)
        btns.addStretch(1)
        lay.addLayout(btns)

        self.pb_diag = QProgressBar()
        self.pb_diag.setRange(0, 0)          # 不确定进度（滚动）
        self.pb_diag.setTextVisible(False)
        self.pb_diag.hide()
        lay.addWidget(self.pb_diag)

        self.tree_diag = QTreeWidget()
        self.tree_diag.setHeaderLabels(["结果", "检查项", "详情"])
        self.tree_diag.setColumnWidth(0, 60)
        self.tree_diag.setColumnWidth(1, 290)
        self.tree_diag.itemDoubleClicked.connect(self._diag_item_fix)
        lay.addWidget(self.tree_diag, 3)

        panel = QGroupBox("结论与建议")
        panel_lay = QVBoxLayout(panel)
        self.lbl_verdict = QLabel("尚未诊断")
        f = self.lbl_verdict.font()
        f.setBold(True)
        self.lbl_verdict.setFont(f)
        panel_lay.addWidget(self.lbl_verdict)
        self.fix_btn_row = QHBoxLayout()
        self.fix_btn_row.setContentsMargins(0, 0, 0, 0)
        panel_lay.addLayout(self.fix_btn_row)
        lay.addWidget(panel, 1)

        hint = QLabel("双击上方未通过项可执行建议修复；下方的建议按钮作用相同。")
        hint.setProperty("hint", True)
        lay.addWidget(hint)
        return w

    def run_diagnosis(self, quick=False, silent=False, goto_diag=False):
        if self._diag_worker or self._busy:
            return
        if goto_diag:
            self.switch_page(3)
        if not silent:
            self._set_busy(True, "诊断中…")
            self.btn_diag.setText("诊断中…")
            self.pb_diag.show()
        self.tree_diag.clear()
        self._diag_results.clear()      # 防止复制报告出现重复/无界增长
        self._clear_fix_buttons()
        self.lbl_verdict.setText("诊断进行中…")
        self._diag_worker = reap(DiagnosisWorker(quick=quick), self)
        self._diag_worker.progress.connect(self._diag_progress)
        self._diag_worker.finished_ok.connect(lambda rs: self._diag_done(rs, silent))
        self._diag_worker.start()
        self.btn_diag_stop.setEnabled(True)

    def _stop_diag(self):
        """中止诊断：当前检查完成后停止，已完成项照常展示。"""
        if self._diag_worker is not None:
            self._diag_worker.cancel()
            self.btn_diag_stop.setEnabled(False)
            self.statusBar().showMessage("正在停止诊断（等当前检查完成）…", 4000)

    def _stop_combo(self):
        """中止组合修复：当前动作完成后停止；已执行的系统操作无法撤销。"""
        if self._combo_worker is not None:
            self._combo_worker.cancel()
            self.btn_combo_stop.setEnabled(False)
            self.statusBar().showMessage(
                "正在中止组合修复（等当前动作完成；已执行的操作不可撤销）…", 6000)

    def _clear_fix_buttons(self):
        while self.fix_btn_row.count():
            item = self.fix_btn_row.takeAt(0)
            if item.widget():
                item.widget().deleteLater()

    def _diag_progress(self, r):
        self._diag_results.append(r)
        mark = netcheck.STATUS_MARK.get(r.status, "?")
        item = QTreeWidgetItem([mark, r.name, r.detail])
        item.setForeground(0, QColor(check_color(r.status)))
        item.setData(0, Qt.UserRole, r)
        self.tree_diag.addTopLevelItem(item)
        self.tree_diag.scrollToBottom()

    def _diag_done(self, results, silent):
        self._release_worker("_diag_worker")
        self.btn_diag_stop.setEnabled(False)
        self.btn_diag.setEnabled(True)
        self.btn_diag.setText("开始完整诊断")
        self.pb_diag.hide()
        if any(r.id == "diag_aborted" for r in results):
            # 中止的不完整结果不得用于判定网络状态（缺 real_conn 会被误判"外网不可达"）
            self.lbl_verdict.setText("诊断已中止（状态保持此前判定）")
            self.statusBar().showMessage("诊断已中止：已完成项保留在列表中", 5000)
            self._set_busy(False)   # 早退也必须复位：否则诊断/修复/组合全被 _busy 闸死
            return
        base, text = netcheck.verdict(results)
        color = status_color(base)
        self._set_status(text, color, sub=f"{time.strftime('%H:%M')} 诊断完成")
        self.lbl_verdict.setText(text)
        self.lbl_verdict.setStyleSheet(f"color:{color};")
        self.tray.set_state(TRAY_STATE.get(base, "offline"), "")
        real = next((r for r in results if r.id == "real_conn"), None)
        self.lbl_probe.setText("连通性探测：" + (real.detail if real else "无数据"))

        # 建议修复按钮（去重，仅未通过项）
        seen = set()
        for r in results:
            if r.status not in (netcheck.FAIL, netcheck.WARN):
                continue
            for fid in r.fix_ids:
                if fid in seen:
                    continue
                act = repairs.get_action(fid)
                if not act:
                    continue
                seen.add(fid)
                b = QPushButton(act.title)
                b.setToolTip(f"{act.description}\n还原：{act.restore_hint}")
                b.clicked.connect(lambda _, a=fid: self.run_repair(a))
                self.fix_btn_row.addWidget(b)
        self.fix_btn_row.addStretch(1)

        self.refresh_adapters()
        if not silent:
            self.statusBar().showMessage(f"诊断完成：{text}", 8000)
        self._set_busy(False)

    def _report_lines(self):
        """诊断报告文本行（复制/导出共用）；无结果或诊断进行中返回 None。

        静默重诊（遥测/托盘触发）不走 _set_busy，按钮仍亮着，而 run_diagnosis 会
        先清空 _diag_results 再逐项回填：中途取到的是缺项集合，verdict 会据它给出
        与界面不符的「结论」（缺 real_conn 即误判外网不可达），所以必须拒绝出报告。
        """
        if self._diag_worker is not None or not self._diag_results:
            return None
        lines = [f"SysKit 诊断报告  {time.strftime('%Y-%m-%d %H:%M')}", "-" * 50]
        aborted = any(r.id == "diag_aborted" for r in self._diag_results)
        for r in self._diag_results:
            lines.append(f"[{netcheck.STATUS_MARK.get(r.status, '?')}] {r.name}")
            lines.append(f"      {r.detail}")
        lines.append("-" * 50)
        if aborted:
            # 中止的不完整结果缺 real_conn，verdict 会误判成「外网不可达」——
            # 报告必须与界面判定一致（界面保持此前状态）
            lines.append("结论：诊断已中止，本次结果不完整（不构成网络状态判定）")
        else:
            base, text = netcheck.verdict(self._diag_results)
            lines.append(f"结论：{text}")
        return lines

    def _no_report_hint(self, verb: str) -> str:
        if self._diag_worker is not None:
            return f"诊断进行中，此刻结果不完整，请等本次诊断结束后再{verb}报告"
        return f"暂无诊断结果可{verb}（请先完成一次诊断）"

    def _copy_report(self):
        from PySide6.QtWidgets import QApplication  # 局部导入：clipboard 依赖 app 实例
        lines = self._report_lines()
        if lines is None:
            self.statusBar().showMessage(self._no_report_hint("复制"), 4000)
            return
        QApplication.clipboard().setText("\n".join(lines))
        self.statusBar().showMessage("报告已复制到剪贴板", 4000)

    def _export_report(self):
        from PySide6.QtWidgets import QFileDialog
        lines = self._report_lines()
        if lines is None:
            self.statusBar().showMessage(self._no_report_hint("导出"), 4000)
            return
        default = os.path.join(os.path.expanduser("~"),
                               f"SysKit诊断报告_{time.strftime('%Y%m%d_%H%M')}.txt")
        path, _sel = QFileDialog.getSaveFileName(self, "导出诊断报告",
                                                 default, "文本文件 (*.txt)")
        if not path:
            return
        try:
            with open(path, "w", encoding="utf-8") as f:
                f.write("\n".join(lines) + "\n")
        except OSError as e:
            QMessageBox.warning(self, config.DISPLAY_NAME, f"导出失败: {e}")
            return
        self.statusBar().showMessage(f"报告已导出: {path}", 6000)

    def _diag_item_fix(self, item, col):
        r = item.data(0, Qt.UserRole)
        if r and r.fix_ids:
            self.run_repair(r.fix_ids[0])

    # ---------------- 修复页 ----------------
    def _build_repair_tab(self) -> QWidget:
        w = QWidget()
        lay = QVBoxLayout(w)
        lay.setContentsMargins(14, 14, 14, 10)

        grp_combo = QGroupBox("一键修复")
        layc = QHBoxLayout(grp_combo)
        self.btn_combo_fast = QPushButton("快速修复")
        self.btn_combo_fast.setProperty("primary", True)
        self.btn_combo_fast.clicked.connect(lambda: self.run_combo("fast"))
        self.btn_combo_full = QPushButton("深度修复")
        self.btn_combo_full.clicked.connect(lambda: self.run_combo("full"))
        self.btn_combo_stack = QPushButton("重置网络栈（需重启）")
        self.btn_combo_stack.setProperty("danger", True)
        self.btn_combo_stack.clicked.connect(lambda: self.run_combo("stack"))
        layc.addWidget(self.btn_combo_fast)
        layc.addWidget(self.btn_combo_full)
        layc.addWidget(self.btn_combo_stack)
        self.btn_combo_stop = QPushButton("中止")
        self.btn_combo_stop.setProperty("danger", True)
        self.btn_combo_stop.setToolTip("中止组合修复：当前动作完成后停止（已执行的系统操作不可撤销），剩余动作跳过")
        self.btn_combo_stop.setEnabled(False)
        self.btn_combo_stop.clicked.connect(self._stop_combo)
        layc.addWidget(self.btn_combo_stop)
        layc.addStretch(1)
        lay.addWidget(grp_combo)

        grp_wifi = QGroupBox("WiFi 专区")
        layw = QHBoxLayout(grp_wifi)
        self.cmb_profile = QComboBox()
        self.cmb_profile.setMinimumWidth(170)
        self.cmb_profile.currentTextChanged.connect(self._update_mac_state)
        self.lbl_mac = QLabel("MAC 随机化：?")
        self.btn_mac_on = QPushButton("开启随机化")
        self.btn_mac_off = QPushButton("关闭随机化")
        self.btn_mac_on.clicked.connect(lambda: self.run_repair("mac_random_on", self._profile_param()))
        self.btn_mac_off.clicked.connect(lambda: self.run_repair("mac_random_off", self._profile_param()))
        self.btn_reconnect = QPushButton("重连当前 WiFi")
        self.btn_reconnect.clicked.connect(lambda: self.run_repair("restart_wlan"))
        self.btn_profiles = QPushButton("刷新")
        self.btn_profiles.clicked.connect(self._refresh_profiles)
        layw.addWidget(QLabel("配置文件:"))
        layw.addWidget(self.cmb_profile)
        layw.addWidget(self.lbl_mac)
        layw.addWidget(self.btn_mac_on)
        layw.addWidget(self.btn_mac_off)
        layw.addWidget(self.btn_reconnect)
        layw.addWidget(self.btn_profiles)
        lay.addWidget(grp_wifi)

        filter_row = QHBoxLayout()
        self.ed_filter = QLineEdit()
        self.ed_filter.setPlaceholderText("筛选修复动作（按名称/描述过滤）…")
        self.ed_filter.setClearButtonEnabled(True)
        self.ed_filter.textChanged.connect(lambda _t: self._apply_action_filter())
        filter_row.addWidget(QLabel("筛选:"))
        filter_row.addWidget(self.ed_filter, 1)
        lay.addLayout(filter_row)

        self.tbl_actions = theme.make_table(["分类", "修复动作", "风险", "操作"],
                                            alternating=False)
        self.tbl_actions.horizontalHeader().setSectionResizeMode(1, QHeaderView.Stretch)
        lay.addWidget(self.tbl_actions, 3)

        grp_log = QGroupBox("修复记录（本次运行）")
        lay2 = QVBoxLayout(grp_log)
        self.list_repair_log = QListWidget()
        self.list_repair_log.setMaximumHeight(130)
        lay2.addWidget(self.list_repair_log)
        lay.addWidget(grp_log, 1)

        self._fill_actions()
        # 配置文件枚举（netsh，可达数百 ms）+ XML glob 不在构造期执行：
        # 首次切到修复页才查（_on_page_changed），加快主窗体首帧
        self._profiles_loaded = False
        return w

    def _fill_actions(self):
        order = {repairs.RISK_SAFE: 0, repairs.RISK_CONFIG: 1, repairs.RISK_AGGRESSIVE: 2}
        actions = sorted(repairs.ACTIONS, key=lambda a: order[a.risk])
        cat_name = {repairs.RISK_SAFE: "安全类", repairs.RISK_CONFIG: "改配置类",
                    repairs.RISK_AGGRESSIVE: "强力类"}
        # 语义角色现取 pal：深绿 #2e7d32 曾是暗卡上对比度最差的一处（补57）
        risk_color = {"safe": theme.pal()["ok"], "config": theme.pal()["warn"],
                      "aggressive": theme.pal()["danger"]}
        self.tbl_actions.setRowCount(len(actions))
        for i, a in enumerate(actions):
            c0 = QTableWidgetItem(cat_name[a.risk])
            c0.setForeground(QColor(risk_color[a.risk]))
            self.tbl_actions.setItem(i, 0, c0)
            self.tbl_actions.setItem(i, 1, QTableWidgetItem(f"{a.title}　— {a.brief}"))
            self.tbl_actions.setItem(i, 2, QTableWidgetItem(repairs.RISK_LABEL[a.risk]))
            btn = QPushButton("执行")
            btn.setToolTip(f"{a.description}\n还原：{act_hint(a)}")
            btn.clicked.connect(lambda _, aid=a.id: self.run_repair(aid))
            self.tbl_actions.setCellWidget(i, 3, btn)
        # 所有行统一行高（曾只设第 0 行 → 首行比其他行高，观感不一致）；
        # 40px 保证行内「执行」按钮完整显示
        self.tbl_actions.verticalHeader().setDefaultSectionSize(40)
        self.tbl_actions.verticalHeader().setVisible(False)

    def _apply_action_filter(self):
        kw = self.ed_filter.text().strip().lower()
        for row in range(self.tbl_actions.rowCount()):
            text = self.tbl_actions.item(row, 1).text().lower() if self.tbl_actions.item(row, 1) else ""
            self.tbl_actions.setRowHidden(row, bool(kw) and kw not in text)

    def _refresh_profiles(self):
        self.cmb_profile.clear()
        self._profiles = repairs.wlan_profiles()
        self.cmb_profile.addItems(self._profiles)
        self._update_mac_state()

    def _profile_param(self):
        return {"profile": self.cmb_profile.currentText()}

    def _update_mac_state(self):
        profile = self.cmb_profile.currentText()
        if not profile:
            self.lbl_mac.setText("MAC 随机化：?")
            return
        st = repairs.mac_random_state(profile)
        txt = {True: "开启", False: "关闭", None: "未知（需管理员读取）"}.get(st, "?")
        self.lbl_mac.setText(f"MAC 随机化：{txt}")

    def _add_repair_record(self, aid, result):
        act = repairs.get_action(aid)
        title = act.title if act else aid
        stamp = time.strftime("%H:%M:%S")
        mark = "√" if result.success else "×"
        self.list_repair_log.insertItem(0, f"{stamp} [{mark}] {title} — {result.message[:90]}")
        if self.list_repair_log.count() > 60:
            self.list_repair_log.takeItem(self.list_repair_log.count() - 1)

    # ---------------- 设置页 ----------------
    def _build_settings_tab(self) -> QWidget:
        w = QWidget()
        outer = QVBoxLayout(w)
        outer.setContentsMargins(14, 14, 14, 10)
        s = settings.load()

        g1 = QGroupBox("后台自动修复")
        form = QFormLayout(g1)
        self.chk_auto = QCheckBox("启用（仅执行安全类动作，改配置类只弹建议）")
        self.chk_auto.setChecked(bool(s.get("auto_repair_enabled", True)))
        form.addRow(self.chk_auto)

        self.spin_interval = QSpinBox()
        self.spin_interval.setRange(10, 600)
        self.spin_interval.setValue(int(s.get("poll_interval_sec", 30)))
        self.spin_interval.setSuffix(" 秒")
        form.addRow("探测间隔", self.spin_interval)

        self.spin_threshold = QSpinBox()
        self.spin_threshold.setRange(1, 10)
        self.spin_threshold.setValue(int(s.get("fail_threshold", 2)))
        self.spin_threshold.setSuffix(" 次")
        form.addRow("连续失败触发修复", self.spin_threshold)

        self.cmb_level = QComboBox()
        self.cmb_level.addItems(["L1 仅刷缓存", "L2 +服务/DHCP", "L3 +重启网卡/WLAN"])
        self.cmb_level.setCurrentIndex(max(0, min(2, int(s.get("auto_repair_max_level", 3)) - 1)))
        form.addRow("自动修复上限", self.cmb_level)
        outer.addWidget(g1)

        g2 = QGroupBox("常规")
        form2 = QFormLayout(g2)
        self.chk_start = QCheckBox("开机自动启动（计划任务，免 UAC）")
        # autostart.enabled() 要 spawn 1-2 个 schtasks /Query（可达 1s），
        # 不在构造期查：首次切到设置页时回填（_on_page_changed）
        self._autostart_loaded = False
        form2.addRow(self.chk_start)

        self.chk_tray = QCheckBox("关闭窗口时最小化到托盘")
        self.chk_tray.setChecked(bool(s.get("minimize_to_tray", True)))
        form2.addRow(self.chk_tray)

        self.chk_tray_rate = QCheckBox("托盘图标悬浮显示实时速率（↓/↑）")
        self.chk_tray_rate.setChecked(bool(s.get("tray_rate_enabled", True)))
        form2.addRow(self.chk_tray_rate)

        self.chk_sound = QCheckBox("断网/恢复声音提醒")
        self.chk_sound.setChecked(bool(s.get("sound_alert_enabled", True)))
        form2.addRow(self.chk_sound)

        self.chk_dark = QCheckBox("深色模式（勾选立即生效）")
        self.chk_dark.setChecked(theme.is_dark())
        self.chk_dark.toggled.connect(self._on_dark_toggled)
        form2.addRow(self.chk_dark)

        self.cmb_dns = QComboBox()
        self.cmb_dns.addItems(list(config.DNS_PRESETS.keys()))
        self.cmb_dns.setCurrentText(str(s.get("dns_preset", "阿里")))
        form2.addRow("DNS 预设（dns_preset 动作使用）", self.cmb_dns)
        outer.addWidget(g2)

        g3 = QGroupBox("剪贴板 / 密码库 / 命令面板")
        form3 = QFormLayout(g3)
        self.chk_clip_images = QCheckBox("剪贴板历史记录图片（存本地缓存目录）")
        self.chk_clip_images.setChecked(bool(s.get("clip_capture_images", True)))
        form3.addRow(self.chk_clip_images)

        self.spin_clip_days = QSpinBox()
        self.spin_clip_days.setRange(0, 365)
        self.spin_clip_days.setValue(int(s.get("clipboard_retain_days", 1)))
        self.spin_clip_days.setSuffix(" 天")
        self.spin_clip_days.setToolTip("剪贴板历史超过该天数自动删除（启动与每日清理；0=永不过期）")
        form3.addRow("剪贴板保留", self.spin_clip_days)

        self.spin_clip_ttl = QSpinBox()
        self.spin_clip_ttl.setRange(0, 300)
        self.spin_clip_ttl.setValue(int(s.get("clip_sensitive_ttl", 25)))
        self.spin_clip_ttl.setSuffix(" 秒")
        self.spin_clip_ttl.setToolTip("密码库复制的密码在剪贴板中存活的时间；0=不清空（仍不进历史）")
        form3.addRow("敏感复制自动清空", self.spin_clip_ttl)

        self.edt_blacklist = QLineEdit(str(s.get("clip_process_blacklist",
                                                  config.DEFAULTS["clip_process_blacklist"])))
        self.edt_blacklist.setToolTip("前台进程在名单内时不记录剪贴板（密码管理器等），逗号分隔")
        form3.addRow("剪贴板不记录进程", self.edt_blacklist)

        self.spin_autolock = QSpinBox()
        self.spin_autolock.setRange(0, 240)
        self.spin_autolock.setValue(int(s.get("vault_autolock_min", 10)))
        self.spin_autolock.setSuffix(" 分钟")
        self.spin_autolock.setToolTip("密码库自动重锁（DPAPI 免密模式）；主密码模式下自动对齐免验证窗口）")
        form3.addRow("密码库自动锁定", self.spin_autolock)

        self.cmb_hotkey = QComboBox()
        self.cmb_hotkey.setEditable(True)
        self.cmb_hotkey.addItems(["Ctrl+Alt+K", "Ctrl+Alt+Space", "Alt+C", "Ctrl+Shift+K", "F9"])
        self.cmb_hotkey.setCurrentText(str(s.get("palette_hotkey", "Ctrl+Alt+K")))
        self.cmb_hotkey.setToolTip("命令面板全局热键（保存后立即生效；被占用时自动降级为托盘菜单呼出）")
        form3.addRow("命令面板热键", self.cmb_hotkey)

        self.cmb_paste_hotkey = QComboBox()
        self.cmb_paste_hotkey.setEditable(True)
        self.cmb_paste_hotkey.addItems(["Ctrl+Win+V", "Ctrl+Alt+V", "Ctrl+Shift+V",
                                        "Ctrl+Alt+B", "Alt+V", ""])
        self.cmb_paste_hotkey.setCurrentText(
            str(s.get("clip_paste_hotkey", config.DEFAULTS["clip_paste_hotkey"])))
        self.cmb_paste_hotkey.setToolTip(
            "剪贴板快捷粘贴面板全局热键：呼出历史列表，回车即粘回呼出前的窗口；\n"
            "留空 = 禁用快捷键（仍可经托盘菜单呼出）；被占用时自动回退默认键并提示")
        form3.addRow("面板粘贴热键", self.cmb_paste_hotkey)
        self.lbl_paste_warn = self._make_office_warn(self.cmb_paste_hotkey)
        form3.addRow("", self.lbl_paste_warn)

        self.cmb_quick_hotkey = QComboBox()
        self.cmb_quick_hotkey.setEditable(True)
        self.cmb_quick_hotkey.addItems(["Ctrl+Alt+V", "Ctrl+Shift+V", "Ctrl+Alt+Z",
                                        "Ctrl+Win+V", ""])
        self.cmb_quick_hotkey.setCurrentText(
            str(s.get("clip_quick_hotkey", config.DEFAULTS["clip_quick_hotkey"])))
        self.cmb_quick_hotkey.setToolTip(
            "快速粘贴最新一条剪贴板历史：按下直接粘进当前窗口，不弹面板；\n"
            "留空 = 禁用快捷键；被占用时自动回退旧键并提示")
        form3.addRow("快速粘贴热键", self.cmb_quick_hotkey)
        self.lbl_quick_warn = self._make_office_warn(self.cmb_quick_hotkey)
        form3.addRow("", self.lbl_quick_warn)
        outer.addWidget(g3)

        btn_row = QHBoxLayout()
        btn_row.addStretch(1)
        btn = QPushButton("保存设置")
        btn.setProperty("primary", True)
        btn.setMinimumWidth(140)
        btn.clicked.connect(self._save_settings)
        btn_row.addWidget(btn)
        outer.addLayout(btn_row)        # 右对齐常规宽度（全窗宽拉伸过重）
        self.btn_save_settings = btn

        note = QLabel(
            f"{config.DISPLAY_NAME} v{config.VERSION}　|　"
            f"日志：syskit.log（正常）与 error.log（错误）分开记录，超过 "
            f"{_logging.LOG_RETENTION_DAYS} 天自动删除。　|　"
            "构建/升级：build.bat 打包，双击 dist 下 setup.exe 安装/升级"
        )
        note.setObjectName("hint")
        note.setWordWrap(True)
        outer.addWidget(note)
        outer.addStretch(1)
        return w

    def _make_office_warn(self, combo):
        """Office 冲突动态警示（硬约束①）：热键为 Ctrl+Alt+V 时显示，改键即隐。"""
        lbl = QLabel("此键会覆盖 Office 选择性粘贴等应用内快捷键，冲突时请改用其他组合。")
        lbl.setObjectName("hint")
        lbl.setWordWrap(True)
        lbl.setStyleSheet(f"color:{theme.pal()['warn']};")

        def sync():
            shown = hotkey.hotkey_display(combo.currentText().strip()) == "Ctrl+Alt+V"
            lbl.setVisible(shown)

        combo.currentTextChanged.connect(sync)
        sync()
        return lbl

    def _save_settings(self):
        # 防御性回填：保存按钮物理上只在设置页可达（进入即回填），但该不变式
        # 是隐式的——未来新增保存入口时，未回填的勾选态会把用户的自启误禁
        if not getattr(self, "_autostart_loaded", True):
            self._autostart_loaded = True
            self.chk_start.setChecked(autostart.enabled())
        # 热键保存前验证：非法输入不得破坏当前已注册的全局热键
        hotkey_text = self.cmb_hotkey.currentText().strip() or "Ctrl+Alt+K"
        if hotkey.parse_hotkey(hotkey_text) is None:
            QMessageBox.warning(self, config.DISPLAY_NAME,
                                f"热键「{self.cmb_hotkey.currentText()}」无法识别（示例：Ctrl+Alt+K）。\n"
                                "本次保存不更改全局热键，其余设置将继续保存。")
            hotkey_text = settings.load().get("palette_hotkey", "Ctrl+Alt+K")
        previous_hotkey = settings.load().get("palette_hotkey", hotkey_text)  # 保存前快照（新键被占用时回退用）
        # 快捷粘贴热键同款验证：非法输入不破坏当前热键；空串 = 显式禁用
        paste_text = self.cmb_paste_hotkey.currentText().strip()
        if paste_text and hotkey.parse_hotkey(paste_text) is None:
            QMessageBox.warning(self, config.DISPLAY_NAME,
                                f"热键「{self.cmb_paste_hotkey.currentText()}」无法识别（示例：Ctrl+Alt+V），"
                                "留空表示禁用。\n本次保存不更改快捷粘贴热键，其余设置将继续保存。")
            paste_text = str(settings.load().get("clip_paste_hotkey",
                                                 config.DEFAULTS["clip_paste_hotkey"]))
        previous_paste = str(settings.load().get("clip_paste_hotkey",
                                                 config.DEFAULTS["clip_paste_hotkey"]))
        # 快速粘贴热键同款验证：非法输入不破坏当前热键；空串 = 显式禁用
        quick_text = self.cmb_quick_hotkey.currentText().strip()
        if quick_text and hotkey.parse_hotkey(quick_text) is None:
            QMessageBox.warning(self, config.DISPLAY_NAME,
                                f"热键「{self.cmb_quick_hotkey.currentText()}」无法识别（示例：Ctrl+Alt+V），"
                                "留空表示禁用。\n本次保存不更改快速粘贴热键，其余设置将继续保存。")
            quick_text = str(settings.load().get("clip_quick_hotkey",
                                                 config.DEFAULTS["clip_quick_hotkey"]))
        previous_quick = str(settings.load().get("clip_quick_hotkey",
                                                 config.DEFAULTS["clip_quick_hotkey"]))
        patch = {
            "auto_repair_enabled": self.chk_auto.isChecked(),
            "poll_interval_sec": self.spin_interval.value(),
            "fail_threshold": self.spin_threshold.value(),
            "auto_repair_max_level": self.cmb_level.currentIndex() + 1,
            "minimize_to_tray": self.chk_tray.isChecked(),
            "tray_rate_enabled": self.chk_tray_rate.isChecked(),
            "sound_alert_enabled": self.chk_sound.isChecked(),
            "dark_mode": self.chk_dark.isChecked(),
            "dns_preset": self.cmb_dns.currentText(),
            "clip_capture_images": self.chk_clip_images.isChecked(),
            "clipboard_retain_days": self.spin_clip_days.value(),
            "clip_sensitive_ttl": self.spin_clip_ttl.value(),
            "clip_process_blacklist": self.edt_blacklist.text().strip(),
            "vault_autolock_min": self.spin_autolock.value(),
            "palette_hotkey": hotkey_text,
            "clip_paste_hotkey": paste_text,
            "clip_quick_hotkey": quick_text,
        }
        if admin.is_admin():
            try:
                if self.chk_start.isChecked() and not autostart.enabled():
                    autostart.enable()
                elif not self.chk_start.isChecked() and autostart.enabled():
                    autostart.disable()
            except Exception:
                LOGGER.warning("更新开机自启失败", exc_info=True)
                QMessageBox.warning(self, config.DISPLAY_NAME, "开机自启设置失败（详见日志），其余设置将继续保存。")
        try:
            settings.save(patch)
        except OSError as e:
            QMessageBox.warning(self, config.DISPLAY_NAME,
                                f"设置写入失败：{e}\n本次改动在当前运行中仍生效，重启后会丢失。")
            return
        self._tray_rate = bool(patch["tray_rate_enabled"])   # 托盘速率开关即时生效
        # 三把全局热键即时重注册（命令面板 / 快速粘贴走通用回退；面板键走
        # 专用回退链，硬约束③——单源见 _rehook_hotkey / _rehook_paste_hotkey）。
        # 局部导入是刻意的补丁点：测试经 palette_mod 属性替换这些入口
        # （面板键经 _rehook_paste_hotkey 内的 palette_mod 属性调用，不在此列）
        from .palette import (setup_global_hotkey, setup_quick_paste_hotkey,
                              teardown_global_hotkey, teardown_quick_paste_hotkey)
        self._rehook_hotkey("palette_hotkey", setup_global_hotkey,
                            teardown_global_hotkey, hotkey_text, previous_hotkey,
                            self.cmb_hotkey, "全局热键暂不可用；托盘菜单与 Ctrl+K 不受影响。")
        self._rehook_hotkey("clip_quick_hotkey", setup_quick_paste_hotkey,
                            teardown_quick_paste_hotkey, quick_text, previous_quick,
                            self.cmb_quick_hotkey, "快速粘贴暂不可用；可经面板热键或托盘菜单呼出。")
        self._rehook_paste_hotkey(paste_text, previous_paste)
        # 主题即时切换（保存入口的兜底路径；勾选本身在 _on_dark_toggled 已即时生效）
        if theme.is_dark() != self.chk_dark.isChecked():
            self._apply_theme(self.chk_dark.isChecked())
        QMessageBox.information(self, config.DISPLAY_NAME, "设置已保存。")

    def _rehook_hotkey(self, key: str, setup, teardown, text: str, previous: str,
                       combo, fail_hint: str):
        """设置保存后即时重注册一把全局热键；新键被占 → 回退旧键（避免全丢）。

        空串 = 显式禁用：只反注册、静默无提示（旧版命令面板分支会对空串
        误报「已被其他程序占用」，合并后按禁用处理）。
        """
        teardown(self)
        if not text or setup(self):
            return
        if text != previous:                         # 新键被占 → 回退旧键
            settings.save({key: previous})
            combo.setCurrentText(previous)
            setup(self)
            QMessageBox.information(
                self, config.DISPLAY_NAME,
                f"热键「{text}」已被其他程序占用，已恢复为「"
                + (hotkey.hotkey_display(previous) or "无热键（已禁用）") + "」。")
        else:                                        # 原键被抢 → 明确告知（不静默丢）
            QMessageBox.warning(self, config.DISPLAY_NAME,
                                f"热键「{text}」注册失败（可能刚被其他程序占用），{fail_hint}")

    def _rehook_paste_hotkey(self, text: str, previous: str):
        """面板粘贴热键重注册 + 专用回退链（硬约束③，绝不钩子强拦）。

        新键（如 Ctrl+Win+V）注册失败 → 尝试 "Ctrl+Alt+V" 预设（快速键未占用
        该组合时；写回设置并在组合框回显）→ 仍失败则禁用面板热键并提示用户
        自定义。与 _rehook_hotkey 分开：回退目标不是旧键而是固定预设。
        经 palette_mod 属性查找调用，保持与 _save_settings 同款测试补丁点。
        """
        from . import palette as _palette_mod
        _palette_mod.teardown_paste_hotkey(self)
        if not text or _palette_mod.setup_paste_hotkey(self):
            return
        fallback = "Ctrl+Alt+V"
        quick_key = str(settings.load().get("clip_quick_hotkey", "") or "").strip().lower()
        if fallback.lower() != quick_key:
            settings.save({"clip_paste_hotkey": fallback})
            self.cmb_paste_hotkey.setCurrentText(fallback)
            if _palette_mod.setup_paste_hotkey(self):
                QMessageBox.information(
                    self, config.DISPLAY_NAME,
                    f"热键「{text}」已被其他程序占用，面板粘贴已回退为「{fallback}」"
                    "（可在设置页改为其他组合）。")
                return
        settings.save({"clip_paste_hotkey": ""})
        self.cmb_paste_hotkey.setCurrentText("")
        QMessageBox.warning(
            self, config.DISPLAY_NAME,
            f"热键「{text}」注册失败（可能被其他程序占用），面板粘贴热键已禁用；"
            "请回设置页自定义其他组合（托盘菜单仍可呼出面板）。")

    def _apply_theme(self, dark: bool):
        from PySide6.QtWidgets import QApplication
        theme.apply(QApplication.instance(), dark=dark)
        self._on_theme_changed()

    def _on_dark_toggled(self, dark: bool):
        """深色勾选即时生效并立即持久化（补57：旧路径要点「保存设置」才切换，
        体感即「不生效/不行」）。"""
        if theme.is_dark() == dark:
            return
        self._apply_theme(dark)
        try:
            settings.save({"dark_mode": dark})
        except Exception:
            LOGGER.warning("深色模式持久化失败", exc_info=True)

    def _on_theme_changed(self):
        """主题切换后的刷新：QSS 热替换只管样式表，固化在 item/QColor/样式表
        字面量里的颜色必须逐处重取 pal（补57 清单：诊断树/修复动作表/事件
        时间线/电池健康色/管理员横幅/代理标签/网卡表；自绘控件靠 update()）。"""
        from PySide6.QtWidgets import QApplication
        for w in QApplication.allWidgets():
            w.update()
        self._style_admin_banner()
        self._refresh_proxy_label()
        self._fill_actions()                      # 修复表风险三色重填
        self.page_dash.reload_events()            # 事件时间线重取 pal
        self.page_system.refresh_theme_colors()   # 电池健康色重填
        if self._diag_results:
            base, text = netcheck.verdict(self._diag_results)
            self._set_status(text, status_color(base),
                             sub=f"{time.strftime('%H:%M')} 诊断")
            self.lbl_verdict.setStyleSheet(f"color:{status_color(base)};")
            for i in range(self.tree_diag.topLevelItemCount()):
                it = self.tree_diag.topLevelItem(i)
                r = it.data(0, Qt.UserRole)
                if r is not None:
                    it.setForeground(0, QColor(check_color(r.status)))
        if getattr(self, "_last_snap", None) is not None:
            self._fill_adapters(self._last_snap)

    # ---------------- 日志页 ----------------
    def _build_log_tab(self) -> QWidget:
        w = QWidget()
        lay = QVBoxLayout(w)
        btns = QHBoxLayout()
        self.cmb_log_src = QComboBox()
        self.cmb_log_src.addItems(["普通日志（syskit.log）", "错误日志（error.log）"])
        self.cmb_log_src.currentIndexChanged.connect(lambda _i: self._reload_log())
        b1 = QPushButton("立即刷新")
        b1.clicked.connect(self._reload_log)
        b2 = QPushButton("打开日志目录")
        b2.clicked.connect(lambda: os.startfile(config.LOG_DIR))
        btns.addWidget(QLabel("来源:"))
        btns.addWidget(self.cmb_log_src)
        btns.addWidget(b1)
        btns.addWidget(b2)
        btns.addStretch(1)
        btns.addWidget(QLabel("超过 7 天的日志自动删除"))
        lay.addLayout(btns)
        self.txt_log = QPlainTextEdit()
        self.txt_log.setReadOnly(True)
        self.txt_log.setMaximumBlockCount(5000)
        lay.addWidget(self.txt_log)
        self._reload_log()
        return w

    def _on_page_changed(self):
        """页面切换：日志增量刷新 + 重 IO 的懒加载（首次进入该页才执行）。"""
        cur = self.stack.currentWidget()
        if cur is self.page_repair and not getattr(self, "_profiles_loaded", True):
            self._profiles_loaded = True
            self._refresh_profiles()
        if cur is self.page_settings and not getattr(self, "_autostart_loaded", True):
            self._autostart_loaded = True
            self.chk_start.setChecked(autostart.enabled())
        # 隐藏页采样被可见性守卫跳过：断流超过图表窗口就清空旧序列，
        # 避免「1 小时前」与现在被画成连续时间线
        if cur is self.page_dash:
            self.page_dash.reset_if_stale()
        if cur is self.page_system:
            self.page_system.reset_if_stale()
        self._refresh_telemetry_scope()
        self._reload_log()

    def _reload_log(self):
        if not hasattr(self, "txt_log"):
            return
        # 构造期首次调用时 page_logs 尚未赋值（_build_log_tab 内部）
        if getattr(self, "page_logs", None) is None or self.stack.currentWidget() is not self.page_logs:
            return
        name = "error.log" if getattr(self, "cmb_log_src", None) and self.cmb_log_src.currentIndex() == 1 else "syskit.log"
        path = os.path.join(config.LOG_DIR, name)
        # 增量读：日志页每 5s 刷新，全量 readlines 最多 5MB 且 setPlainText 会把
        # 用户正在上翻的滚动位置拽回顶部；只追加新字节，Qt 在用户位于底部时才跟随滚动
        state = getattr(self, "_log_state", None)
        if state is None or state[0] != name:               # 首次 / 换源 → 尾部载入
            self._log_state = [name, self._log_tail_load(path, name)]
            return
        offset = state[1]
        try:
            if os.path.getsize(path) < offset:              # 轮转/截断 → 重读尾部
                self._log_state[1] = self._log_tail_load(path, name)
                return
            with open(path, "rb") as f:
                f.seek(offset)
                data = f.read()
            cut = data.rfind(b"\n")                         # 只取完整行，剩余下轮再读
            if cut == -1:
                return
            self._log_state[1] = offset + cut + 1
            chunk = data[: cut + 1].decode("utf-8", errors="replace").rstrip("\n")
            if chunk:
                self.txt_log.appendPlainText(chunk)
        except OSError:
            self.txt_log.setPlainText(f"（{name} 尚无内容）")

    def _log_tail_load(self, path: str, name: str) -> int:
        """尾部载入当前视图，返回新的读取偏移（字节数，供增量续读）。"""
        try:
            with open(path, "rb") as f:
                f.seek(0, os.SEEK_END)
                size = f.tell()
                f.seek(max(0, size - 200_000))
                lines = f.read().decode("utf-8", errors="replace").splitlines()
            self.txt_log.setPlainText("\n".join(lines[-800:]))
            return size
        except OSError:
            self.txt_log.setPlainText(f"（{name} 尚无内容）")
            return 0

    # ---------------- 修复执行 ----------------
    def run_repair(self, aid, param=None):
        act = repairs.get_action(aid)
        if not act:
            return
        if self._busy:
            # 按钮可点但静默无效会被当成"功能坏了"：至少给一句解释
            self.statusBar().showMessage("有诊断/修复任务进行中，请等其完成后再操作", 4000)
            return
        if not admin.is_admin() and act.risk != repairs.RISK_SAFE:
            QMessageBox.warning(self, config.DISPLAY_NAME, "该操作需要管理员权限，请以管理员身份运行。")
            return
        if act.risk != repairs.RISK_SAFE:
            dlg = ConfirmDialog(act)
            if dlg.exec() != ConfirmDialog.Accepted:
                return
        if self._repair_worker:
            QMessageBox.information(self, config.DISPLAY_NAME, "有修复正在进行中，请稍候。")
            return
        self._set_busy(True, f"正在执行：{act.title} …")
        self._repair_worker = reap(RepairWorker(aid, param or {}), self)
        self._repair_worker.done.connect(self._repair_done)
        self._repair_worker.start()

    def _repair_done(self, payload):
        self._release_worker("_repair_worker")
        aid, result = payload
        act = repairs.get_action(aid)
        title = act.title if act else aid
        self._add_repair_record(aid, result)
        LOGGER.info("手动修复 [{}] -> {}: {}", aid, result.success, result.message)
        msg = f"[{'成功' if result.success else '失败'}] {title}\n{result.message}"
        if result.need_reboot:
            msg += "\n\n⚠ 需要重启电脑后生效。"
        QMessageBox.information(self, "修复结果", msg)
        self._set_busy(False)
        self.statusBar().showMessage(f"修复完成：{title}", 6000)
        self._update_mac_state()
        try:
            # 修复（尤其 proxy_off）会改系统代理，修复完成后必须重读，否则标签停留在旧值
            self._refresh_proxy_label()
        except Exception:
            LOGGER.warning("刷新代理状态失败", exc_info=True)
        self.run_diagnosis(quick=True, silent=True)

    def run_combo(self, key):
        if self._busy or self._combo_worker:
            return
        combo_title, ids = repairs.COMBOS[key]
        if key == "stack" and not admin.is_admin():
            QMessageBox.warning(self, config.DISPLAY_NAME, "需要管理员权限。")
            return
        # 改配置类逐项确认（与弹窗文案一致）；无管理员则跳过非 SAFE 项
        confirmed = []
        skipped_no_admin = []
        for aid in ids:
            act = repairs.get_action(aid)
            if not act:
                continue
            if not admin.is_admin() and act.risk != repairs.RISK_SAFE:
                skipped_no_admin.append(act.title)
                continue
            if act.risk != repairs.RISK_SAFE:
                dlg = ConfirmDialog(act)
                if dlg.exec() != ConfirmDialog.Accepted:
                    continue
            confirmed.append(aid)
        if not confirmed:
            if skipped_no_admin:
                QMessageBox.information(
                    self, config.DISPLAY_NAME,
                    "未执行任何动作（缺少管理员权限，且组合内无可自动执行的安全项）。\n"
                    f"受限项：{'、'.join(skipped_no_admin)}")
            return
        desc = "\n".join(f"· {repairs.get_action(a).title}" for a in confirmed)
        if "restore_point" in confirmed:
            # 补47 挂账、审计第 9 批 F4 复核的决策项（文案级预案）：首位还原点
            # 常态 30~90s 且不可中止，确认弹窗必须把这段空等说在前面
            desc += "\n\n（第一步创建系统还原点：常态耗时 30~90 秒，期间无法中止，请耐心等待）"
        extra = f"\n\n已跳过 {len(ids) - len(confirmed)} 项（取消或无权限）。" if len(confirmed) < len(ids) else ""
        dlg = QMessageBox(self)
        dlg.setWindowTitle("确认组合修复")
        dlg.setText(f"{combo_title} 将依次执行：\n\n{desc}{extra}")
        dlg.setStandardButtons(QMessageBox.Ok | QMessageBox.Cancel)
        if dlg.exec() != QMessageBox.Ok:
            return
        self._set_busy(True, f"正在执行：{combo_title} …")
        self._combo_worker = reap(ComboWorker(confirmed), self)
        self._combo_worker.item_done.connect(self._combo_item)
        self._combo_worker.all_done.connect(self._combo_done)
        self._combo_results = []
        self._combo_worker.start()
        self.btn_combo_stop.setEnabled(True)

    def _combo_item(self, payload):
        aid, result = payload
        self._combo_results.append((aid, result))
        self._add_repair_record(aid, result)
        LOGGER.info("组合修复 [{}] -> {}: {}", aid, result.success, result.message)

    def _combo_done(self):
        worker, self._combo_worker = self._combo_worker, None
        if worker is not None:
            worker.wait(3000)
        self.btn_combo_stop.setEnabled(False)
        ok = sum(1 for _, r in self._combo_results if r.success)
        lines = "\n".join(f"{'√' if r.success else '×'} {repairs.get_action(a).title}: {r.message[:80]}"
                          for a, r in self._combo_results)
        self._set_busy(False)
        if getattr(worker, "cancelled", False):
            planned = len(getattr(worker, "action_ids", []) or [])
            skipped = max(0, planned - len(self._combo_results))
            QMessageBox.information(
                self, "组合修复已中止",
                f"已执行 {ok}/{len(self._combo_results)} 项成功，剩余约 {skipped} 项未执行。\n\n"
                f"注意：已执行的系统操作不会自动撤销。\n\n{lines}")
        else:
            QMessageBox.information(self, "组合修复完成", f"{ok}/{len(self._combo_results)} 项成功：\n\n{lines}")
        self.run_diagnosis(quick=True, silent=True)

    # ---------------- 监控回调（UI 线程） ----------------
    def _on_monitor_status(self, state, message):
        tips = {"ok": "网络正常", "no_net": "已连接但无法上网",
                "ncsi_false": "NCSI 误报", "offline": "无网络连接",
                "no_internet": "外网不可达"}
        text = tips.get(state, state)
        color = status_color(state)
        self._set_status(text, color, sub=message)
        self.tray.set_state(state, message)
        if state != self._last_state:            # 仅状态切换记入时间线
            first = self._last_state is None
            self._last_state = state
            self.eventstore.add(state, message)
            self.page_dash.reload_events()
            if not first and settings.load().get("sound_alert_enabled", True):
                self._play_alert(state)
        if state == "ok" and message:
            self.tray.notify(config.DISPLAY_NAME, message, state)

    def _on_monitor_action(self, payload):
        aid, result = payload
        act = repairs.get_action(aid)
        self._add_repair_record(aid, result)
        if not result.success:
            self.tray.notify("自动修复失败", f"{act.title if act else aid}: {result.message[:80]}", "no_net")

    @staticmethod
    def _play_alert(state):
        """断网/恢复提示音（异步播放，不阻塞 UI）。"""
        try:
            import winsound
            sound = "SystemExclamation" if state != "ok" else "SystemAsterisk"
            winsound.PlaySound(sound, winsound.SND_ALIAS | winsound.SND_ASYNC)
        except Exception:
            pass

    def _on_monitor_suggest(self, payload):
        fid, why = payload
        act = repairs.get_action(fid)
        if act:
            self.tray.notify("建议手动修复", f"{act.title}：{why}")

    # ---------------- 关闭 ----------------
    def closeEvent(self, event):
        if settings.load().get("minimize_to_tray", True):
            event.ignore()
            self.hide()
            self.tray.showMessage(config.DISPLAY_NAME, "已最小化到托盘，仍在后台守护网络。", QSystemTrayIcon.Information, 4000)
        else:
            self.quit_app()
            event.accept()

    def quit_app(self):
        self._save_window_state()
        teardown_global_hotkey(self)
        teardown_quick_paste_hotkey(self)
        teardown_paste_hotkey(self)
        self.mon.stop()
        self.tele.stop()
        self.tele.wait(3000)              # 等遥测线程退出，避免 QThread destroyed while running
        try:
            self.mon.join(3)              # join 单位是秒（曾误传 3000 = 50 分钟）；
        except RuntimeError:              # stop() 已同时置 force 唤醒 poll 等待，通常 <1s
            pass
        # 全树扫尾：沿对象树枚举所有仍存活的工作线程统一等待（reap 已挂 parent，
        # findChildren 可枚举；telemetry 的 netsh 最坏 8s → 上限 10s），杜绝退出期
        # "QThread destroyed while running" qFatal 家族
        from PySide6.QtCore import QThread as _QThread
        from PySide6.QtWidgets import QApplication as _QApp
        app_inst = _QApp.instance()
        if app_inst is not None:
            from .system_page import ClipboardTab as _CT
            for widget in app_inst.allWidgets():
                if isinstance(widget, _CT):
                    widget.flush_disk()      # 剪贴板防抖未落盘内容退出前刷盘
            for widget in app_inst.allWidgets():
                for th in widget.findChildren(_QThread):
                    if th.isRunning():
                        # stop() 请求停止（tracert 等）；cancel() 让诊断/组合
                        # 跳过剩余项（补55 B-2：只认 stop 时，卡在 restore_point
                        # 的组合最坏 180s > 10s 上限，退出期仍有 qFatal 窗口）
                        for stop_name in ("stop", "cancel"):
                            fn = getattr(th, stop_name, None)
                            if callable(fn):
                                try:
                                    fn()
                                except Exception:
                                    LOGGER.debug("退出扫尾调用 {} 失败", stop_name)
                        th.wait(10_000)
        # 主窗口 4 worker 显式等待（reap 已挂 self；findChildren 之外的双保险）
        for attr in ("_snap_worker", "_diag_worker", "_repair_worker", "_combo_worker"):
            w = getattr(self, attr, None)
            if w is not None and w.isRunning():
                w.wait(10_000)
            setattr(self, attr, None)
        from .. import proc as _proc_mod
        _proc_mod.close_resident()        # 常驻 PS 会话收尾（EOF 后子进程自然退出）
        self.tele.traffic.save_if_dirty()
        self.tray.hide()
        from PySide6.QtWidgets import QApplication
        QApplication.quit()


def act_hint(a):
    return a.restore_hint
