"""系统助手：性能监控 / 启动项管理 / 电池健康 / 安全清理 / 剪贴板历史 / 场景模式。"""
import glob
import os
import queue
import threading
import time

from PySide6.QtCore import Qt, QBuffer, QThread, QTimer, QUrl, Signal
from PySide6.QtGui import QColor, QDesktopServices, QPixmap
from PySide6.QtWidgets import (QAbstractItemView, QApplication, QCheckBox, QComboBox, QDialog,
                               QFileDialog, QFormLayout, QGroupBox, QHBoxLayout, QHeaderView,
                               QLabel, QLineEdit, QMenu, QMessageBox, QProgressBar, QPushButton,
                               QTableWidgetItem, QTabWidget, QTextEdit, QVBoxLayout, QWidget)

from .charts import LineChart, human_bytes
from . import theme
from .clip_thumbs import icon_for as _clip_icon_for
from .disk_page import DiskTab
from .theme import make_table
from .workers import reap
from .. import admin, clipboard as clipboard_mod, cliprules, modes, settings, winregread
from ..logger import LOGGER

RUN_KEY = r"Software\Microsoft\Windows\CurrentVersion\Run"
DISABLED_KEY = r"Software\NetTool\DisabledRun"
USER_STARTUP = os.path.join(os.environ.get("APPDATA", ""),
                            r"Microsoft\Windows\Start Menu\Programs\Startup")
_STARTUP_DISABLED_SUFFIX = ".disabled"   # 启动文件夹项禁用 = 改名加此后缀


# ---------------- 性能 ----------------

class PerfTab(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        lay = QHBoxLayout(self)
        lay.setContentsMargins(8, 8, 8, 8)
        left = QVBoxLayout()
        self.chart_sys = LineChart("CPU / 内存（%）", series=[("CPU", "@accent"), ("内存", "#8e5ce6")],
                                   y_fixed=(0, 100), fmt=lambda v: f"{v:.0f}%")
        self.chart_disk = LineChart("磁盘读写（1 秒/点）", series=[("读", "@ok"), ("写", "@warn")],
                                    fmt=human_bytes)
        left.addWidget(self.chart_sys, 1)
        left.addWidget(self.chart_disk, 1)
        lay.addLayout(left, 1)

        right = QVBoxLayout()
        self.tbl = make_table(["PID", "进程", "CPU %", "内存"])
        self.tbl.horizontalHeader().setDefaultSectionSize(90)
        self.tbl.horizontalHeader().setStretchLastSection(True)
        self.tbl.setSortingEnabled(True)
        right.addWidget(QLabel("资源占用最高的进程（2 秒刷新）"))
        right.addWidget(self.tbl, 1)
        lay.addLayout(right, 1)

    def reset_if_stale(self, max_gap_sec: float = 600.0):
        """页面隐藏期间采样被可见性守卫跳过：切回时若断流超过图表窗口，
        清空旧序列（避免长时间前的点与现在画成连续曲线）。"""
        last = getattr(self, "_last_tick", None)
        if last is not None and time.time() - last > max_gap_sec:
            self.chart_sys.clear_data()
            self.chart_disk.clear_data()

    def update_tick(self, d):
        self._last_tick = time.time()
        if "cpu" in d:
            self.chart_sys.push([d["cpu"], d["mem"]])
        if "disk_r" in d:
            self.chart_disk.push([d["disk_r"], d["disk_w"]])
        procs = d.get("top_procs")
        if procs is None:
            return
        header = self.tbl.horizontalHeader()
        sort_col = header.sortIndicatorSection()
        sort_order = header.sortIndicatorOrder()
        # 选中按 PID 跟随：整表重建+重排序后行号会指向另一个进程
        selected_pid = None
        if self.tbl.currentItem() is not None:
            selected_pid = self.tbl.item(self.tbl.currentItem().row(), 0).text()
        self.tbl.setSortingEnabled(False)
        self.tbl.setRowCount(len(procs))
        for i, pr in enumerate(procs):
            vals = [str(pr["pid"]), str(pr["name"]), f"{pr['cpu']:.1f}",
                    human_bytes(pr["mem"])]
            for c, v in enumerate(vals):
                self.tbl.setItem(i, c, QTableWidgetItem(v))
        self.tbl.setSortingEnabled(True)
        self.tbl.sortItems(sort_col, sort_order)
        if selected_pid is not None:
            for i in range(self.tbl.rowCount()):
                it = self.tbl.item(i, 0)
                if it is not None and it.text() == selected_pid:
                    self.tbl.setCurrentCell(i, self.tbl.currentColumn() or 0)
                    break


# ---------------- 启动项 ----------------

class StartupTab(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        lay = QVBoxLayout(self)
        lay.setContentsMargins(8, 8, 8, 8)
        row = QHBoxLayout()
        btn_refresh = QPushButton("刷新")
        btn_refresh.clicked.connect(self.reload)
        self.btn_disable = QPushButton("禁用选中项")
        self.btn_disable.clicked.connect(lambda: self._toggle_selected(True))
        self.btn_enable = QPushButton("启用选中项")
        self.btn_enable.clicked.connect(lambda: self._toggle_selected(False))
        row.addWidget(self.btn_disable)
        row.addWidget(self.btn_enable)
        row.addStretch(1)
        row.addWidget(btn_refresh)
        lay.addLayout(row)

        self.tbl = make_table(["名称", "位置", "命令", "状态"], select_rows=True)
        self.tbl.horizontalHeader().setSectionResizeMode(2, QHeaderView.Stretch)
        lay.addWidget(self.tbl)
        self.reload()

    @staticmethod
    def _enum():
        """[(location, name, command, enabled)] location: HKCU-Run / HKLM-Run / 启动文件夹"""
        import winreg

        items = []
        for tag, hive in (("HKCU-Run", winregread._HIVES["HKCU"]),
                          ("HKLM-Run", winregread._HIVES["HKLM"])):
            try:
                k = winreg.OpenKey(hive, RUN_KEY, 0, winreg.KEY_READ)
            except OSError:
                continue
            i = 0
            while True:
                try:
                    name, value, _ = winreg.EnumValue(k, i)
                except OSError:
                    break
                items.append((tag, name, str(value), True))
                i += 1
            k.Close()
        for f in glob.glob(os.path.join(USER_STARTUP, "*")):
            base = os.path.basename(f)
            if base.endswith(_STARTUP_DISABLED_SUFFIX):
                # 已禁用的启动文件夹项：以原名 + 已禁用态列出。否则会显示成
                # 「已启用」且永远无法从 UI 恢复（启用按钮只在禁用态生效），
                # 用户再点一次禁用还会连锁改名 .disabled.disabled
                items.append(("启动文件夹", base[: -len(_STARTUP_DISABLED_SUFFIX)],
                              f, False))
            else:
                items.append(("启动文件夹", base, f, True))
        # 已禁用项
        for name, raw in winregread.values(rf"HKCU\{DISABLED_KEY}").items():
            try:
                loc, cmd = str(raw).split("|", 1)
                items.append((loc, name, cmd, False))
            except ValueError:
                continue
        return items

    def reload(self):
        self.tbl.setRowCount(0)
        try:
            items = self._enum()
        except Exception as e:
            LOGGER.error("启动项枚举失败: {}", e)
            return
        self.tbl.setRowCount(len(items))
        for i, (loc, name, cmd, enabled) in enumerate(items):
            vals = [name, loc, cmd, "已启用" if enabled else "已禁用"]
            for c, v in enumerate(vals):
                it = QTableWidgetItem(v)
                if not enabled:
                    it.setForeground(QColor("#9aa4b2"))
                self.tbl.setItem(i, c, it)
        self.tbl.resizeColumnsToContents()

    def _selected(self):
        rows = sorted({i.row() for i in self.tbl.selectedIndexes()})
        out = []
        for r in rows:
            name = self.tbl.item(r, 0).text()
            loc = self.tbl.item(r, 1).text()
            enabled = self.tbl.item(r, 3).text() == "已启用"
            out.append((loc, name, enabled))
        return out

    def _toggle_selected(self, disable: bool):
        if not admin.is_admin():
            QMessageBox.warning(self, "SysKit", "禁用/启用启动项需要管理员权限（涉及 HKLM）。")
            return
        targets = self._selected()
        if not targets:
            QMessageBox.information(self, "SysKit", "请先在表格中选中要操作的启动项。")
            return
        failed = []
        for loc, name, enabled in targets:
            try:
                if disable and enabled:
                    self._disable_one(loc, name)
                elif not disable and not enabled:
                    self._enable_one(loc, name)
            except Exception as e:
                LOGGER.error("启动项操作失败 {} {}: {}", name, loc, e)
                failed.append(name)
        if failed:
            # HKLM 项非管理员写注册表必 PermissionError：只进日志用户会以为"没按到"
            QMessageBox.warning(self, "SysKit",
                                "以下启动项操作失败（需要管理员权限）：\n" + "\n".join(failed))
        self.reload()

    def _disable_one(self, loc, name):
        import winreg
        if loc == "启动文件夹":
            path = os.path.join(USER_STARTUP, name)
            if os.path.exists(path):
                os.rename(path, path + _STARTUP_DISABLED_SUFFIX)
            return
        hive = winregread._HIVES["HKCU"] if loc == "HKCU-Run" else winregread._HIVES["HKLM"]
        acc = winreg.KEY_READ | winreg.KEY_WRITE
        with winreg.OpenKey(hive, RUN_KEY, 0, acc) as src:
            value, vtype = winreg.QueryValueEx(src, name)
        with winreg.CreateKeyEx(winreg.HKEY_CURRENT_USER, DISABLED_KEY, 0, winreg.KEY_WRITE) as dst:
            winreg.SetValueEx(dst, name, 0, winreg.REG_SZ, f"{loc}|{value}")
        with winreg.OpenKey(hive, RUN_KEY, 0, winreg.KEY_SET_VALUE) as src:
            winreg.DeleteValue(src, name)

    def _enable_one(self, loc, name):
        import winreg
        if loc == "启动文件夹":
            path = os.path.join(USER_STARTUP, name + _STARTUP_DISABLED_SUFFIX)
            if os.path.exists(path):
                os.rename(path, path[: -len(_STARTUP_DISABLED_SUFFIX)])
            return
        stored = winregread.value(rf"HKCU\{DISABLED_KEY}", name)
        if not stored:
            return
        hive = winregread._HIVES["HKCU"] if loc == "HKCU-Run" else winregread._HIVES["HKLM"]
        _loc, value = str(stored).split("|", 1)
        with winreg.OpenKey(hive, RUN_KEY, 0, winreg.KEY_SET_VALUE) as src:
            winreg.SetValueEx(src, name, 0, winreg.REG_SZ, value)
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, DISABLED_KEY, 0, winreg.KEY_SET_VALUE) as dst:
            winreg.DeleteValue(dst, name)


# ---------------- 电池 ----------------

class _BatteryWorker(QThread):
    """电池报告查询（PowerShell CIM，最坏 30s）——后台执行避免冻结 UI。"""

    got = Signal(str)               # 原始 stdout；无论成败都发

    def run(self):
        from .. import proc
        script = (
            "$d=(Get-CimInstance -Namespace root/wmi -ClassName BatteryStaticData "
            "-ErrorAction SilentlyContinue | Select-Object -First 1).DesignedCapacity; "
            "$f=(Get-CimInstance -Namespace root/wmi -ClassName BatteryFullChargedCapacity "
            "-ErrorAction SilentlyContinue | Select-Object -First 1).FullChargedCapacity; "
            "$r=(Get-CimInstance Win32_Battery -ErrorAction SilentlyContinue "
            "| Select-Object -First 1).EstimatedChargeRemaining; "
            "@($d,$f,$r) -join ','")
        try:
            r = proc.run_ps(script, timeout=30, resident=True)
            self.got.emit(r.stdout or "")
        except Exception:
            self.got.emit("")


class BatteryTab(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        lay = QVBoxLayout(self)
        lay.setContentsMargins(8, 8, 8, 8)
        btn = QPushButton("刷新电池报告")
        btn.clicked.connect(self.refresh)
        lay.addWidget(btn, 0, Qt.AlignLeft)

        self.form = QFormLayout()
        self.lbl_design = QLabel("—")
        self.lbl_full = QLabel("—")
        self.lbl_health = QLabel("—")
        self.lbl_remain = QLabel("—")
        self.form.addRow("设计容量", self.lbl_design)
        self.form.addRow("满充容量", self.lbl_full)
        self.form.addRow("健康度（满充/设计）", self.lbl_health)
        self.form.addRow("当前电量", self.lbl_remain)
        lay.addLayout(self.form)
        self.lbl_note = QLabel(" ")
        self.lbl_note.setProperty("hint", True)
        lay.addWidget(self.lbl_note)
        lay.addStretch(1)
        self._initial_refreshed = False   # 懒加载：首次显示本 Tab 才查电池（构造期不跑 PowerShell）
        self._bw = None                   # worker 空闲哨兵（防重入）

    def showEvent(self, event):
        super().showEvent(event)
        if not self._initial_refreshed:
            self._initial_refreshed = True
            self.refresh()

    def refresh(self):
        if self._bw is not None:
            return
        self.lbl_note.setText("查询中…（PowerShell CIM，最长 30 秒）")
        self._bw = reap(_BatteryWorker(), self)
        self._bw.got.connect(self._on_got)
        self._bw.start()

    def _on_got(self, stdout: str):
        self._bw = None
        parts = (stdout or "").strip().split(",")
        try:
            design = int(parts[0]) if len(parts) > 0 and parts[0].strip().isdigit() else None
            full = int(parts[1]) if len(parts) > 1 and parts[1].strip().isdigit() else None
            remain = int(parts[2]) if len(parts) > 2 and parts[2].strip().isdigit() else None
        except (ValueError, IndexError):
            design = full = remain = None
        if not design or not full:
            self.lbl_design.setText("未检测到电池")
            self.lbl_full.setText("—")
            self.lbl_health.setText("—")
            self.lbl_remain.setText("—")
            self.lbl_note.setText("本机未发现电池（台式机或电池信息读取失败）。")
            return
        self.lbl_design.setText(f"{design} mWh")
        self.lbl_full.setText(f"{full} mWh")
        self._last_health = full / design * 100     # 主题切换重填色（补57）
        self._apply_health_color()
        self.lbl_remain.setText(f"{remain}%" if remain is not None else "—")
        self.lbl_note.setText("数据来自 Windows 电池报告（设计容量由厂商提供，仅供参考）。")

    def _apply_health_color(self):
        """电池健康三档色（现取 pal——深浅主题各自可读，补57）。"""
        health = getattr(self, "_last_health", None)
        if health is None:
            return
        pal = theme.pal()
        color = pal["ok"] if health > 80 else pal["warn"] if health > 60 else pal["err"]
        self.lbl_health.setText(f"{health:.1f}%")
        self.lbl_health.setStyleSheet(f"color:{color}; font-weight:bold;")


# ---------------- 清理 ----------------

CLEAN_TARGETS = [
    ("user_temp", "用户临时文件", lambda: os.environ.get("TEMP", "")),
    ("win_temp", "Windows 临时文件", lambda: r"C:\Windows\Temp"),
    ("thumbcache", "缩略图缓存", lambda: os.path.join(
        os.environ.get("LOCALAPPDATA", ""), "Microsoft", "Windows", "Explorer")),
    ("recycle", "回收站", None),
]


class _ScanWorker(QThread):
    done = Signal(dict)

    def run(self):
        sizes = {}
        try:
            for key, _title, getter in CLEAN_TARGETS:
                sizes[key] = self._size(key, getter)
        except Exception:
            from ..logger import LOGGER
            LOGGER.exception("清理扫描线程异常")   # 无论如何发 done：否则按钮永久禁用
        self.done.emit(sizes)

    def _size(self, key, getter):
        total = 0
        if key == "recycle":
            from .. import proc
            r = proc.run_ps(
                "(New-Object -ComObject Shell.Application).NameSpace(0xA).Items() "
                "| ForEach-Object {$_.Size} | Measure-Object -Sum | "
                "Select-Object -ExpandProperty Sum", timeout=30)
            try:
                return int(float(r.stdout.strip() or 0))
            except ValueError:
                return 0
        root = getter()
        if not root or not os.path.isdir(root):
            return 0
        for path in glob.glob(os.path.join(root, "**", "*"), recursive=True):
            try:
                if os.path.isfile(path):
                    total += os.path.getsize(path)
            except OSError:
                continue
        return total


class _CleanWorker(QThread):
    done = Signal(int, list)      # freed, errors
    progress = Signal(str)

    def __init__(self, keys):
        super().__init__()
        self.keys = keys

    def run(self):
        freed = 0
        errors = []
        try:
            for key in self.keys:
                if key == "recycle":
                    self.progress.emit("清空回收站…")
                    from .. import proc
                    proc.run_ps("Clear-RecycleBin -Force -ErrorAction SilentlyContinue", timeout=60)
                    continue
                root = next((g() for k, _t, g in CLEAN_TARGETS if k == key), "")
                self.progress.emit(f"清理 {root} …")
                if not root or not os.path.isdir(root):
                    continue
                for path in glob.glob(os.path.join(root, "**", "*"), recursive=True):
                    try:
                        if key == "thumbcache" and not os.path.basename(path).lower().startswith("thumbcache"):
                            continue
                        if os.path.isfile(path):
                            size = os.path.getsize(path)
                            os.remove(path)
                            freed += size
                    except OSError:
                        errors.append(os.path.basename(path))
        except Exception:
            from ..logger import LOGGER
            LOGGER.exception("清理执行线程异常")   # 无论如何发 done：否则按钮永久禁用
        self.done.emit(freed, errors[:10])


class CleanupTab(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        lay = QVBoxLayout(self)
        lay.setContentsMargins(8, 8, 8, 8)
        self.lbl_tip = QLabel("白名单式清理，只删除下列安全位置的文件；正在使用的文件会自动跳过。")
        self.lbl_tip.setProperty("hint", True)
        lay.addWidget(self.lbl_tip)
        self.checks = {}
        for key, title, _g in CLEAN_TARGETS:
            cb = QCheckBox(title)
            self.checks[key] = cb
            lay.addWidget(cb)
        row = QHBoxLayout()
        self.btn_scan = QPushButton("扫描大小")
        self.btn_scan.clicked.connect(self.scan)
        self.btn_clean = QPushButton("清理选中项")
        self.btn_clean.setProperty("danger", True)
        self.btn_clean.clicked.connect(self.clean)
        row.addWidget(self.btn_scan)
        row.addWidget(self.btn_clean)
        row.addStretch(1)
        lay.addLayout(row)
        self.pb = QProgressBar()
        self.pb.setRange(0, 0)
        self.pb.setTextVisible(False)
        self.pb.hide()
        lay.addWidget(self.pb)
        self.lbl_result = QLabel(" ")
        self.lbl_result.setStyleSheet(f"color:{theme.pal()['ok']};")
        lay.addWidget(self.lbl_result)
        lay.addStretch(1)
        self._scan_worker = None
        self._clean_worker = None

    def scan(self):
        if self._scan_worker:
            return
        self.btn_scan.setEnabled(False)
        self.lbl_result.setText("扫描中…")
        self._scan_worker = reap(_ScanWorker(), self)
        self._scan_worker.done.connect(self._scan_done)
        self._scan_worker.start()

    def _scan_done(self, sizes):
        self._scan_worker = None
        self.btn_scan.setEnabled(True)
        for key, cb in self.checks.items():
            size = sizes.get(key, 0)
            title = next(t for k, t, _g in CLEAN_TARGETS if k == key)
            cb.setText(f"{title}　（{human_bytes(size)}）")

    def clean(self):
        if self._clean_worker:
            return
        keys = [k for k, cb in self.checks.items() if cb.isChecked()]
        if not keys:
            QMessageBox.information(self, "SysKit", "请先勾选要清理的项目（先扫描可看到大小）。")
            return
        self.btn_clean.setEnabled(False)
        self.pb.show()
        self._clean_worker = reap(_CleanWorker(keys), self)
        self._clean_worker.done.connect(self._clean_done)
        self._clean_worker.progress.connect(lambda s: self.lbl_result.setText(s))
        self._clean_worker.start()

    def _clean_done(self, freed, errors):
        self._clean_worker = None
        self.btn_clean.setEnabled(True)
        self.pb.hide()
        msg = f"清理完成，释放 {human_bytes(freed)}"
        if errors:
            msg += f"；跳过 {len(errors)} 个占用中的文件"
        self.lbl_result.setText(msg)
        LOGGER.info("垃圾清理: {}，跳过 {}", human_bytes(freed), len(errors))
        self.scan()


KIND_LABELS = {"text": "📄 文本", "image": "🖼 图片", "file": "📁 文件", "html": "🌐 网页"}
KIND_FILTERS = [("全部", None), ("文本", "text"), ("图片", "image"), ("文件", "file"), ("网页", "html")]


class _ClipActionWorker(QThread):
    """规则动作后台执行：fn(arg) -> 结果描述。无论成败都发 done。"""

    done = Signal(str)

    def __init__(self, fn, arg, parent=None):
        super().__init__(parent)
        self._fn, self._arg = fn, arg

    def run(self):
        try:
            msg = self._fn(self._arg) or "完成"
        except Exception as e:
            msg = f"失败: {e}"
        self.done.emit(msg)


# 大图异步编码阈值（像素数，≈2560×1600）：超过则 PNG 编码/落盘移交后台线程。
# 4K 截图（3840×2160≈8.3MP、33MB 位图）同步 img.save("PNG") 是百毫秒级，
# 而 dataChanged 在 GUI 线程触发——截图瞬间整个界面冻住；小图（2K 及以下
# ≈几十 ms）保持同步路径，行为与测试形态不变。
_ASYNC_ENCODE_PIXELS = 4_000_000


class ClipboardTab(QWidget):
    """剪贴板历史：监听系统剪贴板（文本/图片/文件/HTML），支持搜索/置顶/分类/规则动作。"""

    capture_added = Signal()    # 大图后台捕获入库完成（worker 线程 → GUI 线程刷新）

    def __init__(self, parent=None):
        super().__init__(parent)
        s0 = settings.load()
        max_items = int(s0.get("clipboard_max", 50))
        watch = bool(s0.get("clipboard_watch", False))
        self.hist = clipboard_mod.ClipboardHistory(max_items=max_items)
        lay = QVBoxLayout(self)
        lay.setContentsMargins(8, 8, 8, 8)
        row = QHBoxLayout()
        self.chk_watch = QCheckBox("监听剪贴板")
        self.chk_watch.setChecked(watch)
        self.chk_watch.setToolTip(
            "自动记录复制的文本/图片/文件路径；开启需确认，关闭后停止记录（历史保留）")
        # 仅用户点击触发（隐私暂停的程序化 setChecked 不经过这里）
        self.chk_watch.clicked.connect(self._on_watch_clicked)
        self.cmb_kind = QComboBox()
        self.cmb_kind.addItems([name for name, _ in KIND_FILTERS])
        self.cmb_kind.currentIndexChanged.connect(lambda *_: self.reload())
        self.edt_search = QLineEdit()
        self.edt_search.setPlaceholderText("搜索历史…")
        self.edt_search.setClearButtonEnabled(True)
        self.edt_search.textChanged.connect(lambda *_: self.reload())
        self.btn_pause = QPushButton("隐私暂停")
        self.btn_pause.setToolTip("暂停记录；恢复后也不会补录暂停期间的内容")
        self.btn_pause.setEnabled(watch)            # 监听关着时无「暂停」可言
        self.btn_pause.clicked.connect(self._toggle_pause)
        self.btn_copy = QPushButton("复制选中")
        self.btn_copy.clicked.connect(self._copy_selected)
        self.btn_pin = QPushButton("置顶")
        self.btn_pin.setToolTip("置顶条目不会被自动淘汰，且文本重复时不再入库")
        self.btn_pin.clicked.connect(self._toggle_pin)
        self.btn_del = QPushButton("删除选中")
        self.btn_del.clicked.connect(self._delete_selected)
        self.btn_clear = QPushButton("清空历史")
        self.btn_clear.setProperty("danger", True)
        self.btn_clear.clicked.connect(self._clear_all)
        row.addWidget(self.chk_watch)
        row.addWidget(self.cmb_kind)
        row.addWidget(self.edt_search, 1)
        row.addWidget(self.btn_pause)
        row.addWidget(self.btn_copy)
        row.addWidget(self.btn_pin)
        row.addWidget(self.btn_del)
        row.addWidget(self.btn_clear)
        lay.addLayout(row)

        self.tbl = make_table(["时间", "类型", "内容"], select_rows=True)
        self.tbl.horizontalHeader().setSectionResizeMode(2, QHeaderView.Stretch)
        self.tbl.setSelectionMode(QAbstractItemView.SingleSelection)
        self.tbl.itemDoubleClicked.connect(lambda *_: self._copy_selected())
        self.tbl.itemSelectionChanged.connect(self._show_detail)
        self.tbl.setContextMenuPolicy(Qt.CustomContextMenu)
        self.tbl.customContextMenuRequested.connect(self._show_context_menu)
        lay.addWidget(self.tbl)

        # 详情区：左侧图片缩略图（仅图片条目），右侧提示与规则动作按钮
        bottom = QHBoxLayout()
        self.lbl_thumb = QLabel()
        self.lbl_thumb.setFixedHeight(96)
        self.lbl_thumb.hide()
        bottom.addWidget(self.lbl_thumb)
        col = QVBoxLayout()
        self.lbl_detail = QLabel("选中一条记录查看可执行操作")
        self.lbl_detail.setObjectName("hint")
        col.addWidget(self.lbl_detail)
        self.rules_row = QHBoxLayout()
        col.addLayout(self.rules_row)
        bottom.addLayout(col, 1)
        lay.addLayout(bottom)

        from .clip_paste import hotkey_hint     # 文案跟随设置热键，改键后不再失真
        hk = hotkey_hint()
        paste_tip = (f"全局热键「{hk}」呼出快捷粘贴，回车直接粘进原窗口。" if hk else
                     "快捷粘贴未设热键（可在设置页配置），可经托盘菜单呼出。")
        self.lbl = QLabel(
            ("监听已开启：复制的内容会出现在此列表；双击一行可复制回剪贴板；"
             + paste_tip) if watch else
            "剪贴板监听默认关闭（不记录任何内容）：勾选「监听剪贴板」并确认后开始记录；已记录的历史保留可查。")
        self.lbl.setObjectName("hint")
        lay.addWidget(self.lbl)

        app = QApplication.instance()
        self._clip = app.clipboard() if app else None
        self._last = (self._clip.text() or "") if self._clip else ""
        self._bg = None
        self._watch_connected = False
        self._cap_jobs = queue.Queue()  # 大图后台编码任务（FIFO，保入库顺序）
        self._cap_thread = None         # 懒启动的单工作者线程（见 _queue_image_capture）
        self.capture_added.connect(self._after_add)
        if self._clip and watch:
            self._clip.dataChanged.connect(self._on_clip_change)
            self._watch_connected = True
        # 防抖刷盘：add 只标脏，10s 周期落盘（连发复制不在 UI 线程反复 DPAPI 写）
        self._flush_timer = QTimer(self)
        self._flush_timer.timeout.connect(self._flush_disk)
        self._flush_timer.start(10_000)
        # 按天过期：启动清一次 + 每 24h 清一次（用户可设保留天数，0=永不过期）
        # reload 在前：purge 的清理提示才不会被 reload 覆盖
        self.reload()
        self._purge_expired()
        self._purge_timer = QTimer(self)
        self._purge_timer.timeout.connect(self._purge_expired)
        self._purge_timer.start(24 * 3600_000)

    def _purge_expired(self):
        try:
            days = int(settings.load().get("clipboard_retain_days", 1))
            removed = self.hist.purge_older_than(days)
            # 孤儿载荷清扫：崩溃/落盘后未及入库等留下的无主 PNG 不在任何条目的
            # 淘汰链上，只会随时间累积（截图载荷动辄数 MB）——按内存历史引用集
            # 兜底删除（含 1h mtime 宽限，见 sweep_orphan_files）
            swept = clipboard_mod.sweep_orphan_files(
                it.get("data", "") for it in self.hist.items()
                if it.get("kind") == "image")
            if removed:
                self.reload()
            if swept:
                LOGGER.info("clipboard_files 孤儿载荷清扫: {} 个", swept)
            if removed or swept:
                self.lbl.setText(f"已按保留策略清理 {removed} 条过期剪贴板记录"
                                 + (f"，并清掉 {swept} 个残留图片文件" if swept else ""))
        except Exception:
            pass

    def _flush_disk(self):
        try:
            self.hist.flush_if_dirty()
        except Exception:
            pass

    def flush_disk(self):
        """退出路径调用：立即刷盘（quit_app 全树扫尾时触发）。"""
        self._flush_disk()

    # ---------------- 列表 ----------------

    def _filtered_items(self) -> list:
        kind = KIND_FILTERS[self.cmb_kind.currentIndex()][1]
        return self.hist.search(self.edt_search.text(), kind)

    def reload(self):
        # 批量重填：监听开着时每次复制、搜索时每键都会走这里，
        # 逐行 setItem 的中间态不刷屏（挂起更新统一在尾部一次性重绘）
        self.tbl.setUpdatesEnabled(False)
        try:
            items = self._filtered_items()
            self.tbl.setRowCount(len(items))
            for r, it in enumerate(items):
                try:
                    t = time.strftime("%m-%d %H:%M:%S", time.localtime(int(it.get("ts", 0))))
                except Exception:
                    t = str(it.get("ts", ""))
                col0 = QTableWidgetItem(("📌 " if it.get("pinned") else "") + t)
                col0.setData(Qt.UserRole, int(it.get("idx", -1)))
                self.tbl.setItem(r, 0, col0)
                self.tbl.setItem(r, 1, QTableWidgetItem(
                    KIND_LABELS.get(it.get("kind", "text"), it.get("kind", "text"))))
                item = QTableWidgetItem(clipboard_mod.ClipboardHistory.preview(it.get("text", "")))
                item.setToolTip(it.get("text", "")[:500])
                if it.get("kind") == "image":
                    # 图片条目行内缩略图（补57：不再是纯文字占位）
                    thumb = self._thumb_for(it.get("data", ""))
                    if thumb is not None:
                        item.setIcon(thumb)
                self.tbl.setItem(r, 2, item)
        finally:
            self.tbl.setUpdatesEnabled(True)
        self.lbl.setText(f"共 {len(items)} 条　双击行可复制")

    def _thumb_for(self, name: str):
        """图片条目的行内缩略图；文件丢失/损坏返回 None（单源见 ui/clip_thumbs.py）。"""
        return _clip_icon_for(name)

    def _on_clip_change(self):
        if not self.chk_watch.isChecked():
            try:
                self._last = self._clip.text() or ""   # 与隐私暂停同语义：不补录
            except Exception:
                pass
            return
        try:
            text = self._clip.text() or ""
        except Exception:
            return
        try:
            mime = self._clip.mimeData()
        except Exception:
            mime = None
        # 图片/文件复制通常不带 CF_UNICODETEXT：text 恒为 ""，而 _last 启动初值
        # 与图片落库后（_after_add）也都是 ""——旧版 text == self._last 的文本
        # 去重把截图/文件复制整段吞掉（图片条目根本进不了历史，真机症状
        # 「剪贴板不显示图片」；73d02c1 引入图片捕获起即如此）。带非文本载荷
        # 时绕过该闸；同一内容重复复制由 hist.add 的 md5/载荷去重兜底不重记。
        if text == self._last and not self._has_nontext_payload(mime):
            return
        # 前台进程黑名单（密码管理器等来源不记录）；当前内容吞掉防止恢复后补录
        if not clipboard_mod.should_record_now():
            self._last = text
            return
        added = False
        if mime is not None:
            try:
                if mime.hasUrls():
                    paths = [u.toLocalFile() for u in mime.urls() if u.isLocalFile()]
                    if paths:
                        added = self.hist.add("\n".join(paths), kind="file")
                if (not added and mime.hasImage()
                        and settings.load().get("clip_capture_images", True)):
                    img = self._clip.image()    # 位图须在 GUI 线程同步拷出（剪贴板属主）
                    if img is not None and not img.isNull():
                        if img.width() * img.height() > _ASYNC_ENCODE_PIXELS:
                            # 大图：编码/落盘移交后台（_queue_image_capture）；
                            # added=True 与同步成功同语义——载荷明确是图片，
                            # 不再落 html/text 兜底，条目稍后经信号入列刷新
                            self._queue_image_capture(img)
                            added = True
                        else:
                            added = self._encode_and_add(img)
                if not added and mime.hasHtml():
                    html = mime.html() or ""
                    if html.strip() and html.strip() != text.strip():
                        added = self.hist.add(text or "[HTML]", kind="html", data=html[:20000])
            except Exception:
                added = False
        if not added:
            added = self.hist.add(text)
        if added:
            self._after_add()

    @staticmethod
    def _has_nontext_payload(mime) -> bool:
        """剪贴板是否携带文本以外的载荷（文件列表/图片位图）。

        这些复制的 text() 往往恒为 ""，不能参与基于文本的「内容未变」去重；
        mime 不可用/探测异常一律按纯文本处理（去重闸保持旧语义）。
        """
        if mime is None:
            return False
        try:
            return bool(mime.hasUrls() or mime.hasImage())
        except Exception:
            return False

    def _after_add(self):
        self._last = self._clip.text() or ""
        self.reload()

    def _encode_and_add(self, img) -> bool:
        """PNG 编码 + 落盘 + 入库；任一步失败返回 False。

        落盘失败（磁盘满/目录不可写）不得以 data="" 照常入库：那样会产生
        既不能复制回放、又把后续同尺寸截图挡在「与最新条相同」闸外的僵尸条目
        （本次复制的内容悄然丢失，用户只会觉得「截图又没进历史」）。
        """
        buf = QBuffer()
        buf.open(QBuffer.WriteOnly)
        if not img.save(buf, "PNG"):
            return False
        name = clipboard_mod.save_image_bytes(bytes(buf.data()))
        if not name:
            return False
        return self.hist.add(f"[图片 {img.width()}×{img.height()}]",
                             kind="image", data=name)

    def _queue_image_capture(self, img):
        """大图捕获移交单线程 FIFO 后台：dataChanged 在 GUI 线程触发，4K 截图
        （≈33MB 位图）同步 img.save("PNG") 是百毫秒级阻塞——截图瞬间整个
        界面冻住。QImage 拷出后自带像素数据，工作者线程只做编码/写盘/入库
        （ClipboardHistory 自带 RLock）；完成经 capture_added 排队回 GUI
        线程刷新列表。单工作者 + FIFO 保证连拍多图的入库顺序与复制顺序一致；
        应用退出时守护线程随进程终止，最多丢一条刚复制的截图（与崩溃丢防抖
        窗口内历史同口径，残留 PNG 由孤儿清扫回收）。"""
        if self._cap_thread is None:
            self._cap_thread = threading.Thread(
                target=self._capture_worker, daemon=True,
                name="clip-image-encode")
            self._cap_thread.start()
        self._cap_jobs.put(img)

    def _capture_worker(self):
        while True:
            img = self._cap_jobs.get()
            try:
                added = self._encode_and_add(img)
            except Exception:
                LOGGER.warning("大图后台捕获失败", exc_info=True)
                added = False
            finally:
                self._cap_jobs.task_done()
            if added:
                try:
                    self.capture_added.emit()
                except RuntimeError:
                    break       # C++ 侧已销毁（应用退出）：后续任务没有意义，收摊

    # ---------------- 选中 / 操作 ----------------

    def _selected_index(self) -> int:
        row = self.tbl.currentRow()
        if row < 0:
            return -1
        item = self.tbl.item(row, 0)
        data = item.data(Qt.UserRole) if item else None
        return int(data) if data is not None else -1

    def _selected_item(self):
        idx = self._selected_index()
        items = self.hist.items()
        return items[idx] if 0 <= idx < len(items) else None

    def _copy_selected(self):
        it = self._selected_item()
        if it is None or not self._clip:
            return
        if it.get("kind") == "image":
            from PySide6.QtGui import QImage
            path = clipboard_mod.image_path(it.get("data", ""))
            if path and os.path.exists(path):
                img = QImage(path)
                if not img.isNull():
                    self._clip.setImage(img)
                    self.lbl.setText("已把图片放回剪贴板")
                    return
        self._clip.setText(it.get("text", ""))
        self._last = it.get("text", "")
        self.lbl.setText("已复制到剪贴板")

    def _toggle_pin(self):
        idx = self._selected_index()
        if idx >= 0 and self.hist.toggle_pin(idx):
            self.reload()

    def _toggle_pause(self):
        if self.chk_watch.isChecked():
            self.chk_watch.setChecked(False)
            try:
                self._last = (self._clip.text() or "") if self._clip else ""
            except Exception:
                self._last = ""
            self.btn_pause.setText("恢复监听")
            self.lbl_detail.setText("隐私暂停中：不记录新内容")
        else:
            self.chk_watch.setChecked(True)
            self.btn_pause.setText("隐私暂停")
            self.lbl_detail.setText("已恢复监听")

    # ---------------- 监听开关（默认关 + 确认开启，仿自启 opt-in） ----------------

    def _on_watch_clicked(self, on: bool):
        """用户点击「监听剪贴板」：开启需确认；关闭停止记录但历史保留。

        隐私暂停的程序化 setChecked 不经过这里（clicked 仅用户点击触发）。
        """
        if on and not self._confirm_watch_enable():
            self.chk_watch.setChecked(False)        # 回滚勾选（不触发 clicked）
            return
        self._set_watch(on)
        if not on:
            try:
                self._last = (self._clip.text() or "") if self._clip else ""
            except Exception:
                self._last = ""                     # 吞掉当前内容：重开后不补录
            self.lbl_detail.setText("已关闭监听：不记录新内容（历史保留可查）")
        self.btn_pause.setEnabled(on)
        if on:
            self.btn_pause.setText("隐私暂停")
        try:
            settings.save({"clipboard_watch": on})
        except Exception:
            LOGGER.warning("剪贴板监听开关持久化失败", exc_info=True)

    def _confirm_watch_enable(self) -> bool:
        ret = QMessageBox.question(
            self, "开启剪贴板历史记录",
            "开启后将自动记录复制的文本 / 图片 / 文件路径 / 网页内容（本地加密存储）。\n\n"
            "· 密码管理器等黑名单程序前台时复制的内容不会记录\n"
            "· 密码库 / WiFi 页面的敏感复制不会记录，也不会进 Win+V 系统历史\n"
            "· 关闭监听只停止新记录，已记录的历史保留，可随时手动清空\n\n"
            "确认开启？",
            QMessageBox.Yes | QMessageBox.No)
        return ret == QMessageBox.Yes

    def _set_watch(self, on: bool):
        if self._clip is None:
            return
        if on and not self._watch_connected:
            self._clip.dataChanged.connect(self._on_clip_change)
            self._watch_connected = True
        elif not on and self._watch_connected:
            try:
                self._clip.dataChanged.disconnect(self._on_clip_change)
            except (RuntimeError, TypeError):
                pass
            self._watch_connected = False

    def _delete_selected(self):
        idx = self._selected_index()
        if idx >= 0 and self.hist.remove(idx):
            self.reload()
            self._show_detail()

    # ---------------- 右键菜单 / 预览 ----------------

    def _show_context_menu(self, pos):
        if self._selected_index() < 0:
            return
        it = self._selected_item()
        if it is None:
            return
        is_image = it.get("kind") == "image"
        menu = QMenu(self)
        menu.addAction("复制", self._copy_selected)
        menu.addAction("取消置顶" if it.get("pinned") else "置顶", self._toggle_pin)
        menu.addAction("预览", self._preview_selected)
        if is_image:
            path = clipboard_mod.image_path(it.get("data", ""))
            if path and os.path.exists(path):
                menu.addAction("另存为…", lambda: self._save_image_as(path))
        menu.addSeparator()
        menu.addAction("删除", self._delete_selected)
        menu.exec(self.tbl.viewport().mapToGlobal(pos))

    def _preview_selected(self):
        it = self._selected_item()
        if it is None:
            return
        if it.get("kind") == "image":
            path = clipboard_mod.image_path(it.get("data", ""))
            if path and os.path.exists(path):
                self._show_image_preview(path)
            else:
                QMessageBox.information(self, "预览", "图片文件已不存在。")
            return
        self._show_text_preview(it.get("text", ""))

    def _show_text_preview(self, text: str):
        """全文预览对话框（列表内容列只显示 80 字折叠——长条目从这里看全）。"""
        dlg = QDialog(self)
        dlg.setWindowTitle("剪贴板内容预览")
        dlg.resize(560, 420)
        v = QVBoxLayout(dlg)
        txt = QTextEdit()
        txt.setReadOnly(True)
        txt.setPlainText(text or "")
        v.addWidget(txt)
        row = QHBoxLayout()
        row.addStretch(1)
        btn_copy = QPushButton("复制全文")
        btn_copy.clicked.connect(
            lambda: self._set_clipboard_text(text, "已复制到剪贴板"))
        row.addWidget(btn_copy)
        btn_close = QPushButton("关闭")
        btn_close.clicked.connect(dlg.accept)
        row.addWidget(btn_close)
        v.addLayout(row)
        dlg.exec()

    def _clear_all(self):
        ret = QMessageBox.question(self, "清空剪贴板历史", "确定清空全部历史记录？（置顶条目也会删除）",
                                   QMessageBox.Yes | QMessageBox.No)
        if ret == QMessageBox.Yes:
            self.hist.clear()
            self.reload()
            self._show_detail()

    # ---------------- 规则动作 ----------------

    def _clear_rules(self):
        while self.rules_row.count():
            item = self.rules_row.takeAt(0)
            w = item.widget()
            if w:
                w.deleteLater()

    def _show_detail(self):
        self._clear_rules()
        self.lbl_thumb.hide()
        it = self._selected_item()
        if it is None:
            self.lbl_detail.setText("选中一条记录查看可执行操作")
            return
        if it.get("kind") == "image":
            path = clipboard_mod.image_path(it.get("data", ""))
            pix = QPixmap(path) if path and os.path.exists(path) else QPixmap()
            if not pix.isNull():
                self.lbl_thumb.setPixmap(pix.scaledToHeight(200, Qt.SmoothTransformation))
                self.lbl_thumb.show()
                btn_preview = QPushButton("放大预览")
                btn_preview.clicked.connect(
                    lambda checked=False, p=path: self._show_image_preview(p))
                self.rules_row.addWidget(btn_preview)
                btn_save = QPushButton("另存为…")
                btn_save.clicked.connect(
                    lambda checked=False, p=path: self._save_image_as(p))
                self.rules_row.addWidget(btn_save)
                self.lbl_detail.setText("图片条目：双击可放回剪贴板"
                                        + ("" if pix.isNull() else "；可放大预览或另存为"))
            else:
                self.lbl_detail.setText("图片条目：图片文件已不存在（可能已按保留策略清理）")
            return
        hits = cliprules.detect(it.get("text", ""))
        self.lbl_detail.setText("可执行操作（↗ 需联网）：" if hits else "此内容无可识别操作")
        for hit in hits:
            btn = QPushButton(hit["label"] + (" ↗" if hit.get("online") else ""))
            btn.clicked.connect(lambda checked=False, h=dict(hit): self._run_action(h))
            self.rules_row.addWidget(btn)

    def _show_image_preview(self, path: str):
        """图片大图预览对话框：等比适配屏幕，支持复制/另存为（补57）。"""
        pix = QPixmap(path)
        if pix.isNull():
            QMessageBox.information(self, "预览", "图片文件已不存在。")
            return
        dlg = QDialog(self)
        dlg.setWindowTitle("剪贴板图片预览")
        v = QVBoxLayout(dlg)
        lbl = QLabel()
        screen = QApplication.instance().primaryScreen().availableGeometry()
        max_w, max_h = int(screen.width() * 0.7), int(screen.height() * 0.7)
        scaled = pix.scaled(max_w, max_h, Qt.KeepAspectRatio, Qt.SmoothTransformation)
        lbl.setPixmap(scaled)
        v.addWidget(lbl)
        row = QHBoxLayout()
        btn_copy = QPushButton("复制图片")
        btn_copy.clicked.connect(
            lambda: self._copy_image_file(path, note_source=dlg))
        btn_save = QPushButton("另存为…")
        btn_save.clicked.connect(lambda: self._save_image_as(path))
        btn_close = QPushButton("关闭")
        btn_close.clicked.connect(dlg.accept)
        row.addStretch(1)
        row.addWidget(btn_copy)
        row.addWidget(btn_save)
        row.addWidget(btn_close)
        v.addLayout(row)
        dlg.exec()

    def _copy_image_file(self, path: str, note_source=None):
        from PySide6.QtGui import QImage
        img = QImage(path)
        if img.isNull() or not self._clip:
            return
        self._clip.setImage(img)
        self._last = self._clip.text() or ""
        if note_source is not None:
            self.lbl.setText("已把图片放回剪贴板")

    def _save_image_as(self, path: str):
        default = os.path.join(os.environ.get("USERPROFILE", os.getcwd()),
                               time.strftime("clipboard_%Y%m%d_%H%M%S.png"))
        target, _filt = QFileDialog.getSaveFileName(
            self, "图片另存为", default, "PNG 图片 (*.png)")
        if not target:
            return
        if not target.lower().endswith(".png"):
            target += ".png"
        pix = QPixmap(path)
        if pix.isNull() or not pix.save(target, "PNG"):
            QMessageBox.warning(self, "另存为", "保存失败（路径不可写？）")
        else:
            self.lbl.setText(f"图片已保存到 {target}")

    def _run_action(self, hit):
        aid, arg = hit["id"], hit.get("arg", "")
        if aid == "json_format":
            self._set_clipboard_text(arg, "格式化结果已复制到剪贴板")
        elif aid == "url_qr":
            self._show_qr_dialog(arg)
        elif aid == "url_open":
            QDesktopServices.openUrl(QUrl(arg))
        elif aid == "color_show":
            self._show_color_dialog(arg)
        elif aid == "file_reveal":
            QDesktopServices.openUrl(QUrl.fromLocalFile(os.path.dirname(arg) or arg))
        elif aid == "file_open":
            QDesktopServices.openUrl(QUrl.fromLocalFile(arg))
        elif aid == "domain_dns":
            self._start_bg(self._dns_lookup, arg)
        elif aid in ("domain_ping", "ip_ping"):
            self._start_bg(self._ping_host, arg)
        elif aid == "ip_geo":
            self._start_bg(self._ip_geo, arg)

    def _set_clipboard_text(self, text, note):
        if not self._clip:
            return
        self._clip.setText(text)
        self._last = text
        self.lbl.setText(note)

    def _start_bg(self, fn, arg):
        if getattr(self, "_bg", None):
            return
        self.lbl_detail.setText("执行中…")
        self._bg = reap(_ClipActionWorker(fn, arg), self)
        self._bg.done.connect(self._bg_done)
        self._bg.start()

    def _bg_done(self, msg):
        self._bg = None
        self.lbl_detail.setText(str(msg))

    @staticmethod
    def _dns_lookup(host):
        from .. import dnsquery
        answers = dnsquery.query(host)
        return f"{host} → {', '.join(answers) if answers else '无记录'}"

    @staticmethod
    def _ping_host(host):
        from .. import icmp
        ms = icmp.ping(host)
        return f"{host} 延迟 {ms} ms" if ms is not None else f"{host} ping 不通"

    @staticmethod
    def _ip_geo(ip):
        import json as _json
        from urllib.request import urlopen
        with urlopen(f"http://ip-api.com/json/{ip}?lang=zh-CN", timeout=5) as r:
            d = _json.loads(r.read().decode("utf-8", "replace"))
        if d.get("status") == "success":
            return f"{ip} · {d.get('country', '')} {d.get('regionName', '')} {d.get('city', '')} {d.get('isp', '')}".strip()
        return f"{ip} 归属地查询失败"

    def _show_qr_dialog(self, data):
        try:
            import io
            import qrcode
            img = qrcode.make(data, box_size=8, border=2)
            buf = io.BytesIO()
            img.save(buf, format="PNG")
            pix = QPixmap()
            pix.loadFromData(buf.getvalue(), "PNG")
        except Exception as e:
            QMessageBox.warning(self, "SysKit", f"二维码生成失败: {e}")
            return
        dlg = QDialog(self)
        dlg.setWindowTitle("二维码")
        v = QVBoxLayout(dlg)
        lbl = QLabel()
        lbl.setPixmap(pix.scaled(280, 280, Qt.KeepAspectRatio, Qt.SmoothTransformation))
        lbl.setAlignment(Qt.AlignCenter)
        v.addWidget(lbl)
        tip = QLabel(clipboard_mod.ClipboardHistory.preview(data, 60))
        tip.setObjectName("hint")
        tip.setAlignment(Qt.AlignCenter)
        v.addWidget(tip)
        btn = QPushButton("保存图片")
        btn.clicked.connect(lambda: self._save_qr(buf.getvalue()))
        v.addWidget(btn, 0, Qt.AlignCenter)
        dlg.exec()

    def _save_qr(self, png_bytes):
        from PySide6.QtWidgets import QFileDialog
        path, _ = QFileDialog.getSaveFileName(self, "保存二维码", "clip_qr.png", "PNG (*.png)")
        if path:
            with open(path, "wb") as f:
                f.write(png_bytes)

    def _show_color_dialog(self, hexcolor):
        dlg = QDialog(self)
        dlg.setWindowTitle(f"颜色 {hexcolor}")
        v = QVBoxLayout(dlg)
        swatch = QLabel()
        swatch.setFixedHeight(120)
        swatch.setStyleSheet(f"background:{hexcolor}; border:1px solid #ccc; border-radius:8px;")
        v.addWidget(swatch)
        tip = QLabel(hexcolor)
        tip.setObjectName("hint")
        tip.setAlignment(Qt.AlignCenter)
        v.addWidget(tip)
        dlg.exec()


class ModesTab(QWidget):
    """办公模式 / 游戏模式一键开关。"""

    def __init__(self, parent=None):
        super().__init__(parent)
        lay = QVBoxLayout(self)
        lay.setContentsMargins(8, 8, 8, 8)
        lay.setSpacing(10)

        grp_o = QGroupBox("办公模式")
        lo = QVBoxLayout(grp_o)
        lo.addWidget(QLabel(
            "适合开会/写文档：勿扰（关通知）+ 静音 + 打开常用软件 + 备份当前配置。\n"
            "退出模式会恢复通知并取消静音。"))
        self.chk_open_apps = QCheckBox("同时打开常用软件（记事本 / 计算器 / 资源管理器 / 画图）")
        self.chk_open_apps.setChecked(True)
        self.chk_open_apps.setToolTip("不勾选则只做勿扰+静音+备份，不自动启动程序")
        lo.addWidget(self.chk_open_apps)
        bo = QHBoxLayout()
        self.btn_office = QPushButton("进入办公模式")
        self.btn_office.setProperty("primary", True)
        self.btn_office.setToolTip("按上表步骤依次执行，单步失败不会中断后续步骤")
        self.btn_office.clicked.connect(self._enter_office)
        self.btn_office_off = QPushButton("退出办公模式")
        self.btn_office_off.setToolTip("恢复系统通知并取消静音（不会关闭已打开的软件）")
        self.btn_office_off.clicked.connect(self._exit_office)
        bo.addWidget(self.btn_office)
        bo.addWidget(self.btn_office_off)
        bo.addStretch(1)
        lo.addLayout(bo)
        lay.addWidget(grp_o)

        grp_g = QGroupBox("游戏模式")
        lg = QVBoxLayout(grp_g)
        lg.addWidget(QLabel(
            "适合打游戏：关通知 + 高性能电源 + 暂停 Windows 更新 + 清理白名单后台。\n"
            "退出模式会恢复通知、平衡电源与更新策略。"))
        self.chk_clear_bg = QCheckBox("同时清理白名单后台（OneDrive / 音乐 / 游戏启动器等）")
        self.chk_clear_bg.setChecked(True)
        self.chk_clear_bg.setToolTip("仅结束白名单应用，不会碰 explorer / 系统关键进程")
        lg.addWidget(self.chk_clear_bg)
        bg = QHBoxLayout()
        self.btn_game = QPushButton("进入游戏模式")
        self.btn_game.setProperty("primary", True)
        self.btn_game.setToolTip("暂停更新与高性能电源建议以管理员运行；无管理员时会跳过失败步骤")
        self.btn_game.clicked.connect(self._enter_game)
        self.btn_game_off = QPushButton("退出游戏模式")
        self.btn_game_off.setToolTip("恢复通知、平衡电源与 Windows 更新策略")
        self.btn_game_off.clicked.connect(self._exit_game)
        bg.addWidget(self.btn_game)
        bg.addWidget(self.btn_game_off)
        bg.addStretch(1)
        lg.addLayout(bg)
        tip = QLabel("提示：暂停更新 / 部分电源操作需要管理员权限；可用窗口底部「以管理员重启」后再试。")
        tip.setObjectName("hint")
        lg.addWidget(tip)
        lay.addWidget(grp_g)

        self.txt = make_table(["步骤", "结果", "说明"])
        self.txt.horizontalHeader().setSectionResizeMode(2, QHeaderView.Stretch)
        self.txt.setSelectionMode(QAbstractItemView.NoSelection)
        lay.addWidget(self.txt, 1)
        self.lbl = QLabel("尚未执行模式。点击上方按钮查看逐步结果。")
        self.lbl.setObjectName("hint")
        lay.addWidget(self.lbl)
        self._busy = False

    def _set_mode_busy(self, busy: bool):
        self._busy = busy
        for b in (self.btn_office, self.btn_office_off, self.btn_game, self.btn_game_off):
            b.setEnabled(not busy)

    def _show_steps(self, steps, title):
        self.txt.setRowCount(len(steps))
        try:
            from . import theme
            p = theme.pal()
            ok_color, err_color = p.get("ok", "#2e9e44"), p.get("err", "#d94040")
        except Exception:
            ok_color, err_color = "#2e9e44", "#d94040"
        for i, s in enumerate(steps):
            self.txt.setItem(i, 0, QTableWidgetItem(s.get("name", "")))
            mark = QTableWidgetItem("√ 成功" if s.get("ok") else "× 失败")
            mark.setForeground(QColor(ok_color if s.get("ok") else err_color))
            self.txt.setItem(i, 1, mark)
            self.txt.setItem(i, 2, QTableWidgetItem(s.get("detail", "")))
        ok = sum(1 for s in steps if s.get("ok"))
        self.lbl.setText(f"{title}：{ok}/{len(steps)} 步成功")
        QMessageBox.information(self, title, f"{ok}/{len(steps)} 步成功。\n详见下表逐步说明。")

    def _start_mode(self, fn, title):
        """场景模式统一异步入口：PowerShell/注册表/进程清理在后台线程执行，
        不再冻结 UI（步骤多时可达数十秒）。完成信号回 UI 展示。"""
        if self._busy:
            return
        self._set_mode_busy(True)
        self._mw = reap(_ModeWorker(fn, title), self)
        self._mw.steps_done.connect(self._mode_done)
        self._mw.start()

    def _mode_done(self, steps_title):
        steps, title = steps_title
        self._mw = None
        self._show_steps(steps, title)
        self._set_mode_busy(False)          # 后台执行完成：恢复按钮（原 finally 语义）

    def _enter_office(self):
        self._start_mode(
            lambda: modes.run_office_mode(open_apps=self.chk_open_apps.isChecked()),
            "进入办公模式")

    def _exit_office(self):
        self._start_mode(modes.restore_office_defaults, "退出办公模式")

    def _enter_game(self):
        if self._busy:
            return
        # 与其余三个入口统一走 _start_mode 后台线程：run_game_mode 串行执行
        # powercfg/PowerShell/进程清理，同步跑会冻结 UI 数秒~数十秒
        if not admin.is_admin():
            QMessageBox.warning(
                self, "游戏模式",
                "当前未以管理员运行：\n"
                "· 暂停 Windows 更新 可能失败\n"
                "· 高性能电源 / 关通知 仍会尝试\n\n"
                "可点窗口底部「以管理员重启」后再进入游戏模式。")
        self._start_mode(
            lambda: modes.run_game_mode(clear_bg=self.chk_clear_bg.isChecked()),
            "进入游戏模式")

    def _exit_game(self):
        self._start_mode(modes.restore_game_defaults, "退出游戏模式")


class _ModeWorker(QThread):
    """场景模式后台执行：fn() → [(name, ok, detail)] 经信号回 UI。"""

    steps_done = Signal(list)

    def __init__(self, fn, title, parent=None):
        super().__init__(parent)
        self._fn, self._title = fn, title

    def run(self):
        try:
            steps = self._fn()
        except Exception as e:
            from ..logger import LOGGER
            LOGGER.exception("场景模式线程异常")
            steps = [{"name": self._title, "ok": False, "detail": str(e)}]
        self.steps_done.emit((steps, self._title))


class SystemPage(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        lay = QVBoxLayout(self)
        lay.setContentsMargins(0, 0, 0, 0)
        tabs = QTabWidget()
        self.tabs = tabs                # 供命令面板定位子工具
        self.perf = PerfTab()
        tabs.addTab(self.perf, "性能监控")
        tabs.addTab(StartupTab(), "启动项管理")
        self.battery = BatteryTab()
        tabs.addTab(self.battery, "电池健康")
        tabs.addTab(CleanupTab(), "垃圾清理")
        tabs.addTab(DiskTab(), "磁盘空间")
        tabs.addTab(ClipboardTab(), "剪贴板历史")
        tabs.addTab(ModesTab(), "场景模式")
        lay.addWidget(tabs)

    def refresh_theme_colors(self):
        """主题切换后重填固化色（补57：电池健康三档色）。"""
        self.battery._apply_health_color()

    def update_tick(self, d):
        self.perf.update_tick(d)

    def reset_if_stale(self, max_gap_sec: float = 600.0):
        """转发给性能 Tab（图表在 PerfTab 上）。"""
        self.perf.reset_if_stale(max_gap_sec)
