"""双主题（浅色/深色）QSS + 调色板。图表等自绘控件运行时取 pal() 适配。"""

_LIGHT = {
    "bg": "#f5f7fa", "card": "#ffffff", "border": "#e3e8ef", "text": "#333c48",
    "sub": "#8a93a0", "grid": "#d5dce4", "hover": "#f0f4f9",
    "sel_bg": "#e3edff", "sel_text": "#1f3f77", "accent": "#3576e0",
    "accent_hover": "#2b62c4", "accent_disabled": "#a8c4f0",
    "btn_border": "#d0d7de", "btn_pressed": "#eef4ff", "header_bg": "#f0f3f7",
    "header_text": "#4a5568", "alt_row": "#f8fafc", "danger": "#c62828",
    "danger_hover_bg": "#fdf1f1", "danger_border": "#efc7c7", "scrollbar": "#c9d2dd",
    "tooltip_bg": "#2f3a48", "tooltip_text": "#ffffff",
}
_DARK = {
    "bg": "#151a21", "card": "#1f2630", "border": "#2f3945", "text": "#dfe5ec",
    "sub": "#7d8896", "grid": "#333d4a", "hover": "#26303d",
    "sel_bg": "#2b3b52", "sel_text": "#cfe0ff", "accent": "#4a8af0",
    "accent_hover": "#5d97f2", "accent_disabled": "#3a4a63",
    "btn_border": "#3a4552", "btn_pressed": "#2b3b52", "header_bg": "#232c37",
    "header_text": "#9aa7b5", "alt_row": "#1c232d", "danger": "#ff7b7b",
    "danger_hover_bg": "#332225", "danger_border": "#5a3030", "scrollbar": "#3d4855",
    "tooltip_bg": "#e8edf3", "tooltip_text": "#1f2630",
}

_current = dict(_LIGHT)
_dark = False


def pal() -> dict:
    """当前调色板（自绘控件在 paintEvent 里读取以适配主题）。"""
    return _current


def is_dark() -> bool:
    return _dark


QSS_TEMPLATE = """
* { font-family: "Microsoft YaHei UI"; font-size: 9pt; color: @text; }

QMainWindow, QDialog { background: @bg; }

QTabWidget::pane { border: none; }
QTabBar { qproperty-drawBase: 0; }
QTabBar::tab {
    background: transparent; color: @sub;
    padding: 9px 22px; margin-right: 2px;
    border: none; border-bottom: 3px solid transparent;
    font-size: 10pt;
}
QTabBar::tab:selected { color: @accent; border-bottom: 3px solid @accent; font-weight: bold; }
QTabBar::tab:hover:!selected { color: @accent; }

QGroupBox {
    background: @card; border: 1px solid @border; border-radius: 8px;
    margin-top: 12px; padding: 10px 8px 8px 8px;
    font-weight: bold; color: @text;
}
QGroupBox::title {
    subcontrol-origin: margin; left: 14px; top: 1px; padding: 0 6px;
    color: @accent;
}

QWidget#card {
    background: @card; border: 1px solid @border; border-radius: 10px;
}

QPushButton {
    background: @card; border: 1px solid @btn_border; border-radius: 6px;
    padding: 6px 16px; color: @text;
}
QPushButton:hover { border-color: @accent; color: @accent; }
QPushButton:pressed { background: @btn_pressed; }
QPushButton:disabled { color: @sub; border-color: @border; background: transparent; }

QPushButton[primary="true"] {
    background: @accent; color: #ffffff; border: none; font-weight: bold;
}
QPushButton[primary="true"]:hover { background: @accent_hover; }
QPushButton[primary="true"]:pressed { background: @accent_hover; }
QPushButton[primary="true"]:disabled { background: @accent_disabled; color: @card; }

QPushButton[danger="true"] { color: @danger; border-color: @danger_border; }
QPushButton[danger="true"]:hover { background: @danger_hover_bg; border-color: @danger; }

QTableWidget, QTreeWidget, QListWidget {
    background: @card; border: 1px solid @border; border-radius: 8px;
    padding: 4px; alternate-background-color: @alt_row;
    selection-background-color: @sel_bg; selection-color: @sel_text;
}
QHeaderView::section {
    background: @header_bg; color: @header_text; border: none;
    border-bottom: 1px solid @border; padding: 7px 8px; font-weight: bold;
}
QTableWidget::item, QTreeWidget::item { padding: 3px; }

QPlainTextEdit, QTextEdit {
    background: @card; border: 1px solid @border; border-radius: 8px;
    padding: 6px; font-family: "Consolas", "Microsoft YaHei UI";
}

QComboBox, QSpinBox {
    background: @card; border: 1px solid @btn_border; border-radius: 6px;
    padding: 4px 10px; color: @text;
}
QComboBox:hover, QSpinBox:hover { border-color: @accent; }
QComboBox::drop-down { border: none; width: 24px; }
QComboBox QAbstractItemView {
    background: @card; border: 1px solid @btn_border;
    selection-background-color: @sel_bg; selection-color: @sel_text;
}

QCheckBox { spacing: 8px; color: @text; }
QCheckBox:hover { color: @accent; }

QProgressBar { border: none; background: @grid; border-radius: 3px; max-height: 6px; }
QProgressBar::chunk { background: @accent; border-radius: 3px; }

QStatusBar { background: transparent; color: @sub; }
QStatusBar::item { border: none; }

QScrollBar:vertical { background: transparent; width: 10px; margin: 2px; }
QScrollBar::handle:vertical { background: @scrollbar; border-radius: 5px; min-height: 30px; }
QScrollBar::handle:vertical:hover { background: @accent; }
QScrollBar::add-line:vertical, QScrollBar::sub-line:vertical { height: 0; }
QScrollBar:horizontal { background: transparent; height: 10px; margin: 2px; }
QScrollBar::handle:horizontal { background: @scrollbar; border-radius: 5px; min-width: 30px; }
QScrollBar::add-line:horizontal, QScrollBar::sub-line:horizontal { width: 0; }

QToolTip {
    background: @tooltip_bg; color: @tooltip_text; border: none; border-radius: 4px;
    padding: 6px 8px;
}

QListWidget#sidebar {
    background: @card; border: 1px solid @border; border-radius: 10px;
    padding: 6px; outline: 0; min-width: 168px; max-width: 190px;
}
QListWidget#sidebar::item {
    color: @text; padding: 9px 12px; border-radius: 6px; margin: 1px 2px;
}
QListWidget#sidebar::item:hover:!selected { background: @hover; color: @accent; }
QListWidget#sidebar::item:selected { background: @sel_bg; color: @sel_text; font-weight: bold; }
QListWidget#sidebar::item:disabled {
    color: @sub; font-weight: bold; background: transparent;
    padding-top: 14px; font-size: 8pt;
}

QFrame#statCard {
    background: @card; border: 1px solid @border; border-radius: 10px;
}
"""


def _build_qss(p: dict) -> str:
    qss = QSS_TEMPLATE
    for key, val in p.items():
        qss = qss.replace("@" + key, val)
    return qss


def apply(app, dark: bool = False):
    """应用主题。切换后建议对所有 widget 调用 update() 以刷新自绘控件。"""
    global _current, _dark
    _dark = bool(dark)
    _current = dict(_DARK if _dark else _LIGHT)
    from PySide6.QtGui import QFont
    app.setFont(QFont("Microsoft YaHei UI", 9))
    app.setStyleSheet(_build_qss(_current))
