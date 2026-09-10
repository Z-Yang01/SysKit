"""syskit/telemetry.py 轻量测试：停止事件、网卡过滤、top 进程缓存复用。"""
import types

from syskit import telemetry as tel_mod
from syskit.telemetry import TelemetryWorker, _fmt_nic_rates


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
