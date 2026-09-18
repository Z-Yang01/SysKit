"""托盘：四色状态图标（QPainter 动态绘制）、右键菜单、气泡通知。"""
from PySide6.QtCore import QSize, Qt
from PySide6.QtGui import QAction, QColor, QBrush, QIcon, QPainter, QPixmap, QPen
from PySide6.QtWidgets import QApplication, QMenu, QSystemTrayIcon


from .. import clipboard as clipboard_mod
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
        self._menu = menu = QMenu()   # 显式保引用：setContextMenu 不取得所有权
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
        clip_menu = menu.addMenu("📋 最近剪贴板")
        clip_menu.aboutToShow.connect(lambda: self._fill_clip_menu(clip_menu))
        if getattr(window, "toggle_palette", None):
            act_palette = QAction("⌘ 命令面板", menu)
            act_palette.setToolTip("全局热键呼出，输入即搜")
            act_palette.triggered.connect(window.toggle_palette)
            menu.addAction(act_palette)
        menu.addSeparator()
        menu.addAction(act_quit)
        self.setContextMenu(menu)
        self.activated.connect(self._activated)
        self._state = "offline"

    def _activated(self, reason):
        if reason == QSystemTrayIcon.Trigger:
            self.show_window()

    def _fill_clip_menu(self, m):
        """重建「最近剪贴板」子菜单：最近 5 条，点击回填剪贴板。

        缓存（mtime, items)：每次弹出菜单都新建 ClipboardHistory 会触发
        DPAPI 全量解密（与 palette 同款缓存）；文件 mtime 未变直接复用。
        """
        m.clear()
        try:
            import os
            path = os.path.join(config.APPDATA_DIR, "clipboard.json")
            mtime = os.path.getmtime(path) if os.path.exists(path) else 0
            cache = getattr(self, "_clip_cache", None)
            if cache is None or cache[0] != mtime:
                max_items = int(config.DEFAULTS.get("clipboard_max", 50))
                from .. import settings as settings_mod
                max_items = int(settings_mod.load().get("clipboard_max", max_items))
                hist = clipboard_mod.ClipboardHistory(max_items=max_items)
                cache = (mtime, hist.items()[:5])
                self._clip_cache = cache
            items = cache[1]
        except Exception:
            items = []
        if not items:
            empty = QAction("（空）", m)
            empty.setEnabled(False)
            m.addAction(empty)
            return
        for it in items:
            text = it.get("text", "")
            kind = it.get("kind", "text")
            label = ("🖼 图片条目" if kind == "image" else clipboard_mod.ClipboardHistory.preview(text, 40))
            payload = it.get("data", "") if kind == "image" else text
            act = QAction(label, m)
            act.triggered.connect(lambda checked=False, t=payload, k=kind: self._copy_back(t, k))
            m.addAction(act)

    def _copy_back(self, text, kind="text"):
        app = QApplication.instance()
        if not app or not text:
            return
        clip = app.clipboard()
        if kind == "image":
            from PySide6.QtGui import QImage
            from .. import clipboard as cb
            path = cb.image_path(text)
            try:
                img = QImage(path)
                if not img.isNull():
                    clip.setImage(img)
                    self.notify("剪贴板", "已把图片放回剪贴板")
                    return
            except Exception:
                pass
            return
        clip.setText(text)
        self.notify("剪贴板", "已复制到剪贴板")

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
