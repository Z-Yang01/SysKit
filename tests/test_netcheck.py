"""syskit/netcheck.py 单元测试：15 项网络诊断的状态判定逻辑（全程无真实命令/网络/注册表读）。

Mock 边界（每个测试显式打桩，配合 conftest 哨兵双保险）：
- proc.run            → netsh/ping 等命令返回可控 stdout
- winregread.values / subkeys / pnpcapabilities → 注册表读返回假数据
- dnsquery.query_safe / resolve_system → DNS 解析返回假 IP
- netcheck._http_get / urllib.request.urlopen → HTTP 探测返回假响应
- proxy.get_proxy_status（经由 winregread.values 间接）→ 代理状态
- snapshot.take → run_all 用假快照
"""
import urllib.error
import urllib.request

import pytest

from syskit import config, netcheck
from syskit.snapshot import Adapter, Snapshot


# ---------------- 假数据工厂 ----------------

def make_adapter(**kw):
    defaults = dict(name="WLAN", desc="Intel(R) Wi-Fi 6 AX201", status="Up", if_index=5,
                    ipv4=["192.168.1.10"], gw4=["192.168.1.1"], dns=["223.5.5.5"])
    defaults.update(kw)
    return Adapter(**defaults)


def make_snap(adapters=None, **kw):
    snap = Snapshot()
    snap.adapters = adapters if adapters is not None else []
    snap.ok = True
    for k, v in kw.items():
        setattr(snap, k, v)
    return snap


def patch_winreg(monkeypatch, values=None, subkeys=None, pnp=None):
    """按路径子串分发假注册表数据：values/subkeys 为 {子串: 数据}，pnp 为 pnpcapabilities 固定返回值。"""
    from syskit import winregread

    vmap = values or {}
    smap = subkeys or {}
    monkeypatch.setattr(winregread, "values",
                        lambda path: next((v for k, v in vmap.items() if k in path), {}))
    monkeypatch.setattr(winregread, "subkeys",
                        lambda path: next((v for k, v in smap.items() if k in path), []))
    if pnp is not None:
        monkeypatch.setattr(winregread, "pnpcapabilities", lambda desc, scan=None: pnp)


def make_proc_run(fake_cmd_result, *, netsh_dns="通过 DHCP 配置: 是",
                  ping="已发送 = 3，已接收 = 3，丢失 = 0 (0% 丢失)", netsh_wlan="随机"):
    """按命令关键字分发的假 proc.run。"""
    def _run(cmd, timeout=20):
        c = " ".join(cmd) if isinstance(cmd, list) else str(cmd)
        if "dnsservers" in c:
            return fake_cmd_result(0, netsh_dns)
        if "ping" in c:
            return fake_cmd_result(0, ping)
        if "wlan" in c:
            return fake_cmd_result(0, netsh_wlan)
        return fake_cmd_result(0, "")
    return _run


def patch_dns(monkeypatch, sys_ips=None, ref_ips=None, system_ips=None):
    """query_safe: 系统 DNS(非 223.5.5.5) → sys_ips；公共 DNS → ref_ips。resolve_system → system_ips。"""
    from syskit import dnsquery

    def fake_query(host, server, timeout=3.0):
        if server == config.PUBLIC_DNS_TEST[0]:
            return list(ref_ips or [])
        return list(sys_ips or [])

    monkeypatch.setattr(dnsquery, "query_safe", fake_query)
    monkeypatch.setattr(dnsquery, "resolve_system", lambda host, timeout=4.0: list(system_ips or []))


NCSI_OK_BODY = config.NCSI_EXPECT_CONTENT


def patch_http(monkeypatch, code=204, body="", err=None, dns_ips=None):
    """把 netcheck._http_get 打成固定响应；probe_ncsi 的 DNS 部分一并打桩。"""
    monkeypatch.setattr(netcheck, "_http_get", lambda url, timeout=6.0, need_body=False: (code, body, err))
    if dns_ips is not None:
        from syskit import dnsquery
        monkeypatch.setattr(dnsquery, "resolve_system", lambda host, timeout=4.0: list(dns_ips))


def res(cid, status):
    return netcheck.CheckResult(cid, cid, status)


# ---------------- CheckResult 基础 ----------------

class TestCheckResult:
    def test_line_marks_all_status(self):
        marks = {"pass": "√", "warn": "!", "fail": "×", "skip": "-", "info": "·"}
        for st, mark in marks.items():
            r = netcheck.CheckResult("id", "名字", st, "详情")
            assert r.line() == f"[{mark}] 名字\n      详情"

    def test_unknown_status_mark(self):
        r = netcheck.CheckResult("id", "n", "weird", "d")
        assert "[?]" in r.line()

    def test_fix_ids_default_and_explicit(self):
        assert netcheck.CheckResult("a", "a", netcheck.PASS).fix_ids == []
        assert netcheck.CheckResult("a", "a", netcheck.PASS, fix_ids=["x"]).fix_ids == ["x"]


# ---------------- HTTP 探测（_http_get / probe_*） ----------------

class _FakeResp:
    status = 204

    def read(self, n):
        return b"resp-body"

    def __enter__(self):
        return self

    def __exit__(self, *a):
        return False


class TestHttpGet:
    def test_ok_with_body(self, monkeypatch):
        monkeypatch.setattr(netcheck._OPENER_DIRECT, "open", lambda req, timeout=None: _FakeResp())
        code, body, err = netcheck._http_get("http://example/x", need_body=True)
        assert (code, body, err) == (204, "resp-body", None)

    def test_ok_without_body(self, monkeypatch):
        monkeypatch.setattr(netcheck._OPENER_DIRECT, "open", lambda req, timeout=None: _FakeResp())
        code, body, err = netcheck._http_get("http://example/x")
        assert (code, body, err) == (204, "", None)

    def test_http_error_returns_code(self, monkeypatch):
        def boom(req, timeout=None):
            raise urllib.error.HTTPError("http://example/x", 404, "Not Found", None, None)
        monkeypatch.setattr(netcheck._OPENER_DIRECT, "open", boom)
        assert netcheck._http_get("http://example/x") == (404, "", None)

    def test_generic_error_returns_err_text(self, monkeypatch):
        def boom(req, timeout=None):
            raise OSError("connection refused")
        monkeypatch.setattr(netcheck._OPENER_DIRECT, "open", boom)
        code, body, err = netcheck._http_get("http://example/x")
        assert code is None and body == "" and err == "connection refused"

    def test_direct_opener_and_ua(self, monkeypatch):
        """探测必须直连且带 SysKit UA（原断言在 monitor 侧，probe 收敛后移到这里）。"""
        seen = {}

        def fake_open(req, timeout=None):
            seen["url"] = req.full_url
            seen["ua"] = req.headers.get("User-agent")   # Request 规范化首字母
            return _FakeResp()
        monkeypatch.setattr(netcheck._OPENER_DIRECT, "open", fake_open)
        netcheck._http_get("http://example/x")
        assert seen["url"] == "http://example/x"
        assert seen["ua"] == "SysKit/1.0"


class TestProbe204:
    def test_first_endpoint_ok(self, monkeypatch):
        patch_http(monkeypatch, code=204)
        ok, detail = netcheck.probe_204()
        assert ok is True and detail == config.PROBE_204_URLS[0]

    def test_second_endpoint_ok(self, monkeypatch):
        def fake(url, timeout=6.0, need_body=False):
            if url == config.PROBE_204_URLS[0]:
                return None, "", "timeout"
            return 204, "", None
        monkeypatch.setattr(netcheck, "_http_get", fake)
        ok, detail = netcheck.probe_204()
        assert ok is True and detail == config.PROBE_204_URLS[1]

    def test_all_fail_with_err(self, monkeypatch):
        patch_http(monkeypatch, code=None, err="timeout")
        ok, detail = netcheck.probe_204()
        assert ok is False and detail == "timeout"

    def test_all_fail_no_err_text(self, monkeypatch):
        patch_http(monkeypatch, code=500, err=None)
        ok, detail = netcheck.probe_204()
        assert ok is False and detail == "all failed"


class TestProbeRealInternet:
    def test_204_ok(self, monkeypatch):
        monkeypatch.setattr(netcheck, "_http_get",
                            lambda url, timeout=6.0, need_body=False: (204, "", None))
        ok, detail = netcheck.probe_real_internet()
        assert ok is True and "204探测:OK" in detail

    def test_https_200_rescues(self, monkeypatch):
        def fake(url, timeout=6.0, need_body=False):
            if url == config.PROBE_HTTPS_URL:
                return 200, "", None
            return None, "", "blocked"
        monkeypatch.setattr(netcheck, "_http_get", fake)
        ok, detail = netcheck.probe_real_internet()
        assert ok is True and "HTTPS" in detail

    def test_https_302_counts_as_ok(self, monkeypatch):
        def fake(url, timeout=6.0, need_body=False):
            if url == config.PROBE_HTTPS_URL:
                return 302, "", None
            return None, "", "blocked"
        monkeypatch.setattr(netcheck, "_http_get", fake)
        assert netcheck.probe_real_internet()[0] is True

    def test_all_fail(self, monkeypatch):
        patch_http(monkeypatch, code=None, err="unreachable")
        ok, detail = netcheck.probe_real_internet()
        assert ok is False
        assert "204探测:FAIL" in detail and "FAIL" in detail


class TestProbeNcsi:
    def test_ok(self, monkeypatch):
        patch_http(monkeypatch, code=200, body=NCSI_OK_BODY, dns_ips=[config.NCSI_DNS_EXPECT])
        ok, detail = netcheck.probe_ncsi()
        assert ok is True and "DNS" in detail and "OK" in detail

    def test_web_content_mismatch(self, monkeypatch):
        patch_http(monkeypatch, code=200, body="hijacked page", dns_ips=[config.NCSI_DNS_EXPECT])
        ok, detail = netcheck.probe_ncsi()
        assert ok is False and "内容不符" in detail

    def test_web_error(self, monkeypatch):
        patch_http(monkeypatch, code=None, err="timed out", dns_ips=[config.NCSI_DNS_EXPECT])
        ok, detail = netcheck.probe_ncsi()
        assert ok is False and "timed out" in detail

    def test_dns_fail(self, monkeypatch):
        patch_http(monkeypatch, code=200, body=NCSI_OK_BODY, dns_ips=[])
        ok, detail = netcheck.probe_ncsi()
        assert ok is False and "FAIL" in detail and "超时" in detail

    def test_dns_wrong_ip(self, monkeypatch):
        patch_http(monkeypatch, code=200, body=NCSI_OK_BODY, dns_ips=["1.2.3.4"])
        ok, detail = netcheck.probe_ncsi()
        assert ok is False and "1.2.3.4" in detail


# ---------------- 辅助函数 ----------------

class TestV4Dns:
    def test_skips_ipv6_and_returns_first_v4(self):
        assert netcheck._v4_dns(["fe80::1", "223.5.5.5", "119.29.29.29"]) == "223.5.5.5"

    def test_skips_fec_placeholder_without_colon(self):
        assert netcheck._v4_dns(["fec0"]) == ""

    def test_empty(self):
        assert netcheck._v4_dns([]) == ""

    def test_all_ipv6(self):
        assert netcheck._v4_dns(["fec0:0:0:ffff::1", "2001:db8::1"]) == ""


class TestPingLoss:
    def test_chinese_output(self):
        out = "已发送 = 3，已接收 = 2，丢失 = 1 (33% 丢失)"
        assert netcheck._ping_loss(out) == 33

    def test_zero_loss(self):
        assert netcheck._ping_loss("丢失 = 0 (0% 丢失)") == 0

    def test_english_output(self):
        assert netcheck._ping_loss("Sent = 3, Received = 0, Lost = 3 (100% loss)") == 100

    def test_unparseable(self):
        assert netcheck._ping_loss("garbage") is None


class TestBogus:
    @pytest.mark.parametrize("ip", ["192.168.1.1", "127.0.0.1", "169.254.3.3", "0.0.0.0",
                                    "10.0.0.1", "not-an-ip"])
    def test_bogus_ips(self, ip):
        assert netcheck._bogus([ip]) is True

    def test_valid_public(self):
        assert netcheck._bogus(["223.5.5.5", "1.2.3.4"]) is False

    def test_empty(self):
        assert netcheck._bogus([]) is False


# ---------------- chk_nlasvc ----------------

NSVC = {"NlaSvc": {"status": "Running", "startType": "Automatic"}}


class TestChkNlaSvc:
    def _patch(self, monkeypatch, trigger=None):
        patch_winreg(monkeypatch, subkeys={"TriggerInfo": trigger or []})

    def test_service_missing(self, monkeypatch):
        self._patch(monkeypatch)
        r = netcheck.chk_nlasvc(make_snap())
        assert (r.status, r.detail, r.fix_ids) == (netcheck.FAIL, "服务不存在", ["fix_nlasvc"])

    def test_running_auto_pass(self, monkeypatch):
        self._patch(monkeypatch)
        r = netcheck.chk_nlasvc(make_snap(services=dict(NSVC)))
        assert r.status == netcheck.PASS
        assert "运行中" in r.detail and "Automatic" in r.detail and "触发器=无" in r.detail
        assert r.fix_ids == []

    def test_running_manual_no_trigger_warns(self, monkeypatch):
        """NlaSvc 启动类型异常检测：手动 + 无触发器 → WARN + fix_nlasvc 建议。

        回归：该分支曾写成二元组解包（r.status, r.fix_ids = WARN, ..., [...]）
        必抛 ValueError、被 run_all 兜底降级成原始异常文本并丢失修复建议——
        恰是本工具的核心目标场景；被 pytest.raises(ValueError) 钉死掩盖至今。
        """
        self._patch(monkeypatch)
        svc = {"NlaSvc": {"status": "Running", "startType": "Manual"}}
        r = netcheck.chk_nlasvc(make_snap(services=svc))
        assert r.status == netcheck.WARN
        assert "手动启动且无触发器" in r.detail
        assert r.fix_ids == ["fix_nlasvc"]

    def test_running_manual_with_trigger_pass(self, monkeypatch):
        self._patch(monkeypatch, trigger=["0"])
        svc = {"NlaSvc": {"status": "Running", "startType": "Manual"}}
        r = netcheck.chk_nlasvc(make_snap(services=svc))
        assert r.status == netcheck.PASS and "触发器=有" in r.detail

    def test_stopped_disabled(self, monkeypatch):
        self._patch(monkeypatch)
        svc = {"NlaSvc": {"status": "Stopped", "startType": "Disabled"}}
        r = netcheck.chk_nlasvc(make_snap(services=svc))
        assert r.status == netcheck.FAIL
        assert "已禁用" in r.detail and "误报'无网络'" in r.detail
        assert r.fix_ids == ["fix_nlasvc"]

    def test_stopped_manual_no_trigger_fails(self, monkeypatch):
        self._patch(monkeypatch)
        svc = {"NlaSvc": {"status": "Stopped", "startType": "Manual"}}
        r = netcheck.chk_nlasvc(make_snap(services=svc))
        assert r.status == netcheck.FAIL
        assert "无人拉起它" in r.detail
        assert r.fix_ids == ["fix_nlasvc"]

    def test_stopped_manual_with_trigger_start_fix(self, monkeypatch):
        self._patch(monkeypatch, trigger=["0"])
        svc = {"NlaSvc": {"status": "Stopped", "startType": "Manual"}}
        r = netcheck.chk_nlasvc(make_snap(services=svc))
        assert r.status == netcheck.FAIL and r.detail == "已停止(启动类型=Manual)"
        assert r.fix_ids == ["start_nlasvc"]

    def test_stopped_auto_start_fix(self, monkeypatch):
        self._patch(monkeypatch)
        svc = {"NlaSvc": {"status": "Stopped", "startType": "Automatic"}}
        r = netcheck.chk_nlasvc(make_snap(services=svc))
        assert r.status == netcheck.FAIL and r.detail == "已停止(启动类型=Automatic)"
        assert r.fix_ids == ["start_nlasvc"]


# ---------------- chk_persistent_route ----------------

class TestChkPersistentRoute:
    def test_none(self, monkeypatch):
        r = netcheck.chk_persistent_route(make_snap())
        assert (r.status, r.detail) == (netcheck.PASS, "无持久路由")

    def test_default_route_fail(self, monkeypatch):
        pr = [dict(dest="0.0.0.0/0", nextHop="192.168.1.1", metric=1, if_index=5)]
        r = netcheck.chk_persistent_route(make_snap(persistent_routes=pr))
        assert r.status == netcheck.FAIL
        assert "存在持久默认路由" in r.detail and "192.168.1.1(跃点1)" in r.detail
        assert r.fix_ids == ["del_persistent_route"]

    def test_other_route_warn(self, monkeypatch):
        pr = [dict(dest="10.0.0.0/8", nextHop="192.168.1.254", metric=1, if_index=5)]
        r = netcheck.chk_persistent_route(make_snap(persistent_routes=pr))
        assert r.status == netcheck.WARN
        assert "10.0.0.0/8→192.168.1.254" in r.detail
        assert r.fix_ids == []


# ---------------- chk_static_dns / dns_is_static ----------------

class TestDnsIsStatic:
    def _patch(self, monkeypatch, text, fake_cmd_result):
        from syskit import proc
        monkeypatch.setattr(proc, "run",
                            lambda cmd, timeout=20: fake_cmd_result(0, text))

    def test_chinese_static(self, monkeypatch, fake_cmd_result):
        self._patch(monkeypatch, "静态配置: 是", fake_cmd_result)
        assert netcheck.dns_is_static("WLAN") == (True, "static")

    def test_english_static(self, monkeypatch, fake_cmd_result):
        self._patch(monkeypatch, "Statically configured values", fake_cmd_result)
        assert netcheck.dns_is_static("Ethernet") == (True, "static")

    def test_dhcp(self, monkeypatch, fake_cmd_result):
        self._patch(monkeypatch, "通过 DHCP 配置: 是", fake_cmd_result)
        assert netcheck.dns_is_static("WLAN") == (False, "dhcp")

    def test_dhcp_english(self, monkeypatch, fake_cmd_result):
        self._patch(monkeypatch, "Initiated by Dhcp", fake_cmd_result)
        assert netcheck.dns_is_static("WLAN") == (False, "dhcp")

    def test_unknown(self, monkeypatch, fake_cmd_result):
        self._patch(monkeypatch, "", fake_cmd_result)
        assert netcheck.dns_is_static("WLAN") == (False, "unknown")


class TestChkStaticDns:
    def test_clean_dhcp(self, monkeypatch, fake_cmd_result):
        from syskit import proc
        monkeypatch.setattr(proc, "run", make_proc_run(fake_cmd_result))
        a = make_adapter(dns=["223.5.5.5", "119.29.29.29"])
        r = netcheck.chk_static_dns(make_snap([a]))
        assert r.status == netcheck.PASS
        assert "均为 DHCP 自动获取" in r.detail and r.fix_ids == []

    def test_foreign_dns_blacklist_warns(self, monkeypatch, fake_cmd_result):
        """国外 DNS 黑名单：8.8.8.8 应告警。"""
        from syskit import proc
        monkeypatch.setattr(proc, "run", make_proc_run(fake_cmd_result))
        a = make_adapter(dns=["8.8.8.8", "223.5.5.5"])
        r = netcheck.chk_static_dns(make_snap([a]))
        assert r.status == netcheck.WARN
        assert "8.8.8.8" in r.detail and "DHCP" in r.detail
        assert r.fix_ids == ["dns_auto", "dns_preset"]

    def test_foreign_dns_on_static_adapter(self, monkeypatch, fake_cmd_result):
        from syskit import proc
        monkeypatch.setattr(proc, "run",
                            make_proc_run(fake_cmd_result, netsh_dns="静态配置: 是"))
        a = make_adapter(dns=["1.1.1.1"], status="Disconnected")
        r = netcheck.chk_static_dns(make_snap([a]))
        assert r.status == netcheck.WARN
        assert "静态" in r.detail and "(未连接)" in r.detail and "1.1.1.1" in r.detail
        assert r.fix_ids == ["dns_auto", "dns_preset"]

    def test_static_domestic_dns_info(self, monkeypatch, fake_cmd_result):
        from syskit import proc
        monkeypatch.setattr(proc, "run",
                            make_proc_run(fake_cmd_result, netsh_dns="静态配置: 是"))
        a = make_adapter(dns=["223.5.5.5"])
        r = netcheck.chk_static_dns(make_snap([a]))
        assert r.status == netcheck.INFO
        assert "静态 223.5.5.5" in r.detail and r.fix_ids == []

    def test_problems_and_notes_joined(self, monkeypatch, fake_cmd_result):
        from syskit import proc
        monkeypatch.setattr(proc, "run",
                            make_proc_run(fake_cmd_result, netsh_dns="静态配置: 是"))
        bad = make_adapter(name="WLAN", dns=["8.8.4.4"])
        note = make_adapter(name="以太网", desc="Realtek PCIe GbE", if_index=3, dns=["114.114.114.114"])
        r = netcheck.chk_static_dns(make_snap([bad, note]))
        assert r.status == netcheck.WARN
        # 问题项与提示项以"；"拼接：黑名单问题在前、静态备注在后
        assert r.detail.index("8.8.4.4") < r.detail.index("114.114.114.114")
        assert "；" in r.detail

    def test_virtual_and_wan_miniport_skipped(self, monkeypatch, fake_cmd_result):
        from syskit import proc
        monkeypatch.setattr(proc, "run", make_proc_run(fake_cmd_result))
        v = make_adapter(virtual=True, dns=["8.8.8.8"])
        w = make_adapter(name="WAN Miniport (IP)", desc="WAN Miniport (IP)", dns=["8.8.8.8"])
        r = netcheck.chk_static_dns(make_snap([v, w]))
        assert r.status == netcheck.PASS and "均为 DHCP 自动获取" in r.detail

    def test_ipv6_only_dns_skipped(self, monkeypatch, fake_cmd_result):
        """只有 fec0 占位符（无可用 v4 DNS）的网卡不应触发 netsh。"""
        from syskit import proc
        monkeypatch.setattr(proc, "run", make_proc_run(fake_cmd_result))
        a = make_adapter(dns=["fec0:0:0:ffff::1", "fec0:0:0:ffff::2"])
        r = netcheck.chk_static_dns(make_snap([a]))
        assert r.status == netcheck.PASS

    def test_no_dns_adapters(self, monkeypatch):
        r = netcheck.chk_static_dns(make_snap([make_adapter(dns=[])]))
        assert r.status == netcheck.PASS


# ---------------- chk_gateway ----------------

class TestChkGateway:
    def test_no_up_adapters_skip(self, monkeypatch, fake_cmd_result):
        from syskit import proc
        monkeypatch.setattr(proc, "run", make_proc_run(fake_cmd_result))
        down = make_adapter(status="Disconnected")
        r = netcheck.chk_gateway(make_snap([down]))
        assert (r.status, r.detail) == (netcheck.SKIP, "无已连接的物理网卡，跳过")

    def test_up_without_gw_warn(self, monkeypatch, fake_cmd_result):
        from syskit import proc
        monkeypatch.setattr(proc, "run", make_proc_run(fake_cmd_result))
        r = netcheck.chk_gateway(make_snap([make_adapter(gw4=[])]))
        assert (r.status, r.detail) == (netcheck.WARN, "已连接网卡均无默认网关")

    def test_gateway_reachable_pass(self, monkeypatch, fake_cmd_result):
        from syskit import proc
        monkeypatch.setattr(proc, "run", make_proc_run(fake_cmd_result))
        r = netcheck.chk_gateway(make_snap([make_adapter()]))
        assert r.status == netcheck.PASS
        assert "网关 192.168.1.1: 丢包 0%" in r.detail

    def test_gateway_100_loss_fail(self, monkeypatch, fake_cmd_result):
        """网关 100% 丢包 → FAIL。"""
        from syskit import proc
        monkeypatch.setattr(proc, "run",
                            make_proc_run(fake_cmd_result, ping="丢失 = 3 (100% 丢失)"))
        r = netcheck.chk_gateway(make_snap([make_adapter()]))
        assert r.status == netcheck.FAIL
        assert "丢包 100%" in r.detail

    def test_partial_loss_under_100_pass(self, monkeypatch, fake_cmd_result):
        """丢包 <100% 视为网关可达（源码逻辑：仅 100% 丢包才算失败）。"""
        from syskit import proc
        monkeypatch.setattr(proc, "run",
                            make_proc_run(fake_cmd_result, ping="丢失 = 1 (33% 丢失)"))
        r = netcheck.chk_gateway(make_snap([make_adapter()]))
        assert r.status == netcheck.PASS and "丢包 33%" in r.detail

    def test_unparseable_ping_counts_as_fail(self, monkeypatch, fake_cmd_result):
        from syskit import proc
        monkeypatch.setattr(proc, "run", make_proc_run(fake_cmd_result, ping="??"))
        r = netcheck.chk_gateway(make_snap([make_adapter()]))
        assert r.status == netcheck.FAIL and "丢包 ??%" in r.detail

    def test_mixed_ok_and_fail_warn(self, monkeypatch, fake_cmd_result):
        from syskit import proc

        def _run(cmd, timeout=20):
            gw = cmd[-1]
            out = "丢失 = 0 (0% 丢失)" if gw == "192.168.1.1" else "丢失 = 3 (100% 丢失)"
            return fake_cmd_result(0, out)
        monkeypatch.setattr(proc, "run", _run)
        a1 = make_adapter(name="WLAN", gw4=["192.168.1.1"])
        a2 = make_adapter(name="以太网", desc="Realtek", if_index=3,
                          ipv4=["10.0.0.5"], gw4=["10.0.0.1"])
        r = netcheck.chk_gateway(make_snap([a1, a2]))
        assert r.status == netcheck.WARN and "；" in r.detail

    def test_only_first_gw_pinged(self, monkeypatch, fake_cmd_result):
        """每个网卡只 ping 第一个网关。"""
        from syskit import proc
        seen = []

        def _run(cmd, timeout=20):
            seen.append(cmd[-1])
            return fake_cmd_result(0, "丢失 = 0 (0% 丢失)")
        monkeypatch.setattr(proc, "run", _run)
        a = make_adapter(gw4=["1.1.1.1", "2.2.2.2"])
        r = netcheck.chk_gateway(make_snap([a]))
        assert seen == ["1.1.1.1"] and r.status == netcheck.PASS


# ---------------- chk_dns_compare ----------------

class TestChkDnsCompare:
    ROUTER_DNS = "192.168.1.1"  # 系统_dns 不能等于参照 DNS(223.5.5.5)，否则 fake 难以区分两次查询

    def _snap(self):
        return make_snap([make_adapter(dns=[self.ROUTER_DNS])])

    def test_no_sys_dns_skip(self, monkeypatch):
        r = netcheck.chk_dns_compare(make_snap([make_adapter(dns=[])]))
        assert (r.status, r.detail) == (netcheck.SKIP, "无系统 DNS 可测")

    def test_up_adapter_without_v4_dns_skip(self, monkeypatch):
        a = make_adapter(dns=["fec0:0:0:ffff::1"])
        r = netcheck.chk_dns_compare(make_snap([a]))
        assert r.status == netcheck.SKIP

    def test_ref_dns_unreachable_warn(self, monkeypatch):
        patch_dns(monkeypatch, sys_ips=["1.2.3.4"], ref_ips=[])
        r = netcheck.chk_dns_compare(self._snap())
        assert r.status == netcheck.WARN
        assert "公共 DNS(223.5.5.5) 也不可达" in r.detail

    def test_match_pass(self, monkeypatch):
        patch_dns(monkeypatch, sys_ips=["1.2.3.4"], ref_ips=["1.2.3.4", "5.6.7.8"])
        r = netcheck.chk_dns_compare(self._snap())
        assert r.status == netcheck.PASS
        assert f"系统 DNS({self.ROUTER_DNS}) 解析正常" in r.detail

    def test_sys_dns_dead_fail(self, monkeypatch):
        patch_dns(monkeypatch, sys_ips=[], ref_ips=["1.2.3.4"])
        r = netcheck.chk_dns_compare(self._snap())
        assert r.status == netcheck.FAIL
        assert f"系统 DNS({self.ROUTER_DNS}) 解析 www.baidu.com 超时/失败" in r.detail
        assert "公共 DNS 正常" in r.detail
        assert r.fix_ids == ["dns_auto", "dns_preset"]

    def test_mismatch_warn(self, monkeypatch):
        patch_dns(monkeypatch, sys_ips=["9.9.9.9"], ref_ips=["1.2.3.4"])
        r = netcheck.chk_dns_compare(self._snap())
        assert r.status == netcheck.WARN
        assert "结果不一致" in r.detail and "9.9.9.9" in r.detail


# ---------------- chk_dns_pollution ----------------

class TestChkDnsPollution:
    def _patch(self, monkeypatch, sys_ips, ref_ips):
        patch_dns(monkeypatch, system_ips=sys_ips, ref_ips=ref_ips)

    def test_ref_unreachable(self, monkeypatch):
        self._patch(monkeypatch, ["1.2.3.4"], [])
        r = netcheck.chk_dns_pollution(make_snap())
        assert (r.status, r.detail) == (netcheck.INFO, "参照 DNS 不可达")

    def test_sys_resolve_fail(self, monkeypatch):
        self._patch(monkeypatch, [], ["13.107.4.50"])
        r = netcheck.chk_dns_pollution(make_snap())
        assert r.status == netcheck.FAIL
        assert "系统解析 www.msftconnecttest.com 失败" in r.detail
        assert r.fix_ids == ["dns_auto", "dns_preset"]

    def test_consistent_pass(self, monkeypatch):
        self._patch(monkeypatch, ["13.107.4.50"], ["13.107.4.50"])
        r = netcheck.chk_dns_pollution(make_snap())
        assert r.status == netcheck.PASS and "解析一致" in r.detail

    def test_bogus_result_warn(self, monkeypatch):
        self._patch(monkeypatch, ["192.168.1.1"], ["13.107.4.50"])
        r = netcheck.chk_dns_pollution(make_snap())
        assert r.status == netcheck.WARN
        assert "污染特征" in r.detail

    def test_valid_but_different_info(self, monkeypatch):
        self._patch(monkeypatch, ["20.112.52.29"], ["13.107.4.50"])
        r = netcheck.chk_dns_pollution(make_snap())
        assert r.status == netcheck.INFO
        assert "CDN 分流，非污染" in r.detail


# ---------------- chk_ncsi_probe / chk_real ----------------

class TestChkNcsiProbe:
    def test_pass(self, monkeypatch):
        patch_http(monkeypatch, code=200, body=NCSI_OK_BODY, dns_ips=[config.NCSI_DNS_EXPECT])
        r = netcheck.chk_ncsi_probe(make_snap())
        assert r.status == netcheck.PASS and r.fix_ids == []

    def test_fail_fix_ids(self, monkeypatch):
        patch_http(monkeypatch, code=None, err="timeout", dns_ips=[])
        r = netcheck.chk_ncsi_probe(make_snap())
        assert r.status == netcheck.FAIL
        assert r.fix_ids == ["dns_auto", "dns_preset", "ncsi_restore"]
        assert "Web探测" in r.detail


class TestChkReal:
    def test_pass(self, monkeypatch):
        monkeypatch.setattr(netcheck, "_http_get",
                            lambda url, timeout=6.0, need_body=False: (204, "", None))
        r = netcheck.chk_real(make_snap())
        assert r.status == netcheck.PASS and r.id == "real_conn"

    def test_fail(self, monkeypatch):
        patch_http(monkeypatch, code=None, err="unreachable")
        r = netcheck.chk_real(make_snap())
        assert r.status == netcheck.FAIL and "204探测:FAIL" in r.detail


# ---------------- chk_ncsi_registry ----------------

NCSI_VALS_PATH = "Parameters\\Internet"
POLICY_PATH = "NetworkConnectivityStatusIndicator"


def stock_values(**overrides):
    vals = dict(config.NCSI_STOCK)
    vals.update(overrides)
    for k, v in list(vals.items()):
        if v is None:
            del vals[k]
    return vals


class TestChkNcsiRegistry:
    def test_unreadable_fail(self, monkeypatch):
        patch_winreg(monkeypatch, values={})
        r = netcheck.chk_ncsi_registry(make_snap())
        assert (r.status, r.detail) == (netcheck.FAIL, "无法读取 NCSI 注册表（需管理员？）")

    def test_stock_pass(self, monkeypatch):
        patch_winreg(monkeypatch, values={NCSI_VALS_PATH: stock_values(),
                                          POLICY_PATH: {}})
        r = netcheck.chk_ncsi_registry(make_snap())
        assert r.status == netcheck.PASS
        assert "与原版默认一致" in r.detail
        assert "www.msftconnecttest.com/connecttest.txt" in r.detail
        assert r.fix_ids == []

    def test_probing_disabled_warn(self, monkeypatch):
        patch_winreg(monkeypatch, values={NCSI_VALS_PATH: stock_values(EnableActiveProbing=0),
                                          POLICY_PATH: {}})
        r = netcheck.chk_ncsi_registry(make_snap())
        assert r.status == netcheck.WARN
        assert "EnableActiveProbing=0（主动探测被关闭）" in r.detail
        assert r.fix_ids == ["ncsi_restore"]

    def test_missing_value_warn(self, monkeypatch):
        vals = stock_values(ActiveWebProbeHost=None)
        patch_winreg(monkeypatch, values={NCSI_VALS_PATH: vals, POLICY_PATH: {}})
        r = netcheck.chk_ncsi_registry(make_snap())
        assert r.status == netcheck.WARN and "ActiveWebProbeHost 缺失" in r.detail

    def test_wrong_value_warn(self, monkeypatch):
        vals = stock_values(ActiveWebProbeHost="evil.example.com")
        patch_winreg(monkeypatch, values={NCSI_VALS_PATH: vals, POLICY_PATH: {}})
        r = netcheck.chk_ncsi_registry(make_snap())
        assert r.status == netcheck.WARN
        assert "ActiveWebProbeHost=evil.example.com(默认www.msftconnecttest.com)" in r.detail

    def test_policy_noactiveprobe_warn(self, monkeypatch):
        patch_winreg(monkeypatch, values={NCSI_VALS_PATH: stock_values(),
                                          POLICY_PATH: {"NoActiveProbe": 1}})
        r = netcheck.chk_ncsi_registry(make_snap())
        assert r.status == netcheck.WARN and "组策略 NoActiveProbe=1" in r.detail


# ---------------- chk_power ----------------

class TestChkPower:
    def test_enabled_off_warns(self, monkeypatch):
        patch_winreg(monkeypatch, subkeys={"4d36e972": ["0000"]}, pnp="enabled_off")
        r = netcheck.chk_power(make_snap([make_adapter()]))
        assert r.status == netcheck.WARN
        assert "允许断电" in r.detail and "频繁掉线" in r.detail
        assert r.fix_ids == ["power_off"]

    def test_disabled_pass(self, monkeypatch):
        patch_winreg(monkeypatch, subkeys={"4d36e972": ["0000"]}, pnp="disabled")
        r = netcheck.chk_power(make_snap([make_adapter()]))
        assert r.status == netcheck.PASS and "已禁止断电" in r.detail

    def test_unknown_pnp_skipped_no_caps_skip(self, monkeypatch):
        patch_winreg(monkeypatch, subkeys={}, pnp=None)
        r = netcheck.chk_power(make_snap([make_adapter()]))
        assert (r.status, r.detail) == (netcheck.SKIP, "无法读取网卡注册表")

    def test_caps_but_no_items_pass(self, monkeypatch):
        patch_winreg(monkeypatch, subkeys={"4d36e972": ["0000"]}, pnp=None)
        r = netcheck.chk_power(make_snap([make_adapter()]))
        assert (r.status, r.detail) == (netcheck.PASS, "未发现可断电网卡")

    def test_virtual_adapter_skipped(self, monkeypatch):
        patch_winreg(monkeypatch, subkeys={"4d36e972": ["0000"]}, pnp="enabled_off")
        r = netcheck.chk_power(make_snap([make_adapter(virtual=True)]))
        assert r.status == netcheck.PASS and "未发现可断电网卡" in r.detail

    def test_empty_desc_skipped(self, monkeypatch):
        patch_winreg(monkeypatch, subkeys={"4d36e972": ["0000"]}, pnp="enabled_off")
        r = netcheck.chk_power(make_snap([make_adapter(desc="", name="空描述")]))
        assert r.status == netcheck.PASS and "未发现可断电网卡" in r.detail

    def test_unreadable_class_key_scans_registry_only_once(self, monkeypatch):
        """类键读不到 → pnp_states 返回 None；逐网卡各自重扫正是本检查要省的 N+1。"""
        from syskit import winregread
        monkeypatch.setattr(winregread, "subkeys", lambda path: [])
        real = winregread.pnp_states
        scans = []

        def _counting():
            scans.append(1)
            return real()

        monkeypatch.setattr(winregread, "pnp_states", _counting)
        snap = make_snap([make_adapter(name=f"n{i}", desc=f"desc{i}") for i in range(4)])
        r = netcheck.chk_power(snap)
        assert (r.status, r.detail) == (netcheck.SKIP, "无法读取网卡注册表")
        assert len(scans) == 1


# ---------------- chk_services ----------------

class TestChkServices:
    def test_all_running_pass(self, monkeypatch):
        svc = {n: {"status": "Running", "startType": "Auto"}
               for n in ("Dhcp", "Dnscache", "netprofm", "WlanSvc")}
        r = netcheck.chk_services(make_snap(services=svc))
        assert r.status == netcheck.PASS
        assert "Dhcp=Running/Auto" in r.detail and "WlanSvc=Running/Auto" in r.detail

    def test_dhcp_stopped_warns(self, monkeypatch):
        svc = {"Dhcp": {"status": "Stopped", "startType": "Auto"}}
        r = netcheck.chk_services(make_snap(services=svc))
        assert r.status == netcheck.WARN and r.fix_ids == ["restart_services"]
        assert "Dhcp=Stopped/Auto" in r.detail

    def test_wlansvc_stopped_tolerated(self, monkeypatch):
        svc = {n: {"status": "Running", "startType": "Auto"} for n in ("Dhcp", "Dnscache", "netprofm")}
        svc["WlanSvc"] = {"status": "Stopped", "startType": "Manual"}
        r = netcheck.chk_services(make_snap(services=svc))
        assert r.status == netcheck.PASS and "WlanSvc=Stopped/Manual" in r.detail

    def test_no_data(self, monkeypatch):
        r = netcheck.chk_services(make_snap(services={}))
        assert r.status == netcheck.PASS and r.detail == "无数据"


# ---------------- chk_proxy ----------------

class TestChkProxy:
    def _patch(self, monkeypatch, status):
        from syskit import proxy
        monkeypatch.setattr(proxy, "get_proxy_status", lambda: status)

    def test_static_proxy_on_warns(self, monkeypatch):
        """系统代理残留检测：代理开着 → WARN。"""
        self._patch(monkeypatch, {"enabled": True, "server": "127.0.0.1:7890",
                                  "autoconfig": False, "pac_url": ""})
        r = netcheck.chk_proxy(make_snap())
        assert r.status == netcheck.WARN
        assert "系统代理已开启: 127.0.0.1:7890" in r.detail
        assert r.fix_ids == ["proxy_off"]

    def test_pac_config_warns(self, monkeypatch):
        self._patch(monkeypatch, {"enabled": False, "server": "",
                                  "autoconfig": True, "pac_url": "http://pac.local/proxy.pac?x=1"})
        r = netcheck.chk_proxy(make_snap())
        assert r.status == netcheck.WARN
        assert "PAC 自动代理脚本" in r.detail and "http://pac.local/proxy.pac?x=1" in r.detail
        assert r.fix_ids == ["proxy_off"]

    def test_off_pass(self, monkeypatch):
        self._patch(monkeypatch, {"enabled": False, "server": "",
                                  "autoconfig": False, "pac_url": ""})
        r = netcheck.chk_proxy(make_snap())
        assert r.status == netcheck.PASS and r.detail == "未启用系统代理"


# ---------------- chk_routes ----------------

ROUTE = dict(dest="0.0.0.0/0", nextHop="192.168.1.1", metric=0, if_metric=25, if_index=5, store="ActiveStore")


class TestChkRoutes:
    def test_no_default_routes(self, monkeypatch):
        r = netcheck.chk_routes(make_snap([make_adapter()]))
        assert r.status == netcheck.PASS and r.detail == "无默认路由（可能未连接）"

    def test_single_active_pass(self, monkeypatch):
        a = make_adapter()
        r = netcheck.chk_routes(make_snap([a], default_routes=[ROUTE]))
        assert r.status == netcheck.PASS
        assert "活动默认路由: 192.168.1.1(if5, 跃点25)" in r.detail

    def test_phantom_route_fail(self, monkeypatch):
        """幽灵路由：接口已断开却占据路由表。"""
        down = make_adapter(if_index=9, status="Disconnected")
        phantom = dict(ROUTE, nextHop="192.168.1.254", if_index=9)
        r = netcheck.chk_routes(make_snap([down], default_routes=[phantom]))
        assert r.status == netcheck.FAIL
        assert "幽灵默认路由" in r.detail and "192.168.1.254(if9, 跃点25)" in r.detail
        assert r.fix_ids == ["del_persistent_route"]
        assert "来自持久路由" not in r.detail

    def test_phantom_from_persistent_route(self, monkeypatch):
        down = make_adapter(if_index=9, status="Disconnected")
        phantom = dict(ROUTE, nextHop="192.168.1.254", if_index=9)
        pr = [dict(dest="0.0.0.0/0", nextHop="192.168.1.254", metric=1, if_index=9)]
        r = netcheck.chk_routes(make_snap([down], default_routes=[phantom], persistent_routes=pr))
        assert r.status == netcheck.FAIL and "来自持久路由" in r.detail

    def test_multiple_default_gateways_warn(self, monkeypatch):
        a1 = make_adapter(if_index=5)
        a2 = make_adapter(name="以太网", desc="Realtek", if_index=3,
                          ipv4=["10.0.0.5"], gw4=["10.0.0.1"])
        r1 = ROUTE
        r2 = dict(dest="0.0.0.0/0", nextHop="10.0.0.1", metric=0, if_metric=30, if_index=3,
                  store="ActiveStore")
        r = netcheck.chk_routes(make_snap([a1, a2], default_routes=[r1, r2]))
        assert r.status == netcheck.WARN and "多个默认网关" in r.detail

    def test_low_metric_tunnel_warn(self, monkeypatch):
        tun = make_adapter(name="Clash", desc="Wintun Userspace Tunnel", if_index=20,
                           ipv4=["198.18.0.1"], gw4=["198.18.0.1"])
        phys = make_adapter(name="以太网", desc="Realtek", if_index=3,
                            ipv4=["10.0.0.5"], gw4=[])
        rt = dict(dest="0.0.0.0/0", nextHop="198.18.0.1", metric=0, if_metric=0, if_index=20,
                  store="ActiveStore")
        r = netcheck.chk_routes(make_snap([tun, phys], default_routes=[rt],
                                          if_metrics={20: 5, 3: 25}))
        assert r.status == netcheck.WARN
        assert "隧道网卡跃点不高于物理网卡" in r.detail and "Clash" in r.detail

    def test_high_metric_tunnel_ok(self, monkeypatch):
        tun = make_adapter(name="Clash", desc="Wintun Userspace Tunnel", if_index=20,
                           ipv4=["198.18.0.1"], gw4=["198.18.0.1"])
        phys = make_adapter(name="以太网", desc="Realtek", if_index=3,
                            ipv4=["10.0.0.5"], gw4=[])
        rt = dict(dest="0.0.0.0/0", nextHop="198.18.0.1", metric=0, if_metric=0, if_index=20,
                  store="ActiveStore")
        r = netcheck.chk_routes(make_snap([tun, phys], default_routes=[rt],
                                          if_metrics={20: 100, 3: 25}))
        assert r.status == netcheck.PASS

    def test_tunnel_without_default_route_ignored(self, monkeypatch):
        """无默认路由的隧道不参与出口竞争。"""
        tun = make_adapter(name="Clash", desc="Wintun Userspace Tunnel", if_index=20,
                           ipv4=["198.18.0.1"], gw4=[])
        r = netcheck.chk_routes(make_snap([tun], default_routes=[], if_metrics={20: 1}))
        assert r.status == netcheck.PASS and "无默认路由" in r.detail


# ---------------- chk_ipv6 ----------------

class TestChkIpv6:
    def _run(self, monkeypatch, vals):
        patch_winreg(monkeypatch, values={"Parameters\\Internet": vals})
        return netcheck.chk_ipv6(make_snap())

    def test_with_v6_values(self, monkeypatch):
        r = self._run(monkeypatch, stock_values())
        assert r.status == netcheck.INFO and "NCSI V6 探测值: 有" in r.detail

    def test_without_v6_values(self, monkeypatch):
        vals = stock_values(ActiveWebProbeHostV6=None, ActiveWebProbePathV6=None,
                            ActiveWebProbeContentV6=None, ActiveDnsProbeContentV6=None)
        r = self._run(monkeypatch, vals)
        assert r.status == netcheck.INFO and "NCSI V6 探测值: 无" in r.detail

    def test_unreadable(self, monkeypatch):
        r = self._run(monkeypatch, {})
        assert r.status == netcheck.INFO and "NCSI V6 探测值: 无" in r.detail


# ---------------- chk_wlan ----------------

def _run_factory(netsh_wlan=""):
    def _run(cmd, timeout=20):
        from syskit.proc import CmdResult
        return CmdResult(0, netsh_wlan, "")
    return _run


class TestChkWlan:
    def _run(self, monkeypatch, wlan_text, settings=""):
        from syskit import proc
        monkeypatch.setattr(proc, "run", _run_factory(netsh_wlan=settings))
        return netcheck.chk_wlan(make_snap(wlan_text=wlan_text))

    def test_disconnected(self, monkeypatch):
        r = self._run(monkeypatch, "SSID: 无\n状态: 已断开")
        assert r.status == netcheck.INFO and "当前未连接 WiFi" in r.detail

    def test_disconnected_english(self, monkeypatch):
        r = self._run(monkeypatch, "State: disconnected")
        assert "当前未连接 WiFi" in r.detail

    def test_connected_ssid(self, monkeypatch):
        r = self._run(monkeypatch, "SSID: HomeWiFi\nBSSID 1: aa:bb:cc")
        assert "已连接: HomeWiFi" in r.detail and r.status == netcheck.INFO

    def test_mac_randomization_warns(self, monkeypatch):
        """用真实 netsh 输出格式（词组间有说明文字与冒号）锁定 WARN。
        旧正则的紧邻匹配在真实输出上永不命中，该 WARN 从未产生过。"""
        for text in ("MAC randomization for all networks: ENABLED",
                     "MAC 随机化(全部网络): 已启用"):
            r = self._run(monkeypatch, "SSID: HomeWiFi", settings=text)
            assert r.status == netcheck.WARN, text
            assert "MAC 随机化已启用" in r.detail and "已连接: HomeWiFi" in r.detail

    def test_mac_randomization_disabled_no_warn(self, monkeypatch):
        r = self._run(monkeypatch, "SSID: HomeWiFi",
                      settings="MAC randomization for all networks: DISABLED")
        assert r.status == netcheck.INFO

    def test_no_text(self, monkeypatch):
        r = self._run(monkeypatch, "")
        assert r.detail == "无法读取 WLAN 设置"


# ---------------- run_all 聚合 ----------------

def full_snap():
    a = make_adapter()
    svc = {"NlaSvc": {"status": "Running", "startType": "Automatic"},
           "Dhcp": {"status": "Running", "startType": "Auto"},
           "Dnscache": {"status": "Running", "startType": "Auto"},
           "netprofm": {"status": "Running", "startType": "Auto"},
           "WlanSvc": {"status": "Running", "startType": "Auto"}}
    return make_snap([a], services=svc, wlan_text="SSID: HomeWiFi")


def patch_all_boundaries(monkeypatch, fake_cmd_result):
    """run_all 所需的全部边界打桩：所有检查均可执行完且不触发真实系统调用。"""
    from syskit import dnsquery, proc
    monkeypatch.setattr(proc, "run", make_proc_run(fake_cmd_result))
    monkeypatch.setattr(snapshot_module(), "take", lambda use_ps=True: full_snap())
    patch_winreg(monkeypatch, values={NCSI_VALS_PATH: stock_values(), POLICY_PATH: {}},
                 subkeys={"4d36e972": ["0000"], "TriggerInfo": []}, pnp="disabled")
    monkeypatch.setattr(dnsquery, "query_safe", lambda host, server, timeout=3.0: ["1.2.3.4"])
    monkeypatch.setattr(dnsquery, "resolve_system", lambda host, timeout=4.0: [config.NCSI_DNS_EXPECT])
    monkeypatch.setattr(netcheck, "_http_get",
                        lambda url, timeout=6.0, need_body=False:
                        (200, NCSI_OK_BODY, None) if need_body else (204, "", None))


def snapshot_module():
    import syskit.snapshot as m
    return m


ALL_IDS = ["nlasvc", "persistent_route", "static_dns", "proxy", "gateway", "dns_compare",
           "dns_pollution", "ncsi_probe", "real_conn", "ncsi_reg", "power_mgmt",
           "services", "routes", "ipv6", "wlan"]
QUICK_EXCLUDED = {"dns_pollution", "ncsi_reg", "power_mgmt", "ipv6", "wlan"}


class TestRunAll:
    def test_full_run_returns_15_ordered_results(self, monkeypatch, fake_cmd_result):
        patch_all_boundaries(monkeypatch, fake_cmd_result)
        results = netcheck.run_all()
        assert [r.id for r in results] == ALL_IDS
        assert all(isinstance(r, netcheck.CheckResult) for r in results)

    def test_quick_mode_subset(self, monkeypatch, fake_cmd_result):
        patch_all_boundaries(monkeypatch, fake_cmd_result)
        results = netcheck.run_all(quick=True)
        ids = [r.id for r in results]
        assert len(ids) == 10
        assert not (set(ids) & QUICK_EXCLUDED)
        assert set(ids) == set(ALL_IDS) - QUICK_EXCLUDED

    def test_progress_called_per_check(self, monkeypatch, fake_cmd_result):
        patch_all_boundaries(monkeypatch, fake_cmd_result)
        seen = []
        results = netcheck.run_all(progress=seen.append)
        assert len(seen) == 15
        assert seen == results  # 逐项且按序回调
        assert all(r.id == ALL_IDS[i] for i, r in enumerate(seen))

    def test_exception_in_check_becomes_warn_result(self, monkeypatch, fake_cmd_result):
        """单检查抛异常不中断整体 → WARN '检查执行异常'。"""
        from syskit import proc

        def boom(cmd, timeout=20):
            raise RuntimeError("mock boom")
        monkeypatch.setattr(proc, "run", boom)
        monkeypatch.setattr(snapshot_module(), "take", lambda use_ps=True: full_snap())
        results = netcheck.run_all()
        by_id = {r.id: r for r in results}
        assert len(results) == 15
        # 依赖 proc.run 的检查（static_dns/gateway/wlan）均降级为 WARN 而非崩溃
        for cid in ("chk_static_dns", "chk_gateway", "chk_wlan"):
            assert by_id[cid].status == netcheck.WARN
            assert "检查执行异常" in by_id[cid].detail
            assert "mock boom" in by_id[cid].detail

    def test_nlasvc_manual_no_trigger_flows_through_run_all(self, monkeypatch, fake_cmd_result):
        """NlaSvc 运行中+手动+无触发器：经 run_all 正常产出 WARN 与修复建议（不触发异常兜底）。"""
        patch_all_boundaries(monkeypatch, fake_cmd_result)
        snap = full_snap()
        snap.services["NlaSvc"] = {"status": "Running", "startType": "Manual"}
        monkeypatch.setattr(snapshot_module(), "take", lambda use_ps=True: snap)
        results = netcheck.run_all()
        by_id = {r.id: r for r in results}
        assert len(results) == 15
        assert by_id["nlasvc"].status == netcheck.WARN
        assert by_id["nlasvc"].fix_ids == ["fix_nlasvc"]
        assert "检查执行异常" not in by_id["nlasvc"].detail


# ---------------- verdict 综合判定 ----------------

class TestVerdict:
    def test_normal(self):
        base, text = netcheck.verdict([res("real_conn", netcheck.PASS),
                                       res("ncsi_probe", netcheck.PASS),
                                       res("dns_compare", netcheck.PASS)])
        assert base == "normal" and text.startswith("网络正常")

    def test_ncsi_false_alarm(self, monkeypatch):
        """典型场景：探测失败但真实连通 → NCSI 误报判定。"""
        base, text = netcheck.verdict([res("real_conn", netcheck.PASS),
                                       res("ncsi_probe", netcheck.FAIL),
                                       res("dns_compare", netcheck.PASS)])
        assert base == "ncsi_false_alarm"
        assert "NCSI 误报" in text and "实际有网" in text

    def test_dns_broken(self):
        base, _ = netcheck.verdict([res("real_conn", netcheck.PASS),
                                    res("ncsi_probe", netcheck.PASS),
                                    res("dns_compare", netcheck.FAIL)])
        assert base == "dns_broken"

    def test_no_internet(self):
        base, _ = netcheck.verdict([res("real_conn", netcheck.FAIL),
                                    res("ncsi_probe", netcheck.FAIL),
                                    res("dns_compare", netcheck.SKIP)])
        assert base == "no_internet"

    def test_fail_count_appended(self):
        base, text = netcheck.verdict([res("real_conn", netcheck.FAIL),
                                       res("gateway", netcheck.FAIL),
                                       res("ncsi_probe", netcheck.FAIL)])
        assert base == "no_internet"
        assert "失败项: 3 个" in text

    def test_dns_bad_not_fail_does_not_alter(self):
        """dns_compare 仅 WARN（非 FAIL）不影响 normal 判定。"""
        base, text = netcheck.verdict([res("real_conn", netcheck.PASS),
                                       res("ncsi_probe", netcheck.PASS),
                                       res("dns_compare", netcheck.WARN)])
        assert base == "normal" and "失败项" not in text

    def test_empty_results(self):
        base, text = netcheck.verdict([])
        assert base == "no_internet" and "失败项" not in text


# ---------------- 中止与 VPN 代理降级分支（2026-09-22 补强） ----------------

class TestRunAllAbort:
    def test_should_stop_aborts_remaining_and_appends_skip(self, monkeypatch, fake_cmd_result):
        patch_all_boundaries(monkeypatch, fake_cmd_result)
        seen = []
        state = {"n": 0}

        def stop_after_first():
            state["n"] += 1
            return state["n"] > 1

        results = netcheck.run_all(progress=seen.append, should_stop=stop_after_first)
        assert len(results) == 2
        assert results[0].id == ALL_IDS[0]          # 首个检查照常完成
        aborted = results[1]
        assert aborted.id == "diag_aborted"
        assert aborted.status == netcheck.SKIP
        assert "中止" in aborted.name
        assert seen == results                       # skip 记录也经 progress 回报


class TestChkProxyTunnelAware:
    def _patch(self, monkeypatch, status):
        from syskit import proxy
        monkeypatch.setattr(proxy, "get_proxy_status", lambda: status)

    def test_proxy_with_tunnel_up_degrades_to_info(self, monkeypatch):
        """VPN/Clash 在线时代理多为预期：INFO + 不建议 proxy_off。"""
        self._patch(monkeypatch, {"enabled": True, "server": "127.0.0.1:7890",
                                  "autoconfig": False, "pac_url": ""})
        tun = make_adapter(name="Clash", desc="WireGuard Tunnel", status="Up")
        r = netcheck.chk_proxy(make_snap([tun]))
        assert r.status == netcheck.INFO
        assert r.fix_ids == []
        assert "Clash" in r.detail
        assert "勿盲目关闭" in r.detail

    def test_proxy_without_tunnel_still_warns(self, monkeypatch):
        """同状态但无隧道网卡 → 维持 WARN + proxy_off（护栏不误伤普通场景）。"""
        self._patch(monkeypatch, {"enabled": True, "server": "127.0.0.1:7890",
                                  "autoconfig": False, "pac_url": ""})
        plain = make_adapter(name="WLAN", desc="Intel(R) Wi-Fi 6 AX201", status="Up")
        r = netcheck.chk_proxy(make_snap([plain]))
        assert r.status == netcheck.WARN
        assert r.fix_ids == ["proxy_off"]
