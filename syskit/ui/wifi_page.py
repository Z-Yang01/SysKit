"""WiFi 工具集：周边 AP 扫描 / 信道占用分析 / 二维码分享 / 已存密码查看。"""
import io
import re
import time

from PySide6.QtCore import Qt, QThread, QTimer, Signal
from PySide6.QtGui import QColor, QPixmap
from PySide6.QtWidgets import (QAbstractItemView, QCheckBox, QComboBox, QFormLayout,
                               QGroupBox, QHBoxLayout, QHeaderView, QLabel,
                               QMessageBox, QPushButton, QTableWidget, QTableWidgetItem,
                               QTabWidget, QVBoxLayout, QWidget)

from .charts import BarChart
from .. import admin, proc
from ..logger import LOGGER

SIGNAL_COLORS = [(60, "#2e9e44"), (40, "#ef6c00"), (0, "#c62828")]


def _signal_color(pct: int) -> str:
    for floor, color in SIGNAL_COLORS:
        if pct >= floor:
            return color
    return "#c62828"


def scan_networks() -> list:
    """解析 netsh wlan show networks mode=bssid → [{ssid,bssid,signal,channel,radio,auth}]。

    需要系统「位置」服务开启（Win11 对 WLAN 枚举的隐私要求）。
    """
    r = proc.run(["netsh", "wlan", "show", "networks", "mode=bssid"], timeout=20)
    if r.code != 0 and ("位置" in r.stdout or "Location" in r.stdout):
        raise RuntimeError("需要开启系统「位置」服务才能扫描 WiFi（设置→隐私和安全性→位置）")
    aps, cur, net_auth = [], None, ""
    for line in r.stdout.splitlines():
        if re.match(r"\s*SSID\s+\d+\s*[:：]", line):
            continue
        m = re.match(r"\s*SSID\s*[:：]\s*(.*)", line)
        if m:
            cur = m.group(1).strip()
            net_auth = ""
            continue
        m = re.match(r"\s*BSSID\s+\d*\s*[:：]\s*(\S+)", line)
        if m:
            cur = {
                "ssid": cur or "(隐藏)",
                "bssid": m.group(1),
                "signal": 0, "channel": 0, "radio": "", "auth": net_auth,
            }
            aps.append(cur)
            continue
        m = re.match(r"\s*(?:信号|Signal)\s*[:：]\s*(\d+)%?", line)
        if m and isinstance(cur, dict):
            cur["signal"] = int(m.group(1))
            continue
        m = re.match(r"\s*(?:信道|Channel)\s*[:：]\s*(\d+)", line)
        if m and isinstance(cur, dict):
            cur["channel"] = int(m.group(1))
            continue
        m = re.match(r"\s*(?:无线电类型|Radio type)\s*[:：]\s*(\S+)", line)
        if m and isinstance(cur, dict):
            cur["radio"] = m.group(1)
            continue
        m = re.match(r"\s*(?:身份验证|Authentication)\s*[:：]\s*(\S+)", line)
        if m:
            net_auth = m.group(1)
    for ap in aps:
        if not ap["auth"]:
            ap["auth"] = net_auth
        ch = ap["channel"]
        ap["band"] = "2.4G" if 0 < ch <= 14 else ("5G" if ch > 14 else "?")
    aps.sort(key=lambda a: -a["signal"])
    return aps


def profile_password(name: str):
    """(认证类型, 明文密码)。需管理员才能取回密码。"""
    r = proc.run(["netsh", "wlan", "show", "profile", f"name={name}", "key=clear"], timeout=15)
    text = r.stdout
    auth = ""
    m = re.search(r"(?:身份验证|Authentication)\s*[:：]\s*(\S+)", text)
    if m:
        auth = m.group(1)
    pwd = ""
    m = re.search(r"(?:关键内容|Key Content)\s*[:：]\s*(.+)", text)
    if m:
        pwd = m.group(1).strip()
    return auth, pwd


class ScanWorker(QThread):
    done = Signal(list)
    failed = Signal(str)

    def run(self):
        try:
            self.done.emit(scan_networks())
        except Exception as e:
            self.failed.emit(str(e))


class WifiPage(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        self._aps = []
        self._worker = None
        lay = QVBoxLayout(self)
        lay.setContentsMargins(0, 0, 0, 0)
        tabs = QTabWidget()
        tabs.addTab(self._build_scan(), "周边 AP")
        tabs.addTab(self._build_channels(), "信道分析")
        tabs.addTab(self._build_share(), "二维码分享")
        tabs.addTab(self._build_passwords(), "已存密码")
        lay.addWidget(tabs)
        self._scan_timer = QTimer(self)
        self._scan_timer.timeout.connect(lambda: self.scan(auto=True))
        self.scan()

    # ---- 周边 AP ----
    def _build_scan(self):
        w = QWidget()
        lay = QVBoxLayout(w)
        row = QHBoxLayout()
        self.btn_scan = QPushButton("扫描")
        self.btn_scan.clicked.connect(lambda: self.scan())
        self.chk_auto = QCheckBox("每 8 秒自动刷新")
        self.chk_auto.toggled.connect(
            lambda on: self._scan_timer.start(8000) if on else self._scan_timer.stop())
        row.addWidget(self.btn_scan)
        row.addWidget(self.chk_auto)
        row.addStretch(1)
        self.lbl_scan = QLabel(" ")
        self.lbl_scan.setStyleSheet("color:#8a93a0;")
        row.addWidget(self.lbl_scan)
        lay.addLayout(row)

        self.tbl_aps = QTableWidget(0, 6)
        self.tbl_aps.setHorizontalHeaderLabels(["SSID", "BSSID", "信号", "信道", "频段", "认证"])
        self.tbl_aps.horizontalHeader().setSectionResizeMode(0, QHeaderView.Stretch)
        self.tbl_aps.setEditTriggers(QAbstractItemView.NoEditTriggers)
        self.tbl_aps.setSelectionBehavior(QAbstractItemView.SelectRows)
        self.tbl_aps.setAlternatingRowColors(True)
        self.tbl_aps.verticalHeader().setVisible(False)
        lay.addWidget(self.tbl_aps)

        self.lbl_loc = QLabel("提示：Win11 扫描周边 WiFi 需要系统「位置」服务开启。")
        self.lbl_loc.setStyleSheet("color:#8a93a0;")
        lay.addWidget(self.lbl_loc)
        return w

    def scan(self, auto=False):
        if self._worker:
            return
        self.btn_scan.setEnabled(False)
        if not auto:
            self.lbl_scan.setText("扫描中…")
        self._worker = ScanWorker()
        self._worker.done.connect(self._scan_done)
        self._worker.failed.connect(self._scan_failed)
        self._worker.start()

    def _scan_done(self, aps):
        self._worker = None
        self.btn_scan.setEnabled(True)
        self._aps = aps
        self.lbl_scan.setText(f"共发现 {len(aps)} 个 BSSID　{time.strftime('%H:%M:%S')}")
        self.tbl_aps.setRowCount(len(aps))
        for i, ap in enumerate(aps):
            color = QColor(_signal_color(ap["signal"]))
            vals = [ap["ssid"], ap["bssid"], f"{ap['signal']}%", str(ap["channel"] or "—"),
                    ap["band"], ap["auth"]]
            for c, v in enumerate(vals):
                item = QTableWidgetItem(v)
                if c == 2:
                    item.setForeground(QColor(color))
                self.tbl_aps.setItem(i, c, item)
        self._update_channel_charts()

    def _scan_failed(self, msg):
        self._worker = None
        self.btn_scan.setEnabled(True)
        self.lbl_scan.setText(f"扫描失败：{msg}")

    # ---- 信道分析 ----
    def _build_channels(self):
        w = QWidget()
        lay = QVBoxLayout(w)
        self.chart_24 = BarChart("2.4GHz 信道占用（AP 数量）")
        self.chart_5g = BarChart("5GHz 信道占用（AP 数量）")
        lay.addWidget(self.chart_24)
        lay.addWidget(self.chart_5g)
        self.lbl_advice = QLabel("扫描后显示推荐信道。")
        self.lbl_advice.setStyleSheet("color:#3576e0; font-weight:bold;")
        lay.addWidget(self.lbl_advice)
        return w

    def _update_channel_charts(self):
        c24, c5 = {}, {}
        for ap in self._aps:
            if not ap["channel"]:
                continue
            bucket = c24 if ap["band"] == "2.4G" else c5
            bucket.setdefault(ap["channel"], []).append(ap["signal"])
        def bars(counter, channels):
            return [(ch, len(counter[ch]), "#3576e0" if len(counter[ch]) <= 2
                     else "#ef6c00" if len(counter[ch]) <= 4 else "#c62828")
                    for ch in channels if ch in counter]
        self.chart_24.set_bars(bars(c24, list(range(1, 14))))
        self.chart_5g.set_bars(bars(c5, sorted(c5)))
        # 推荐信道
        best24 = min((c for c in (1, 6, 11)), key=lambda c: len(c24.get(c, [])))
        n24 = len(c24.get(best24, []))
        if c5:
            best5 = min(c5, key=lambda c: len(c5[c]))
            advice = f"推荐：2.4G 用信道 {best24}（当前 {n24} 个 AP）；5G 用信道 {best5}（当前 {len(c5[best5])} 个 AP）"
        else:
            advice = f"推荐：2.4G 用信道 {best24}（当前 {n24} 个 AP）；未发现 5G 网络"
        self.lbl_advice.setText(advice)

    # ---- 二维码分享 ----
    def _build_share(self):
        w = QWidget()
        lay = QHBoxLayout(w)
        left = QFormLayout()
        self.cmb_share = QComboBox()
        self.cmb_share.setMinimumWidth(220)
        btn_gen = QPushButton("生成二维码")
        btn_gen.setProperty("primary", True)
        btn_gen.clicked.connect(self._gen_qr)
        self.btn_save_qr = QPushButton("保存图片")
        self.btn_save_qr.setEnabled(False)
        self.btn_save_qr.clicked.connect(self._save_qr)
        left.addRow("WiFi 配置文件:", self.cmb_share)
        left.addRow(btn_gen)
        left.addRow(self.btn_save_qr)
        left_lay = QVBoxLayout()
        left_lay.addLayout(left)
        left_lay.addStretch(1)
        lay.addLayout(left_lay, 1)

        self.lbl_qr = QLabel("生成后显示在此")
        self.lbl_qr.setAlignment(Qt.AlignCenter)
        self.lbl_qr.setMinimumSize(240, 240)
        self.lbl_qr.setStyleSheet("background:#fff; border:1px solid #e3e8ef; border-radius:8px;")
        lay.addWidget(self.lbl_qr)
        self._refresh_share_profiles()
        return w

    def _refresh_share_profiles(self):
        from ..repairs import wlan_profiles
        self.cmb_share.clear()
        self.cmb_share.addItems(wlan_profiles())

    def _gen_qr(self):
        name = self.cmb_share.currentText()
        if not name:
            return
        auth, pwd = profile_password(name)
        auth_type = "nopass"
        if auth:
            a = auth.upper()
            auth_type = "WEP" if "WEP" in a else "SAE" if "WPA3" in a else \
                "WPA" if ("WPA" in a or "WPA2" in a) else "nopass"
        ssid_esc = name.replace("\\", "\\\\").replace(";", "\\;").replace(",", "\\,") \
            .replace(":", "\\:").replace('"', '\\"')
        pwd_esc = pwd.replace("\\", "\\\\").replace(";", "\\;").replace(",", "\\,") \
            .replace(":", "\\:").replace('"', '\\"')
        data = f"WIFI:T:{auth_type};S:{ssid_esc};"
        if auth_type != "nopass":
            data += f"P:{pwd_esc};"
        data += ";"
        try:
            import qrcode
            img = qrcode.make(data, box_size=8, border=2)
            buf = io.BytesIO()
            img.save(buf, format="PNG")
            pix = QPixmap()
            pix.loadFromData(buf.getvalue(), "PNG")
            self.lbl_qr.setPixmap(pix.scaled(240, 240, Qt.KeepAspectRatio, Qt.SmoothTransformation))
            self._qr_png = buf.getvalue()
            self.btn_save_qr.setEnabled(True)
            self.lbl_qr.setToolTip(data)
        except Exception as e:
            QMessageBox.warning(self, "NetTool", f"二维码生成失败: {e}")

    def _save_qr(self):
        if not getattr(self, "_qr_png", None):
            return
        from PySide6.QtWidgets import QFileDialog
        path, _ = QFileDialog.getSaveFileName(self, "保存二维码", "wifi_qr.png", "PNG (*.png)")
        if path:
            with open(path, "wb") as f:
                f.write(self._qr_png)

    # ---- 已存密码 ----
    def _build_passwords(self):
        w = QWidget()
        lay = QVBoxLayout(w)
        row = QHBoxLayout()
        self.btn_pw = QPushButton("读取全部已存密码")
        self.btn_pw.setProperty("primary", True)
        self.btn_pw.clicked.connect(self._load_passwords)
        self.btn_copy = QPushButton("复制选中密码")
        self.btn_copy.clicked.connect(self._copy_password)
        row.addWidget(self.btn_pw)
        row.addWidget(self.btn_copy)
        row.addStretch(1)
        self.lbl_pw = QLabel(" ")
        self.lbl_pw.setStyleSheet("color:#8a93a0;")
        row.addWidget(self.lbl_pw)
        lay.addLayout(row)

        self.tbl_pw = QTableWidget(0, 3)
        self.tbl_pw.setHorizontalHeaderLabels(["名称", "认证", "密码"])
        self.tbl_pw.horizontalHeader().setSectionResizeMode(0, QHeaderView.Stretch)
        self.tbl_pw.setEditTriggers(QAbstractItemView.NoEditTriggers)
        self.tbl_pw.setAlternatingRowColors(True)
        self.tbl_pw.verticalHeader().setVisible(False)
        lay.addWidget(self.tbl_pw)
        return w

    def _load_passwords(self):
        if self._worker:
            return
        if not admin.is_admin():
            QMessageBox.warning(self, "NetTool", "读取密码需要管理员权限（key=clear 接口限制）。")
            return
        self.btn_pw.setEnabled(False)
        self.lbl_pw.setText("读取中…")
        self._worker = _PasswordWorker()
        self._worker.done.connect(self._pw_done)
        self._worker.start()

    def _pw_done(self, rows):
        self._worker = None
        self.btn_pw.setEnabled(True)
        self.tbl_pw.setRowCount(len(rows))
        for i, (name, auth, pwd) in enumerate(rows):
            for c, v in enumerate([name, auth, pwd or "(开放网络)"]):
                self.tbl_pw.setItem(i, c, QTableWidgetItem(v))
        self.lbl_pw.setText(f"共 {len(rows)} 个配置文件")

    def _copy_password(self):
        row = self.tbl_pw.currentRow()
        if row < 0:
            return
        from PySide6.QtWidgets import QApplication
        QApplication.clipboard().setText(self.tbl_pw.item(row, 2).text())
        self.lbl_pw.setText("密码已复制")


class _PasswordWorker(QThread):
    done = Signal(list)

    def run(self):
        from ..repairs import wlan_profiles
        rows = []
        for name in wlan_profiles():
            try:
                auth, pwd = profile_password(name)
                rows.append((name, auth, pwd))
            except Exception:
                rows.append((name, "?", "?"))
        self.done.emit(rows)
