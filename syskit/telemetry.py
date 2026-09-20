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

# 只有「窗口可见且停在对应页」时才需要的昂贵分组：
#   lat   —— 每 3 秒两轮 ICMP（概览页延迟卡片/曲线）
#   wifi  —— 每 5 秒一次 netsh wlan 子进程（概览页信号曲线）
#   procs —— 每 2 秒一遍进程表（系统页 Top 进程）
# 托盘常驻是绝大多数时间的常态，那时这三组采了没有任何消费者。
HEAVY_GROUPS = ("lat", "wifi", "procs")
# 各页面看得懂的分组口径（main_window._refresh_telemetry_scope 共用，避免双源字面量）
GROUPS_DASH = frozenset({"lat", "wifi"})
GROUPS_SYSTEM = frozenset({"procs"})


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
        self._need = set(HEAVY_GROUPS)
        self._gw_ts = 0           # 上次刷新网关时的 tick（刷新走 PowerShell，见 _sample）
        self.traffic = TrafficStore()
        psutil.cpu_percent(interval=None)   # 预热第一次调用

    def stop(self):
        self._stop_evt.set()

    def set_need(self, need):
        """告诉引擎「界面现在看得懂哪些分组」，缺席的昂贵采样本轮直接跳过。

        默认全开：不接线（CLI / 测试 / 独立使用）时行为与从前一致。
        None 等价于「全都要」。速率、CPU/内存、今日流量恒采（托盘与流量统计要）。
        """
        self._need = set(HEAVY_GROUPS) if need is None else set(need)

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

        if self._tickno % 3 == 0 and "lat" in self._need:     # 3 秒一轮延迟
            # 刷新网关要起一次 PowerShell（秒级）。旧写法在 run() 里每 300 拍无条件刷，
            # 但它唯一的服务对象就是这里的网关延迟——托盘常驻时（约 288 次/天）全是空转。
            if self._tickno - self._gw_ts >= 300:
                self.refresh_gateway()
                self._gw_ts = self._tickno
            payload["lat_gw"] = icmp.ping(self._gateway) if self._gateway else None
            payload["lat_pub"] = icmp.ping("223.5.5.5")
            payload["gateway"] = self._gateway
        if self._tickno % 5 == 0 and "wifi" in self._need:    # 5 秒一轮 WiFi 信号
            self._wifi_signal = self._read_wifi_signal()
            payload["wifi_signal"] = self._wifi_signal
        if self._tickno % 2 == 0 and "procs" in self._need:   # 2 秒一轮 Top 进程
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
        每次新建对象会导致读数恒为接近 0。

        优化：优先走缓存（self._procs），仅当缓存为空（首启/全退出/每 60 秒
        重建轮）才 process_iter 全表枚举；非重建轮对已退出 pid 清缓存。
        注意 _top_procs 只在偶数 tick 被调用（%2 门），重建轮必须取偶数
        （%60==0），否则该分支永不命中、新启动的进程永远进不了表。
        """
        rebuild = self._tickno % 60 == 0 or not self._procs
        if self._tickno % 60 == 0:
            self._procs = {}
        procs = []
        if rebuild:
            it = psutil.process_iter(["pid", "name"])
        else:
            it = list(self._procs.values())     # 仅缓存对象，不做全表枚举
        seen = set()
        for p in it:
            try:
                pid = p.pid
                seen.add(pid)
                if rebuild:
                    cached = p
                    self._procs[pid] = cached
                else:
                    cached = self._procs.get(pid, p)
                cpu = cached.cpu_percent(interval=None)
                if cpu < 0.5:
                    continue
                mem = cached.memory_info().rss
                # 枚举属性里的 name 可能为空（进程受保护/快照拿不到），这时直接写
                # str(pid) 会让下面「问一次活对象」的兜底永远走不到——真名就再也拿不到了。
                name = (p.info or {}).get("name") or ""
                if not name:
                    try:
                        name = cached.name() or str(pid)
                    except Exception:
                        name = str(pid)
                procs.append({"pid": pid, "name": name, "cpu": cpu, "mem": mem})
            except (psutil.NoSuchProcess, psutil.AccessDenied, psutil.ZombieProcess):
                self._procs.pop(getattr(p, "pid", -1), None)
                continue
            except Exception:
                # 单个进程信息读取异常不应中断整轮采样
                continue
        if not rebuild:
            for pid in list(self._procs.keys()):
                if pid not in seen:             # 清掉已退出的 pid
                    del self._procs[pid]
        procs.sort(key=lambda x: x["cpu"], reverse=True)
        return procs[:n]
