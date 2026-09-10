"""双主题（浅色/深色）QSS + 调色板。图表等自绘控件运行时取 pal() 适配。"""

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
}

_current = dict(_LIGHT)
_dark = False


def pal() -> dict:
    """当前调色板（自绘控件在 paintEvent 里读取以适配主题）。"""
    return _current


def is_dark() -> bool:
    return _dark


QSS_TEMPLATE = """
* { font-family: "Microsoft YaHei UI", "Segoe UI", sans-serif; font-size: 9pt; color: @text; }

QMainWindow, QDialog, QWidget#pageRoot { background: @bg; }

QTabWidget::pane {
    border: none; background: transparent; padding-top: 4px;
}
QTabBar { qproperty-drawBase: 0; }
QTabBar::tab {
    background: transparent; color: @sub;
    padding: 10px 20px; margin-right: 4px;
    border: none; border-bottom: 3px solid transparent;
    font-size: 10pt;
}
QTabBar::tab:selected {
    color: @accent; border-bottom: 3px solid @accent; font-weight: bold;
}
QTabBar::tab:hover:!selected { color: @accent; background: @hover; border-radius: 6px 6px 0 0; }

QGroupBox {
    background: @card; border: 1px solid @border; border-radius: 10px;
    margin-top: 14px; padding: 12px 10px 10px 10px;
    font-weight: bold; color: @text;
}
QGroupBox::title {
    subcontrol-origin: margin; left: 14px; top: 0px; padding: 0 8px;
    color: @accent; background: @card;
}

QWidget#card, QFrame#statCard {
    background: @card; border: 1px solid @border; border-radius: 12px;
}

QPushButton {
    background: @card; border: 1px solid @btn_border; border-radius: 7px;
    padding: 7px 16px; color: @text; min-height: 18px;
}
QPushButton:hover { border-color: @accent; color: @accent; background: @hover; }
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
QTableWidget::item, QTreeWidget::item { padding: 4px 2px; }

QPlainTextEdit, QTextEdit {
    background: @card; border: 1px solid @border; border-radius: 10px;
    padding: 8px; font-family: "Consolas", "Cascadia Mono", "Microsoft YaHei UI";
}

QLineEdit, QComboBox, QSpinBox {
    background: @card; border: 1px solid @btn_border; border-radius: 7px;
    padding: 5px 10px; color: @text; min-height: 20px;
}
QLineEdit:hover, QComboBox:hover, QSpinBox:hover { border-color: @accent; }
QLineEdit:focus, QComboBox:focus, QSpinBox:focus { border-color: @accent; }
QComboBox::drop-down { border: none; width: 26px; }
QComboBox QAbstractItemView {
    background: @card; border: 1px solid @btn_border; border-radius: 6px;
    selection-background-color: @sel_bg; selection-color: @sel_text;
    outline: 0;
}

QCheckBox { spacing: 8px; color: @text; padding: 2px; }
QCheckBox:hover { color: @accent; }
QCheckBox::indicator { width: 16px; height: 16px; border-radius: 4px; border: 1px solid @btn_border; background: @card; }
QCheckBox::indicator:checked { background: @accent; border-color: @accent; }

QProgressBar {
    border: none; background: @grid; border-radius: 4px; max-height: 8px;
    text-align: center; color: transparent;
}
QProgressBar::chunk { background: @accent; border-radius: 4px; }

QStatusBar { background: transparent; color: @sub; min-height: 22px; }
QStatusBar::item { border: none; }

QScrollBar:vertical { background: transparent; width: 11px; margin: 2px; }
QScrollBar::handle:vertical { background: @scrollbar; border-radius: 5px; min-height: 32px; }
QScrollBar::handle:vertical:hover { background: @accent; }
QScrollBar::add-line:vertical, QScrollBar::sub-line:vertical { height: 0; }
QScrollBar::add-page:vertical, QScrollBar::sub-page:vertical { background: transparent; }
QScrollBar:horizontal { background: transparent; height: 11px; margin: 2px; }
QScrollBar::handle:horizontal { background: @scrollbar; border-radius: 5px; min-width: 32px; }
QScrollBar::handle:horizontal:hover { background: @accent; }
QScrollBar::add-line:horizontal, QScrollBar::sub-line:horizontal { width: 0; }

QToolTip {
    background: @tooltip_bg; color: @tooltip_text; border: none; border-radius: 5px;
    padding: 7px 9px; font-size: 9pt;
}

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

QDialog { background: @bg; }
QLabel#hint { color: @sub; }
QLabel#sectionTitle { font-size: 14pt; font-weight: bold; color: @text; }
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
