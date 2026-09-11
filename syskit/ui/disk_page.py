"""磁盘空间：目录占用扫描 / 文件类型分布 / 大文件排行 / 回收站删除。

删除只移入回收站（diskusage.delete_to_recycle_bin），删除完成后自动重新扫描，
与垃圾清理 Tab 的「清理后重扫」保持一致，避免内存模型与真实占用脱节。
"""
import os
import subprocess
import threading
import time

from PySide6.QtCore import Qt, QThread, Signal
from PySide6.QtWidgets import (QComboBox, QHBoxLayout, QHeaderView, QLabel,
                               QMessageBox, QProgressBar, QPushButton,
                               QSplitter, QTableWidget, QTableWidgetItem,
                               QTreeWidget, QTreeWidgetItem, QVBoxLayout,
                               QWidget)

from .. import diskusage
from ..logger import LOGGER
from .charts import BarChart, human_bytes

_BAR_COLORS = ["#3576e0", "#8e5ce6", "#2e9e44", "#ef6c00",
               "#c62828", "#00838f", "#6d4c41", "#546e7a"]

ROLE_NODE = Qt.UserRole        # 树节点对应的 DirNode
ROLE_FILLED = Qt.UserRole + 1  # 子节点是否已懒加载
ROLE_SIZE = Qt.UserRole + 2    # 表格大小列的原始字节数（用于排序）

DELETE_WARN_BYTES = 10 * 1024 ** 3


class _SizeItem(QTableWidgetItem):
    def __lt__(self, other):
        return self.data(ROLE_SIZE) < other.data(ROLE_SIZE)


class DiskScanWorker(QThread):
    progress = Signal(str)
    done = Signal(object)   # ScanResult 或 None（异常）

    def __init__(self, path, parent=None):
        super().__init__(parent)
        self.path = path
        self._stop = threading.Event()

    def stop(self):
        self._stop.set()

    def run(self):
        try:
            res = diskusage.scan_tree(
                self.path, stop=self._stop,
                progress=lambda n, b, cur: self.progress.emit(
                    f"已扫描 {n:,} 个文件 / {human_bytes(b)}　{cur}"))
        except Exception:
            LOGGER.exception("磁盘扫描失败: {}", self.path)
            res = None
        self.done.emit(res)


class DeleteWorker(QThread):
    done = Signal(int, list)   # 成功数, 失败路径

    def __init__(self, paths, parent=None):
        super().__init__(parent)
        self.paths = paths

    def run(self):
        try:
            ok, errors = diskusage.delete_to_recycle_bin(self.paths)
        except Exception:
            LOGGER.exception("移入回收站失败")
            ok, errors = 0, list(self.paths)
        self.done.emit(ok, errors)


def _norm(path: str) -> str:
    return os.path.normcase(os.path.abspath(path))


def _is_subpath(path: str, ancestor: str) -> bool:
    p, a = _norm(path), _norm(ancestor).rstrip("\\/") + os.sep
    return p != a and p.startswith(a)


class DiskTab(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        self._result = None
        self._worker = None
        self._delete_worker = None
        self._scanned_path = ""   # 当前结果对应的路径（删除后重扫用它）
        self._pending_msg = ""    # 删除结果，等重扫完成后一并展示

        lay = QVBoxLayout(self)
        lay.setContentsMargins(8, 8, 8, 8)

        tip = QLabel("扫描磁盘占用，定位空间大头；删除仅移入回收站，可在系统回收站还原。")
        tip.setProperty("hint", True)
        lay.addWidget(tip)

        row = QHBoxLayout()
        self.combo = QComboBox()
        self.combo.setMinimumWidth(280)
        row.addWidget(self.combo)
        self.btn_scan = QPushButton("扫描")
        self.btn_scan.setProperty("primary", True)
        self.btn_scan.clicked.connect(self.scan)
        row.addWidget(self.btn_scan)
        self.btn_stop = QPushButton("停止")
        self.btn_stop.setEnabled(False)
        self.btn_stop.clicked.connect(self._stop_scan)
        row.addWidget(self.btn_stop)
        row.addStretch(1)
        lay.addLayout(row)

        self.pb = QProgressBar()
        self.pb.setRange(0, 0)
        self.pb.setTextVisible(False)
        self.pb.hide()
        lay.addWidget(self.pb)
        self.lbl_progress = QLabel(" ")
        lay.addWidget(self.lbl_progress)

        split = QSplitter()
        self.tree = QTreeWidget()
        self.tree.setHeaderLabels(["名称", "大小", "占比", "文件数"])
        self.tree.setColumnWidth(0, 240)
        self.tree.setColumnWidth(1, 80)
        self.tree.setColumnWidth(2, 60)
        self.tree.setColumnWidth(3, 60)
        self.tree.itemExpanded.connect(self._fill_children)
        split.addWidget(self.tree)

        right = QWidget()
        rlay = QVBoxLayout(right)
        rlay.setContentsMargins(0, 0, 0, 0)
        self.chart = BarChart("文件类型占用 Top 8", fmt=human_bytes)
        rlay.addWidget(self.chart)
        self.lbl_files = QLabel("大文件 Top 200（双击定位所在文件夹，选中行可删除）")
        self.lbl_files.setProperty("hint", True)
        rlay.addWidget(self.lbl_files)
        self.table = QTableWidget(0, 3)
        self.table.setHorizontalHeaderLabels(["文件", "大小", "修改时间"])
        self.table.verticalHeader().hide()
        self.table.setSelectionBehavior(QTableWidget.SelectRows)
        self.table.setEditTriggers(QTableWidget.NoEditTriggers)
        self.table.itemDoubleClicked.connect(self._reveal)
        self.table.horizontalHeader().setSectionResizeMode(0, QHeaderView.Stretch)
        self.table.setColumnWidth(1, 90)
        self.table.setColumnWidth(2, 130)
        rlay.addWidget(self.table)
        split.addWidget(right)
        split.setSizes([460, 460])
        lay.addWidget(split, 1)

        bottom = QHBoxLayout()
        self.btn_delete = QPushButton("删除选中项（进回收站）")
        self.btn_delete.setProperty("danger", True)
        self.btn_delete.clicked.connect(self.delete_checked)
        bottom.addWidget(self.btn_delete)
        bottom.addStretch(1)
        self.lbl_result = QLabel(" ")
        bottom.addWidget(self.lbl_result)
        lay.addLayout(bottom)

        self.refresh_drives()

    # ---- 扫描 ----

    def refresh_drives(self):
        try:
            drives = diskusage.list_drives()
        except Exception:
            LOGGER.exception("枚举磁盘失败")
            drives = []
        cur = self.combo.currentData()
        self.combo.clear()
        for d in drives:
            self.combo.addItem(
                f"{d.letter}　已用 {human_bytes(d.used)} / {human_bytes(d.total)}",
                d.mountpoint)
        if cur:
            i = self.combo.findData(cur)
            if i >= 0:
                self.combo.setCurrentIndex(i)
        idle = self._worker is None
        self.combo.setEnabled(idle and bool(drives))
        self.btn_scan.setEnabled(idle and bool(drives))

    def scan(self, path=None):
        if self._worker:
            return
        path = path or self.combo.currentData()
        if not path:
            QMessageBox.information(self, "SysKit", "未找到可用磁盘。")
            return
        self._scanned_path = path
        self.btn_scan.setEnabled(False)
        self.combo.setEnabled(False)
        self.btn_stop.setEnabled(True)
        self.btn_delete.setEnabled(False)
        self.pb.show()
        self.lbl_progress.setText(f"正在扫描 {path} …")
        self.lbl_result.setText(" ")
        self._worker = DiskScanWorker(path)
        self._worker.progress.connect(self.lbl_progress.setText)
        self._worker.done.connect(self._scan_done)
        self._worker.start()

    def _stop_scan(self):
        if self._worker:
            self.btn_stop.setEnabled(False)
            self._worker.stop()

    def _scan_done(self, res):
        self._worker = None
        self.pb.hide()
        self.btn_stop.setEnabled(False)
        self.refresh_drives()
        self.btn_delete.setEnabled(True)
        if res is None:
            self._pending_msg = ""
            self.lbl_progress.setText("扫描失败，详见日志。")
            return
        self._result = res
        msg = f"扫描完成：{res.file_count:,} 个文件，耗时 {res.duration:.1f} 秒"
        if res.error_count:
            msg += f"，跳过 {res.error_count:,} 个（无权限/被占用）"
        if res.cancelled:
            msg += "（已停止，结果不完整）"
        if self._pending_msg:
            msg = f"{self._pending_msg}；{msg}"
            self._pending_msg = ""
        self.lbl_result.setText(msg)
        self.lbl_result.setStyleSheet("color:#2e9e44;")
        self._render()

    # ---- 渲染 ----

    def _render(self):
        res = self._result
        self.tree.clear()
        root_item = self._make_item(res.root, res.root.total)
        self.tree.addTopLevelItem(root_item)
        root_item.setExpanded(True)

        top = sorted(res.ext_stats.items(), key=lambda kv: -kv[1])[:8]
        self.chart.set_bars([
            (key, size, _BAR_COLORS[i % len(_BAR_COLORS)])
            for i, (key, size) in enumerate(top)])

        self.table.setSortingEnabled(False)
        self.table.setRowCount(len(res.top_files))
        for r, (size, path, mtime) in enumerate(res.top_files):
            name = QTableWidgetItem(os.path.basename(path))
            name.setData(Qt.UserRole, path)
            name.setToolTip(path)
            it_size = _SizeItem(human_bytes(size))
            it_size.setData(ROLE_SIZE, size)
            it_size.setTextAlignment(Qt.AlignRight | Qt.AlignVCenter)
            it_time = QTableWidgetItem(time.strftime("%Y-%m-%d %H:%M", time.localtime(mtime)))
            self.table.setItem(r, 0, name)
            self.table.setItem(r, 1, it_size)
            self.table.setItem(r, 2, it_time)
        self.table.setSortingEnabled(True)
        self.table.sortItems(1, Qt.DescendingOrder)

    def _make_item(self, node, root_total):
        item = QTreeWidgetItem()
        item.setText(0, node.name)
        item.setText(1, human_bytes(node.total))
        item.setText(2, f"{node.total / root_total * 100:.1f}%" if root_total else "-")
        item.setText(3, f"{node.file_count:,}")
        item.setData(0, ROLE_NODE, node)
        item.setFlags(Qt.ItemIsEnabled | Qt.ItemIsSelectable | Qt.ItemIsUserCheckable)
        item.setCheckState(0, Qt.Unchecked)
        if node.children:
            item.addChild(QTreeWidgetItem())   # 占位，展开时替换
        return item

    def _fill_children(self, item):
        if item.data(0, ROLE_FILLED) or not self._result:
            return
        node = item.data(0, ROLE_NODE)
        item.takeChildren()
        for child in sorted(node.children, key=lambda c: -c.total):
            item.addChild(self._make_item(child, self._result.root.total))
        item.setData(0, ROLE_FILLED, True)

    # ---- 删除 ----

    def _collect(self):
        """返回 [(path, size)]：目录树勾选项 + 大文件表选中行；父子同时勾选时只保留祖先。"""
        sel = []
        root_path = self._result.root.path

        def walk(item):
            for i in range(item.childCount()):
                walk(item.child(i))
            if item.checkState(0) == Qt.Checked:
                node = item.data(0, ROLE_NODE)
                if _norm(node.path) != _norm(root_path):
                    sel.append((node.path, node.total))

        for i in range(self.tree.topLevelItemCount()):
            walk(self.tree.topLevelItem(i))
        for row in range(self.table.rowCount()):
            if self.table.item(row, 0).isSelected():
                path = self.table.item(row, 0).data(Qt.UserRole)
                sel.append((path, self.table.item(row, 1).data(ROLE_SIZE) or 0))

        kept = []
        for path, size in sorted(sel, key=lambda s: len(s[0])):
            if any(_is_subpath(path, kp) for kp, _s in kept):
                continue
            kept.append((path, size))
        return kept

    def delete_checked(self):
        if self._delete_worker or not self._result:
            return
        sel = self._collect()
        if not sel:
            QMessageBox.information(self, "SysKit",
                                    "请先在目录树勾选要删除的项，或在下方大文件列表选中行。")
            return
        total = sum(s for _p, s in sel)
        msg = f"将把 {len(sel)} 项（共 {human_bytes(total)}）移入回收站。\n\n" \
              "删除后可在系统回收站还原。确定继续？"
        if total > DELETE_WARN_BYTES:
            msg = "⚠ 选中内容超过 10 GB。\n\n" + msg
        if QMessageBox.question(self, "确认删除", msg) != QMessageBox.Yes:
            return
        self.btn_delete.setEnabled(False)
        self.lbl_result.setText("正在移入回收站…")
        self._delete_worker = DeleteWorker([p for p, _s in sel])
        self._delete_worker.done.connect(self._delete_done)
        self._delete_worker.start()

    def _delete_done(self, ok, errors):
        self._delete_worker = None
        self.btn_delete.setEnabled(True)
        msg = f"已删除 {ok} 项"
        if errors:
            msg += f"；{len(errors)} 个失败（可能无权限，见日志）"
            LOGGER.warning("回收站删除失败: {}", errors[:5])
        else:
            LOGGER.info("磁盘清理: 移入回收站 {} 项", ok)
        if ok:
            self._pending_msg = msg
            self.scan(self._scanned_path)   # 重扫刷新所有视图（与垃圾清理 Tab 一致）
        else:
            self.lbl_result.setText(msg)
            self.lbl_result.setStyleSheet("color:#ef6c00;")

    # ---- 其它 ----

    def _reveal(self, item):
        path = item.data(Qt.UserRole)
        if path and os.path.exists(path):
            subprocess.Popen(["explorer", "/select,", os.path.normpath(path)])
