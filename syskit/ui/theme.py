"""双主题（浅色/深色）QSS + 调色板。图表等自绘控件运行时取 pal() 适配。

除调色板与 QSS 外，这里还提供三个 UI 辅助：
- ``add_shadow`` —— 给卡片类控件加柔和投影，拉开层次；
- ``make_table`` —— 页面表格的公共构造约定（只读、无行号头、隔行底色）；
- ``app_logo_pixmap`` —— 代码绘制的 SysKit 标识（不依赖打包资源，onefile 也安全）。
"""

_LIGHT = {
    "bg": "#f3f5f9", "card": "#ffffff", "border": "#e2e8f0", "text": "#2d3748",
    "sub": "#7b8794", "grid": "#d0d7e0", "hover": "#eef2f7",
    "sel_bg": "#dcebff", "sel_text": "#1a3a6b", "accent": "#3576e0",
    "accent_hover": "#2a63c7", "accent_disabled": "#a8c4f0",
    "btn_border": "#cfd6e0", "btn_pressed": "#e8f0fc", "header_bg": "#edf1f6",
    "header_text": "#4a5568", "alt_row": "#f7f9fc", "danger": "#c62828",
    "danger_hover_bg": "#fdf1f1", "danger_border": "#efc7c7", "scrollbar": "#c5cddb",
    "tooltip_bg": "#2f3a48", "tooltip_text": "#ffffff",
    "ok": "#2e9e44", "warn": "#ef6c00", "err": "#d94040", "info": "#3576e0",
    # 柔和底色（徽章/强调块）
    "accent_soft": "#e7eefc", "ok_soft": "#e6f4e9", "warn_soft": "#fdf1e5",
    "err_soft": "#fdecec", "brand_sub": "#8a93a0",
}
_DARK = {
    "bg": "#12161c", "card": "#1c232e", "border": "#2c3642", "text": "#e2e8f0",
    "sub": "#8b96a5", "grid": "#303a47", "hover": "#243040",
    "sel_bg": "#2a3d58", "sel_text": "#d5e4ff", "accent": "#4a8af0",
    "accent_hover": "#5d97f2", "accent_disabled": "#3a4a63",
    "btn_border": "#3a4554", "btn_pressed": "#2a3d58", "header_bg": "#222b36",
    "header_text": "#9aa7b5", "alt_row": "#181f28", "danger": "#ff8a80",
    "danger_hover_bg": "#332225", "danger_border": "#5a3030", "scrollbar": "#3d4855",
    "tooltip_bg": "#e8edf3", "tooltip_text": "#1c232e",
    "ok": "#4caf50", "warn": "#ffb74d", "err": "#ef5350", "info": "#64b5f6",
    "accent_soft": "#1e2f49", "ok_soft": "#1b2f22", "warn_soft": "#33281a",
    "err_soft": "#33211f", "brand_sub": "#8b96a5",
}

_current = dict(_LIGHT)
_dark = False


def pal() -> dict:
    """当前调色板（自绘控件在 paintEvent 里读取以适配主题）。"""
    return _current


def is_dark() -> bool:
    return _dark


def sub_color() -> str:
    """次要文字色（提示/说明标签统一使用，避免写死浅色灰）。"""
    return _current["sub"]


def add_shadow(widget, blur: int = 22, dy: int = 4, alpha: int = 38):
    """给卡片类控件加柔和投影。注意：Qt 的 graphics effect 每控件一个，覆盖式设置。"""
    try:
        from PySide6.QtGui import QColor
        from PySide6.QtWidgets import QGraphicsDropShadowEffect
        eff = QGraphicsDropShadowEffect(widget)
        eff.setBlurRadius(blur)
        eff.setOffset(0, dy)
        eff.setColor(QColor(12, 22, 44, alpha))
        widget.setGraphicsEffect(eff)
        return eff
    except Exception:
        return None


def make_table(headers, select_rows: bool = False, alternating: bool = True):
    """页面表格统一构造：0 行、只读、隐藏行号头。

    全站 16 张表都重复这四行（QTableWidget(0, n) + 表头 + NoEditTriggers +
    verticalHeader 隐藏），改一处约定比改 16 处更可靠。列宽/拉伸策略各表差异
    是真实的（NLM 判定列吃剩余宽度、DNS 说明列、进程表按列排序…），故留在调用侧。
    """
    from PySide6.QtWidgets import QAbstractItemView, QTableWidget

    t = QTableWidget(0, len(headers))
    t.setHorizontalHeaderLabels(list(headers))
    t.setEditTriggers(QAbstractItemView.NoEditTriggers)
    t.verticalHeader().setVisible(False)
    if alternating:
        t.setAlternatingRowColors(True)
    if select_rows:
        t.setSelectionBehavior(QAbstractItemView.SelectRows)
    return t


_LOGO_CACHE = {}        # {size: QPixmap}——启动期渲染 ~15 次（app_icon×2×7 尺寸 + 侧栏）
_ICON_CACHE = None      # 静态内容，进程级一份


def app_logo_pixmap(size: int = 34):
    """代码绘制 SysKit 标识：蓝色圆角底 + 白色 S（与 assets/app.ico 观感一致）。

    不走文件资源——PyInstaller onefile 未打包 assets，用绘制保证任何分发方式都可用。
    结果按尺寸缓存：绘制是确定性纯函数（颜色写死与主题无关），启动期
    app_icon() 两处调用 + 侧栏 logo 共渲染 ~15 次，QPainter 画字 ~5ms/次。
    """
    global _ICON_CACHE
    pm = _LOGO_CACHE.get(size)
    if pm is not None:
        return pm
    from PySide6.QtCore import QRectF, Qt
    from PySide6.QtGui import QColor, QFont, QLinearGradient, QPainter, QPixmap

    _ICON_CACHE = None                  # 底层位图变了，旧 QIcon 不再复用
    pm = QPixmap(size, size)
    pm.fill(Qt.transparent)
    p = QPainter(pm)
    try:
        p.setRenderHint(QPainter.Antialiasing)
        grad = QLinearGradient(0, 0, size, size)
        grad.setColorAt(0.0, QColor("#4c8bf5"))
        grad.setColorAt(1.0, QColor("#2a63c7"))
        p.setBrush(grad)
        p.setPen(Qt.NoPen)
        p.drawRoundedRect(QRectF(0, 0, size, size), size * 0.26, size * 0.26)
        p.setPen(QColor("#ffffff"))
        f = QFont("Microsoft YaHei UI", int(size * 0.52))
        f.setBold(True)
        p.setFont(f)
        p.drawText(QRectF(0, 0, size, size), Qt.AlignCenter, "S")
    finally:
        p.end()
    _LOGO_CACHE[size] = pm
    return pm


def app_icon():
    """应用图标（窗口标题栏 / 任务栏 / Alt+Tab）：多尺寸代码绘制，与 exe 图标观感一致。"""
    global _ICON_CACHE
    if _ICON_CACHE is not None:
        return _ICON_CACHE
    from PySide6.QtGui import QIcon

    icon = QIcon()
    for size in (16, 24, 32, 48, 64, 128, 256):
        icon.addPixmap(app_logo_pixmap(size))
    _ICON_CACHE = icon
    return icon


QSS_TEMPLATE = """
* { font-family: "Microsoft YaHei UI", "Segoe UI", sans-serif; font-size: 9pt; color: @text; }

QMainWindow, QDialog, QWidget#pageRoot { background: @bg; }

QToolTip {
    background: @tooltip_bg; color: @tooltip_text; border: none; border-radius: 6px;
    padding: 7px 10px; font-size: 9pt;
}

/* ---------------- 选项卡 ---------------- */
QTabWidget::pane { border: none; background: transparent; padding-top: 6px; }
QTabBar { qproperty-drawBase: 0; }
QTabBar::tab {
    background: transparent; color: @sub;
    padding: 9px 18px; margin-right: 2px;
    border: none; border-bottom: 3px solid transparent;
    font-size: 10pt;
}
QTabBar::tab:selected { color: @accent; border-bottom: 3px solid @accent; font-weight: bold; }
QTabBar::tab:hover:!selected { color: @accent; background: @hover; border-radius: 8px 8px 0 0; }
QTabBar::tab:disabled { color: @border; }

/* ---------------- 容器 ---------------- */
QGroupBox {
    background: @card; border: 1px solid @border; border-radius: 12px;
    margin-top: 16px; padding: 14px 12px 12px 12px;
    font-weight: bold; color: @text;
}
QGroupBox::title {
    subcontrol-origin: margin; left: 16px; top: 0px; padding: 0 8px;
    color: @accent; background: @card;
}
QWidget#card, QFrame#statCard, QFrame#brandCard {
    background: @card; border: 1px solid @border; border-radius: 12px;
}
QLabel#card { padding: 18px 14px; }
QSplitter::handle { background: @border; }
QSplitter::handle:horizontal { width: 2px; }
QSplitter::handle:vertical { height: 2px; }
QSplitter::handle:hover { background: @accent; }

/* ---------------- 按钮 ---------------- */
QPushButton {
    background: @card; border: 1px solid @btn_border; border-radius: 8px;
    padding: 7px 16px; color: @text; min-height: 20px;
}
QPushButton:hover { border-color: @accent; color: @accent; background: @hover; }
QPushButton:focus { border-color: @accent; outline: none; }
QPushButton:pressed { background: @btn_pressed; }
QPushButton:disabled { color: @sub; border-color: @border; background: transparent; }

QPushButton[primary="true"] {
    background: @accent; color: #ffffff; border: none; font-weight: bold;
    padding: 8px 20px;
}
QPushButton[primary="true"]:hover { background: @accent_hover; }
QPushButton[primary="true"]:pressed { background: @accent_hover; }
QPushButton[primary="true"]:disabled { background: @accent_disabled; color: @card; }

QPushButton[danger="true"] { color: @danger; border-color: @danger_border; }
QPushButton[danger="true"]:hover { background: @danger_hover_bg; border-color: @danger; }

QPushButton[ghost="true"] {
    background: transparent; border: 1px solid transparent; color: @accent;
}
QPushButton[ghost="true"]:hover { background: @accent_soft; border-color: @accent_soft; }

QToolButton { background: transparent; border: none; border-radius: 6px; padding: 4px; }
QToolButton:hover { background: @hover; }

/* ---------------- 列表 / 表格 / 树 ---------------- */
QTableWidget, QTreeWidget, QListWidget {
    background: @card; border: 1px solid @border; border-radius: 10px;
    padding: 4px; alternate-background-color: @alt_row;
    selection-background-color: @sel_bg; selection-color: @sel_text;
    outline: 0;
}
QHeaderView::section {
    background: @header_bg; color: @header_text; border: none;
    border-bottom: 1px solid @border; border-right: 1px solid @border;
    padding: 8px 10px; font-weight: bold;
}
QHeaderView::section:hover { background: @hover; color: @accent; }
QTableWidget::item, QTreeWidget::item, QListWidget::item { padding: 5px 3px; }
QTableWidget::item:hover, QTreeWidget::item:hover { background: @hover; }
QTreeWidget::branch { background: transparent; }

/* ---------------- 文本区 ---------------- */
QPlainTextEdit, QTextEdit {
    background: @card; border: 1px solid @border; border-radius: 10px;
    padding: 8px; font-family: "Consolas", "Cascadia Mono", "Microsoft YaHei UI";
}

/* ---------------- 输入控件 ---------------- */
QLineEdit, QComboBox, QSpinBox {
    background: @card; border: 1px solid @btn_border; border-radius: 8px;
    padding: 5px 10px; color: @text; min-height: 20px;
}
QLineEdit:hover, QComboBox:hover, QSpinBox:hover { border-color: @accent; }
QLineEdit:focus, QComboBox:focus, QSpinBox:focus { border-color: @accent; }
QLineEdit:disabled, QComboBox:disabled, QSpinBox:disabled { color: @sub; background: @bg; }
QComboBox::drop-down { border: none; border-left: 1px solid @border; width: 26px; }
QComboBox QAbstractItemView {
    background: @card; border: 1px solid @btn_border; border-radius: 8px;
    selection-background-color: @sel_bg; selection-color: @sel_text;
    outline: 0; padding: 4px;
}

/* ---------------- 勾选 ---------------- */
QCheckBox { spacing: 8px; color: @text; padding: 2px; }
QCheckBox:hover { color: @accent; }
QCheckBox::indicator { width: 16px; height: 16px; border-radius: 4px; border: 1px solid @btn_border; background: @card; }
QCheckBox::indicator:hover { border-color: @accent; }
QCheckBox::indicator:checked { background: @accent; border-color: @accent; }
QCheckBox::indicator:disabled { border-color: @border; background: @bg; }

/* ---------------- 进度条 ---------------- */
QProgressBar {
    border: none; background: @grid; border-radius: 4px; max-height: 8px;
    text-align: center; color: transparent;
}
QProgressBar::chunk { background: @accent; border-radius: 4px; }

/* ---------------- 状态栏 ---------------- */
QStatusBar { background: transparent; color: @sub; min-height: 24px; }
QStatusBar::item { border: none; }

/* ---------------- 滚动条 ---------------- */
QScrollBar:vertical { background: transparent; width: 11px; margin: 2px; }
QScrollBar::handle:vertical { background: @scrollbar; border-radius: 5px; min-height: 32px; }
QScrollBar::handle:vertical:hover { background: @accent; }
QScrollBar::add-line:vertical, QScrollBar::sub-line:vertical { height: 0; }
QScrollBar::add-page:vertical, QScrollBar::sub-page:vertical { background: transparent; }
QScrollBar:horizontal { background: transparent; height: 11px; margin: 2px; }
QScrollBar::handle:horizontal { background: @scrollbar; border-radius: 5px; min-width: 32px; }
QScrollBar::handle:horizontal:hover { background: @accent; }
QScrollBar::add-line:horizontal, QScrollBar::sub-line:horizontal { width: 0; }
QScrollBar::add-page:horizontal, QScrollBar::sub-page:horizontal { background: transparent; }
QScrollBar::corner { background: transparent; }

/* ---------------- 侧边栏 ---------------- */
QListWidget#sidebar {
    background: @card; border: 1px solid @border; border-radius: 12px;
    padding: 8px; outline: 0; min-width: 176px; max-width: 200px;
}
QListWidget#sidebar::item {
    color: @text; padding: 10px 14px; border-radius: 8px; margin: 2px 3px;
}
QListWidget#sidebar::item:hover:!selected { background: @hover; color: @accent; }
QListWidget#sidebar::item:selected {
    background: @sel_bg; color: @sel_text; font-weight: bold;
}
QListWidget#sidebar::item:disabled {
    color: @sub; font-weight: 600; background: transparent;
    padding-top: 16px; padding-bottom: 6px; font-size: 8pt;
}

/* ---------------- 菜单 / 消息框 ---------------- */
QMenu {
    background: @card; border: 1px solid @border; border-radius: 10px; padding: 6px;
}
QMenu::item { padding: 7px 22px 7px 14px; border-radius: 6px; color: @text; }
QMenu::item:selected { background: @accent_soft; color: @accent; }
QMenu::item:disabled { color: @sub; }
QMenu::separator { height: 1px; background: @border; margin: 6px 8px; }
QMenuBar { background: transparent; }
QMenuBar::item { padding: 5px 10px; border-radius: 6px; }
QMenuBar::item:selected { background: @accent_soft; color: @accent; }

QMessageBox { background: @bg; }
QMessageBox QLabel { color: @text; }
QMessageBox QPushButton { min-width: 72px; }

/* ---------------- 语义化标签 ---------------- */
QDialog { background: @bg; }
QLabel#hint { color: @sub; }
QLabel[hint="true"] { color: @sub; }
QLabel#sectionTitle { font-size: 14pt; font-weight: bold; color: @text; }
QLabel#brandTitle { font-size: 13pt; font-weight: bold; color: @text; }
QLabel#brandSub { font-size: 8pt; color: @brand_sub; }
QLabel#statTitle { color: @brand_sub; font-size: 9pt; background: transparent; border: none; }
QLabel#statValue { background: transparent; border: none; font-weight: bold; }
QLabel#badge {
    background: @accent_soft; color: @accent; border: none; border-radius: 8px;
    padding: 3px 9px; font-size: 8pt; font-weight: bold;
}
QLabel#badgeWarn {
    background: @warn_soft; color: @warn; border: none; border-radius: 8px;
    padding: 3px 9px; font-size: 8pt; font-weight: bold;
}
"""


def _build_qss(p: dict) -> str:
    qss = QSS_TEMPLATE
    # 长键优先，避免 @accent 抢先替换掉 @accent_hover 等更长 token
    for key in sorted(p.keys(), key=len, reverse=True):
        qss = qss.replace("@" + key, p[key])
    return qss


def apply(app, dark: bool = False):
    """应用主题。切换后建议对所有 widget 调用 update() 以刷新自绘控件。"""
    global _current, _dark
    _dark = bool(dark)
    _current = dict(_DARK if _dark else _LIGHT)
    from PySide6.QtGui import QFont
    app.setFont(QFont("Microsoft YaHei UI", 9))
    app.setStyleSheet(_build_qss(_current))
