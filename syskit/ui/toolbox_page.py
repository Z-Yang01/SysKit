"""工具箱：速度测试 / 局域网设备发现 / DNS查询与子网计算 / 持续ping与路由追踪 / 端口占用 / Hosts 编辑。"""
import ipaddress
import os
import re
import socket
import subprocess
import threading
import time
import urllib.request
from concurrent.futures import ThreadPoolExecutor

import psutil
from PySide6.QtCore import Qt, QThread, QTimer, Signal
from PySide6.QtGui import QColor
from PySide6.QtWidgets import (QAbstractItemView, QButtonGroup, QCheckBox, QComboBox, QFormLayout,
                               QGroupBox, QHBoxLayout, QHeaderView, QLabel, QLineEdit,
                               QListWidget, QMessageBox, QProgressBar, QPushButton, QRadioButton,
                               QSpinBox, QTableWidget, QTableWidgetItem, QTabWidget,
                               QVBoxLayout, QWidget)

from .charts import LineChart, human_bytes
from .. import admin, hostsed, icmp, dnsquery
from ..logger import LOGGER

TEST_URLS = [
    "https://dldir1.qq.com/weixin/Windows/WeChatSetup.exe",
    "https://dldir1.qq.com/qqfile/qq/QQNT/Windows/QQ_9.9.9_250avis_x64.exe",
    "https://mirrors.aliyun.com/debian-cd/current/amd64/iso-cd/debian-12.9.0-amd64-netinst.iso",
]


class SpeedTestWorker(QThread):
    progress = Signal(float, float)   # instant_Bps, total_MB
    done = Signal(bool, float)

    def __init__(self, seconds=8, parent=None):
        super().__init__(parent)
        self.seconds = seconds

    def run(self):
        for url in TEST_URLS:
            # 每个 URL 独立计数/计时：避免跨 URL 累计字节导致速率高估
            stop = threading.Event()
            counter = {"bytes": 0}
            req = urllib.request.Request(url, headers={"User-Agent": "SysKit/1.0"})
            try:
                resp = urllib.request.urlopen(req, timeout=10)
            except Exception:
                continue
            start = time.time()

            def measure():
                last_t, last_b = start, 0
                while not stop.is_set():
                    time.sleep(1)
                    now = time.time()
                    counter["instant"] = (counter["bytes"] - last_b) / max(0.001, now - last_t)
                    last_t, last_b = now, counter["bytes"]
                    self.progress.emit(counter["instant"], counter["bytes"] / 2**20)
                    if now - start >= self.seconds:
                        stop.set()

            threading.Thread(target=measure, daemon=True).start()
            try:
                while not stop.is_set():
                    chunk = resp.read(65536)
                    if not chunk:
                        break
                    counter["bytes"] += len(chunk)
            except Exception:
                pass
            finally:
                resp.close()
                stop.set()
            elapsed = time.time() - start
            if counter["bytes"] > 1024 * 512:
                self.done.emit(True, counter["bytes"] / max(0.001, elapsed))
                return
        self.done.emit(False, 0.0)


class SpeedTestTab(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        lay = QVBoxLayout(self)
        row = QHBoxLayout()
        self.btn_start = QPushButton("开始测速")
        self.btn_start.setProperty("primary", True)
        self.btn_start.clicked.connect(self.start)
        row.addWidget(self.btn_start)
        self.lbl_status = QLabel("点击开始，将下载约几十 MB 流量")
        self.lbl_status.setProperty("hint", True)
        row.addWidget(self.lbl_status, 1)
        lay.addLayout(row)

        self.pb = QProgressBar()
        self.pb.setRange(0, 0)
        self.pb.setTextVisible(False)
        self.pb.hide()
        lay.addWidget(self.pb)

        self.lbl_big = QLabel("— Mbps")
        f = self.lbl_big.font()
        f.setPointSize(20)
        f.setBold(True)
        self.lbl_big.setAlignment(Qt.AlignCenter)
        lay.addWidget(self.lbl_big)

        hist_grp = QGroupBox("历史结果（本次运行）")
        hlay = QVBoxLayout(hist_grp)
        self.list_hist = QListWidget()
        hlay.addWidget(self.list_hist)
        lay.addWidget(hist_grp, 1)
        self._worker = None

    def start(self):
        if self._worker:
            return
        self.btn_start.setEnabled(False)
        self.pb.show()
        self.lbl_status.setText("测速中…")
        self._worker = SpeedTestWorker()
        self._worker.progress.connect(self._on_progress)
        self._worker.done.connect(self._on_done)
        self._worker.start()

    def _on_progress(self, instant_bps, total_mb):
        self.lbl_status.setText(f"实时 {human_bytes(instant_bps)}/s　已下载 {total_mb:.1f} MB")

    def _on_done(self, ok, avg_bps):
        self._worker = None
        self.btn_start.setEnabled(True)
        self.pb.hide()
        mbps = avg_bps * 8 / 2**20
        if ok:
            self.lbl_big.setText(f"{mbps:.1f} Mbps（{human_bytes(avg_bps)}/s）")
            self.list_hist.insertItem(0, f"{time.strftime('%H:%M:%S')}　{mbps:.1f} Mbps")
            LOGGER.info("速度测试完成: {} Mbps", f"{mbps:.1f}")
        else:
            self.lbl_big.setText("测速失败")
            self.lbl_status.setText("所有测速源均不可达，请检查网络")


# ---------------- 局域网设备发现 ----------------

class _LanScanWorker(QThread):
    done = Signal(list)

    def run(self):
        # 无论如何都要发 done：否则 LanTab._worker 永不复位，「扫描局域网」按钮永久禁用
        try:
            rows = self._scan()
        except Exception:
            LOGGER.exception("局域网扫描失败")
            rows = []
        self.done.emit(rows)

    def _scan(self) -> list:
        subnet = None
        for addr in psutil.net_if_addrs().values():
            for a in addr:
                if a.family == socket.AF_INET and a.address and not a.address.startswith("127.") \
                        and not a.address.startswith("169.254."):
                    net = ipaddress.ip_network(f"{a.address}/255.255.255.0", strict=False)
                    subnet = net
                    break
            if subnet:
                break
        if not subnet:
            return []
        hosts = [str(h) for h in subnet.hosts()]
        results = {}

        def probe(ip):
            ms = icmp.ping(ip, timeout_ms=400)
            if ms is not None:
                results[ip] = ms

        with ThreadPoolExecutor(max_workers=32) as ex:
            list(ex.map(probe, hosts))
        macs = {}
        try:
            r = subprocess.run(["arp", "-a"], capture_output=True, timeout=15,
                               creationflags=subprocess.CREATE_NO_WINDOW)
            for line in r.stdout.decode("mbcs", errors="replace").splitlines():
                m = re.search(r"(\d+\.\d+\.\d+\.\d+)\s+([\da-fA-F-]{17})", line)
                if m:
                    macs[m.group(1)] = m.group(2)
        except Exception:
            # arp 不可用只影响 MAC 列，存活设备照常展示
            LOGGER.warning("arp -a 解析失败，MAC 列留空", exc_info=True)
        return [(ip, macs.get(ip, "?"), f"{results[ip]:.0f}") for ip in sorted(
            results, key=lambda x: tuple(int(o) for o in x.split(".")))]


class LanTab(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        lay = QVBoxLayout(self)
        row = QHBoxLayout()
        self.btn_scan = QPushButton("扫描局域网")
        self.btn_scan.setProperty("primary", True)
        self.btn_scan.clicked.connect(self.start)
        row.addWidget(self.btn_scan)
        self.lbl_status = QLabel("扫描当前 /24 网段（约 5-10 秒），列出存活设备与 MAC")
        self.lbl_status.setProperty("hint", True)
        row.addWidget(self.lbl_status, 1)
        lay.addLayout(row)

        self.tbl = QTableWidget(0, 3)
        self.tbl.setHorizontalHeaderLabels(["IP 地址", "MAC 地址", "延迟 ms"])
        self.tbl.horizontalHeader().setSectionResizeMode(QHeaderView.Stretch)
        self.tbl.setEditTriggers(QAbstractItemView.NoEditTriggers)
        self.tbl.setAlternatingRowColors(True)
        self.tbl.verticalHeader().setVisible(False)
        lay.addWidget(self.tbl)
        self._worker = None

    def start(self):
        if self._worker:
            return
        self.btn_scan.setEnabled(False)
        self.lbl_status.setText("扫描中…")
        self.tbl.setRowCount(0)
        self._worker = _LanScanWorker()
        self._worker.done.connect(self._done)
        self._worker.start()

    def _done(self, rows):
        self._worker = None
        self.btn_scan.setEnabled(True)
        self.tbl.setRowCount(len(rows))
        for i, (ip, mac, ms) in enumerate(rows):
            for c, v in enumerate([ip, mac, ms]):
                self.tbl.setItem(i, c, QTableWidgetItem(v))
        self.lbl_status.setText(f"发现 {len(rows)} 台设备")


# ---------------- DNS 查询 + 子网计算 ----------------

class DnsSubnetTab(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        lay = QHBoxLayout(self)

        grp_dns = QGroupBox("DNS 查询")
        dlay = QVBoxLayout(grp_dns)
        row1 = QHBoxLayout()
        self.ed_domain = QLineEdit()
        self.ed_domain.setPlaceholderText("要查询的域名，如 www.baidu.com")
        self.ed_domain.setText("www.baidu.com")
        row1.addWidget(QLabel("域名:"))
        row1.addWidget(self.ed_domain, 1)
        dlay.addLayout(row1)
        row2 = QHBoxLayout()
        self.cmb_server = QComboBox()
        self.cmb_server.addItems(["系统默认", "223.5.5.5（阿里）", "119.29.29.29（腾讯）",
                                  "114.114.114.114", "8.8.8.8（Google）"])
        row2.addWidget(QLabel("DNS 服务器:"))
        row2.addWidget(self.cmb_server, 1)
        dlay.addLayout(row2)
        btn = QPushButton("查询")
        btn.clicked.connect(self.query)
        dlay.addWidget(btn, 0, Qt.AlignLeft)
        self.list_dns = QListWidget()
        dlay.addWidget(self.list_dns)
        lay.addWidget(grp_dns, 1)

        grp_sub = QGroupBox("子网计算器")
        slay = QVBoxLayout(grp_sub)
        row3 = QHBoxLayout()
        self.ed_ip = QLineEdit("192.168.1.10")
        self.spin_prefix = QSpinBox()
        self.spin_prefix.setRange(8, 32)
        self.spin_prefix.setValue(24)
        row3.addWidget(QLabel("IP:"))
        row3.addWidget(self.ed_ip, 1)
        row3.addWidget(QLabel("/"))
        row3.addWidget(self.spin_prefix)
        btn_calc = QPushButton("计算")
        btn_calc.clicked.connect(self.calc)
        row3.addWidget(btn_calc)
        slay.addLayout(row3)
        self.list_sub = QListWidget()
        slay.addWidget(self.list_sub)
        lay.addWidget(grp_sub, 1)

    def query(self):
        self.list_dns.clear()
        domain = self.ed_domain.text().strip()
        if not domain:
            return
        server = self.cmb_server.currentText().split("（")[0]
        try:
            if server == "系统默认":
                import socket as _s
                infos = _s.getaddrinfo(domain, None, _s.AF_INET)
                ips = sorted({i[4][0] for i in infos})
            else:
                ips = dnsquery.query(domain, server, timeout=3)
        except Exception as e:
            self.list_dns.addItem(f"查询失败: {e}")
            return
        for ip in ips:
            self.list_dns.addItem(f"{ip}")

    def calc(self):
        self.list_sub.clear()
        try:
            net = ipaddress.ip_network(f"{self.ed_ip.text().strip()}/{self.spin_prefix.value()}",
                                       strict=False)
        except ValueError as e:
            self.list_sub.addItem(f"输入无效: {e}")
            return
        # 小网段沿用 hosts()（含 /31、/32 特例）；大网段用算术推导首尾，避免物化冻结。
        if net.num_addresses <= 256:
            hosts = list(net.hosts())
            range_str = f"{hosts[0]} ~ {hosts[-1]}" if hosts else "无"
        else:
            ip_type = ipaddress.IPv6Address if net.version == 6 else ipaddress.IPv4Address
            n, b = int(net.network_address), int(net.broadcast_address)
            range_str = (f"{ip_type(n + 1)} ~ {ip_type(b - 1)}"
                         if b - n > 1 else "无")
        rows = [("网络地址", str(net.network_address)),
                ("子网掩码", str(net.netmask)),
                ("广播地址", str(net.broadcast_address)),
                ("可用主机范围", range_str),
                ("可用主机数", str(max(0, net.num_addresses - 2))),
                ("总地址数", str(net.num_addresses))]
        for k, v in rows:
            self.list_sub.addItem(f"{k}: {v}")


# ---------------- 持续 ping + 路由追踪 ----------------

class _TracertWorker(QThread):
    line = Signal(dict)

    def __init__(self, target, parent=None):
        super().__init__(parent)
        self.target = target
        # 不可命名为 _stop：会覆盖 Thread._stop()，导致 join() 抛 TypeError
        self._stop_evt = threading.Event()

    def stop(self):
        self._stop_evt.set()

    def run(self):
        try:
            p = subprocess.Popen(
                ["tracert", "-d", "-w", "600", "-h", "15", self.target],
                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                creationflags=subprocess.CREATE_NO_WINDOW)
            for raw in p.stdout:
                if self._stop_evt.is_set():
                    p.kill()
                    return
                line = raw.decode("mbcs", errors="replace").strip()
                m = re.match(r"^\s*(\d{1,2})\s+(.*)", line)
                if not m:
                    continue
                hop = int(m.group(1))
                rest = m.group(2)
                ms_list = [int(x) for x in re.findall(r"(\d+)\s*ms", rest)]
                ip_m = re.search(r"(\d{1,3}(?:\.\d{1,3}){3})", rest)
                self.line.emit({"hop": hop,
                                "ms": ms_list,
                                "ip": ip_m.group(1) if ip_m else "*"})
        except Exception:
            pass


class PingRouteTab(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        lay = QVBoxLayout(self)

        row = QHBoxLayout()
        row.addWidget(QLabel("目标:"))
        self.ed_target = QLineEdit("223.5.5.5")
        row.addWidget(self.ed_target, 1)
        self.btn_ping = QPushButton("开始持续 ping")
        self.btn_ping.clicked.connect(self.toggle_ping)
        row.addWidget(self.btn_ping)
        self.btn_route = QPushButton("路由追踪")
        self.btn_route.clicked.connect(self.tracert)
        row.addWidget(self.btn_route)
        lay.addLayout(row)

        self.chart = LineChart("持续 ping 延迟（1 秒/点）", series=[("延迟", "#3576e0")],
                               seconds=180, fmt=lambda v: f"{v:.0f}ms")
        lay.addWidget(self.chart)

        self.tbl_route = QTableWidget(0, 3)
        self.tbl_route.setHorizontalHeaderLabels(["跳数", "延迟 ms", "IP"])
        self.tbl_route.horizontalHeader().setSectionResizeMode(2, QHeaderView.Stretch)
        self.tbl_route.setEditTriggers(QAbstractItemView.NoEditTriggers)
        self.tbl_route.setAlternatingRowColors(True)
        self.tbl_route.verticalHeader().setVisible(False)
        self.tbl_route.setMaximumHeight(220)
        lay.addWidget(self.tbl_route)

        self._ping_timer = QTimer(self)
        self._ping_timer.timeout.connect(self._ping_once)
        self._tracert = None

    def toggle_ping(self):
        if self._ping_timer.isActive():
            self._ping_timer.stop()
            self.btn_ping.setText("开始持续 ping")
        else:
            self.chart.clear_data()
            self._ping_timer.start(1000)
            self._ping_once()
            self.btn_ping.setText("停止 ping")

    def _ping_once(self):
        target = self.ed_target.text().strip()
        if not target:
            return
        ms = icmp.ping(target)
        self.chart.push([ms])

    def tracert(self):
        target = self.ed_target.text().strip()
        if not target or self._tracert:
            return
        self.tbl_route.setRowCount(0)
        self.btn_route.setEnabled(False)
        self._tracert = _TracertWorker(target)
        self._tracert.line.connect(self._route_line)
        self._tracert.finished.connect(self._tracert_finished)
        self._tracert.start()

    def _tracert_finished(self):
        # 必须复位，否则二次追踪被 self._tracert 守卫永久挡住
        self._tracert = None
        self.btn_route.setEnabled(True)

    def _route_line(self, hop):
        row = self.tbl_route.rowCount()
        self.tbl_route.insertRow(row)
        ms_text = ", ".join(f"{x} ms" for x in hop["ms"]) or "超时"
        for c, v in enumerate([str(hop["hop"]), ms_text, hop["ip"]]):
            self.tbl_route.setItem(row, c, QTableWidgetItem(v))


# ---------------- 端口占用查看器 ----------------

class _PortWorker(QThread):
    done = Signal(list)

    def run(self):
        rows = []
        try:
            conns = psutil.net_connections(kind="inet")
        except psutil.AccessDenied:
            self.done.emit([])
            return
        proc_cache = {}
        for c in conns:
            try:
                pid = c.pid or 0
                if pid not in proc_cache:
                    p = psutil.Process(pid) if pid else None
                    proc_cache[pid] = (p.name() if p else "System", p.exe() if p else "") \
                        if p else ("System", "")
                pname, pexe = proc_cache[pid]
                laddr = f"{c.laddr.ip}:{c.laddr.port}" if c.laddr else "-"
                raddr = f"{c.raddr.ip}:{c.raddr.port}" if c.raddr else "-"
                rows.append({"proto": "TCP" if c.type == 1 else "UDP",
                             "tcp": c.type == 1,
                             "laddr": laddr, "raddr": raddr,
                             "status": c.status if c.status != "NONE" else "-",
                             "pid": pid, "name": pname, "exe": pexe})
            except (psutil.NoSuchProcess, psutil.AccessDenied, psutil.ZombieProcess):
                continue
            except Exception:
                continue
        rows.sort(key=lambda r: (not r["tcp"], r["laddr"]))
        self.done.emit(rows)


class PortTab(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        lay = QVBoxLayout(self)
        row = QHBoxLayout()
        self.btn_refresh = QPushButton("刷新")
        self.btn_refresh.setProperty("primary", True)
        self.btn_refresh.clicked.connect(self.refresh)
        row.addWidget(self.btn_refresh)
        self.chk_auto = QCheckBox("每 5 秒自动刷新")
        self.chk_auto.toggled.connect(
            lambda on: self._timer.start(5000) if on else self._timer.stop())
        row.addWidget(self.chk_auto)
        self.chk_listen = QCheckBox("只看监听端口")
        self.chk_listen.toggled.connect(lambda _on: self._apply_filter())
        row.addWidget(self.chk_listen)
        row.addWidget(QLabel("筛选:"))
        self.ed_filter = QLineEdit()
        self.ed_filter.setPlaceholderText("端口 / PID / 进程名 / IP…")
        self.ed_filter.setClearButtonEnabled(True)
        self.ed_filter.textChanged.connect(lambda _t: self._apply_filter())
        row.addWidget(self.ed_filter, 1)
        self.btn_kill = QPushButton("结束选中进程")
        self.btn_kill.setProperty("danger", True)
        self.btn_kill.clicked.connect(self._kill_selected)
        row.addWidget(self.btn_kill)
        lay.addLayout(row)

        self.tbl = QTableWidget(0, 6)
        self.tbl.setHorizontalHeaderLabels(["协议", "本地地址", "远程地址", "状态", "PID", "进程"])
        self.tbl.horizontalHeader().setSectionResizeMode(1, QHeaderView.Stretch)
        self.tbl.setSelectionBehavior(QAbstractItemView.SelectRows)
        self.tbl.setEditTriggers(QAbstractItemView.NoEditTriggers)
        self.tbl.setAlternatingRowColors(True)
        self.tbl.verticalHeader().setVisible(False)
        lay.addWidget(self.tbl)

        self.lbl_status = QLabel(" ")
        self.lbl_status.setProperty("hint", True)
        lay.addWidget(self.lbl_status)

        self._timer = QTimer(self)
        self._timer.timeout.connect(self.refresh)
        self._worker = None
        self._auto_paused = False
        self._initial_refreshed = False   # 懒加载：首次显示本 Tab 才扫描（启动期不做端口枚举）

    def showEvent(self, event):
        super().showEvent(event)
        if not self._initial_refreshed:
            self._initial_refreshed = True
            self.refresh()
        if self.chk_auto.isChecked() or getattr(self, "_auto_paused", False):
            self.chk_auto.setChecked(True)
            self._timer.start(5000)      # 页面重新可见：恢复自动刷新
            self._auto_paused = False

    def hideEvent(self, event):
        super().hideEvent(event)
        if self._timer.isActive():       # 页面不可见：停表省 netstat/CPU
            self._timer.stop()
            self._auto_paused = True

    def refresh(self):
        if self._worker:
            return
        self.btn_refresh.setEnabled(False)
        self._worker = _PortWorker()
        self._worker.done.connect(self._done)
        self._worker.start()

    def _done(self, rows):
        self._worker = None
        self.btn_refresh.setEnabled(True)
        # 保留选中 PID，自动刷新后不丢焦点
        selected_pid = 0
        cur = self.tbl.currentRow()
        if 0 <= cur < self.tbl.rowCount() and self.tbl.item(cur, 4):
            try:
                selected_pid = int(self.tbl.item(cur, 4).text())
            except ValueError:
                selected_pid = 0
        self._rows = rows
        self.lbl_status.setText(f"共 {len(rows)} 条连接　{time.strftime('%H:%M:%S')}")
        self._render(rows)
        self._apply_filter()
        if selected_pid:
            for row in range(self.tbl.rowCount()):
                item = self.tbl.item(row, 4)
                if item and item.text() == str(selected_pid):
                    self.tbl.selectRow(row)
                    break

    def _render(self, rows):
        self.tbl.setRowCount(len(rows))
        listen_color = QColor("#3576e0")
        for i, r in enumerate(rows):
            vals = [r["proto"], r["laddr"], r["raddr"], r["status"],
                    str(r["pid"]) if r["pid"] else "-", r["name"]]
            for c, v in enumerate(vals):
                item = QTableWidgetItem(v)
                if r["status"] == "LISTEN":
                    item.setForeground(listen_color)
                self.tbl.setItem(i, c, item)
            self.tbl.item(i, 5).setToolTip(r["exe"] or r["name"])

    def _apply_filter(self):
        kw = self.ed_filter.text().strip().lower()
        only_listen = self.chk_listen.isChecked()
        for row in range(self.tbl.rowCount()):
            cells = [self.tbl.item(row, c) for c in range(6)]
            text = " ".join(it.text().lower() for it in cells if it is not None)
            status_item = cells[3]
            status = status_item.text() if status_item is not None else ""
            visible = (not kw or kw in text) and (not only_listen or status == "LISTEN")
            self.tbl.setRowHidden(row, not visible)

    def _kill_selected(self):
        row = self.tbl.currentRow()
        if row < 0 or self.tbl.isRowHidden(row):
            QMessageBox.information(self, "提示", "请先在列表中选中一条连接。")
            return
        cell_pid = self.tbl.item(row, 4)
        cell_name = self.tbl.item(row, 5)
        if cell_pid is None or cell_name is None:
            return
        pid = int(cell_pid.text()) if cell_pid.text() != "-" else 0
        pname = cell_name.text()
        if not pid:
            QMessageBox.information(self, "提示", "该连接为系统级（无归属进程），无法结束。")
            return
        ret = QMessageBox.question(
            self, "确认结束进程",
            f"确定结束进程 {pname} (PID {pid}) 吗？\n\n"
            "该进程的所有连接将断开；若是系统关键进程可能导致功能异常。",
            QMessageBox.Yes | QMessageBox.No)
        if ret != QMessageBox.Yes:
            return
        try:
            psutil.Process(pid).kill()
            from ..logger import LOGGER
            LOGGER.warning("已结束进程 {} (PID {})", pname, pid)
        except psutil.AccessDenied:
            QMessageBox.warning(self, "提示", "权限不足，无法结束该进程（可能需要管理员）。")
        except psutil.NoSuchProcess:
            QMessageBox.information(self, "提示", "进程已自行退出。")
        except Exception as e:
            QMessageBox.warning(self, "提示", f"结束进程失败: {e}")
        self.refresh()


class HostsTab(QWidget):
    """Hosts 文件编辑器：表格化增删改，改前自动备份，保存需管理员权限。

    增/删/切换只改内存列表并标脏；「保存」一次性写回（写前备份原文件）。
    path 参数供测试注入临时文件，默认系统 hosts。
    """

    def __init__(self, parent=None, path: str = None):
        super().__init__(parent)
        self._path = path or hostsed.HOSTS_PATH
        self._entries = []
        self._dirty = False
        lay = QVBoxLayout(self)
        lay.setContentsMargins(8, 8, 8, 8)

        self.lbl_info = QLabel()
        self.lbl_info.setObjectName("hint")
        lay.addWidget(self.lbl_info)

        row = QHBoxLayout()
        self.edt_ip = QLineEdit()
        self.edt_ip.setPlaceholderText("IP，如 127.0.0.1")
        self.edt_host = QLineEdit()
        self.edt_host.setPlaceholderText("主机名，如 api.example.com")
        self.edt_cmt = QLineEdit()
        self.edt_cmt.setPlaceholderText("备注（可选）")
        self.btn_add = QPushButton("添加条目")
        self.btn_add.setProperty("primary", True)
        self.btn_add.clicked.connect(self._add)
        for w in (self.edt_ip, self.edt_host, self.edt_cmt):
            row.addWidget(w, 1)
        row.addWidget(self.btn_add)
        lay.addLayout(row)

        self.tbl = QTableWidget(0, 4)
        self.tbl.setHorizontalHeaderLabels(["状态", "IP", "主机名", "备注"])
        self.tbl.horizontalHeader().setSectionResizeMode(3, QHeaderView.Stretch)
        self.tbl.setSelectionBehavior(QAbstractItemView.SelectRows)
        self.tbl.setSelectionMode(QAbstractItemView.SingleSelection)
        self.tbl.setEditTriggers(QAbstractItemView.NoEditTriggers)
        self.tbl.setAlternatingRowColors(True)
        self.tbl.verticalHeader().setVisible(False)
        self.tbl.itemSelectionChanged.connect(self._refresh_buttons)
        lay.addWidget(self.tbl, 1)

        brow = QHBoxLayout()
        self.btn_toggle = QPushButton("启用 / 禁用")
        self.btn_toggle.clicked.connect(self._toggle)
        self.btn_del = QPushButton("删除选中")
        self.btn_del.setProperty("danger", True)
        self.btn_del.clicked.connect(self._remove)
        self.btn_reload = QPushButton("放弃更改")
        self.btn_reload.clicked.connect(self.load)
        self.btn_save = QPushButton("保存（自动备份）")
        self.btn_save.clicked.connect(self._save)
        self.lbl_dirty = QLabel("")
        self.lbl_dirty.setObjectName("hint")
        brow.addWidget(self.lbl_dirty)
        brow.addStretch(1)
        for b in (self.btn_toggle, self.btn_del, self.btn_reload, self.btn_save):
            brow.addWidget(b)
        lay.addLayout(brow)
        self.load()

    # ---------------- 数据 ----------------

    def load(self):
        """读取 hosts（或放弃更改）。文件不可读时显示空态。"""
        self._entries = hostsed.read_entries(self._path)
        self._dirty = False
        admin_ok = admin.is_admin()
        self.lbl_info.setText(
            f"{self._path}　·　" +
            ("管理员：可保存" if admin_ok else "非管理员：只能查看，保存会被系统拒绝"))
        self.btn_save.setEnabled(admin_ok)
        self._reload_table()

    def _reload_table(self):
        self.tbl.setRowCount(len(self._entries))
        for r, e in enumerate(self._entries):
            if e.get("kind") == "entry":
                status = "✔ 启用"
            elif e.get("kind") == "disabled":
                status = "✘ 禁用"
            else:
                status = "—"
            item0 = QTableWidgetItem(status)
            item0.setData(Qt.UserRole, r)
            self.tbl.setItem(r, 0, item0)
            self.tbl.setItem(r, 1, QTableWidgetItem(e.get("ip", "")))
            self.tbl.setItem(r, 2, QTableWidgetItem(
                e.get("host", "") + (" " + " ".join(e.get("aliases") or [])
                                     if e.get("aliases") else "")))
            self.tbl.setItem(r, 3, QTableWidgetItem(e.get("comment", "")))
        self._update_dirty()
        self._refresh_buttons()

    def _selected_index(self) -> int:
        row = self.tbl.currentRow()
        if row < 0:
            return -1
        item = self.tbl.item(row, 0)
        data = item.data(Qt.UserRole) if item else None
        return int(data) if data is not None else -1

    def _mark_dirty(self):
        self._dirty = True
        self._update_dirty()

    def _update_dirty(self):
        self.lbl_dirty.setText("● 未保存" if self._dirty else "")
        self.btn_reload.setEnabled(self._dirty)

    def _refresh_buttons(self):
        has = self._selected_index() >= 0
        self.btn_toggle.setEnabled(has)
        self.btn_del.setEnabled(has)

    # ---------------- 操作 ----------------

    def _add(self):
        ip = self.edt_ip.text().strip()
        host = self.edt_host.text().strip()
        if not ip or not host:
            QMessageBox.information(self, "SysKit", "请填写 IP 和主机名")
            return
        try:
            ipaddress.ip_address(ip)
        except ValueError:
            QMessageBox.warning(self, "SysKit", f"IP 格式不合法：{ip}")
            return
        if hostsed.find_entry(self._entries, host) >= 0:
            QMessageBox.warning(self, "SysKit", f"主机名 {host} 已存在（启用或禁用状态），请先删除或直接启用。")
            return
        hostsed.add_entry(self._entries, ip, host, self.edt_cmt.text())
        self.edt_ip.clear()
        self.edt_host.clear()
        self.edt_cmt.clear()
        self._mark_dirty()
        self._reload_table()

    def _toggle(self):
        idx = self._selected_index()
        if idx < 0:
            return
        if self._entries[idx].get("kind") not in ("entry", "disabled"):
            return
        hostsed.toggle_entry(self._entries, idx)
        self._mark_dirty()
        self._reload_table()

    def _remove(self):
        idx = self._selected_index()
        if idx >= 0:
            hostsed.remove_entry(self._entries, idx)
            self._mark_dirty()
            self._reload_table()

    def _save(self):
        if not self._dirty:
            return
        if not admin.is_admin():
            QMessageBox.warning(self, "SysKit",
                                "写入 hosts 需要管理员权限。\n请以管理员身份运行 SysKit 后重试。")
            return
        try:
            backup = hostsed.write_entries(self._entries, self._path, backup=True)
        except OSError as e:
            QMessageBox.warning(self, "SysKit", f"保存失败：{e}")
            return
        self._dirty = False
        self._update_dirty()
        LOGGER.info("hosts 已保存，备份: {}", backup or "无")
        QMessageBox.information(self, "SysKit",
                                "已保存" + (f"（备份：{backup}）" if backup else ""))


class ToolboxPage(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        lay = QVBoxLayout(self)
        lay.setContentsMargins(0, 0, 0, 0)
        self.tabs = QTabWidget()        # 供命令面板定位子工具
        tabs = self.tabs
        tabs.addTab(SpeedTestTab(), "速度测试")
        tabs.addTab(LanTab(), "局域网设备")
        tabs.addTab(PortTab(), "端口 / 进程")
        tabs.addTab(DnsSubnetTab(), "DNS / 子网")
        tabs.addTab(PingRouteTab(), "ping / 路由")
        tabs.addTab(HostsTab(), "Hosts 编辑")
        lay.addWidget(tabs)
