"""系统助手：性能监控 / 启动项管理 / 电池健康 / 安全清理。"""
import glob
import os
import shutil
import tempfile

import psutil
from PySide6.QtCore import Qt, QThread, Signal
from PySide6.QtGui import QColor
from PySide6.QtWidgets import (QAbstractItemView, QCheckBox, QFormLayout, QGroupBox,
                               QHBoxLayout, QHeaderView, QLabel, QListWidget, QListWidgetItem,
                               QMessageBox, QProgressBar, QPushButton, QTableWidget,
                               QTableWidgetItem, QTabWidget, QVBoxLayout, QWidget)

from .charts import LineChart, human_bytes
from .. import admin, winregread
from ..logger import LOGGER

RUN_KEY = r"Software\Microsoft\Windows\CurrentVersion\Run"
DISABLED_KEY = r"Software\NetTool\DisabledRun"
USER_STARTUP = os.path.join(os.environ.get("APPDATA", ""),
                            r"Microsoft\Windows\Start Menu\Programs\Startup")


# ---------------- 性能 ----------------

class PerfTab(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        lay = QHBoxLayout(self)
        lay.setContentsMargins(8, 8, 8, 8)
        left = QVBoxLayout()
        self.chart_sys = LineChart("CPU / 内存（%）", series=[("CPU", "#3576e0"), ("内存", "#8e5ce6")],
                                   y_fixed=(0, 100), fmt=lambda v: f"{v:.0f}%")
        self.chart_disk = LineChart("磁盘读写（1 秒/点）", series=[("读", "#2e9e44"), ("写", "#ef6c00")],
                                    fmt=human_bytes)
        left.addWidget(self.chart_sys, 1)
        left.addWidget(self.chart_disk, 1)
        lay.addLayout(left, 1)

        right = QVBoxLayout()
        self.tbl = QTableWidget(0, 4)
        self.tbl.setHorizontalHeaderLabels(["PID", "进程", "CPU %", "内存"])
        self.tbl.horizontalHeader().setDefaultSectionSize(90)
        self.tbl.horizontalHeader().setStretchLastSection(True)
        self.tbl.setEditTriggers(QAbstractItemView.NoEditTriggers)
        self.tbl.setAlternatingRowColors(True)
        self.tbl.setSortingEnabled(True)
        self.tbl.verticalHeader().setVisible(False)
        right.addWidget(QLabel("资源占用最高的进程（2 秒刷新）"))
        right.addWidget(self.tbl, 1)
        lay.addLayout(right, 1)

    def update_tick(self, d):
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
        self.tbl.setSortingEnabled(False)
        self.tbl.setRowCount(len(procs))
        for i, pr in enumerate(procs):
            vals = [str(pr["pid"]), str(pr["name"]), f"{pr['cpu']:.1f}",
                    human_bytes(pr["mem"])]
            for c, v in enumerate(vals):
                self.tbl.setItem(i, c, QTableWidgetItem(v))
        self.tbl.setSortingEnabled(True)
        self.tbl.sortItems(sort_col, sort_order)


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

        self.tbl = QTableWidget(0, 4)
        self.tbl.setHorizontalHeaderLabels(["名称", "位置", "命令", "状态"])
        self.tbl.horizontalHeader().setSectionResizeMode(2, QHeaderView.Stretch)
        self.tbl.setSelectionBehavior(QAbstractItemView.SelectRows)
        self.tbl.setEditTriggers(QAbstractItemView.NoEditTriggers)
        self.tbl.setAlternatingRowColors(True)
        self.tbl.verticalHeader().setVisible(False)
        lay.addWidget(self.tbl)
        self.reload()

    @staticmethod
    def _enum():
        """[(location, name, command, enabled)] location: HKCU-Run / HKLM-Run / 启动文件夹"""
        items = []
        for tag, hive in (("HKCU-Run", winregread._HIVES["HKCU"]),
                          ("HKLM-Run", winregread._HIVES["HKLM"])):
            import winreg
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
            items.append(("启动文件夹", os.path.basename(f), f, True))
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
        if disable and not admin.is_admin():
            QMessageBox.warning(self, "NetTool", "禁用/启用启动项需要管理员权限（涉及 HKLM）。")
            return
        targets = self._selected()
        if not targets:
            QMessageBox.information(self, "NetTool", "请先在表格中选中要操作的启动项。")
            return
        for loc, name, enabled in targets:
            try:
                if disable and enabled:
                    self._disable_one(loc, name)
                elif not disable and not enabled:
                    self._enable_one(loc, name)
            except Exception as e:
                LOGGER.error("启动项操作失败 {} {}: {}", name, loc, e)
        self.reload()

    def _disable_one(self, loc, name):
        import winreg
        if loc == "启动文件夹":
            path = os.path.join(USER_STARTUP, name)
            if os.path.exists(path):
                os.rename(path, path + ".disabled")
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
            path = os.path.join(USER_STARTUP, name + ".disabled")
            if os.path.exists(path):
                os.rename(path, path[:-len(".disabled")])
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
        self.lbl_note.setStyleSheet("color:#8a93a0;")
        lay.addWidget(self.lbl_note)
        lay.addStretch(1)
        self.refresh()

    def refresh(self):
        from .. import proc
        script = (
            "$d=(Get-CimInstance -Namespace root/wmi -ClassName BatteryStaticData "
            "-ErrorAction SilentlyContinue | Select-Object -First 1).DesignedCapacity; "
            "$f=(Get-CimInstance -Namespace root/wmi -ClassName BatteryFullChargedCapacity "
            "-ErrorAction SilentlyContinue | Select-Object -First 1).FullChargedCapacity; "
            "$r=(Get-CimInstance Win32_Battery -ErrorAction SilentlyContinue "
            "| Select-Object -First 1).EstimatedChargeRemaining; "
            "@($d,$f,$r) -join ','")
        r = proc.run_ps(script, timeout=30)
        parts = r.stdout.strip().split(",")
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
        health = full / design * 100
        color = "#2e9e44" if health > 80 else "#ef6c00" if health > 60 else "#c62828"
        self.lbl_health.setText(f"{health:.1f}%")
        self.lbl_health.setStyleSheet(f"color:{color}; font-weight:bold;")
        self.lbl_remain.setText(f"{remain}%" if remain is not None else "—")
        self.lbl_note.setText("数据来自 Windows 电池报告（设计容量由厂商提供，仅供参考）。")


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
        for key, _title, getter in CLEAN_TARGETS:
            sizes[key] = self._size(key, getter)
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
                except OSError as e:
                    errors.append(os.path.basename(path))
        self.done.emit(freed, errors[:10])


class CleanupTab(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        lay = QVBoxLayout(self)
        lay.setContentsMargins(8, 8, 8, 8)
        self.lbl_tip = QLabel("白名单式清理，只删除下列安全位置的文件；正在使用的文件会自动跳过。")
        self.lbl_tip.setStyleSheet("color:#8a93a0;")
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
        self.lbl_result.setStyleSheet("color:#2e9e44;")
        lay.addWidget(self.lbl_result)
        lay.addStretch(1)
        self._scan_worker = None
        self._clean_worker = None

    def scan(self):
        if self._scan_worker:
            return
        self.btn_scan.setEnabled(False)
        self.lbl_result.setText("扫描中…")
        self._scan_worker = _ScanWorker()
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
            QMessageBox.information(self, "NetTool", "请先勾选要清理的项目（先扫描可看到大小）。")
            return
        self.btn_clean.setEnabled(False)
        self.pb.show()
        self._clean_worker = _CleanWorker(keys)
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


class SystemPage(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        lay = QVBoxLayout(self)
        lay.setContentsMargins(0, 0, 0, 0)
        tabs = QTabWidget()
        self.perf = PerfTab()
        tabs.addTab(self.perf, "性能监控")
        tabs.addTab(StartupTab(), "启动项管理")
        tabs.addTab(BatteryTab(), "电池健康")
        tabs.addTab(CleanupTab(), "垃圾清理")
        lay.addWidget(tabs)

    def update_tick(self, d):
        self.perf.update_tick(d)
