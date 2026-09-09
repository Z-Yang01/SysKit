"""UI 验收辅助：依次切换全部页面并各存一张截图。"""
import os
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from syskit import logger

logger.setup(verbose_console=False)

from PySide6.QtCore import QTimer
from PySide6.QtWidgets import QApplication

from syskit.ui.main_window import MainWindow

SHOT_DIR = os.path.join(tempfile.gettempdir(), "syskit_shots")
os.makedirs(SHOT_DIR, exist_ok=True)
PAGE_COUNT = 9
app = QApplication(sys.argv)
win = MainWindow()
win.show()
state = {"i": 0}


def shoot():
    i = state["i"]
    win.grab().save(os.path.join(SHOT_DIR, f"ui_tab{i}.png"))
    print("saved", i)
    state["i"] += 1
    QTimer.singleShot(400, cycle)


def cycle():
    i = state["i"]
    if i >= PAGE_COUNT:
        app.quit()
        return
    win.switch_page(i)
    QTimer.singleShot(1200, shoot)


QTimer.singleShot(25000, cycle)
app.exec()
