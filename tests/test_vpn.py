"""syskit/vpn.py 与 monitor VPN 护栏测试。"""
from types import SimpleNamespace

import pytest

from syskit import monitor as mon_mod
from syskit import vpn
from syskit.monitor import Monitor
from syskit.repairs import RepairResult
from syskit.snapshot import Adapter, Snapshot


def _snap(tunnels=(), routes=None, persistent=None):
    s = Snapshot()
    s.adapters = list(tunnels)
    s.ok = True
    s.default_routes = list(routes or [])
    s.persistent_routes = list(persistent or [])
    s.if_metrics = {}
    return s


def _ad(name="Clash TAP", desc=None, status="Up", idx=12):
    return Adapter(name=name, desc=desc if desc is not None else name,
                   status=status, if_index=idx, ipv4=["10.0.0.2"])


class TestListTunnels:
    def test_detects_up_tunnel(self):
        s = _snap(tunnels=[_ad("wintun0", "Wintun Userspace Driver"),
                           _ad("WLAN", "Intel Wi-Fi", idx=5)])
        outs = vpn.list_tunnels(s)
        assert len(outs) == 1 and outs[0]["name"] == "wintun0"

    def test_ignores_down_tunnel(self):
        s = _snap(tunnels=[_ad("OpenVPN", "OpenVPN TAP", status="Down")])
        assert vpn.list_tunnels(s) == []

    def test_is_tunnel_up_and_skip(self):
        s = _snap(tunnels=[_ad("Clash", "Clash TUN Adapter")])
        assert vpn.is_tunnel_up(s) is True
        assert vpn.should_skip_auto_repair(s) is True
        s2 = _snap(tunnels=[_ad("WLAN", "Intel Wi-Fi")])
        assert vpn.is_tunnel_up(s2) is False


class TestChecklist:
    def test_tunnel_up_ok(self):
        s = _snap(tunnels=[_ad("wg0", "WireGuard Tunnel")])
        steps = vpn.vpn_checklist(s)
        names = [x["name"] for x in steps]
        assert "VPN/隧道网卡" in names
        t = next(x for x in steps if x["name"] == "VPN/隧道网卡")
        assert t["ok"] and t["level"] == "ok"

    def test_proxy_warn_when_no_tunnel(self, monkeypatch):
        monkeypatch.setattr(vpn.proxy, "get_proxy_status",
                            lambda: {"enabled": True, "server": "127.0.0.1:7890",
                                     "autoconfig": False, "pac_url": ""})
        steps = vpn.vpn_checklist(_snap())
        p = next(x for x in steps if x["name"] == "系统代理")
        assert p["level"] == "warn"
        assert "7890" in p["detail"]

    def test_persistent_route_warn(self):
        s = _snap(persistent=[{"dest": "0.0.0.0/0", "nextHop": "192.168.1.1"}])
        steps = vpn.vpn_checklist(s)
        pr = next(x for x in steps if x["name"] == "持久默认路由")
        assert pr["level"] == "warn"


class TestMonitorGuard:
    def test_ladder_skips_disruptive_when_tunnel(self, monkeypatch):
        m = Monitor()
        executed = []

        class FakeAct:
            def __init__(self, fid):
                self.id = fid
                self.title = fid

            def execute(self, param):
                executed.append(self.id)
                return RepairResult(True, "ok")

        monkeypatch.setattr(mon_mod.repairs, "get_action",
                            lambda fid: FakeAct(fid))
        monkeypatch.setattr(mon_mod, "probe_quick", lambda: False)
        monkeypatch.setattr(mon_mod.time, "sleep", lambda s: None)
        monkeypatch.setattr("syskit.snapshot.take",
                            lambda use_ps=True: _snap(tunnels=[_ad("tun", "WireGuard")]))
        m._run_ladder(3)
        assert "renew_dhcp" not in executed
        assert "restart_wlan" not in executed
        assert "toggle_adapter" not in executed
        assert "flush_dns" in executed
        assert "start_nlasvc" in executed

    def test_ladder_full_without_tunnel(self, monkeypatch):
        m = Monitor()
        executed = []

        class FakeAct:
            def __init__(self, fid):
                self.id = fid
                self.title = fid

            def execute(self, param):
                executed.append(self.id)
                return RepairResult(False, "x")

        monkeypatch.setattr(mon_mod.repairs, "get_action",
                            lambda fid: FakeAct(fid))
        monkeypatch.setattr(mon_mod, "probe_quick", lambda: False)
        monkeypatch.setattr(mon_mod.time, "sleep", lambda s: None)
        monkeypatch.setattr("syskit.snapshot.take",
                            lambda use_ps=True: _snap(tunnels=[_ad("WLAN", "Intel")]))
        m._run_ladder(3)
        assert executed == ["flush_dns", "start_nlasvc", "renew_dhcp",
                            "restart_wlan", "toggle_adapter"]
