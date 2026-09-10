"""telemetry / vpn / admin / autostart 未覆盖分支补齐测试。

所有系统调用（proc.run / run_ps / ShellExecuteW / IsUserAnAdmin）一律显式 mock，
不触网、不写注册表、不触发 UAC。fake/helper 全部内联于本文件。
"""
import ctypes
import sys
import types

import psutil
import pytest

from syskit import admin, autostart, proc, telemetry as tel_mod, vpn
from syskit.proc import CmdResult
from syskit.snapshot import Adapter, Snapshot


# ---------------- 内联 fake / helper ----------------

def _ns(**kw):
    return types.SimpleNamespace(**kw)


def _nic(recv, sent):
    return types.SimpleNamespace(bytes_recv=recv, bytes_sent=sent)


_UNSET = object()


def _ad(name="WLAN", desc=None, status="Up", idx=1, ipv4=_UNSET):
    return Adapter(name=name, desc=desc if desc is not None else name,
                   status=status, if_index=idx,
                   ipv4=["192.168.1.10"] if ipv4 is _UNSET else ipv4)


def _snap(adapters=(), routes=None, persistent=None):
    s = Snapshot()
    s.adapters = list(adapters)
    s.ok = True
    s.default_routes = list(routes or [])
    s.persistent_routes = list(persistent or [])
    s.if_metrics = {}
    return s


def _patch_shell32(monkeypatch, name, replacement):
    """ctypes.windll.shell32 为 LibraryLoader 缓存的单例 WinDLL；
    函数指针首次访问后也缓存在实例上，因此对实例 setattr 即可拦截 admin.py 的调用。"""
    monkeypatch.setattr(ctypes.windll.shell32, name, replacement, raising=False)


def _proc_run_scripted(monkeypatch, codes):
    """把 proc.run 替换为按序返回给定 returncode 的假实现，返回调用记录。"""
    calls = []

    def fake_run(cmd, timeout=20):
        calls.append(list(cmd))
        i = min(len(calls) - 1, len(codes) - 1)
        return CmdResult(code=codes[i], stdout="ok", stderr="")

    monkeypatch.setattr(proc, "run", fake_run)
    return calls


class _FakeEvent:
    """第 N 次 wait() 后置位，让 TelemetryWorker.run() 跑固定圈数后自然退出。"""

    def __init__(self, stop_after=1):
        self.waits = 0
        self._stop_after = stop_after
        self._flag = False

    def is_set(self):
        return self._flag

    def wait(self, timeout=None):
        self.waits += 1
        if self.waits >= self._stop_after:
            self._flag = True
        return True


def _patch_sample_deps(monkeypatch):
    """_sample 依赖的全部系统级数据源替换为假数据。"""
    monkeypatch.setattr(tel_mod.psutil, "net_io_counters",
                        lambda pernic=False: {"以太网": _nic(200, 100)})
    monkeypatch.setattr(tel_mod.psutil, "cpu_percent", lambda interval=None: 7.5)
    monkeypatch.setattr(tel_mod.psutil, "virtual_memory",
                        lambda: _ns(percent=48.0, total=16 * 2 ** 30, available=8 * 2 ** 30))
    monkeypatch.setattr(tel_mod.psutil, "disk_io_counters",
                        lambda: _ns(read_bytes=3000, write_bytes=2000))
    monkeypatch.setattr(tel_mod.psutil, "process_iter", lambda attrs=None: iter([]))


# ================= telemetry =================

class TestFmtNicRates:
    def test_skips_adapter_absent_in_last(self):
        # cur 里有、last 里没有的网卡（刚插入/首拍）不计速率
        down, up = tel_mod._fmt_nic_rates({}, {"以太网": _nic(100, 50)})
        assert (down, up) == (0, 0)

    def test_skips_loopback_and_excluded_adapters(self):
        last = {"Loopback Pseudo-Interface 1": _nic(1, 1),
                "Hyper-V Virtual Ethernet Adapter": _nic(2, 2),
                "以太网": _nic(100, 50)}
        cur = {"Loopback Pseudo-Interface 1": _nic(9, 9),
               "Hyper-V Virtual Ethernet Adapter": _nic(9, 9),
               "以太网": _nic(130, 70)}
        # 环回与虚拟网卡 continue 掉，只计物理网卡
        assert tel_mod._fmt_nic_rates(last, cur) == (30, 20)

    def test_counter_reset_clamped_to_zero(self):
        down, up = tel_mod._fmt_nic_rates(
            {"以太网": _nic(500, 500)}, {"以太网": _nic(100, 100)})
        assert (down, up) == (0, 0)


class TestStop:
    def test_stop_sets_event(self):
        w = tel_mod.TelemetryWorker()
        assert not w._stop_evt.is_set()
        w.stop()
        assert w._stop_evt.is_set()


class TestRefreshGateway:
    def test_refresh_reads_gateway(self, monkeypatch):
        monkeypatch.setattr(tel_mod.icmp, "default_gateway", lambda: "10.0.0.138")
        w = tel_mod.TelemetryWorker()
        w.refresh_gateway()
        assert w._gateway == "10.0.0.138"


class TestWifiSignal:
    @pytest.mark.parametrize("stdout,expected", [
        ("已断开状态连接", None),
        ("State : disconnected", None),
        ("信号 : 84%", 84),
        ("Signal : 65%", 65),
        ("没有任何信号信息", None),
    ])
    def test_read_wifi_signal(self, monkeypatch, stdout, expected):
        monkeypatch.setattr(tel_mod.proc, "run",
                            lambda cmd, timeout=20: CmdResult(0, stdout, ""))
        w = tel_mod.TelemetryWorker()
        assert w._read_wifi_signal() == expected


class TestTopProcs:
    def test_resets_cache_every_60_ticks(self, monkeypatch):
        class P:
            def __init__(self, pid, name, cpu):
                self.pid = pid
                self.info = {"pid": pid, "name": name}
                self._cpu = cpu

            def cpu_percent(self, interval=None):
                return self._cpu

            def memory_info(self):
                return _ns(rss=2048)

        p = P(7, "a.exe", 3.0)
        monkeypatch.setattr(tel_mod.psutil, "process_iter", lambda attrs=None: iter([p]))
        w = tel_mod.TelemetryWorker()
        w._procs = {99: object()}       # 上个快照的陈旧 pid
        w._tickno = 61                  # 61 % 60 == 1 → 重建快照
        out = w._top_procs()
        assert 99 not in w._procs and w._procs[7] is p
        assert out[0]["pid"] == 7 and out[0]["mem"] == 2048

    def test_dead_process_cleaned_up(self, monkeypatch):
        class Dead:
            pid = 11
            info = {"pid": 11, "name": "gone.exe"}

            def cpu_percent(self, interval=None):
                raise psutil.NoSuchProcess(11)

        d = Dead()
        monkeypatch.setattr(tel_mod.psutil, "process_iter", lambda attrs=None: iter([d]))
        w = tel_mod.TelemetryWorker()
        w._procs = {11: d}
        w._tickno = 2
        assert w._top_procs() == []
        assert 11 not in w._procs       # 退出的 pid 被清出缓存

    def test_low_cpu_process_filtered(self, monkeypatch):
        class P:
            def __init__(self, pid, name, cpu):
                self.pid = pid
                self.info = {"pid": pid, "name": name}
                self._cpu = cpu

            def cpu_percent(self, interval=None):
                return self._cpu

            def memory_info(self):
                return _ns(rss=1)

        procs = [P(1, "busy.exe", 5.0), P(2, "idle.exe", 0.1)]   # 0.1 < 0.5 → continue
        monkeypatch.setattr(tel_mod.psutil, "process_iter",
                            lambda attrs=None: iter(procs))
        w = tel_mod.TelemetryWorker()
        w._tickno = 4
        out = w._top_procs()
        assert [x["pid"] for x in out] == [1]

    def test_empty_name_falls_back_to_pid(self, monkeypatch):
        class P:
            pid = 33
            info = {"pid": 33, "name": None}

            def cpu_percent(self, interval=None):
                return 9.9

            def memory_info(self):
                return _ns(rss=1)

        monkeypatch.setattr(tel_mod.psutil, "process_iter", lambda attrs=None: iter([P()]))
        w = tel_mod.TelemetryWorker()
        w._tickno = 4                   # 不触发快照重建
        out = w._top_procs()
        assert out[0]["name"] == "33"


class TestSample:
    def test_first_tick_baseline(self, monkeypatch):
        _patch_sample_deps(monkeypatch)
        w = tel_mod.TelemetryWorker()
        w._tickno = 0                   # → 1：无延迟/WiFi/Top 轮次
        p = w._sample()
        assert w._tickno == 1
        assert "down" not in p and "up" not in p        # 无上一拍基线
        assert p["disk_r"] == 0 and p["disk_w"] == 0    # 磁盘首拍记 0
        assert "lat_gw" not in p and "wifi_signal" not in p and "top_procs" not in p
        assert "today_sent" in p and "today_recv" in p

    def test_second_tick_diffs_and_top(self, monkeypatch):
        _patch_sample_deps(monkeypatch)
        w = tel_mod.TelemetryWorker()
        w._tickno = 1                   # → 2：进入 Top 进程轮次
        w._last_net = {"以太网": _nic(150, 120), "已拔出": _nic(10, 10)}
        w._last_disk = _ns(read_bytes=2500, write_bytes=2600)
        p = w._sample()
        assert p["down"] == 50
        assert p["up"] == 0             # 发送计数回退被钳为 0
        assert p["disk_r"] == 500 and p["disk_w"] == 0
        assert "top_procs" in p
        assert "lat_gw" not in p and "wifi_signal" not in p

    def test_latency_tick(self, monkeypatch):
        _patch_sample_deps(monkeypatch)
        monkeypatch.setattr(tel_mod.icmp, "default_gateway", lambda: "192.168.1.1")
        monkeypatch.setattr(tel_mod.icmp, "ping",
                            lambda host, timeout_ms=1500: 13.0)
        w = tel_mod.TelemetryWorker()
        w._gateway = "192.168.1.1"
        w._tickno = 2                   # → 3：延迟轮次
        p = w._sample()
        assert p["lat_gw"] == 13.0 and p["lat_pub"] == 13.0
        assert p["gateway"] == "192.168.1.1"
        assert "top_procs" not in p and "wifi_signal" not in p

    def test_latency_tick_without_gateway(self, monkeypatch):
        _patch_sample_deps(monkeypatch)
        monkeypatch.setattr(tel_mod.icmp, "ping", lambda host, timeout_ms=1500: None)
        w = tel_mod.TelemetryWorker()
        w._gateway = ""
        w._tickno = 2
        p = w._sample()
        assert p["lat_gw"] is None      # 无网关不发起 ping
        assert p["lat_pub"] is None

    def test_wifi_tick(self, monkeypatch):
        _patch_sample_deps(monkeypatch)
        monkeypatch.setattr(tel_mod.proc, "run",
                            lambda cmd, timeout=20: CmdResult(0, "信号 : 77%", ""))
        w = tel_mod.TelemetryWorker()
        w._tickno = 4                   # → 5：WiFi 轮次
        p = w._sample()
        assert p["wifi_signal"] == 77
        assert "lat_gw" not in p

    def test_disk_counters_none(self, monkeypatch):
        _patch_sample_deps(monkeypatch)
        monkeypatch.setattr(tel_mod.psutil, "disk_io_counters", lambda: None)
        w = tel_mod.TelemetryWorker()
        w._tickno = 0
        p = w._sample()
        assert "disk_r" not in p and "disk_w" not in p
        assert w._last_disk is None


class TestRunLoop:
    @staticmethod
    def _patch_all(monkeypatch):
        _patch_sample_deps(monkeypatch)
        monkeypatch.setattr(tel_mod.icmp, "default_gateway", lambda: "192.168.1.1")
        monkeypatch.setattr(tel_mod.icmp, "ping", lambda host, timeout_ms=1500: 13)
        monkeypatch.setattr(tel_mod.proc, "run",
                            lambda cmd, timeout=20: CmdResult(0, "Signal : 60%", ""))

    def test_run_emits_payload_and_refreshes_gateway_every_300(self, monkeypatch):
        self._patch_all(monkeypatch)
        w = tel_mod.TelemetryWorker()
        w._stop_evt = _FakeEvent(stop_after=1)
        w._tickno = 299                 # 采样后为 300 → 命中 %300 网关刷新
        w._gateway = "192.168.1.1"
        gw_calls = []
        monkeypatch.setattr(w, "refresh_gateway", lambda: gw_calls.append(1))
        seen = []
        w.tick.connect(seen.append)
        w.run()
        assert len(seen) == 1
        payload = seen[0]
        for key in ("cpu", "mem", "mem_used_gb", "disk_r", "disk_w", "lat_gw",
                    "lat_pub", "gateway", "wifi_signal", "top_procs",
                    "today_sent", "today_recv"):
            assert key in payload, key
        assert payload["cpu"] == 7.5 and payload["wifi_signal"] == 60
        assert gw_calls == [1, 1]       # run() 开头一次 + 第 300 拍一次
        assert w._stop_evt.is_set()

    def test_run_saves_traffic_every_60_ticks(self, monkeypatch):
        self._patch_all(monkeypatch)
        w = tel_mod.TelemetryWorker()
        w._stop_evt = _FakeEvent(stop_after=60)
        monkeypatch.setattr(w, "_sample", lambda: None)     # 只验证循环机制
        saves = []
        monkeypatch.setattr(w.traffic, "save_if_dirty", lambda: saves.append(1))
        emitted = []
        w.tick.connect(emitted.append)
        w.run()
        assert saves == [1]             # 第 60 拍落盘一次
        assert emitted == []            # 空 payload 不发射
        assert w._stop_evt.waits == 60

    def test_run_swallows_sample_exception(self, monkeypatch):
        self._patch_all(monkeypatch)
        w = tel_mod.TelemetryWorker()
        w._stop_evt = _FakeEvent(stop_after=2)
        calls = []

        def boom():
            calls.append(1)
            raise RuntimeError("采样失败")

        monkeypatch.setattr(w, "_sample", boom)
        w.run()
        assert len(calls) == 2          # 异常被记录并吞掉，循环继续到停止位


# ================= vpn =================

def _no_proxy():
    return {"enabled": False, "server": "", "autoconfig": False, "pac_url": ""}


class TestListTunnels:
    def test_matches_by_name_when_desc_plain(self):
        s = _snap(adapters=[_ad("uu加速器-VPN", "Ethernet 2", idx=3)])
        out = vpn.list_tunnels(s)
        assert len(out) == 1
        assert out[0]["if_index"] == 3
        assert out[0]["ipv4"] == ["192.168.1.10"]

    def test_ipv4_none_becomes_empty_list(self):
        a = _ad("tailscale0", "Tailscale Tunnel", idx=4, ipv4=None)
        out = vpn.list_tunnels(_snap(adapters=[a]))
        assert out[0]["ipv4"] == []

    def test_skips_non_up_adapters(self):
        s = _snap(adapters=[_ad("wg0", "WireGuard Tunnel", status="Down"),
                            _ad("以太网", "Realtek GigE", idx=1)])
        assert vpn.list_tunnels(s) == []

    def test_snap_defaults_through_snapshot_take(self, monkeypatch):
        # 不传 snap 时经 _snap() → snapshot.take() 取当前状态
        monkeypatch.setattr("syskit.snapshot.take",
                            lambda use_ps=True: _snap(adapters=[_ad("zerotier", "ZeroTier One", idx=6)]))
        out = vpn.list_tunnels()
        assert [t["name"] for t in out] == ["zerotier"]


class TestGuards:
    def test_is_tunnel_up_and_skip_flag(self):
        s = _snap(adapters=[_ad("clash0", "Clash TUN")])
        assert vpn.is_tunnel_up(s) is True
        assert vpn.should_skip_auto_repair(s) is True
        assert vpn.is_tunnel_up(_snap(adapters=[_ad("WLAN", "Intel Wi-Fi")])) is False

    def test_guard_note_text(self):
        assert "VPN" in vpn.auto_repair_guard_note()
        assert "跳过" in vpn.auto_repair_guard_note()


class TestChecklistBranches:
    def test_enabled_proxy_hint(self, monkeypatch):
        monkeypatch.setattr(vpn.proxy, "get_proxy_status",
                            lambda: {"enabled": True, "server": "127.0.0.1:7890",
                                     "autoconfig": False, "pac_url": ""})
        steps = vpn.vpn_checklist(_snap())
        p = next(x for x in steps if x["name"] == "系统代理")
        assert p["level"] == "warn"
        assert "127.0.0.1:7890" in p["detail"]

    def test_persistent_default_route_warn(self, monkeypatch):
        monkeypatch.setattr(vpn.proxy, "get_proxy_status", _no_proxy)
        s = _snap(persistent=[{"dest": "0.0.0.0/0", "nextHop": "192.168.1.1"},
                              {"dest": "10.8.0.0/24", "nextHop": "10.8.0.1"}])
        steps = vpn.vpn_checklist(s)
        pr = next(x for x in steps if x["name"] == "持久默认路由")
        assert pr["level"] == "warn"
        assert "192.168.1.1" in pr["detail"]    # 只汇总 0.0.0.0/0 条目

    def test_pac_autoconfig_branch(self, monkeypatch):
        monkeypatch.setattr(vpn.proxy, "get_proxy_status",
                            lambda: {"enabled": False, "server": "", "autoconfig": True,
                                     "pac_url": "http://pac.example.com:8080/wpad.dat"})
        steps = vpn.vpn_checklist(_snap())
        p = next(x for x in steps if x["name"] == "系统代理")
        assert p["level"] == "warn"
        assert "pac.example.com" in p["detail"]

    def test_no_proxy_branch(self, monkeypatch):
        monkeypatch.setattr(vpn.proxy, "get_proxy_status", _no_proxy)
        steps = vpn.vpn_checklist(_snap())
        p = next(x for x in steps if x["name"] == "系统代理")
        assert p["level"] == "ok" and "未启用系统代理" in p["detail"]

    def test_multi_default_route_drift(self, monkeypatch):
        monkeypatch.setattr(vpn.proxy, "get_proxy_status", _no_proxy)
        adapters = [_ad("以太网", "Realtek GigE", idx=1),
                    _ad("WLAN", "Intel Wi-Fi", idx=2)]
        routes = [{"if_index": 1, "nextHop": "192.168.1.1", "metric": 0, "if_metric": 25},
                  {"if_index": 2, "nextHop": "10.0.0.1", "metric": 5, "if_metric": 30}]
        steps = vpn.vpn_checklist(_snap(adapters=adapters, routes=routes))
        d = next(x for x in steps if x["name"] == "默认路由出口")
        assert d["level"] == "warn" and "漂移" in d["detail"]
        assert "以太网→192.168.1.1(跃点25)" in d["detail"]
        assert "WLAN→10.0.0.1(跃点35)" in d["detail"]

    def test_single_default_route_physical(self, monkeypatch):
        monkeypatch.setattr(vpn.proxy, "get_proxy_status", _no_proxy)
        adapters = [_ad("WLAN", "Intel Wi-Fi", idx=2)]
        routes = [{"if_index": 2, "nextHop": "192.168.1.1", "metric": 0, "if_metric": 25}]
        steps = vpn.vpn_checklist(_snap(adapters=adapters, routes=routes))
        d = next(x for x in steps if x["name"] == "默认路由出口")
        assert d["ok"] and d["level"] == "ok"
        assert "WLAN → 192.168.1.1" in d["detail"]
        assert "隧道在用" not in d["detail"]

    def test_single_default_route_tunnel(self, monkeypatch):
        monkeypatch.setattr(vpn.proxy, "get_proxy_status", _no_proxy)
        adapters = [_ad("wintun0", "Wintun Userspace Driver", idx=9)]
        routes = [{"if_index": 9, "nextHop": None, "metric": 0, "if_metric": 0}]
        steps = vpn.vpn_checklist(_snap(adapters=adapters, routes=routes))
        d = next(x for x in steps if x["name"] == "默认路由出口")
        assert d["ok"]
        assert "隧道在用" in d["detail"]
        assert "→ ?" in d["detail"]     # nextHop 缺失显示占位符


class TestSummarize:
    def test_counts_fails_and_warns(self):
        steps = [{"level": "fail"}, {"level": "warn"}, {"level": "warn"}]
        assert vpn.summarize(steps) == "发现 1 项异常、2 项提醒"

    def test_warn_only(self):
        assert vpn.summarize([{"level": "warn"}, {"level": "ok"}]) == "基本可用，有 1 项需注意"

    def test_clean(self):
        assert vpn.summarize([{"level": "ok"}, {"level": "info"}]) == "未发现明显 VPN 相关冲突"


# ================= admin =================

class TestIsAdmin:
    def test_true_when_shell32_says_yes(self, monkeypatch):
        _patch_shell32(monkeypatch, "IsUserAnAdmin", lambda: 1)
        assert admin.is_admin() is True

    def test_false_when_shell32_says_no(self, monkeypatch):
        _patch_shell32(monkeypatch, "IsUserAnAdmin", lambda: 0)
        assert admin.is_admin() is False

    def test_false_when_api_raises(self, monkeypatch):
        def boom():
            raise OSError("权限查询失败")

        _patch_shell32(monkeypatch, "IsUserAnAdmin", boom)
        assert admin.is_admin() is False

    def test_false_on_non_windows(self, monkeypatch):
        monkeypatch.setattr(sys, "platform", "linux")
        assert admin.is_admin() is False


class TestIsFrozen:
    def test_flag_reflected(self, monkeypatch):
        monkeypatch.delattr(sys, "frozen", raising=False)
        assert admin.is_frozen() is False
        monkeypatch.setattr(sys, "frozen", True, raising=False)
        assert admin.is_frozen() is True


class TestElevateAndRestart:
    def test_source_mode_builds_module_params(self, monkeypatch):
        calls = []
        _patch_shell32(monkeypatch, "ShellExecuteW",
                       lambda *a: calls.append(a) or 33)
        monkeypatch.setattr(sys, "argv", ["syskit", "--fix", "dns"], raising=False)
        assert admin.elevate_and_restart() is True      # 33 > 32
        _, verb, target, params, _dir, show = calls[0]
        assert verb == "runas"
        assert target == sys.executable
        assert params == "-m syskit --fix dns"
        assert show == admin.SW_SHOWNORMAL

    def test_source_mode_empty_args(self, monkeypatch):
        calls = []
        _patch_shell32(monkeypatch, "ShellExecuteW",
                       lambda *a: calls.append(a) or 33)
        monkeypatch.setattr(sys, "argv", ["syskit"], raising=False)
        admin.elevate_and_restart()
        assert calls[0][3] == "-m syskit "              # 空参数回退到 argv[1:]

    def test_frozen_mode_uses_cli_args_and_reports_failure(self, monkeypatch):
        calls = []
        _patch_shell32(monkeypatch, "ShellExecuteW",
                       lambda *a: calls.append(a) or 5)
        monkeypatch.setattr(sys, "frozen", True, raising=False)
        assert admin.elevate_and_restart(["--tray"]) is False   # 5 <= 32
        assert calls[0][2] == sys.executable
        assert calls[0][3] == "--tray"

    def test_non_windows_returns_false(self, monkeypatch):
        monkeypatch.setattr(sys, "platform", "linux")
        assert admin.elevate_and_restart(["--x"]) is False


# ================= autostart =================

class TestCommand:
    def test_format(self):
        assert autostart._command() == f'"{sys.executable}" --minimized'


class TestEnable:
    def test_requires_admin(self, monkeypatch):
        calls = _proc_run_scripted(monkeypatch, [0])
        monkeypatch.setattr(admin, "is_admin", lambda: False)
        assert autostart.enable() is False
        assert calls == []              # 未提权不得碰 schtasks

    def test_refuses_source_mode(self, monkeypatch):
        calls = _proc_run_scripted(monkeypatch, [0])
        monkeypatch.setattr(admin, "is_admin", lambda: True)
        monkeypatch.delattr(sys, "frozen", raising=False)
        assert autostart.enable() is False
        assert calls == []              # 源码模式直接拒绝

    def test_creates_task_and_cleans_legacy(self, monkeypatch):
        calls = _proc_run_scripted(monkeypatch, [0, 0])
        monkeypatch.setattr(admin, "is_admin", lambda: True)
        monkeypatch.setattr(sys, "frozen", True, raising=False)
        assert autostart.enable() is True
        assert len(calls) == 2
        legacy, create = calls
        assert "/Delete" in legacy
        assert legacy[legacy.index("/TN") + 1] == autostart.LEGACY_TASK_NAME
        assert "/Create" in create
        assert create[create.index("/TN") + 1] == autostart.TASK_NAME
        assert create[create.index("/TR") + 1] == autostart._command()
        assert "HIGHEST" in create

    def test_returns_false_when_create_fails(self, monkeypatch):
        _proc_run_scripted(monkeypatch, [0, -1])
        monkeypatch.setattr(admin, "is_admin", lambda: True)
        monkeypatch.setattr(sys, "frozen", True, raising=False)
        assert autostart.enable() is False


class TestDisable:
    def test_deletes_both_tasks(self, monkeypatch):
        calls = _proc_run_scripted(monkeypatch, [0, 0])
        assert autostart.disable() is True
        assert len(calls) == 2
        assert calls[0][calls[0].index("/TN") + 1] == autostart.TASK_NAME
        assert calls[1][calls[1].index("/TN") + 1] == autostart.LEGACY_TASK_NAME

    def test_new_task_delete_failure_reports_false(self, monkeypatch):
        _proc_run_scripted(monkeypatch, [-1, 0])
        assert autostart.disable() is False


class TestEnabled:
    def test_new_task_exists(self, monkeypatch):
        calls = _proc_run_scripted(monkeypatch, [0])
        assert autostart.enabled() is True
        assert len(calls) == 1          # 新任务命中即短路

    def test_falls_back_to_legacy_task(self, monkeypatch):
        calls = _proc_run_scripted(monkeypatch, [-1, 0])
        assert autostart.enabled() is True
        assert len(calls) == 2
        assert calls[1][calls[1].index("/TN") + 1] == autostart.LEGACY_TASK_NAME

    def test_none_exists(self, monkeypatch):
        _proc_run_scripted(monkeypatch, [-1, -1])
        assert autostart.enabled() is False
