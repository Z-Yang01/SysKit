"""WiFi 工具集：周边 AP 扫描 / 信道占用分析 / 二维码分享 / 已存密码查看。"""
import io
import re
import time

from PySide6.QtCore import Qt, QThread, QTimer, Signal
from PySide6.QtGui import QColor, QPixmap
from PySide6.QtWidgets import (QCheckBox, QComboBox, QFormLayout,
                               QHBoxLayout, QHeaderView, QLabel,
                               QMessageBox, QPushButton, QTableWidgetItem,
                               QTabWidget, QVBoxLayout, QWidget)

from .charts import BarChart
from . import theme
from .theme import make_table
from .workers import reap
from .. import admin, proc


def _signal_color(pct: int) -> str:
    """信号强度语义色（现取 pal——深浅主题各自可读，补57）。"""
    pal = theme.pal()
    if pct >= 60:
        return pal["ok"]
    if pct >= 40:
        return pal["warn"]
    return pal["err"]


def scan_networks() -> list:
    """解析 netsh wlan show networks mode=bssid → [{ssid,bssid,signal,channel,radio,auth}]。

    需要系统「位置」服务开启（Win11 对 WLAN 枚举的隐私要求）。
    """
    r = proc.run(["netsh", "wlan", "show", "networks", "mode=bssid"], timeout=20)
    if r.code != 0 and ("位置" in r.stdout or "Location" in r.stdout):
        raise RuntimeError("需要开启系统「位置」服务才能扫描 WiFi（设置→隐私和安全性→位置）")
    aps, cur, net_auth = [], None, ""
    for line in r.stdout.splitlines():
        # SSID 名在带索引行上（真实 netsh 输出：`SSID 1 : MyHomeWiFi`；
        # `show networks` 输出不存在裸 `SSID:` 行——那来自 show interfaces）。
        # 隐藏网络的 SSID 行值为空 → cur 为空串，落到 `(隐藏)` 兜底。
        m = re.match(r"\s*SSID\s+\d+\s*[:：]\s*(.*)", line)
        if m:
            cur = m.group(1).strip()
            net_auth = ""
            continue
        m = re.match(r"\s*SSID\s*[:：]\s*(.*)", line)   # 兼容异常/旧格式，正常不命中
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
            if isinstance(cur, dict) and not cur["auth"]:
                cur["auth"] = net_auth      # 认证行在 BSSID 之后时补上（限当前网络）
    for ap in aps:
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


class _ProfilesWorker(QThread):
    """WLAN 配置文件枚举（netsh，可达数百 ms）：后台取，回 UI 填下拉。"""
    done = Signal(list)

    def run(self):
        from ..repairs import wlan_profiles
        try:
            self.done.emit(wlan_profiles())
        except Exception:
            self.done.emit([])


class _ProfilePwdWorker(QThread):
    """读取配置文件明文密码（netsh key=clear，超时 15s）：后台线程执行。"""
    done = Signal(str, str, str)   # (profile, auth, password)——带名字防错配

    def __init__(self, name, parent=None):
        super().__init__(parent)
        self._name = name

    def run(self):
        try:
            auth, pwd = profile_password(self._name)
        except Exception:
            auth, pwd = "", ""
        self.done.emit(self._name, auth, pwd)


class WifiPage(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        self._aps = []
        self._scan_worker = None
        self._pw_worker = None
        self._share_worker = None
        self._qr_worker = None
        self._initial_scanned = False
        self._resume_auto = False
        lay = QVBoxLayout(self)
        lay.setContentsMargins(0, 0, 0, 0)
        tabs = QTabWidget()
        tabs.addTab(self._build_scan(), "周边 AP")
        tabs.addTab(self._build_channels(), "信道分析")
        tabs.addTab(self._build_share(), "二维码分享")
        tabs.addTab(self._build_passwords(), "已存密码")
        lay.addWidget(tabs)
        self._scan_timer = QTimer(self)
        self._scan_timer.timeout.connect(self._auto_scan_tick)
        # 不在构造时扫描：主窗体会一次性建好全部页，避免用户从未打开 WiFi 页也跑 netsh

    def showEvent(self, event):
        super().showEvent(event)
        if not self._initial_scanned:
            self._initial_scanned = True
            self.scan()
            self._refresh_share_profiles()   # netsh 配置文件枚举同样延迟到首显
        if self._resume_auto and self.chk_auto.isChecked():
            self._scan_timer.start(8000)
            self._resume_auto = False

    def hideEvent(self, event):
        super().hideEvent(event)
        # 页面不可见时停掉自动扫描，省 netsh/CPU
        if self._scan_timer.isActive():
            self._scan_timer.stop()
            self._resume_auto = True

    def _auto_scan_tick(self):
        if not self.isVisible():
            return
        self.scan(auto=True)

    def _on_auto_toggled(self, on):
        if on and self.isVisible():
            self._scan_timer.start(8000)
        elif not on:
            self._scan_timer.stop()

    # ---- 周边 AP ----
    def _build_scan(self):
        w = QWidget()
        lay = QVBoxLayout(w)
        row = QHBoxLayout()
        self.btn_scan = QPushButton("扫描")
        self.btn_scan.clicked.connect(lambda: self.scan())
        self.chk_auto = QCheckBox("每 8 秒自动刷新")
        self.chk_auto.toggled.connect(self._on_auto_toggled)
        row.addWidget(self.btn_scan)
        row.addWidget(self.chk_auto)
        row.addStretch(1)
        self.lbl_scan = QLabel(" ")
        self.lbl_scan.setProperty("hint", True)
        row.addWidget(self.lbl_scan)
        lay.addLayout(row)

        self.tbl_aps = make_table(["SSID", "BSSID", "信号", "信道", "频段", "认证"],
                                  select_rows=True)
        self.tbl_aps.horizontalHeader().setSectionResizeMode(0, QHeaderView.Stretch)
        lay.addWidget(self.tbl_aps)

        self.lbl_loc = QLabel("提示：Win11 扫描周边 WiFi 需要系统「位置」服务开启。")
        self.lbl_loc.setProperty("hint", True)
        lay.addWidget(self.lbl_loc)
        return w

    def scan(self, auto=False):
        if self._scan_worker:
            return
        self.btn_scan.setEnabled(False)
        if not auto:
            self.lbl_scan.setText("扫描中…")
        self._scan_worker = reap(ScanWorker(), self)
        self._scan_worker.done.connect(self._scan_done)
        self._scan_worker.failed.connect(self._scan_failed)
        self._scan_worker.start()

    def _scan_done(self, aps):
        self._scan_worker = None
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
        self._scan_worker = None
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
        self.lbl_advice.setStyleSheet(f"color:{theme.pal()['accent']}; font-weight:bold;")
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
            # @role 语义色：BarChart 绘制时现取 pal，切主题自动跟随（补57）
            return [(ch, len(counter[ch]), "@accent" if len(counter[ch]) <= 2
                     else "@warn" if len(counter[ch]) <= 4 else "@err")
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
        # 白底保留（二维码可扫性），边框走主题 token：深色下不再是亮白硬边框
        self.lbl_qr.setStyleSheet(f"background:#fff; border:1px solid {theme.pal()['border']};"
                                  "border-radius:8px;")
        lay.addWidget(self.lbl_qr)
        # 配置文件枚举（netsh）不在构造期执行：由 showEvent 首访触发
        return w

    def _refresh_share_profiles(self):
        """配置文件枚举走后台线程（netsh 同步可达数百 ms，不在 showEvent 里冻 UI）。"""
        if self._share_worker is not None:
            return
        self._share_worker = reap(_ProfilesWorker(), self)
        self._share_worker.done.connect(self._share_profiles_done)
        self._share_worker.start()

    def _share_profiles_done(self, names):
        self._share_worker = None
        self.cmb_share.clear()
        self.cmb_share.addItems(names)

    def _gen_qr(self):
        name = self.cmb_share.currentText()
        if not name or self._qr_worker is not None:
            return
        # 读取明文密码（netsh key=clear，超时 15s）走后台线程
        self._qr_worker = reap(_ProfilePwdWorker(name), self)
        self._qr_worker.done.connect(self._qr_ready)
        self._qr_worker.start()

    def _qr_ready(self, profile, auth, pwd):
        self._qr_worker = None
        # 用发起时的 profile 名渲染：完成后用户可能已切走下拉，
        # 若仍用 currentText 会生成「A 的 SSID + B 的密码」错配二维码
        if self.cmb_share.currentText() != profile:
            self.lbl_qr.setText(f"{profile} 的二维码已过期（下拉已切换），请重新生成")
            self.lbl_qr.setStyleSheet("")       # 覆盖白底样式恢复默认 hint 观感
            self.lbl_qr.setAlignment(Qt.AlignCenter)
            self.btn_save_qr.setEnabled(False)
            self._qr_png = None
            return
        name = profile
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
            QMessageBox.warning(self, "SysKit", f"二维码生成失败: {e}")

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
        self.chk_show_pw = QCheckBox("显示密码")
        self.chk_show_pw.toggled.connect(self._toggle_pw_mask)
        row.addWidget(self.btn_pw)
        row.addWidget(self.btn_copy)
        row.addWidget(self.chk_show_pw)
        row.addStretch(1)
        self.lbl_pw = QLabel(" ")
        self.lbl_pw.setProperty("hint", True)
        row.addWidget(self.lbl_pw)
        lay.addLayout(row)

        self.tbl_pw = make_table(["名称", "认证", "密码"])
        self._pw_rows = []               # 明文缓存（掩码/显示切换用）
        self.tbl_pw.horizontalHeader().setSectionResizeMode(0, QHeaderView.Stretch)
        lay.addWidget(self.tbl_pw)
        return w

    def _load_passwords(self):
        if self._pw_worker:
            return
        if not admin.is_admin():
            QMessageBox.warning(self, "SysKit", "读取密码需要管理员权限（key=clear 接口限制）。")
            return
        self.btn_pw.setEnabled(False)
        self.lbl_pw.setText("读取中…")
        self._pw_worker = reap(_PasswordWorker(), self)
        self._pw_worker.done.connect(self._pw_done)
        self._pw_worker.start()

    def _pw_done(self, rows):
        self._pw_worker = None
        self.btn_pw.setEnabled(True)
        self._pw_rows = list(rows)
        self._reload_pw_table()
        self.lbl_pw.setText(f"共 {len(rows)} 个配置文件（密码默认掩码，勾选「显示密码」查看）")

    def _reload_pw_table(self):
        rows = self._pw_rows
        self.tbl_pw.setRowCount(len(rows))
        for i, (name, auth, pwd) in enumerate(rows):
            shown = "••••••••" if not self.chk_show_pw.isChecked() else (pwd or "(开放网络)")
            for c, v in enumerate([name, auth, shown]):
                self.tbl_pw.setItem(i, c, QTableWidgetItem(v))

    def _toggle_pw_mask(self):
        self._reload_pw_table()

    def _copy_password(self):
        row = self.tbl_pw.currentRow()
        if row < 0 or row >= len(self._pw_rows):
            return
        from PySide6.QtWidgets import QApplication
        from .. import clipboard as clipboard_mod
        pwd = self._pw_rows[row][2] or ""
        clipboard_mod.mark_sensitive(pwd)      # WLAN 密码同走敏感路径：不进历史
        QApplication.clipboard().setText(pwd)
        clipboard_mod.exclude_os_history()     # 不进 Win+V 历史 / 不上云
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
