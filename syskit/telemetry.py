"""遥测引擎：1 秒采样网络速率 / CPU / 内存 / 磁盘，3 秒 ICMP 延迟，5 秒 WiFi 信号，Top 进程。

Qt Signal tick(dict) 每秒推送，UI 侧消费（仪表盘/托盘悬浮速率/系统页）。
"""
import re
import threading

import psutil
from PySide6.QtCore import QThread, Signal

from . import icmp, proc
from .stores import TrafficStore
from .logger import LOGGER

_PHYSICAL_EXCLUDE = ("loopback", "vmnet", "vmware", "vethernet", "bluetooth", "wan miniport",
                     "tap", "wintun", "utun", "vgate", "hyper-v")


def _fmt_nic_rates(last, cur):
    """按物理特征过滤网卡，返回 (down_Bps, up_Bps)。"""
    down = up = 0
    for name, c in cur.items():
        lname = name.lower()
        if lname == "loopback" or any(x in lname for x in _PHYSICAL_EXCLUDE):
            continue
        p = last.get(name)
        if not p:
            continue
        down += max(0, c.bytes_recv - p.bytes_recv)
        up += max(0, c.bytes_sent - p.bytes_sent)
    return down, up


class TelemetryWorker(QThread):
    tick = Signal(dict)

    def __init__(self, parent=None):
        super().__init__(parent)
        # 不可命名为 _stop：与 threading.Thread 习惯对齐，避免与其它线程封装混淆
        # （本类基于 QThread，应用 wait() 而非 join()）
        self._stop_evt = threading.Event()
        self._last_net = None
        self._last_disk = None
        self._tickno = 0
        self._gateway = ""
        self._wifi_signal = None
        self._procs = {}          # pid -> psutil.Process（跨 tick 复用，保证 cpu_percent 有基线）
        self.traffic = TrafficStore()
        psutil.cpu_percent(interval=None)   # 预热第一次调用

    def stop(self):
        self._stop_evt.set()

    def refresh_gateway(self):
        self._gateway = icmp.default_gateway()

    def run(self):
        self.refresh_gateway()
        save_counter = 0
        while not self._stop_evt.is_set():
            try:
                payload = self._sample()
                if payload:
                    self.tick.emit(payload)
            except Exception:
                LOGGER.exception("遥测采样异常")
            save_counter += 1
            if save_counter >= 60:
                self.traffic.save_if_dirty()
                save_counter = 0
            if self._tickno % 300 == 0:      # 每 5 分钟刷新一次网关
                self.refresh_gateway()
            self._stop_evt.wait(1)

    def _sample(self):
        self._tickno += 1
        now = psutil.net_io_counters(pernic=True)
        payload = {}

        if self._last_net is not None:
            down, up = _fmt_nic_rates(self._last_net, now)
            self.traffic.add(up, down)
            payload["down"] = down
            payload["up"] = up
        self._last_net = now

        payload["cpu"] = psutil.cpu_percent(interval=None)
        mem = psutil.virtual_memory()
        payload["mem"] = mem.percent
        payload["mem_used_gb"] = (mem.total - mem.available) / 2**30

        disk = psutil.disk_io_counters()
        if disk and self._last_disk:
            payload["disk_r"] = max(0, disk.read_bytes - self._last_disk.read_bytes)
            payload["disk_w"] = max(0, disk.write_bytes - self._last_disk.write_bytes)
        elif disk:
            payload["disk_r"] = payload["disk_w"] = 0
        self._last_disk = disk

        if self._tickno % 3 == 0:            # 3 秒一轮延迟
            payload["lat_gw"] = icmp.ping(self._gateway) if self._gateway else None
            payload["lat_pub"] = icmp.ping("223.5.5.5")
            payload["gateway"] = self._gateway
        if self._tickno % 5 == 0:            # 5 秒一轮 WiFi 信号
            self._wifi_signal = self._read_wifi_signal()
            payload["wifi_signal"] = self._wifi_signal
        if self._tickno % 2 == 0:            # 2 秒一轮 Top 进程
            payload["top_procs"] = self._top_procs()
        sent, recv = self.traffic.today()
        payload["today_sent"] = sent
        payload["today_recv"] = recv
        return payload

    def _read_wifi_signal(self):
        """当前 WiFi 信号百分比；未连接返回 None。netsh 约 100ms，5 秒一次可接受。"""
        r = proc.run(["netsh", "wlan", "show", "interfaces"], timeout=8)
        if "已断开" in r.stdout or "disconnected" in r.stdout.lower():
            return None
        m = re.search(r"(?:信号|Signal)\s*[:：]\s*(\d+)%", r.stdout)
        return int(m.group(1)) if m else None

    def _top_procs(self, n=8):
        """Top 进程。复用 psutil.Process 实例：cpu_percent 需要跨调用基线，
        每次新建对象会导致读数恒为接近 0。"""
        # 每约 60 秒重建一次进程快照，丢掉已退出的 pid
        if self._tickno % 60 == 1:
            self._procs = {}
        procs = []
        for p in psutil.process_iter(["pid", "name"]):
            try:
                pid = p.pid
                cached = self._procs.get(pid)
                if cached is None:
                    cached = p
                    self._procs[pid] = cached
                cpu = cached.cpu_percent(interval=None)
                if cpu < 0.5:
                    continue
                mem = cached.memory_info().rss
                procs.append({"pid": pid, "name": p.info["name"] or str(pid),
                              "cpu": cpu, "mem": mem})
            except (psutil.NoSuchProcess, psutil.AccessDenied):
                self._procs.pop(getattr(p, "pid", -1), None)
                continue
        procs.sort(key=lambda x: x["cpu"], reverse=True)
        return procs[:n]
