"""GUI 应用入口：单实例互斥、Fusion 风格、托盘常驻、崩溃捕获。"""
import ctypes
import faulthandler
import os
import sys
import traceback

from .. import config


def _enable_crash_log():
    """windowed 打包后 stdout/stderr 为 None，异常无处可去——落盘到 crash.log。"""
    try:
        os.makedirs(config.LOCALAPPDATA_DIR, exist_ok=True)
        path = os.path.join(config.LOCALAPPDATA_DIR, "crash.log")
        fh = open(path, "a", encoding="utf-8", buffering=1)
        fh.write(f"\n===== launch {__import__('time').strftime('%F %T')} pid={os.getpid()} =====\n")
        faulthandler.enable(fh)

        def _hook(tp, val, tb):
            traceback.print_exception(tp, val, tb, file=fh)

        sys.excepthook = _hook
        import threading
        threading.excepthook = lambda args: _hook(*args)
        return fh
    except Exception:
        return None


def run_gui():
    mutex = ctypes.windll.kernel32.CreateMutexW(None, False, "SysKit_SingleInstance_Mutex")
    if ctypes.windll.kernel32.GetLastError() == 183:  # ERROR_ALREADY_EXISTS
        from PySide6.QtWidgets import QMessageBox, QApplication
        app = QApplication(sys.argv)
        QMessageBox.information(None, config.DISPLAY_NAME, f"{config.DISPLAY_NAME} 已在运行（请查看系统托盘）。")
        return

    from PySide6.QtCore import Qt, qInstallMessageHandler
    from PySide6.QtWidgets import QApplication
    from .main_window import MainWindow
    from . import theme
    from .. import logger as _logging

    _enable_crash_log()
    _logging.setup(verbose_console=False)

    def _qt_msg_handler(msg_type, context, message):
        # windowed 打包无 stderr，Qt 默认处理器会 fail-fast(0xc0000409)——接管到日志
        try:
            from ..logger import LOGGER
            LOGGER.debug("[qt] {}", message)
        except Exception:
            pass

    qInstallMessageHandler(_qt_msg_handler)

    QApplication.setHighDpiScaleFactorRoundingPolicy(
        Qt.HighDpiScaleFactorRoundingPolicy.PassThrough)
    app = QApplication(sys.argv)
    app.setStyle("Fusion")
    from .. import settings
    theme.apply(app, dark=bool(settings.load().get("dark_mode", False)))
    app.setApplicationName(config.APP_NAME)
    app.setQuitOnLastWindowClosed(False)

    minimized = "--minimized" in sys.argv
    win = MainWindow(minimized=minimized)
    if not minimized:
        win.show()
    sys.exit(app.exec())
