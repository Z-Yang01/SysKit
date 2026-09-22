"""syskit/snapshot.py 单元测试：PowerShell JSON 快照解析（纯解析逻辑，不跑真实 PowerShell）。

Mock 边界（配合 conftest 哨兵双保险）：
- proc.run_ps 打桩返回可控 stdout（空输出 / 带杂讯 JSON / 损坏 JSON / 全量数据），
  conftest 哨兵保证漏打桩时直接失败而非真实执行。

历史记录（已清算）：
1. physical_up_adapters 的 docstring 曾与实现不一致（只看 Up+IPv4），已按实现
   对齐 docstring（迭代日志疑似 bug #10 结案）。
"""
import json

import pytest

from syskit import proc, snapshot
from syskit.snapshot import Adapter, Snapshot, _as_list, _clean, _parse_json_loose, current_ssid, take


# ---------------- dataclass 辅助 ----------------

def make_adapter(**kw):
    defaults = dict(name="WLAN", desc="Intel(R) Wi-Fi 6 AX201", status="Up", if_index=5,
                    mac="AA-BB-CC-DD-EE-FF", speed="1 Gbps", virtual=False,
                    ipv4=["192.168.1.10"], gw4=["192.168.1.1"], dns=["223.5.5.5"])
    defaults.update(kw)
    return Adapter(**defaults)


class TestSnapshotHelpers:
    # （test_adapter_found_and_missing 已删除：Snapshot.adapter() 是仅测试存活的
    #   重复查找助手——生产一律 next(a for a in snap.adapters if ...)，已删源码。）

    def test_physical_up_adapters_filter(self):
        """只保留 status==Up 且有 ipv4 的网卡（见文件头疑似 bug 1 的 doc 不一致）。"""
        up = make_adapter()
        up_no_ip = make_adapter(name="无IP", ipv4=[])
        down = make_adapter(name="断开", status="Disconnected")
        snap = Snapshot()
        snap.adapters = [up, up_no_ip, down]
        assert snap.physical_up_adapters() == [up]

    def test_physical_up_case_insensitive(self):
        a = make_adapter(status="UP")
        snap = Snapshot()
        snap.adapters = [a]
        assert snap.physical_up_adapters() == [a]

    def test_service_lookup(self):
        snap = Snapshot()
        snap.services = {"Dhcp": {"status": "Running", "startType": "Auto"}}
        assert snap.service("Dhcp")["status"] == "Running"
        assert snap.service("Nope") is None

    def test_snapshot_defaults(self):
        snap = Snapshot()
        assert snap.adapters == [] and snap.ok is False and snap.error == ""
        assert snap.services == {} and snap.if_metrics == {}


# ---------------- _clean / _as_list / _parse_json_loose ----------------

class TestClean:
    @pytest.mark.parametrize("raw,expect", [
        (None, ""),
        ("  x  ", "x"),
        ("", ""),
        (5, "5"),
        (0, "0"),
        ("中 文", "中 文"),
    ])
    def test_clean(self, raw, expect):
        assert _clean(raw) == expect


class TestAsList:
    @pytest.mark.parametrize("raw,expect", [
        (None, []),
        ([1, 2], [1, 2]),
        ("single", ["single"]),
        ({"a": 1}, [{"a": 1}]),
        ([], []),
    ])
    def test_as_list(self, raw, expect):
        assert _as_list(raw) == expect


class TestParseJsonLoose:
    def test_clean_json(self):
        assert _parse_json_loose('{"a": 1}') == {"a": 1}

    def test_noise_around_json(self):
        text = "WARNING: xxx\n  {\"a\": [1,2]}  \ntrailing"
        assert _parse_json_loose(text) == {"a": [1, 2]}

    def test_nested_braces_take_outermost(self):
        text = "noise {\"o\": {\"b\": 2}} tail"
        assert _parse_json_loose(text) == {"o": {"b": 2}}

    def test_no_braces_returns_none(self):
        assert _parse_json_loose("no json here") is None

    def test_empty_text_returns_none(self):
        assert _parse_json_loose("") is None

    def test_lonely_open_brace_returns_none(self):
        assert _parse_json_loose("{") is None

    def test_broken_inner_raises(self):
        """截取到花括号但内容损坏 → 抛 JSONDecodeError（由 take 兜底）。"""
        with pytest.raises(json.JSONDecodeError):
            _parse_json_loose("{bad json}")


# ---------------- 采集脚本口径（文本契约，不跑 PowerShell） ----------------

class TestPsScriptShape:
    """锁住采集脚本的性能与口径：这三条都是实测出来的结论，改回去就会退回 6.8s 或让
    网关/DNS 判定对象漂移。

    - Get-NetIPConfiguration 单跑使整脚本 4.2s -> 6.8s（它内部还会再解析一遍连通性），
      已换成 Get-NetIPAddress + Get-DnsClientServerAddress + 复用默认路由；
    - DNS 查询不能带 -AddressFamily IPv4：旧口径里未显式配 DNS 的网卡会带 IPv6 链路项
      （fec0:0:0:ffff::x），只查 IPv4 会让这些网卡 dns 变空，静态 IP 判定对象漂移。
    """

    def _code(self):
        """脚本去掉 # 注释行（注释里要写为什么换掉旧命令，不能反过来把断言绑死）。"""
        return "\n".join(l for l in snapshot._PS_SCRIPT.splitlines() if not l.lstrip().startswith("#"))

    def test_slow_ipconfiguration_cmdlet_is_gone(self):
        code = self._code()
        assert "Get-NetIPConfiguration" not in code
        assert "Get-NetIPAddress" in code

    def test_default_route_scanned_once(self):
        """0/0 路由只查一遍，ipcfg 的 gw4 与 routes4 共用同一结果。"""
        code = self._code()
        assert code.count("DestinationPrefix -eq '0.0.0.0/0'") == 1
        ipcfg_block = code.split("$out.ipcfg =")[1].split("$out.routes4")[0]
        assert "$defRoutes4" in ipcfg_block

    def test_dns_query_not_filtered_to_ipv4(self):
        line = next(l for l in self._code().splitlines() if "Get-DnsClientServerAddress" in l)
        assert "AddressFamily" not in line


# ---------------- take ----------------

def full_ps_data():
    """模拟 PowerShell ConvertTo-Json 的全量输出结构。"""
    return {
        "adapters": [
            {"name": "WLAN", "desc": "Intel(R) Wi-Fi 6 AX201", "status": "Up",
             "ifIndex": 5, "mac": "AA-BB-CC-DD-EE-FF", "speed": "1.2 Gbps",
             "virtual": False},
            {"name": "以太网", "desc": "Realtek PCIe GbE", "status": "Disconnected",
             "ifIndex": 3, "mac": "11-22-33-44-55-66", "speed": "",
             "virtual": False},
        ],
        "ipcfg": [
            {"ifIndex": 5, "name": "WLAN", "ipv4": ["192.168.1.10", ""],
             "gw4": ["192.168.1.1"], "dns": ["223.5.5.5", "fec0:0:0:ffff::1"]},
        ],
        "routes4": [
            {"ifIndex": 5, "nextHop": "192.168.1.1", "routeMetric": 0,
             "ifMetric": 25, "store": "ActiveStore"},
        ],
        "routesPersistent": [
            {"dest": "0.0.0.0/0", "nextHop": "172.16.2.1", "metric": 1, "ifIndex": 3},
        ],
        "connProfiles": [
            {"name": "家庭网络", "ifIndex": 5, "category": "Private",
             "v4": "Internet", "v6": "LocalNetwork"},
        ],
        "services": [
            {"name": "Dhcp", "status": "Running", "startType": "Automatic"},
            {"name": "Dnscache", "status": "Running", "startType": "Auto"},
        ],
        "wlanIfs": "  There is 1 interface on the system:\n\n    SSID: HomeWiFi  ",
        "ifMetrics": [
            {"ifIndex": 5, "name": "WLAN", "metric": 25},
            {"ifIndex": 3, "name": "以太网", "metric": 35},
        ],
    }


def patch_ps(monkeypatch, fake_cmd_result, stdout="", stderr="", code=0):
    monkeypatch.setattr(proc, "run_ps",
                        lambda script, timeout=40, resident=False:
                        fake_cmd_result(code, stdout, stderr))


class TestTakeFailures:
    def test_empty_stdout_with_stderr(self, monkeypatch, fake_cmd_result):
        patch_ps(monkeypatch, fake_cmd_result, stdout="", stderr="command not found")
        snap = take()
        assert snap.ok is False and snap.error == "command not found"
        assert snap.adapters == []

    def test_empty_stdout_blank_stderr(self, monkeypatch, fake_cmd_result):
        patch_ps(monkeypatch, fake_cmd_result, stdout="   \n", stderr="")
        snap = take()
        assert snap.ok is False and snap.error == "PowerShell 无输出"

    def test_garbage_without_braces(self, monkeypatch, fake_cmd_result):
        patch_ps(monkeypatch, fake_cmd_result, stdout="some localized error text")
        snap = take()
        assert snap.ok is False and snap.error == "PowerShell 返回空数据"

    def test_broken_json_reports_parse_error(self, monkeypatch, fake_cmd_result):
        patch_ps(monkeypatch, fake_cmd_result, stdout="{oops}")
        snap = take()
        assert snap.ok is False
        assert snap.error.startswith("JSON 解析失败")
        assert "Expecting" in snap.error              # 异常信息带上下文


class TestTakeSuccess:
    def test_full_snapshot_parse(self, monkeypatch, fake_cmd_result):
        patch_ps(monkeypatch, fake_cmd_result,
                 stdout=json.dumps(full_ps_data(), ensure_ascii=False))
        snap = take()
        assert snap.ok is True and snap.error == ""

        # 网卡基础字段
        wlan, eth = snap.adapters
        assert (wlan.name, wlan.desc, wlan.status, wlan.if_index) == \
            ("WLAN", "Intel(R) Wi-Fi 6 AX201", "Up", 5)
        assert wlan.mac == "AA-BB-CC-DD-EE-FF" and wlan.speed == "1.2 Gbps"
        assert eth.name == "以太网" and eth.status == "Disconnected"

        # ipcfg / connProfiles 按 ifIndex 合并（空串过滤）
        assert wlan.ipv4 == ["192.168.1.10"]
        assert wlan.gw4 == ["192.168.1.1"]
        assert wlan.dns == ["223.5.5.5", "fec0:0:0:ffff::1"]
        assert wlan.conn_name == "家庭网络"
        assert wlan.conn_v4 == "Internet"
        assert wlan.conn_category == "Private"
        assert eth.ipv4 == [] and eth.conn_name == ""    # 无匹配 ipcfg

        # 路由 / 持久路由 / 连接配置
        assert snap.default_routes == [dict(dest="0.0.0.0/0", nextHop="192.168.1.1",
                                            metric=0, if_metric=25, if_index=5,
                                            store="ActiveStore")]
        assert snap.persistent_routes == [dict(dest="0.0.0.0/0", nextHop="172.16.2.1",
                                               metric=1, if_index=3)]
        assert snap.conn_profiles == [dict(name="家庭网络", if_index=5,
                                           v4="Internet", v6="LocalNetwork")]

        # 服务 / wlan 文本 / 接口跃点
        assert snap.services == {"Dhcp": {"status": "Running", "startType": "Automatic"},
                                 "Dnscache": {"status": "Running", "startType": "Auto"}}
        assert "SSID: HomeWiFi" in snap.wlan_text
        assert snap.if_metrics == {5: 25, 3: 35}

    def test_single_item_dicts_wrapped(self, monkeypatch, fake_cmd_result):
        """ConvertTo-Json 对单项集合输出 dict 而非 list → _as_list 兜底。"""
        data = full_ps_data()
        for key in ("adapters", "ipcfg", "routes4", "routesPersistent",
                    "connProfiles", "services", "ifMetrics"):
            data[key] = data[key][0]
        patch_ps(monkeypatch, fake_cmd_result, stdout=json.dumps(data, ensure_ascii=False))
        snap = take()
        assert snap.ok is True
        assert [a.name for a in snap.adapters] == ["WLAN"]
        assert snap.default_routes[0]["nextHop"] == "192.168.1.1"
        assert snap.persistent_routes[0]["nextHop"] == "172.16.2.1"
        assert snap.conn_profiles[0]["name"] == "家庭网络"
        assert set(snap.services) == {"Dhcp"}
        assert snap.if_metrics == {5: 25}

    def test_missing_sections_default_empty(self, monkeypatch, fake_cmd_result):
        patch_ps(monkeypatch, fake_cmd_result, stdout=json.dumps({"adapters": []}))
        snap = take()
        assert snap.ok is True
        assert (snap.default_routes == [] and snap.persistent_routes == []
                and snap.conn_profiles == [] and snap.services == {}
                and snap.if_metrics == {} and snap.wlan_text == "")

    def test_ifindex_missing_or_blank_defaults_zero(self, monkeypatch, fake_cmd_result):
        data = {"adapters": [{"name": "X", "status": "Up", "ifIndex": None}],
                "ipcfg": [{"ifIndex": "", "ipv4": ["10.0.0.9"]}]}
        patch_ps(monkeypatch, fake_cmd_result, stdout=json.dumps(data))
        snap = take()
        a = snap.adapters[0]
        assert a.if_index == 0
        assert a.ipv4 == ["10.0.0.9"]                    # 双双落到 ifIndex 0 → 仍合并

    def test_run_ps_called_with_default_timeout(self, monkeypatch, fake_cmd_result):
        seen = {}

        def fake_ps(script, timeout=40, resident=False):
            seen["timeout"] = timeout
            seen["resident"] = resident
            return fake_cmd_result(0, json.dumps({"adapters": []}))
        monkeypatch.setattr(proc, "run_ps", fake_ps)
        take()
        assert seen["timeout"] == 40
        assert seen["resident"] is True            # 快照走常驻会话（补56）


# ---------------- current_ssid ----------------

class TestCurrentSsid:
    def test_english_colon(self):
        assert current_ssid("SSID: HomeWiFi") == "HomeWiFi"

    def test_full_width_colon(self):
        assert current_ssid("SSID：家里WiFi") == "家里WiFi"

    def test_empty_quoted_ssid_returns_empty(self):
        assert current_ssid('SSID: ""') == ""

    def test_bssid_not_matched(self):
        """\\b 词边界保证 BSSID 行不会被误认成 SSID。"""
        assert current_ssid("    BSSID 1: aa:bb:cc:dd:ee:ff") == ""

    def test_first_ssid_wins_in_real_output(self):
        text = "  Name: WLAN\n  Description: Intel AX201\n  SSID: Home-5G\n" \
               "  BSSID 1: aa:bb\n  BSS Type: Infrastructure\n  SSID: Home-2G"
        assert current_ssid(text) == "Home-5G"

    def test_surrounding_whitespace_stripped(self):
        assert current_ssid("  SSID:   Spaced   ") == "Spaced"

    def test_empty_text(self):
        assert current_ssid("") == ""
        assert current_ssid() == ""
