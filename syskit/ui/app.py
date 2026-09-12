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


def _prepare_data_dirs():
    """启动即建好数据目录；便携模式首次运行时从旧 %APPDATA% 位置迁移历史数据。

    反向场景（便携 → 非便携）不做自动回迁——避免覆盖 %APPDATA% 里可能更新的
    数据（§16 原状态语义）；但若检测到安装目录 data\\ 中有历史数据且当前
    数据目录为全新，必须记日志指明数据实际位置，防止用户误以为丢失。
    """
    import shutil

    from ..logger import LOGGER
    try:
        os.makedirs(config.APPDATA_DIR, exist_ok=True)
        os.makedirs(config.LOCALAPPDATA_DIR, exist_ok=True)
        if not getattr(config, "_PORTABLE", ""):
            if getattr(sys, "frozen", False):
                leftover_cfg = os.path.join(os.path.dirname(sys.executable),
                                            "data", "config")
                has_leftover = os.path.exists(os.path.join(leftover_cfg, "settings.json")) \
                    or os.path.exists(os.path.join(leftover_cfg, "vault.db"))
                fresh_current = not os.path.exists(
                    os.path.join(config.APPDATA_DIR, "settings.json"))
                if has_leftover and fresh_current:
                    LOGGER.warning(
                        "检测到安装目录 {} 中存在便携模式历史数据（settings/密码库/剪贴板），"
                        "当前以非便携模式运行（数据目录 {}）。数据未丢失；如需沿用，"
                        "请将上述目录内容复制到 {}，或重装时勾选「数据保存在安装目录」。",
                        leftover_cfg, config.APPDATA_DIR, config.APPDATA_DIR)
            return
        old, new = config._APP_DATA_ROOT, config.APPDATA_DIR
        # 跳过条件收紧为"关键文件已在"：上次迁移部分失败（如磁盘满中断）时
        # 新目录只有残缺文件，重跑迁移以 copy2 覆盖补齐（源是唯一真本，安全）
        key_present = any(os.path.exists(os.path.join(new, n))
                          for n in ("settings.json", "vault.db"))
        if not os.path.isdir(old) or key_present:
            return                         # 无旧数据 / 已有新数据（不覆盖）
        for name in ("settings.json", "vault.db", "vault.key",
                     "clipboard.json", "traffic.json", "events.json"):
            src = os.path.join(old, name)
            if os.path.exists(src):
                shutil.copy2(src, os.path.join(new, name))
                # 迁移后源文件改名退役：不留可用的明文副本（clipboard.json 常含
                # 敏感内容）；如需回退改回原名即可（保守可逆，§15）
                os.replace(src, src + ".migrated")
        src_files = os.path.join(old, "clipboard_files")
        if os.path.isdir(src_files):
            dst_files = os.path.join(new, "clipboard_files")
            os.makedirs(dst_files, exist_ok=True)
            for n in os.listdir(src_files):
                shutil.copy2(os.path.join(src_files, n), os.path.join(dst_files, n))
        LOGGER.info("便携模式：历史数据已从 {} 迁移至 {}（源文件已改名 .migrated 退役）",
                    old, new)
    except Exception:
        LOGGER.exception("数据目录准备/迁移失败（不影响启动）")


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

    if sys.platform == "win32" and not getattr(sys, "frozen", False):
        # 仅源码运行需要：声明独立任务栏身份，避免归入 python.exe 分组。
        # 打包 exe 已内嵌 app.ico，任务栏按 exe 本体显示图标与分组；
        # 再设 AUMID 反而会让 Windows 找不到该 AUMID 对应的快捷方式图标源，
        # 任务栏回退成默认白图标。
        try:
            ctypes.windll.shell32.SetCurrentProcessExplicitAppUserModelID(config.APP_NAME)
        except Exception:
            pass

    _enable_crash_log()
    _logging.setup(verbose_console=False)
    _prepare_data_dirs()

    def _qt_msg_handler(msg_type, context, message):
        # windowed 打包无 stderr，Qt 默认处理器会 fail-fast(0xc0000409)——接管到日志。
        # qFatal（QThread destroyed / QObject 析构序等）是原生崩溃前的最后痕迹，
        # 必须同步直写 crash.log 并 flush：loguru 走 syskit.log，缓冲/锁可能来不及。
        try:
            if msg_type == 3:               # QtFatalMsg
                fh = getattr(_qt_msg_handler, "_fatal_fh", None)
                if fh is None:
                    os.makedirs(config.LOCALAPPDATA_DIR, exist_ok=True)
                    fh = open(os.path.join(config.LOCALAPPDATA_DIR, "crash.log"),
                              "a", encoding="utf-8", buffering=1)
                    _qt_msg_handler._fatal_fh = fh
                import time as _t
                fh.write(f"\n[{_t.strftime('%F %T')}] QtFATAL: {message}\n")
                fh.flush()
                os.fsync(fh.fileno())
        except Exception:
            pass
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
    app.setWindowIcon(theme.app_icon())     # 标题栏/任务栏/Alt+Tab 统一蓝色 S 标识
    from .. import settings
    theme.apply(app, dark=bool(settings.load().get("dark_mode", False)))
    app.setApplicationName(config.APP_NAME)
    app.setQuitOnLastWindowClosed(False)

    minimized = "--minimized" in sys.argv
    win = MainWindow(minimized=minimized)
    if not minimized:
        win.show()
    sys.exit(app.exec())
