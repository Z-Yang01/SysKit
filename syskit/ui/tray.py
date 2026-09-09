"""托盘：四色状态图标（QPainter 动态绘制）、右键菜单、气泡通知。"""
from PySide6.QtCore import QSize, Qt
from PySide6.QtGui import QAction, QColor, QBrush, QIcon, QPainter, QPixmap, QPen
from PySide6.QtWidgets import QMenu, QSystemTrayIcon


from .. import config
STATE_COLORS = {
    "ok": QColor("#2e9e44"),
    "no_net": QColor("#f2a33c"),
    "ncsi_false": QColor("#f2a33c"),
    "offline": QColor("#9e9e9e"),
    "no_internet": QColor("#d94040"),
}


def state_icon(state: str, size: int = 64) -> QIcon:
    base = QColor("#3576e0") if state == "ok" else STATE_COLORS.get(state, QColor("#9e9e9e"))
    pm = QPixmap(QSize(size, size))
    pm.fill(Qt.transparent)
    p = QPainter(pm)
    p.setRenderHint(QPainter.Antialiasing)
    p.setPen(QPen(QColor(0, 0, 0, 40), 2))
    p.setBrush(QBrush(base))
    p.drawEllipse(4, 4, size - 8, size - 8)
    # 高光
    p.setPen(Qt.NoPen)
    p.setBrush(QBrush(QColor(255, 255, 255, 70)))
    p.drawEllipse(size // 5, size // 6, size // 3, size // 4)
    p.end()
    return QIcon(pm)


class TrayHost(QSystemTrayIcon):
    def __init__(self, window, parent=None):
        super().__init__(state_icon("offline"), parent)
        self.window = window
        self._base_tip = config.DISPLAY_NAME
        self._rate_text = ""
        self.setToolTip(config.DISPLAY_NAME)
        menu = QMenu()
        act_open = QAction("打开主界面", menu)
        act_check = QAction("立即检测", menu)
        act_fix = QAction("快速修复", menu)
        act_quit = QAction("退出", menu)
        act_open.triggered.connect(self.show_window)
        act_check.triggered.connect(lambda: window.run_diagnosis(quick=True))
        act_fix.triggered.connect(lambda: window.run_repair("flush_dns"))
        act_quit.triggered.connect(window.quit_app)
        for a in (act_open, act_check, act_fix):
            menu.addAction(a)
        menu.addSeparator()
        menu.addAction(act_quit)
        self.setContextMenu(menu)
        self.activated.connect(self._activated)
        self._state = "offline"

    def _activated(self, reason):
        if reason == QSystemTrayIcon.Trigger:
            self.show_window()

    def show_window(self):
        w = self.window
        w.showNormal()
        w.raise_()
        w.activateWindow()

    def set_state(self, state: str, message: str = ""):
        self._state = state
        self.setIcon(state_icon(state))
        tips = {
            "ok": "网络正常",
            "no_net": "已连接但无法上网",
            "ncsi_false": "NCSI 误报",
            "no_internet": "外网不可达",
            "offline": "无网络连接",
        }
        self._base_tip = f"{config.DISPLAY_NAME} — {tips.get(state, state)}" + (f"（{message}）" if message else "")
        self._refresh_tip()

    def update_rate(self, down_bps: float, up_bps: float):
        """托盘悬浮显示实时速率。"""
        from .charts import human_bytes
        self._rate_text = f"↓ {human_bytes(down_bps)}/s   ↑ {human_bytes(up_bps)}/s"
        self._refresh_tip()

    def _refresh_tip(self):
        tip = self._base_tip
        if self._rate_text:
            tip = f"{self._base_tip}\n{self._rate_text}"
        self.setToolTip(tip)

    def notify(self, title: str, body: str, state: str = None):
        if state:
            self.set_state(state)
        self.showMessage(title, body, QSystemTrayIcon.Information, 6000)
