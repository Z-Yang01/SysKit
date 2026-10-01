"""syskit/telemetry.py 轻量测试：停止事件、网卡过滤、top 进程缓存复用、按可见性分工采样。"""
import types

from syskit import telemetry as tel_mod
from syskit.telemetry import TelemetryWorker, _fmt_nic_rates, HEAVY_GROUPS


def test_stop_evt_used_not_shadowing():
    w = TelemetryWorker()
    # 停止用 Event，不把实例属性叫 _stop（避免和 Thread 私有方法混淆）
    assert isinstance(w._stop_evt, type(tel_mod.threading.Event()))
    assert not hasattr(w, "_stop") or not isinstance(getattr(w, "_stop", None), type(w._stop_evt))
    w.stop()
    assert w._stop_evt.is_set()


def test_fmt_nic_rates_filters_loopback_and_physical():
    def nic(recv, sent):
        return types.SimpleNamespace(bytes_recv=recv, bytes_sent=sent)

    last = {"以太网": nic(100, 50), "Loopback Pseudo Interface 1": nic(999, 999),
            "VMware Network Adapter VMnet8": nic(500, 500)}
    cur = {"以太网": nic(130, 70), "Loopback Pseudo Interface 1": nic(1000, 1000),
           "VMware Network Adapter VMnet8": nic(600, 600)}
    down, up = _fmt_nic_rates(last, cur)
    assert down == 30 and up == 20                  # 只计物理网卡差值


def test_top_procs_reuses_process_objects(monkeypatch):
    """cpu_percent 需要跨 tick 基线：同一 pid 必须复用同一 Process 实例。"""
    created = []

    class FakeProc:
        def __init__(self, pid, name):
            created.append(pid)
            self.pid = pid
            self.info = {"pid": pid, "name": name}
            self._cpu = 12.5

        def cpu_percent(self, interval=None):
            return self._cpu

        def memory_info(self):
            return types.SimpleNamespace(rss=1024 * 1024)

    fake_procs = [FakeProc(11, "busy.exe"), FakeProc(22, "idle.exe")]
    fake_procs[1]._cpu = 0.1

    def fake_iter(attrs=None):
        return iter(fake_procs)

    monkeypatch.setattr(tel_mod.psutil, "process_iter", fake_iter)

    w = TelemetryWorker.__new__(TelemetryWorker)   # 跳过 __init__ 里的真实 psutil 预热
    w._procs = {}
    w._tickno = 2

    out1 = w._top_procs()
    assert len(out1) == 1 and out1[0]["name"] == "busy.exe"
    n_after_first = len(created)

    w._tickno = 3
    out2 = w._top_procs()
    assert len(out2) == 1
    assert len(created) == n_after_first            # 未再新建 Process
    assert w._procs[11] is fake_procs[0]


def test_top_procs_name_fallback_and_dead_pid_eviction(monkeypatch):
    """两条冷门分支：info 里没名字要回落到 Process.name()（它自己也可能抛），非重建轮
    还要把已失效的 pid 从缓存里摘掉，否则缓存只增不减。"""
    class Proc:
        def __init__(self, pid, name_raises=False):
            self.pid = pid
            self.info = {"pid": pid, "name": ""}   # 枚举属性里没名字
            self._name_raises = name_raises

        def cpu_percent(self, interval=None):
            return 9.9

        def memory_info(self):
            return types.SimpleNamespace(rss=2048)

        def name(self):
            if self._name_raises:
                raise tel_mod.psutil.AccessDenied("被保护进程")
            return "renamed.exe"

    monkeypatch.setattr(tel_mod.psutil, "process_iter",
                        lambda attrs=None: iter([Proc(33), Proc(44, name_raises=True)]))
    w = TelemetryWorker.__new__(TelemetryWorker)
    w._procs = {}
    w._tickno = 2
    out = w._top_procs()                            # 重建轮：两个都入缓存
    assert [p["name"] for p in out] == ["renamed.exe", "44"]   # 后者 name() 抛 → 退回 pid
    assert set(w._procs) == {33, 44}

    w._procs[55] = object()                         # 缓存里一个取 pid 就抛的失效对象
    w._tickno = 4                                   # 偶数非重建轮：只遍历缓存
    out2 = w._top_procs()
    assert len(out2) == 2
    assert 55 not in w._procs                       # 本轮没进 seen → 清掉
    assert set(w._procs) == {33, 44}


# ---------------- 按可见性分工采样 ----------------

def _bare_worker(monkeypatch):
    """不起线程、psutil/icmp/proc 全假的裸 worker，用来数「某项采样到底跑了几次」。"""
    w = TelemetryWorker.__new__(TelemetryWorker)
    w._stop_evt = tel_mod.threading.Event()
    w._last_net = None
    w._last_disk = None
    w._tickno = 0
    w._gateway = "192.168.1.1"
    w._wifi_signal = None
    w._procs = {}
    w._need = set(HEAVY_GROUPS)
    w._gw_ts = 0
    w.traffic = types.SimpleNamespace(add=lambda u, d: None, today=lambda: (1, 2))
    w.rate_stats = types.SimpleNamespace(add=lambda u, d, s=None: None,
                                         set_ssid=lambda s: None)
    w._wifi_ssid = ""

    calls = {"ping": [], "netsh": []}
    nic = types.SimpleNamespace(bytes_recv=1000, bytes_sent=500)
    monkeypatch.setattr(tel_mod.psutil, "net_io_counters",
                        lambda pernic=False: {"以太网": nic})
    monkeypatch.setattr(tel_mod.psutil, "cpu_percent", lambda interval=None: 7.0)
    monkeypatch.setattr(tel_mod.psutil, "virtual_memory",
                        lambda: types.SimpleNamespace(percent=40.0, total=10, available=6))
    monkeypatch.setattr(tel_mod.psutil, "disk_io_counters", lambda: None)
    monkeypatch.setattr(tel_mod.psutil, "process_iter", lambda attrs=None: iter(()))
    monkeypatch.setattr(tel_mod.icmp, "ping",
                        lambda host, timeout_ms=1500: calls["ping"].append(host) or 5.0)
    monkeypatch.setattr(tel_mod.proc, "run",
                        lambda cmd, timeout=20: calls["netsh"].append(cmd)
                        or tel_mod.proc.CmdResult(0, "信号 : 72%\n", ""))
    return w, calls


def test_default_samples_everything(monkeypatch):
    """不接线（CLI / 测试 / 独立使用）时行为与改造前一致：昂贵项照采。"""
    w, calls = _bare_worker(monkeypatch)
    keys = set().union(*[w._sample() for _ in range(6)])
    assert {"lat_gw", "lat_pub", "wifi_signal", "top_procs"} <= keys
    assert len(calls["ping"]) == 4                   # tick 3 与 6 各两轮
    assert len(calls["netsh"]) == 1                  # tick 5 一次 netsh wlan


def test_scope_without_dashboard_drops_heavy_samples(monkeypatch):
    """托盘驻留（need 为空集）：一个子进程、一次 ICMP 都不发，速率与流量照常。"""
    w, calls = _bare_worker(monkeypatch)
    w.set_need([])
    payloads = [w._sample() for _ in range(6)]
    assert calls["ping"] == [] and calls["netsh"] == []
    assert not {"lat_gw", "lat_pub", "wifi_signal", "top_procs"} & set().union(*payloads)
    d = payloads[-1]
    assert d["down"] == 0 and d["cpu"] == 7.0        # 托盘与系统基础项仍在
    assert d["today_sent"] == 1


def test_set_need_none_restores_full_scope(monkeypatch):
    w, calls = _bare_worker(monkeypatch)
    w.set_need([])
    w.set_need(None)
    keys = set().union(*[w._sample() for _ in range(6)])
    assert "wifi_signal" in keys and len(calls["netsh"]) == 1


# ---------------- 网关刷新（一次 PowerShell）随消费方走 ----------------

def _counting_refresh(monkeypatch, w):
    gw = []
    monkeypatch.setattr(w, "refresh_gateway", lambda: gw.append(w._tickno))
    return gw


def test_gateway_refresh_keeps_five_minute_cadence_when_consumed(monkeypatch):
    """仪表盘在看延迟：仍然每 300 拍刷一次网关（与改造前同频）。"""
    w, _ = _bare_worker(monkeypatch)
    gw = _counting_refresh(monkeypatch, w)
    for _ in range(600):
        w._sample()
    assert gw == [300, 600]


def test_gateway_idle_in_tray_never_spawns_powershell(monkeypatch):
    """托盘常驻（need 空集）：600 拍一次网关都不刷，也不再空转 PowerShell。"""
    w, _ = _bare_worker(monkeypatch)
    w.set_need([])
    gw = _counting_refresh(monkeypatch, w)
    for _ in range(600):
        w._sample()
    assert gw == []


def test_gateway_refreshed_once_on_returning_to_dashboard(monkeypatch):
    """挂机 2 小时（网关可能已因漫游/DHCP 变了）后回到仪表盘：第一拍就补刷一次，
    之后重新按 300 拍节奏走，而不是等下一个 %300 窗口。"""
    w, _ = _bare_worker(monkeypatch)
    w.set_need([])
    gw = _counting_refresh(monkeypatch, w)
    for _ in range(500):
        w._sample()
    assert gw == []
    w.set_need({"lat"})
    keys = set()
    for _ in range(6):                             # 501..506：3 的倍数有 501、504
        keys |= set(w._sample())
    assert gw == [501]                             # 陈旧超 300 拍 → 命中即刷一次
    assert "lat_gw" in keys
    for _ in range(400):                           # 507..906：下一个刷新点只有 801
        w._sample()
    assert gw == [501, 801]
