"""工具箱：速度测试 / 局域网设备发现 / DNS查询与子网计算 / 持续ping与路由追踪 / 端口占用 / Hosts 编辑 / DNS 测速 / 应用联网控制。"""
import ipaddress
import os
import re
import socket
import subprocess
import threading
import time
import traceback
import urllib.request
from concurrent.futures import ThreadPoolExecutor

import psutil
from PySide6.QtCore import QModelIndex, Qt, QThread, QTimer, Signal
from PySide6.QtGui import QColor
from PySide6.QtWidgets import (QAbstractItemView, QCheckBox, QComboBox,
                               QGroupBox, QHBoxLayout, QHeaderView, QLabel, QLineEdit,
                               QListWidget, QMessageBox, QProgressBar, QPushButton,
                               QSpinBox, QTableWidgetItem, QTabWidget,
                               QVBoxLayout, QWidget)

from . import theme
from .charts import LineChart, human_bytes
from .theme import make_table
from .workers import reap
from .. import admin, appnet, hostsed, icmp, dnsquery, stores
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


def _clean_history(hist):
    """speedtest.json 的元素级防护（审计第 9 批 C-2）。

    load_json 只兜底「解析失败」；合法 JSON 但元素不是 dict / 字段类型漂移
    （如 ["x"] 或 {"mbps": "快"}）曾让 SpeedTestTab 构造期抛异常 → 整个应用
    无法启动。这里对齐 appnet.load_entries 的容忍口径：脏条目剔除，不炸。
    """
    if not isinstance(hist, list):
        return []
    return [e for e in hist if isinstance(e, dict)]


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

        hist_grp = QGroupBox("历史结果（已持久化，最多 200 条）")
        hlay = QVBoxLayout(hist_grp)
        self.list_hist = QListWidget()
        self.list_hist.setAlternatingRowColors(True)
        hlay.addWidget(self.list_hist)
        hist_btns = QHBoxLayout()
        self.btn_hist_clear = QPushButton("清空历史")
        self.btn_hist_clear.clicked.connect(self._clear_history)
        hist_btns.addStretch(1)
        hist_btns.addWidget(self.btn_hist_clear)
        hlay.addLayout(hist_btns)
        lay.addWidget(hist_grp, 1)
        self._worker = None
        self._history = stores.load_json("speedtest.json", [])
        self._history = _clean_history(self._history)
        for e in self._history[:200]:
            self.list_hist.addItem(self._hist_label(e))

    _HIST_FILE = "speedtest.json"
    _HIST_LIMIT = 200

    @staticmethod
    def _hist_label(e: dict) -> str:
        if not isinstance(e, dict) or not e.get("ok"):
            ts = e.get("ts", "") if isinstance(e, dict) else ""
            return f"{ts}　测速失败"
        try:
            mbps = f"{float(e.get('mbps', 0)):.1f}"
        except (TypeError, ValueError):
            mbps = "?"
        return f"{e.get('ts', '')}　{mbps} Mbps"

    def _record_history(self, ok: bool, mbps: float):
        entry = {"ts": time.strftime("%m-%d %H:%M"), "mbps": round(float(mbps), 1),
                 "ok": bool(ok)}
        self._history.insert(0, entry)
        del self._history[self._HIST_LIMIT:]
        if not stores.save_json(self._HIST_FILE, self._history):
            LOGGER.warning("测速历史写盘失败: {}", self._HIST_FILE)
        self.list_hist.insertItem(0, self._hist_label(entry))
        while self.list_hist.count() > self._HIST_LIMIT:
            self.list_hist.takeItem(self.list_hist.count() - 1)

    def _clear_history(self):
        if self.list_hist.count() and QMessageBox.question(
                self, "清空历史", "确定清空全部测速历史？",
                QMessageBox.Yes | QMessageBox.No) != QMessageBox.Yes:
            return
        self._history = []
        if not stores.save_json(self._HIST_FILE, self._history):
            # 界面已清但盘上还是旧数据：失败必须留痕，否则重启后历史「复活」像见鬼
            LOGGER.warning("测速历史清空写盘失败: {}", self._HIST_FILE)
        self.list_hist.clear()

    def start(self):
        if self._worker:
            return
        self.btn_start.setEnabled(False)
        self.pb.show()
        self.lbl_status.setText("测速中…")
        self._worker = reap(SpeedTestWorker(), self)
        self._worker.progress.connect(self._on_progress)
        self._worker.done.connect(self._on_done)
        self._worker.start()

    def _on_progress(self, instant_bps, total_mb):
        self.lbl_status.setText(f"实时 {human_bytes(instant_bps)}/s　已下载 {total_mb:.1f} MB")

    def _on_done(self, ok, avg_bps):
        self._worker = None
        self.btn_start.setEnabled(True)
        self.pb.hide()
        # Mbps 按十进制（10^6）计——运营商/测速网站口径；曾按 2^20 计，
        # 与用户拿去对比的宽带数值差 ~4.9%（显示偏高被误认为"虚标"的反向）
        mbps = avg_bps * 8 / 1_000_000
        if ok:
            self.lbl_big.setText(f"{mbps:.1f} Mbps（{human_bytes(avg_bps)}/s）")
            LOGGER.info("速度测试完成: {} Mbps", f"{mbps:.1f}")
        else:
            self.lbl_big.setText("测速失败")
            self.lbl_status.setText("所有测速源均不可达，请检查网络")
        self._record_history(ok, mbps if ok else 0.0)   # 失败也记录（趋势可见）


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

    # 与 telemetry._PHYSICAL_EXCLUDE 同款：WSL/Hyper-V/VPN 虚拟网卡优先时
    # 会扫到虚拟网段，「发现 0 台设备」但真实局域网被跳过
    _EXCLUDE = ("loopback", "vmnet", "vmware", "vethernet", "bluetooth",
                "wan miniport", "tap", "wintun", "utun", "vgate", "hyper-v")

    def _scan(self) -> list:
        subnet = None
        ifaces = psutil.net_if_addrs()
        for name, addr in ifaces.items():
            lname = name.lower()
            if lname == "loopback" or any(x in lname for x in self._EXCLUDE):
                continue
            for a in addr:
                if a.family == socket.AF_INET and a.address and not a.address.startswith("127.") \
                        and not a.address.startswith("169.254."):
                    net = ipaddress.ip_network(f"{a.address}/255.255.255.0", strict=False)
                    subnet = net
                    break
            if subnet:
                break
        if not subnet:
            # 全是虚拟网卡/找不到候选时，回退旧逻辑（无名字过滤的枚举序）
            for addr in ifaces.values():
                for a in addr:
                    if a.family == socket.AF_INET and a.address and not a.address.startswith("127.") \
                            and not a.address.startswith("169.254."):
                        subnet = ipaddress.ip_network(f"{a.address}/255.255.255.0", strict=False)
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

        self.tbl = make_table(["IP 地址", "MAC 地址", "延迟 ms"])
        self.tbl.horizontalHeader().setSectionResizeMode(QHeaderView.Stretch)
        lay.addWidget(self.tbl)
        self._worker = None

    def start(self):
        if self._worker:
            return
        self.btn_scan.setEnabled(False)
        self.lbl_status.setText("扫描中…")
        self.tbl.setRowCount(0)
        self._worker = reap(_LanScanWorker(), self)
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

class _DnsQueryWorker(QThread):
    """DNS 查询后台线程：系统默认分支走 getaddrinfo（无超时参数，系统 DNS
    故障时最长可冻 UI 数十秒），指定服务器分支超时 3s——都不该在 UI 线程跑。"""
    done = Signal(list)            # IP 列表，或 ["查询失败: ..."]

    def __init__(self, domain, server, parent=None):
        super().__init__(parent)
        self._domain = domain
        self._server = server

    def run(self):
        try:
            if self._server == "系统默认":
                infos = socket.getaddrinfo(self._domain, None, socket.AF_INET)
                self.done.emit(sorted({i[4][0] for i in infos}))
            else:
                self.done.emit(dnsquery.query(self._domain, self._server, timeout=3))
        except Exception as e:
            self.done.emit([f"查询失败: {e}"])


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
        self.ed_domain.returnPressed.connect(self.query)   # 回车即查（原须点按钮）
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
        # 上一查询未完成时忽略本次点击（防抖），不清空在跑查询将要回填的列表
        if getattr(self, "_qw", None) is not None:
            return
        self.list_dns.clear()
        domain = self.ed_domain.text().strip()
        if not domain:
            return
        server = self.cmb_server.currentText().split("（")[0]
        self._qw = reap(_DnsQueryWorker(domain, server), self)
        self._qw.done.connect(self._query_done)
        self._qw.start()

    def _query_done(self, rows):
        self._qw = None
        for ip in rows:
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
            LOGGER.warning("tracert 线程异常", exc_info=True)


# ---------------- DNS 速度对比 ----------------

_DNS_SPEED_SERVERS = [
    ("223.5.5.5", "阿里"), ("119.29.29.29", "腾讯"), ("114.114.114.114", "114"),
    ("180.76.76.76", "百度"), ("1.1.1.1", "Cloudflare"), ("8.8.8.8", "Google"),
]


class _DnsSpeedWorker(QThread):
    """并发测各 DNS 服务器的解析延迟/成功率（每服务器串行 N 次）。
    结果逐个经 server_done 回 UI（单条超时也不拖总时长）。"""
    server_done = Signal(str, object)   # (server_ip, dict|None)  None=全部失败

    def __init__(self, domain, times, parent=None):
        super().__init__(parent)
        self._domain = domain
        self._times = max(1, min(5, int(times)))

    def run(self):
        def probe(server):
            lat = []
            for _ in range(self._times):
                t0 = time.perf_counter()
                try:
                    dnsquery.query(self._domain, server, timeout=1.5, attempts=1)
                    lat.append((time.perf_counter() - t0) * 1000)
                except Exception:
                    continue
            if lat:
                self.server_done.emit(server, {
                    "avg": sum(lat) / len(lat), "ok": len(lat),
                    "total": self._times})
            else:
                self.server_done.emit(server, None)

        with ThreadPoolExecutor(max_workers=len(_DNS_SPEED_SERVERS)) as ex:
            list(ex.map(probe, [s for s, _n in _DNS_SPEED_SERVERS]))


class DnsSpeedTab(QWidget):
    """DNS 测速：并发测各公共 DNS 的解析延迟/成功率，推荐最优（配合修复页
    dns_preset 使用）。查询在后台线程——单服务器超时不得冻 UI。"""

    def __init__(self, parent=None):
        super().__init__(parent)
        lay = QVBoxLayout(self)
        row = QHBoxLayout()
        row.addWidget(QLabel("测试域名:"))
        self.ed_domain = QLineEdit("www.baidu.com")
        row.addWidget(self.ed_domain, 1)
        row.addWidget(QLabel("次数:"))
        self.spin_times = QSpinBox()
        self.spin_times.setRange(1, 5)
        self.spin_times.setValue(3)
        self.spin_times.setSuffix(" 次")
        row.addWidget(self.spin_times)
        self.btn_start = QPushButton("开始测速")
        self.btn_start.setProperty("primary", True)
        self.btn_start.clicked.connect(self.start)
        row.addWidget(self.btn_start)
        lay.addLayout(row)

        self.tbl = make_table(["DNS 服务器", "平均延迟", "成功率", "说明"], select_rows=True)
        header = self.tbl.horizontalHeader()
        header.setSectionResizeMode(0, QHeaderView.Stretch)
        header.setStretchLastSection(True)
        lay.addWidget(self.tbl, 1)

        self.lbl_recommend = QLabel("点击「开始测速」，对常用公共 DNS 并发解析对比。")
        self.lbl_recommend.setProperty("hint", True)
        self.lbl_recommend.setWordWrap(True)
        lay.addWidget(self.lbl_recommend)
        self._worker = None
        self._results = {}
        self._times = self.spin_times.value()

    def start(self):
        if self._worker:
            return
        domain = self.ed_domain.text().strip()
        if not domain:
            QMessageBox.information(self, "SysKit", "请输入要解析的测试域名。")
            return
        self.btn_start.setEnabled(False)
        # 次数在开跑时定死并锁住控件：跑完前改档位会让表格分母与实际探测次数不符
        self._times = self.spin_times.value()
        self.spin_times.setEnabled(False)
        self.lbl_recommend.setText("测速中…")
        self.tbl.setRowCount(0)
        self._results = {}
        self._worker = reap(_DnsSpeedWorker(domain, self._times), self)
        self._worker.server_done.connect(self._server_done)
        self._worker.finished.connect(self._finished)
        self._worker.start()

    def _server_done(self, server, info):
        name = next((n for s, n in _DNS_SPEED_SERVERS if s == server), server)
        self._results[server] = (name, info)
        self._render()

    def _finished(self):
        self._worker = None
        self.btn_start.setEnabled(True)
        self.spin_times.setEnabled(True)
        self._recommend()

    def _render(self):
        order = sorted(self._results.items(),
                       key=lambda kv: (kv[1][1] is None,
                                       kv[1][1]["avg"] if kv[1][1] else 0))
        self.tbl.setRowCount(len(order))
        for i, (server, (name, info)) in enumerate(order):
            if info is None:
                vals = [f"{name} ({server})", "—", f"0/{self._times}",
                        "全部超时/失败"]
            else:
                vals = [f"{name} ({server})", f"{info['avg']:.0f} ms",
                        f"{info['ok']}/{info['total']}",
                        "可用" if info["ok"] == info["total"] else "部分丢包"]
            for c, v in enumerate(vals):
                self.tbl.setItem(i, c, QTableWidgetItem(v))

    def _recommend(self):
        ok_rows = [(s, n, info) for s, (n, info) in self._results.items() if info]
        if not ok_rows:
            self.lbl_recommend.setText("全部 DNS 均解析失败——当前网络可能断网，"
                                       "或测试域名不可解析。可先在「诊断」页体检。")
            return
        best = min(ok_rows, key=lambda kv: kv[2]["avg"])
        self.lbl_recommend.setText(
            f"推荐：{best[1]}（{best[0]}）　平均 {best[2]['avg']:.0f} ms、"
            f"成功率 {best[2]['ok']}/{best[2]['total']}。"
            "可在「修复」页用 dns_preset 动作设置静态 DNS。")


class _PingWorker(QThread):
    """单发 ICMP：icmp.ping 同步等待最长 1.5s，放后台线程跑——
    持续 ping 一个不通的目标时，UI 线程不能每秒被冻 1.5s。"""
    done = Signal(object)          # float|None（延迟 ms；不可达为 None）

    def __init__(self, target, parent=None):
        super().__init__(parent)
        self._target = target

    def run(self):
        self.done.emit(icmp.ping(self._target))


class PingRouteTab(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        lay = QVBoxLayout(self)

        row = QHBoxLayout()
        row.addWidget(QLabel("目标:"))
        self.ed_target = QLineEdit("223.5.5.5")
        self.ed_target.returnPressed.connect(self._return_start_ping)   # 回车即开始
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

        self.tbl_route = make_table(["跳数", "延迟 ms", "IP"])
        self.tbl_route.horizontalHeader().setSectionResizeMode(2, QHeaderView.Stretch)
        self.tbl_route.setMaximumHeight(220)
        lay.addWidget(self.tbl_route)

        self._ping_timer = QTimer(self)
        self._ping_timer.timeout.connect(self._ping_once)
        self._tracert = None
        self._ping_was_active = False
        self._ping_worker = None

    def showEvent(self, event):
        super().showEvent(event)
        if self._ping_was_active and not self._ping_timer.isActive():
            self._ping_timer.start(1000)     # 页面重新可见：恢复持续 ping
            self._ping_was_active = False
            # 定时器恢复了，按钮必须同步回「停止 ping」——否则用户点「开始」
            # 实际触发的是停止分支（文案与状态相反）
            self.btn_ping.setText("停止 ping")

    def hideEvent(self, event):
        super().hideEvent(event)
        # 页面不可见时停持续 ping（icmp 同步调用会卡 UI 线程最长 1.5s/次）
        if self._ping_timer.isActive():
            self._ping_timer.stop()
            self._ping_was_active = True
            self.btn_ping.setText("开始持续 ping")

    def toggle_ping(self):
        if self._ping_timer.isActive():
            self._ping_timer.stop()
            self.btn_ping.setText("开始持续 ping")
        else:
            self.chart.clear_data()
            self._ping_timer.start(1000)
            self._ping_once()
            self.btn_ping.setText("停止 ping")

    def _return_start_ping(self):
        """目标框回车：未在跑则开始持续 ping；在跑则不重复启动（Esc 语义交给按钮）。"""
        if not self._ping_timer.isActive():
            self.toggle_ping()

    def _ping_once(self):
        target = self.ed_target.text().strip()
        # 上一发还在跑就跳过本 tick：不可达目标 ping 耗时(1.5s)可能超过定时周期(1s)
        if not target or self._ping_worker is not None:
            return
        self._ping_worker = reap(_PingWorker(target), self)
        self._ping_worker.done.connect(self._ping_done)
        self._ping_worker.start()

    def _ping_done(self, ms):
        self._ping_worker = None
        self.chart.push([ms])

    def tracert(self):
        target = self.ed_target.text().strip()
        if self._tracert:                        # 运行中再点 → 中止（stop() 此前是死代码）
            self._tracert.stop()
            self.btn_route.setText("路由追踪")
            return
        if not target:
            return
        self.tbl_route.setRowCount(0)
        self.btn_route.setEnabled(True)
        self.btn_route.setText("停止追踪")
        self._tracert = reap(_TracertWorker(target), self)
        self._tracert.line.connect(self._route_line)
        self._tracert.finished.connect(self._tracert_finished)
        self._tracert.start()

    def _tracert_finished(self):
        # 必须复位，否则二次追踪被 self._tracert 守卫永久挡住
        self._tracert = None
        self.btn_route.setText("路由追踪")

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

        self.tbl = make_table(["协议", "本地地址", "远程地址", "状态", "PID", "进程"],
                              select_rows=True)
        self.tbl.horizontalHeader().setSectionResizeMode(1, QHeaderView.Stretch)
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
        self._worker = reap(_PortWorker(), self)
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
        # LISTEN 行语义蓝：现取 pal（每次刷新重填，切主题自动跟随，补57）
        listen_color = QColor(theme.pal()["accent"])
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

        self.tbl = make_table(["状态", "IP", "主机名", "备注"], select_rows=True)
        self.tbl.horizontalHeader().setSectionResizeMode(3, QHeaderView.Stretch)
        self.tbl.setSelectionMode(QAbstractItemView.SingleSelection)
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
        if hostsed.find_entry(self._entries, host, ip=ip) >= 0:
            QMessageBox.warning(self, "SysKit", f"条目 {ip} → {host} 已存在（启用或禁用状态），请先删除或直接启用。")
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


class _AppNetWorker(QThread):
    """应用联网控制的一次性后台任务。

    netsh 与 PowerShell 都是百毫秒到秒级的阻塞命令（一次刷新要查全部规则），
    放 UI 线程会明显卡顿。异常也必须发 done，否则 self._worker 永不复位、
    按钮永久禁用。
    """
    done = Signal(object)          # (ok: bool, 消息: str, payload: None | (entries, states))

    def __init__(self, job, parent=None):
        super().__init__(parent)
        self._job = job

    def run(self):
        try:
            self.done.emit(self._job())
        except Exception as e:
            LOGGER.error("应用联网任务异常:\n{}", traceback.format_exc())
            self.done.emit((False, f"操作失败：{e}", None))


def _appnet_state_text(entry, states):
    """台账意图 × 防火墙实际 → 一行用户看得懂的状态。

    两者都要显示：只看台账会把「用户手动在 wf.msc 里删了规则」画成仍在拦截；
    只看防火墙则分不清「已放行」和「规则停用」。
    查不到的一律回落成「未校验」（没查过、PS 不可用、或这条刚被本次操作改过而快照作废，
    三者对用户都是同一件事：这一格现在不代表防火墙事实）。
    """
    actual = None if states is None else states.get(appnet.norm_path(entry.get("program", "")))
    if actual is None:
        return "未校验（点「刷新状态」查询防火墙）"
    if actual == "missing":
        return "规则已被外部删除（点「禁止联网」可重建）"
    if actual == "drift":
        return "规则动作已被改成非「阻止」，实际不拦（在 wf.msc 里核对）"
    want_block = bool(entry.get("blocked", True))
    if actual == "on":
        return "已禁止联网" if want_block else "不一致：规则在拦，台账却记为放行"
    return "已放行" if not want_block else "不一致：规则已停用，台账却记为禁止"


class AppNetTab(QWidget):
    """应用联网控制：给指定程序加 / 去 Windows 防火墙出站块，实现「这个软件不许联网」。

    构造期只读本地台账、**零系统命令**——MainWindow 测试会整树构造本页，
    任何隐式 netsh 调用都会被 conftest 红线 1 的哨兵击杀；真实拦截状态要用户
    点「刷新状态」才去防火墙查（也是为了避免每次切页都拉一次 PowerShell）。
    """

    def __init__(self, parent=None):
        super().__init__(parent)
        lay = QVBoxLayout(self)
        lay.setContentsMargins(8, 8, 8, 8)

        self.lbl_info = QLabel()
        self.lbl_info.setProperty("hint", True)
        self.lbl_info.setWordWrap(True)
        lay.addWidget(self.lbl_info)

        row = QHBoxLayout()
        self.btn_add = QPushButton("添加程序…")
        self.btn_add.setProperty("primary", True)
        self.btn_add.clicked.connect(self._pick_and_add)
        self.btn_block = QPushButton("禁止联网")
        self.btn_block.clicked.connect(lambda: self._apply(True))
        self.btn_allow = QPushButton("允许联网")
        self.btn_allow.clicked.connect(lambda: self._apply(False))
        self.btn_del = QPushButton("移除")
        self.btn_del.setProperty("danger", True)
        self.btn_del.clicked.connect(self._remove)
        self.btn_refresh = QPushButton("刷新状态")
        self.btn_refresh.clicked.connect(self.refresh)
        for b in (self.btn_add, self.btn_block, self.btn_allow, self.btn_del,
                  self.btn_refresh):
            row.addWidget(b)
        row.addStretch(1)
        lay.addLayout(row)

        self.tbl = make_table(["程序", "完整路径", "联网状态", "加入时间"], select_rows=True)
        self.tbl.setSelectionMode(QAbstractItemView.SingleSelection)
        self.tbl.horizontalHeader().setSectionResizeMode(1, QHeaderView.Stretch)
        self.tbl.setColumnWidth(0, 170)
        self.tbl.setColumnWidth(2, 250)
        self.tbl.itemSelectionChanged.connect(self._refresh_buttons)
        lay.addWidget(self.tbl, 1)

        self.lbl_status = QLabel("")
        self.lbl_status.setProperty("hint", True)
        self.lbl_status.setWordWrap(True)
        lay.addWidget(self.lbl_status)

        self._entries = []
        self._states = None            # None = 尚未向防火墙查过
        self._touched = ""             # 本次写操作影响的程序，成功后作废其快照格
        self._worker = None
        self._load_ledger()

    # ---------------- 数据与渲染 ----------------

    def _load_ledger(self):
        """只读本地台账（零命令）。用于构造期与每次操作后的回显。"""
        self._entries = appnet.load_entries()
        self._render()

    def _render(self):
        # 先记住选中的是**哪个程序**而不是第几行：setRowCount 只裁末行，中间被 pop 后
        # currentRow 会原地落到隔壁程序身上，三个操作按钮随之指向另一个 exe
        picked = self._selected_program()
        self.tbl.setRowCount(len(self._entries))
        for r, e in enumerate(self._entries):
            program = e.get("program", "")
            item0 = QTableWidgetItem(os.path.basename(program))
            item0.setData(Qt.UserRole, program)
            self.tbl.setItem(r, 0, item0)
            self.tbl.setItem(r, 1, QTableWidgetItem(program))
            self.tbl.setItem(r, 2, QTableWidgetItem(_appnet_state_text(e, self._states)))
            self.tbl.setItem(r, 3, QTableWidgetItem(e.get("added", "")))
        target = -1
        if picked:
            for r, e in enumerate(self._entries):
                if appnet.norm_path(e.get("program", "")) == appnet.norm_path(picked):
                    target = r
                    break
        if target >= 0:
            self.tbl.setCurrentCell(target, 0)
        else:
            self.tbl.setCurrentIndex(QModelIndex())   # 原选中程序已被移除：不许按钮指向隔壁
        self._update_info()
        self._refresh_buttons()

    def _update_info(self):
        admin_ok = admin.is_admin()
        self.lbl_info.setText(
            f"共 {len(self._entries)} 个受控程序　·　" +
            ("管理员：可增删规则" if admin_ok else "非管理员：只能查看，改动需管理员权限") +
            "　·　规则即 Windows 防火墙出站块（dir=out action=block），"
            "在 wf.msc 可见可改；只拦该 exe 自身发起的连接，借系统服务（svchost / BITS）"
            "联网的部分不受影响。")

    def _selected_program(self) -> str:
        row = self.tbl.currentRow()
        item = self.tbl.item(row, 0) if row >= 0 else None
        data = item.data(Qt.UserRole) if item else None
        return data or ""

    def _refresh_buttons(self):
        has = bool(self._selected_program())
        admin_ok = admin.is_admin()
        busy = self._worker is not None
        for b in (self.btn_block, self.btn_allow, self.btn_del):
            b.setEnabled(has and admin_ok and not busy)
        self.btn_add.setEnabled(admin_ok and not busy)
        self.btn_refresh.setEnabled(not busy)

    # ---------------- 后台执行 ----------------

    def _start(self, job, touched: str = ""):
        """touched：本次写操作影响的程序路径，成功后作废它在防火墙快照里的那一格。

        快照不作废就会拿**改动前**的实际状态去比**改动后**的台账：点完「允许联网」
        状态栏写着 ✔ 已允许联网，同一行的第三列却显示「已禁止联网」甚至
        「不一致：规则在拦，台账却记为放行」，两句话同屏互相打脸。
        """
        if self._worker:
            return
        self._touched = touched
        self._worker = reap(_AppNetWorker(job), self)
        self._worker.done.connect(self._on_done)
        self._worker.finished.connect(self._on_finished)
        self._refresh_buttons()
        self._worker.start()

    def _on_done(self, result):
        ok, msg, payload = result
        if payload is not None:
            self._entries, self._states = payload
            self._render()
        else:
            touched, self._touched = self._touched, ""
            if ok and touched and isinstance(self._states, dict):
                self._states.pop(appnet.norm_path(touched), None)
            self._load_ledger()          # 台账可能已被这次操作改写
        self.lbl_status.setText(("✔ " if ok else "✘ ") + msg)

    def _on_finished(self):
        self._worker = None
        self._refresh_buttons()

    def refresh(self):
        """向防火墙查一次真实规则状态（PowerShell 不可用时降级为「未校验」）。"""
        def job():
            entries = appnet.load_entries()
            states = appnet.query_states([e["program"] for e in entries])
            msg = "状态已刷新" if states is not None else \
                "已读取清单，但防火墙状态查询失败（PowerShell / NetSecurity 不可用）"
            return True, msg, (entries, states)
        self._start(job)

    # ---------------- 操作 ----------------

    def _pick_and_add(self):
        from PySide6.QtWidgets import QFileDialog
        path, _ = QFileDialog.getOpenFileName(
            self, "选择要控制联网的程序", "", "程序 (*.exe);;所有文件 (*.*)")
        if not path:
            return
        def job():
            ok, msg, _idx = appnet.add_program(path, blocked=True)
            return ok, msg, None
        self._start(job, touched=path)

    def _apply(self, blocked: bool):
        program = self._selected_program()
        if not program:
            return
        def job():
            ok, msg = appnet.set_program_blocked(program, blocked)
            return ok, msg, None
        self._start(job, touched=program)

    def _remove(self):
        program = self._selected_program()
        if not program:
            return
        def job():
            ok, msg = appnet.drop_program(program)
            return ok, msg, None
        self._start(job, touched=program)


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
        # 追加在末尾：palette 的 ping/dns 跳转与既有测试依赖上面各 Tab 的索引
        tabs.addTab(DnsSpeedTab(), "DNS 测速")
        tabs.addTab(AppNetTab(), "应用联网")
        lay.addWidget(tabs)
