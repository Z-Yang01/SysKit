"""syskit/icmp.py 单元测试：IcmpSendEcho 结果判定与默认网关获取（全程不发真实 ICMP/不真解析 DNS）。

Mock 边界（配合 conftest 哨兵双保险）：
- icmp._iphlpapi 整体替换为假 API 对象（记录 Create/SendEcho/CloseHandle，不触真实 iphlpapi）；
- socket.gethostbyname 按用例返回假 IP 或抛 OSError（icmp 内引用的是全局 socket 模块，
  monkeypatch 自动还原）；
- default_gateway 依赖的 proc.run_ps 替换为假返回（不启动 PowerShell）。

疑似业务 bug（只记录，不改源码）：
1. syskit/icmp.py:41 `if not handle or handle == _INVALID`：IcmpCreateFile 的 restype 声明为
   c_void_p，ctypes 会把返回值 (HANDLE)-1（INVALID_HANDLE_VALUE）自动转换为 None，因此
   `handle == _INVALID` 恒为 False，属死代码（行为仍正确，None 已被 `not handle` 分支覆盖）。
"""
import ctypes
import socket
import struct


from syskit import icmp, proc


# ---------------- 假 iphlpapi ----------------

class FakeIcmpApi:
    """替换 icmp._iphlpapi：记录调用、按预设返回/抛错，不接触真实 ICMP 栈。"""

    def __init__(self):
        self.handle = 0xABCD                  # 0 / INVALID 触发提前返回
        self.echo_result = 1                  # IcmpSendEcho 返回值（0=失败）
        self.reply_status = 0                 # 0 = IP_SUCCESS
        self.reply_rtt = 42
        self.send_error = None                # 置位则 SendEcho 抛异常
        self.created = []
        self.closed = []
        self.sent = []

    def IcmpCreateFile(self):
        self.created.append(True)
        return self.handle

    def IcmpCloseHandle(self, handle):
        self.closed.append(handle)
        return 1

    def IcmpSendEcho(self, handle, dest_ip, payload, payload_len,
                     opts, buf, buf_size, timeout_ms):
        self.sent.append(dict(handle=handle, dest=dest_ip, payload=bytes(payload),
                              size=buf_size, timeout=timeout_ms))
        if self.send_error is not None:
            raise self.send_error
        if self.echo_result:
            reply = icmp._ICMP_ECHO_REPLY()
            reply.Status = self.reply_status
            reply.RoundTripTime = self.reply_rtt
            ctypes.memmove(buf, ctypes.byref(reply), ctypes.sizeof(reply))
        return self.echo_result


def patch_api(monkeypatch, api=None):
    api = api or FakeIcmpApi()
    monkeypatch.setattr(icmp, "_iphlpapi", api)
    return api


def patch_resolve(monkeypatch, ip="192.168.1.1", err=None):
    def _resolve(host):
        if err is not None:
            raise err
        return ip
    monkeypatch.setattr(icmp.socket, "gethostbyname", _resolve)


# ---------------- ping ----------------

class TestPing:
    def test_success_returns_float_rtt(self, monkeypatch):
        api = patch_api(monkeypatch)
        patch_resolve(monkeypatch, "192.168.1.1")
        assert icmp.ping("host", timeout_ms=1500) == 42.0
        assert len(api.created) == 1
        assert api.closed == [0xABCD]                       # finally 必关句柄
        sent = api.sent[0]
        assert sent["handle"] == 0xABCD
        # 目标 IP 已转网络序 32 位整数
        assert sent["dest"] == struct.unpack("<I", socket.inet_aton("192.168.1.1"))[0]
        assert sent["payload"] == icmp._PAYLOAD
        assert sent["payload"].startswith(b"SysKit")        # 品牌标识（曾残留旧名 NetTool）
        assert sent["timeout"] == 1500

    def test_reply_buffer_has_error_headroom(self, monkeypatch):
        """缓冲区 = 结构体 + 负载 + IP头60 + 8 + 16 预留（防差错响应溢出）。"""
        api = patch_api(monkeypatch)
        patch_resolve(monkeypatch)
        icmp.ping("host")
        expect = ctypes.sizeof(icmp._ICMP_ECHO_REPLY) + len(icmp._PAYLOAD) + 68 + 16
        assert api.sent[0]["size"] == expect

    def test_dns_failure_returns_none_without_handle(self, monkeypatch):
        """域名解析失败：连 ICMP 句柄都不应创建。"""
        api = patch_api(monkeypatch)
        patch_resolve(monkeypatch, err=OSError("no such host"))
        assert icmp.ping("nope.example") is None
        assert api.created == [] and api.closed == []

    def test_zero_handle_returns_none(self, monkeypatch):
        api = patch_api(monkeypatch)
        api.handle = 0
        patch_resolve(monkeypatch)
        assert icmp.ping("host") is None
        assert api.closed == []                              # 提前返回，不关句柄

    def test_invalid_handle_value_returns_none(self, monkeypatch):
        """记录疑似 bug 1：INVALID_HANDLE_VALUE 经 c_void_p 变 None，此分支走 `not handle`。"""
        api = patch_api(monkeypatch)
        api.handle = 0xFFFFFFFFFFFFFFFF
        patch_resolve(monkeypatch)
        assert icmp.ping("host") is None
        assert api.closed == []

    def test_echo_returns_zero_means_timeout(self, monkeypatch):
        """IcmpSendEcho 返回 0（超时/丢包）→ None。"""
        api = patch_api(monkeypatch)
        api.echo_result = 0
        patch_resolve(monkeypatch)
        assert icmp.ping("host") is None
        assert api.closed == [0xABCD]

    def test_nonzero_reply_status_returns_none(self, monkeypatch):
        """应答状态非 IP_SUCCESS（如 11010 请求超时）→ None。"""
        api = patch_api(monkeypatch)
        api.reply_status = 11010
        patch_resolve(monkeypatch)
        assert icmp.ping("host") is None

    def test_sendecho_exception_degrades_and_closes(self, monkeypatch):
        """SendEcho 抛异常 → None，句柄仍在 finally 关闭。"""
        api = patch_api(monkeypatch)
        api.send_error = RuntimeError("mock boom")
        patch_resolve(monkeypatch)
        assert icmp.ping("host") is None
        assert api.closed == [0xABCD]

    def test_bad_ip_string_hits_generic_except(self, monkeypatch):
        """解析出的字符串不是合法 IPv4 → inet_aton 抛 OSError → 通用 except 兜底。"""
        api = patch_api(monkeypatch)
        patch_resolve(monkeypatch, ip="999.999.999.999")
        assert icmp.ping("host") is None
        assert api.closed == [0xABCD]


# ---------------- default_gateway ----------------

class TestDefaultGateway:
    def _patch_ps(self, monkeypatch, fake_cmd_result, stdout, code=0):
        monkeypatch.setattr(proc, "run_ps",
                            lambda script, timeout=40, resident=False:
                            fake_cmd_result(code, stdout))

    def test_valid_gateway(self, monkeypatch, fake_cmd_result):
        self._patch_ps(monkeypatch, fake_cmd_result, "192.168.1.1\r\n")
        assert icmp.default_gateway() == "192.168.1.1"

    def test_strips_whitespace(self, monkeypatch, fake_cmd_result):
        self._patch_ps(monkeypatch, fake_cmd_result, "  10.0.0.1  \n")
        assert icmp.default_gateway() == "10.0.0.1"

    def test_empty_output(self, monkeypatch, fake_cmd_result):
        self._patch_ps(monkeypatch, fake_cmd_result, "")
        assert icmp.default_gateway() == ""

    def test_ipv6_gateway_rejected(self, monkeypatch, fake_cmd_result):
        """IPv6 网关（无 3 个点）→ 空串。"""
        self._patch_ps(monkeypatch, fake_cmd_result, "fe80::1")
        assert icmp.default_gateway() == ""

    def test_two_dots_rejected(self, monkeypatch, fake_cmd_result):
        self._patch_ps(monkeypatch, fake_cmd_result, "192.168.1")
        assert icmp.default_gateway() == ""

    def test_ps_script_queries_default_route(self, monkeypatch, fake_cmd_result):
        """发出的 PowerShell 应查询 0.0.0.0/0 路由并按双跃点排序。"""
        seen = {}

        def fake_ps(script, timeout=40, resident=False):
            seen["script"] = script
            seen["timeout"] = timeout
            seen["resident"] = resident
            return fake_cmd_result(0, "192.168.1.1")
        monkeypatch.setattr(proc, "run_ps", fake_ps)
        icmp.default_gateway()
        assert "Get-NetRoute" in seen["script"] and "0.0.0.0/0" in seen["script"]
        assert "NextHop" in seen["script"] and seen["timeout"] == 25
        assert seen["resident"] is True            # 网关查询走常驻会话（补56）
