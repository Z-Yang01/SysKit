"""系统助手：性能监控 / 启动项管理 / 电池健康 / 安全清理 / 剪贴板历史 / 场景模式。"""
import glob
import os
import shutil
import tempfile
import time

import psutil
from PySide6.QtCore import Qt, QThread, QTimer, Signal
from PySide6.QtGui import QColor, QClipboard
from PySide6.QtWidgets import (QAbstractItemView, QApplication, QCheckBox, QComboBox, QFormLayout,
                               QGroupBox, QHBoxLayout, QHeaderView, QLabel, QListWidget,
                               QListWidgetItem, QMessageBox, QProgressBar, QPushButton,
                               QTableWidget, QTableWidgetItem, QTabWidget, QVBoxLayout, QWidget)

from .charts import LineChart, human_bytes
from .disk_page import DiskTab
from .. import admin, clipboard as clipboard_mod, modes, settings, winregread
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
            QMessageBox.warning(self, "SysKit", "禁用/启用启动项需要管理员权限（涉及 HKLM）。")
            return
        targets = self._selected()
        if not targets:
            QMessageBox.information(self, "SysKit", "请先在表格中选中要操作的启动项。")
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
            QMessageBox.information(self, "SysKit", "请先勾选要清理的项目（先扫描可看到大小）。")
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


class ClipboardTab(QWidget):
    """剪贴板历史：监听系统剪贴板文本，列表可复制/删除/清空。"""

    def __init__(self, parent=None):
        super().__init__(parent)
        max_items = int(settings.load().get("clipboard_max", 50))
        self.hist = clipboard_mod.ClipboardHistory(max_items=max_items)
        lay = QVBoxLayout(self)
        lay.setContentsMargins(8, 8, 8, 8)
        row = QHBoxLayout()
        self.chk_watch = QCheckBox("监听剪贴板（自动记录文本）")
        self.chk_watch.setChecked(True)
        self.btn_copy = QPushButton("复制选中")
        self.btn_copy.clicked.connect(self._copy_selected)
        self.btn_del = QPushButton("删除选中")
        self.btn_del.clicked.connect(self._delete_selected)
        self.btn_clear = QPushButton("清空历史")
        self.btn_clear.setProperty("danger", True)
        self.btn_clear.clicked.connect(self._clear_all)
        row.addWidget(self.chk_watch)
        row.addStretch(1)
        row.addWidget(self.btn_copy)
        row.addWidget(self.btn_del)
        row.addWidget(self.btn_clear)
        lay.addLayout(row)

        self.tbl = QTableWidget(0, 2)
        self.tbl.setHorizontalHeaderLabels(["时间", "内容"])
        self.tbl.horizontalHeader().setSectionResizeMode(1, QHeaderView.Stretch)
        self.tbl.setSelectionBehavior(QAbstractItemView.SelectRows)
        self.tbl.setSelectionMode(QAbstractItemView.SingleSelection)
        self.tbl.setEditTriggers(QAbstractItemView.NoEditTriggers)
        self.tbl.setAlternatingRowColors(True)
        self.tbl.verticalHeader().setVisible(False)
        self.tbl.itemDoubleClicked.connect(lambda *_: self._copy_selected())
        lay.addWidget(self.tbl)
        self.lbl = QLabel("监听剪贴板后，复制的文本会出现在此列表；双击一行可复制回剪贴板。")
        self.lbl.setObjectName("hint")
        self.lbl.setStyleSheet("color:#8a93a0;")
        lay.addWidget(self.lbl)

        app = QApplication.instance()
        self._clip = app.clipboard() if app else None
        self._last = (self._clip.text() or "") if self._clip else ""
        if self._clip:
            self._clip.dataChanged.connect(self._on_clip_change)
        self.reload()

    def reload(self):
        items = self.hist.items()
        self.tbl.setRowCount(len(items))
        for i, it in enumerate(items):
            try:
                t = time.strftime("%m-%d %H:%M:%S", time.localtime(int(it.get("ts", 0))))
            except Exception:
                t = str(it.get("ts", ""))
            self.tbl.setItem(i, 0, QTableWidgetItem(t))
            preview = clipboard_mod.ClipboardHistory.preview(it.get("text", ""))
            item = QTableWidgetItem(preview)
            item.setToolTip(it.get("text", "")[:500])
            self.tbl.setItem(i, 1, item)
        self.lbl.setText(f"共 {len(items)} 条　双击行可复制")

    def _on_clip_change(self):
        if not self.chk_watch.isChecked():
            return
        try:
            text = self._clip.text() if self._clip else ""
        except Exception:
            return
        if not text or text == self._last:
            return
        self._last = text
        if self.hist.add(text):
            self.reload()

    def _selected_index(self):
        row = self.tbl.currentRow()
        return row if 0 <= row < self.tbl.rowCount() else -1

    def _copy_selected(self):
        idx = self._selected_index()
        if idx < 0 or not self._clip:
            return
        items = self.hist.items()
        if idx < len(items):
            self._clip.setText(items[idx]["text"])
            self.lbl.setText("已复制到剪贴板")

    def _delete_selected(self):
        idx = self._selected_index()
        if idx >= 0 and self.hist.remove(idx):
            self.reload()

    def _clear_all(self):
        ret = QMessageBox.question(self, "清空剪贴板历史", "确定清空全部历史记录？",
                                   QMessageBox.Yes | QMessageBox.No)
        if ret == QMessageBox.Yes:
            self.hist.clear()
            self.reload()


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
        tip.setStyleSheet("color:#8a93a0;")
        lg.addWidget(tip)
        lay.addWidget(grp_g)

        self.txt = QTableWidget(0, 3)
        self.txt.setHorizontalHeaderLabels(["步骤", "结果", "说明"])
        self.txt.horizontalHeader().setSectionResizeMode(2, QHeaderView.Stretch)
        self.txt.setEditTriggers(QAbstractItemView.NoEditTriggers)
        self.txt.setAlternatingRowColors(True)
        self.txt.verticalHeader().setVisible(False)
        self.txt.setSelectionMode(QAbstractItemView.NoSelection)
        lay.addWidget(self.txt, 1)
        self.lbl = QLabel("尚未执行模式。点击上方按钮查看逐步结果。")
        self.lbl.setObjectName("hint")
        self.lbl.setStyleSheet("color:#8a93a0;")
        lay.addWidget(self.lbl)
        self._busy = False

    def _set_mode_busy(self, busy: bool):
        self._busy = busy
        for b in (self.btn_office, self.btn_office_off, self.btn_game, self.btn_game_off):
            b.setEnabled(not busy)

    def _show_steps(self, steps, title):
        self.txt.setRowCount(len(steps))
        ok_color = theme_ok = None
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

    def _enter_office(self):
        if self._busy:
            return
        self._set_mode_busy(True)
        try:
            steps = modes.run_office_mode(open_apps=self.chk_open_apps.isChecked())
            self._show_steps(steps, "进入办公模式")
        finally:
            self._set_mode_busy(False)

    def _exit_office(self):
        if self._busy:
            return
        self._set_mode_busy(True)
        try:
            self._show_steps(modes.restore_office_defaults(), "退出办公模式")
        finally:
            self._set_mode_busy(False)

    def _enter_game(self):
        if self._busy:
            return
        if not admin.is_admin():
            QMessageBox.warning(
                self, "游戏模式",
                "当前未以管理员运行：\n"
                "· 暂停 Windows 更新 可能失败\n"
                "· 高性能电源 / 关通知 仍会尝试\n\n"
                "可点窗口底部「以管理员重启」后再进入游戏模式。")
        self._set_mode_busy(True)
        try:
            steps = modes.run_game_mode(clear_bg=self.chk_clear_bg.isChecked())
            self._show_steps(steps, "进入游戏模式")
        finally:
            self._set_mode_busy(False)

    def _exit_game(self):
        if self._busy:
            return
        self._set_mode_busy(True)
        try:
            self._show_steps(modes.restore_game_defaults(), "退出游戏模式")
        finally:
            self._set_mode_busy(False)


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
        tabs.addTab(DiskTab(), "磁盘空间")
        tabs.addTab(ClipboardTab(), "剪贴板历史")
        tabs.addTab(ModesTab(), "场景模式")
        lay.addWidget(tabs)

    def update_tick(self, d):
        self.perf.update_tick(d)
