"""工具箱纯计算函数单元测试（不发起任何真实网络/子进程）。

覆盖范围：
- syskit/ui/toolbox_page.py :: DnsSubnetTab.calc()   子网计算（网络/掩码/广播/主机范围/主机数/总数）
- syskit/ui/toolbox_page.py :: DnsSubnetTab.query()  DNS 查询 UI 胶水（服务器名解析、错误提示、结果填充；
                                                      真实解析函数一律 monkeypatch）
- syskit/dnsquery.py           DNS 报文构造/解析（用假 socket 直接喂构造好的响应字节）
- syskit/ui/charts.py :: human_bytes（toolbox_page 使用的纯换算函数）
- syskit/config.py             DNS 预设等常量数据一致性
"""
import ipaddress
import socket
import struct

import pytest

from syskit import config, dnsquery

# ---------------- 通用小工具 ----------------


def _rows(listw):
    """读 QListWidget 全部条目文本。"""
    return [listw.item(i).text() for i in range(listw.count())]


@pytest.fixture
def dns_tab(qtbot):
    from syskit.ui.toolbox_page import DnsSubnetTab
    tab = DnsSubnetTab()
    qtbot.addWidget(tab)
    return tab


# ---------------- DnsSubnetTab.calc：子网计算 ----------------


def test_calc_typical_26(dns_tab):
    """典型输入 192.168.1.130/26（主机地址，非对齐，strict=False 应自动取网络号）。"""
    dns_tab.ed_ip.setText("192.168.1.130")
    dns_tab.spin_prefix.setValue(26)
    dns_tab.calc()
    assert _rows(dns_tab.list_sub) == [
        "网络地址: 192.168.1.128",
        "子网掩码: 255.255.255.192",
        "广播地址: 192.168.1.191",
        "可用主机范围: 192.168.1.129 ~ 192.168.1.190",
        "可用主机数: 62",
        "总地址数: 64",
    ]


def test_calc_default_inputs_24(dns_tab):
    """控件默认值 192.168.1.10/24 直接点计算。"""
    dns_tab.calc()
    assert _rows(dns_tab.list_sub) == [
        "网络地址: 192.168.1.0",
        "子网掩码: 255.255.255.0",
        "广播地址: 192.168.1.255",
        "可用主机范围: 192.168.1.1 ~ 192.168.1.254",
        "可用主机数: 254",
        "总地址数: 256",
    ]


def test_calc_via_button_click(dns_tab):
    """通过真实按钮触发计算（信号连接正确性），且重复计算先清空旧结果。

    注：btn_calc 在 DnsSubnetTab.__init__ 中是局部变量（未绑到 self），
    故用 findChildren 按文本找到“计算”按钮再点击。
    """
    from PySide6.QtWidgets import QPushButton
    dns_tab.ed_ip.setText("10.0.0.5")
    dns_tab.spin_prefix.setValue(30)
    btn = next(b for b in dns_tab.findChildren(QPushButton) if b.text() == "计算")
    btn.click()
    rows = _rows(dns_tab.list_sub)
    assert rows == [
        "网络地址: 10.0.0.4",
        "子网掩码: 255.255.255.252",
        "广播地址: 10.0.0.7",
        "可用主机范围: 10.0.0.5 ~ 10.0.0.6",
        "可用主机数: 2",
        "总地址数: 4",
    ]
    # 再次计算（换输入）不残留旧行
    dns_tab.ed_ip.setText("10.0.0.9")
    btn.click()
    rows2 = _rows(dns_tab.list_sub)
    assert len(rows2) == 6 and rows2[0] == "网络地址: 10.0.0.8"


@pytest.mark.parametrize("ip, prefix, expected", [
    # /30：4 个地址，2 个可用
    ("10.0.0.5", 30, ["网络地址: 10.0.0.4", "子网掩码: 255.255.255.252",
                      "广播地址: 10.0.0.7", "可用主机范围: 10.0.0.5 ~ 10.0.0.6",
                      "可用主机数: 2", "总地址数: 4"]),
    # /31（RFC 3021）：CPython hosts() 返回两个地址，但代码按 num-2 计数为 0（见疑似 bug 记录）
    ("10.0.0.5", 31, ["网络地址: 10.0.0.4", "子网掩码: 255.255.255.254",
                      "广播地址: 10.0.0.5", "可用主机范围: 10.0.0.4 ~ 10.0.0.5",
                      "可用主机数: 0", "总地址数: 2"]),
    # /32：单机
    ("10.0.0.5", 32, ["网络地址: 10.0.0.5", "子网掩码: 255.255.255.255",
                      "广播地址: 10.0.0.5", "可用主机范围: 10.0.0.5 ~ 10.0.0.5",
                      "可用主机数: 0", "总地址数: 1"]),
    # 全零 IP + /24
    ("0.0.0.0", 24, ["网络地址: 0.0.0.0", "子网掩码: 255.255.255.0",
                     "广播地址: 0.0.0.255", "可用主机范围: 0.0.0.1 ~ 0.0.0.254",
                     "可用主机数: 254", "总地址数: 256"]),
    # 各八位组 255 + /32
    ("255.255.255.255", 32, ["网络地址: 255.255.255.255", "子网掩码: 255.255.255.255",
                             "广播地址: 255.255.255.255", "可用主机范围: 255.255.255.255 ~ 255.255.255.255",
                             "可用主机数: 0", "总地址数: 1"]),
])
def test_calc_boundaries(dns_tab, ip, prefix, expected):
    dns_tab.ed_ip.setText(ip)
    dns_tab.spin_prefix.setValue(prefix)
    dns_tab.calc()
    assert _rows(dns_tab.list_sub) == expected


@pytest.mark.parametrize("bad", [
    "", "   ",                    # 空 / 纯空白
    "abc",                        # 非数字
    "300.1.1.1", "1.2.3.999",     # 越界八位组
    "1..2.3",                     # 多个点 / 空八位组
    "1.2.3", "1.2.3.4.5",         # 八位组数量错
    "192.168.1.1/24",             # 输入框里自带掩码（拼接后变成 .../24/24）
    "192.168.1.1:80",             # 带端口
])
def test_calc_invalid_input_shows_error_not_crash(dns_tab, bad):
    dns_tab.ed_ip.setText(bad)
    dns_tab.calc()  # 不应抛异常
    rows = _rows(dns_tab.list_sub)
    assert len(rows) == 1
    assert rows[0].startswith("输入无效")


def test_calc_prefix_spinbox_reachable_range(dns_tab):
    """前缀选择范围 8..32；/8 现已用地址算术推导首尾，不再 list(hosts()) 物化。"""
    assert dns_tab.spin_prefix.minimum() == 8
    assert dns_tab.spin_prefix.maximum() == 32
    assert dns_tab.spin_prefix.value() == 24


def test_calc_ipv4_slash8_does_not_freeze(dns_tab):
    """/8 约 1677 万地址：不得物化 hosts()，应瞬间出结果。"""
    dns_tab.ed_ip.setText("10.0.0.1")
    dns_tab.spin_prefix.setValue(8)
    dns_tab.calc()
    rows = _rows(dns_tab.list_sub)
    assert rows[3] == "可用主机范围: 10.0.0.1 ~ 10.255.255.254"
    assert rows[4] == "可用主机数: 16777214"


def test_calc_ipv6_accepted_documented(dns_tab):
    """IPv6 输入：spin_prefix 会把 126 钳位为 32；/32 大网段用算术推导，不冻结。"""
    dns_tab.ed_ip.setText("::1")
    dns_tab.spin_prefix.setValue(126)               # 被钳位为 32
    assert dns_tab.spin_prefix.value() == 32
    dns_tab.calc()
    rows = _rows(dns_tab.list_sub)
    assert rows[0] == "网络地址: ::"
    assert rows[1] == "子网掩码: ffff:ffff::"
    assert rows[2] == "广播地址: ::ffff:ffff:ffff:ffff:ffff:ffff"
    assert rows[3] == "可用主机范围: ::1 ~ ::ffff:ffff:ffff:ffff:ffff:fffe"
    assert rows[4] == "可用主机数: 79228162514264337593543950334"  # 2**96 - 2
    assert rows[5] == "总地址数: 79228162514264337593543950336"   # 2**96


# ---------------- DnsSubnetTab.query：DNS 查询 UI 胶水（全部 mock，不出网） ----------------


def test_query_empty_domain_noop(dns_tab):
    """空域名：清空列表后直接返回，不发查询。"""
    dns_tab.list_dns.addItem("旧结果")
    dns_tab.ed_domain.setText("   ")
    dns_tab.query()
    assert dns_tab.list_dns.count() == 0


def test_query_success_custom_server(dns_tab, monkeypatch):
    calls = {}

    def fake_query(host, server, timeout=3.0, qtype=1):
        calls.update(host=host, server=server, timeout=timeout)
        return ["1.2.3.4", "5.6.7.8"]

    monkeypatch.setattr(dnsquery, "query", fake_query)
    dns_tab.ed_domain.setText(" example.com ")
    dns_tab.cmb_server.setCurrentIndex(1)      # “223.5.5.5（阿里）”
    dns_tab.query()
    assert _rows(dns_tab.list_dns) == ["1.2.3.4", "5.6.7.8"]
    assert calls == {"host": "example.com", "server": "223.5.5.5", "timeout": 3.0}


def test_query_failure_shows_error(dns_tab, monkeypatch):
    def boom(host, server, timeout=3.0, qtype=1):
        raise RuntimeError("超时")

    monkeypatch.setattr(dnsquery, "query", boom)
    dns_tab.ed_domain.setText("example.com")
    dns_tab.cmb_server.setCurrentIndex(2)      # “119.29.29.29（腾讯）”
    dns_tab.query()
    assert _rows(dns_tab.list_dns) == ["查询失败: 超时"]


def test_query_system_default_dedup_sorted(dns_tab, monkeypatch):
    """系统默认分支：getaddrinfo 结果去重 + 排序。"""
    infos = [(socket.AF_INET, 1, 6, "", ("9.9.9.9", 0)),
             (socket.AF_INET, 1, 6, "", ("1.1.1.1", 0)),
             (socket.AF_INET, 1, 6, "", ("9.9.9.9", 0))]
    monkeypatch.setattr(socket, "getaddrinfo", lambda *a, **k: infos)
    dns_tab.ed_domain.setText("example.com")
    dns_tab.cmb_server.setCurrentIndex(0)      # 系统默认
    dns_tab.query()
    assert _rows(dns_tab.list_dns) == ["1.1.1.1", "9.9.9.9"]


def test_query_system_default_error_shown(dns_tab, monkeypatch):
    def boom(*a, **k):
        raise socket.gaierror(-2, "Name or service not known")

    monkeypatch.setattr(socket, "getaddrinfo", boom)
    dns_tab.ed_domain.setText("no-such.invalid")
    dns_tab.cmb_server.setCurrentIndex(0)
    dns_tab.query()
    rows = _rows(dns_tab.list_dns)
    assert len(rows) == 1 and rows[0].startswith("查询失败")


# ---------------- dnsquery：报文构造 / 响应解析（假 socket，无真实 UDP） ----------------

QUESTION = b"\x03www\x05baidu\x03com\x00" + b"\x00\x01\x00\x01"   # www.baidu.com A IN


def _header(txid, flags=0x8180, qd=1, an=1):
    return struct.pack(">HHHHHH", txid, flags, qd, an, 0, 0)


def _answer(rtype=1, rdata=b"\x5d\xb8\xd8\x22", name=b"\xc0\x0c", ttl=300):
    """默认：名字用压缩指针 0xC00C（指向 Question），A 记录 93.184.216.34。"""
    return name + struct.pack(">HHIH", rtype, 1, ttl, len(rdata)) + rdata


@pytest.fixture
def fake_dns_socket(monkeypatch):
    """替换 dnsquery 使用的 socket 类为假实现；固定 os.urandom 使 txid 确定。"""
    class FakeDnsSocket:
        response = b""
        instances = []

        def __init__(self, *args, **kwargs):
            self.sent_to = []
            self.timeouts = []
            self.closed = False
            type(self).instances.append(self)

        def settimeout(self, t):
            self.timeouts.append(t)

        def sendto(self, data, addr):
            self.sent_to.append((data, addr))

        def recvfrom(self, bufsize):
            return type(self).response, ("223.5.5.5", 53)

        def close(self):
            self.closed = True

    monkeypatch.setattr(dnsquery.socket, "socket", FakeDnsSocket)
    monkeypatch.setattr(dnsquery.os, "urandom", lambda n: b"\xab\xcd")
    FakeDnsSocket.instances = []
    return FakeDnsSocket


def test_query_builds_rfc1035_packet(fake_dns_socket):
    """构造的查询报文：头部/flag/qdcount/QNAME/qtype/qclass + 收发参数。"""
    fake_dns_socket.response = _header(0xABCD) + QUESTION + _answer()
    ips = dnsquery.query("www.baidu.com", "223.5.5.5", timeout=2.5)
    assert ips == ["93.184.216.34"]

    s = fake_dns_socket.instances[-1]
    assert s.timeouts == [2.5]
    assert s.closed is True
    (pkt, addr), = s.sent_to
    assert addr == ("223.5.5.5", 53)
    assert struct.unpack(">HHHHHH", pkt[:12]) == (0xABCD, 0x0100, 1, 0, 0, 0)
    assert pkt[12:27] == b"\x03www\x05baidu\x03com\x00"
    assert pkt[27:] == b"\x00\x01\x00\x01"          # qtype=A(1), qclass=IN(1)


def test_query_parses_a_and_aaaa_answers(fake_dns_socket):
    """多应答：A + AAAA（名字均为压缩指针），分别 inet_ntoa/ntop。"""
    v6 = struct.pack(">8H", 0x2001, 0x0DB8, 0, 0, 0, 0, 0, 1)
    fake_dns_socket.response = (
        _header(0xABCD, an=2) + QUESTION
        + _answer(rtype=1, rdata=b"\x01\x02\x03\x04")
        + _answer(rtype=28, rdata=v6))
    assert dnsquery.query("www.baidu.com", "223.5.5.5", qtype=28) == \
        ["1.2.3.4", "2001:db8::1"]


def test_query_skips_question_with_compression_pointer(fake_dns_socket):
    """Question 区名字也用压缩指针时（非典型服务器），仍应正确定位 Answer 区。"""
    q = b"\xc0\x0c" + b"\x00\x01\x00\x01"
    fake_dns_socket.response = _header(0xABCD) + q + _answer()
    assert dnsquery.query("www.baidu.com", "223.5.5.5") == ["93.184.216.34"]


def test_query_rejects_mismatched_txid(fake_dns_socket):
    fake_dns_socket.response = _header(0x1234) + QUESTION + _answer()
    with pytest.raises(RuntimeError, match="响应异常"):
        dnsquery.query("www.baidu.com", "223.5.5.5")


def test_query_rejects_zero_answer_records(fake_dns_socket):
    fake_dns_socket.response = _header(0xABCD, an=0) + QUESTION
    with pytest.raises(RuntimeError, match="响应异常"):
        dnsquery.query("www.baidu.com", "223.5.5.5")


def test_query_rejects_nonzero_rcode(fake_dns_socket):
    """rcode=3（NXDOMAIN），但带 1 条应答以越过 an==0 检查。"""
    fake_dns_socket.response = _header(0xABCD, flags=0x8183, an=1) + QUESTION + _answer()
    with pytest.raises(RuntimeError, match="错误码 3"):
        dnsquery.query("www.baidu.com", "223.5.5.5")


def test_query_no_a_aaaa_records_raises(fake_dns_socket):
    """只有 CNAME（type 5）应答时给出明确错误。"""
    target = b"\x03abc\xc0\x0c"
    cname = _answer(rtype=5, rdata=target)
    fake_dns_socket.response = _header(0xABCD) + QUESTION + cname
    with pytest.raises(RuntimeError, match="没有 A/AAAA"):
        dnsquery.query("www.baidu.com", "223.5.5.5")


def test_query_safe_returns_empty_on_error(fake_dns_socket):
    fake_dns_socket.response = _header(0xDEAD) + QUESTION + _answer()  # txid 不匹配
    assert dnsquery.query_safe("www.baidu.com", "223.5.5.5") == []


def test_query_idna_label_length_prefix(fake_dns_socket):
    """回归：长度前缀必须等于标签 IDNA 编码后的字节数（'例子' 2 字符 → 11 字节）。

    原实现用 len(label)（字符数）写长度前缀，中文域名会构造出畸形 QNAME。
    """
    fake_dns_socket.response = _header(0xABCD) + QUESTION + _answer()
    assert dnsquery.query("例子.com", "223.5.5.5") == ["93.184.216.34"]
    (pkt, _), = fake_dns_socket.instances[-1].sent_to
    enc = "例子".encode("idna")                      # b'xn--fsqu00a'（11 字节）
    assert enc == b"xn--fsqu00a"
    assert pkt[12:] == bytes([len(enc)]) + enc + b"\x03com\x00" + b"\x00\x01\x00\x01"


def test_encode_qname_ascii_matches_legacy_format():
    """回归：纯 ASCII 域名编码结果与旧实现逐字节一致（不改变既有行为）。"""
    assert dnsquery._encode_qname("www.baidu.com") == b"\x03www\x05baidu\x03com\x00"
    assert dnsquery._encode_qname("a.b") == b"\x01a\x01b\x00"


def test_encode_qname_rejects_illegal_label():
    from syskit import dnsquery as dq
    with pytest.raises(ValueError, match="非法 DNS 标签"):
        dq._encode_qname("")
    with pytest.raises(ValueError, match="非法 DNS 标签"):
        dq._encode_qname("x" * 64 + ".com")          # 单标签超过 63 字节


# ---------------- dnsquery.resolve_system（mock getaddrinfo，不出网） ----------------


def test_resolve_system_sorted_dedup(monkeypatch):
    infos = [(socket.AF_INET, 1, 6, "", ("2.2.2.2", 0)),
             (socket.AF_INET, 1, 6, "", ("1.1.1.1", 0)),
             (socket.AF_INET, 1, 6, "", ("2.2.2.2", 0))]
    monkeypatch.setattr(socket, "getaddrinfo", lambda *a, **k: infos)
    assert dnsquery.resolve_system("x.example") == ["1.1.1.1", "2.2.2.2"]


def test_resolve_system_error_returns_empty(monkeypatch):
    def boom(*a, **k):
        raise OSError("gaierror")

    monkeypatch.setattr(socket, "getaddrinfo", boom)
    assert dnsquery.resolve_system("x.example") == []


# ---------------- charts.human_bytes（toolbox_page 使用的纯换算） ----------------


@pytest.mark.parametrize("v, expected", [
    (0, "0B"),
    (512, "512B"),
    (1023.9, "1024B"),          # 边界取整行为（<1024 判定后再格式化）
    (1024, "1.0KB"),
    (1500, "1.5KB"),
    (1024 ** 2, "1.0MB"),
    (5 * 1024 ** 3, "5.0GB"),
    (2 * 1024 ** 4, "2.0TB"),
    (3 * 1024 ** 5, "3072.0TB"),  # 超出 TB 后停在 TB 档
    (-800, "-800B"),
])
def test_human_bytes(v, expected):
    from syskit.ui.charts import human_bytes
    assert human_bytes(v) == expected


# ---------------- config：DNS 预设与常量数据一致性 ----------------


def test_dns_presets_exactly_two_valid_ipv4():
    assert config.DNS_PRESETS, "DNS_PRESETS 不应为空"
    for name, pair in config.DNS_PRESETS.items():
        assert isinstance(name, str) and name
        assert len(pair) == 2, f"预设 {name} 应恰有两个 DNS"
        primary, secondary = pair
        assert ipaddress.ip_address(primary).version == 4, f"预设 {name} 主 {primary}"
        assert ipaddress.ip_address(secondary).version == 4, f"预设 {name} 备 {secondary}"
        assert primary != secondary, f"预设 {name} 主备相同"


def test_defaults_dns_preset_key_exists():
    assert config.DEFAULTS["dns_preset"] in config.DNS_PRESETS


def test_public_dns_test_and_blacklist_valid():
    assert len(config.PUBLIC_DNS_TEST) == 2
    for ip in config.PUBLIC_DNS_TEST:
        assert ipaddress.ip_address(ip).version == 4
    for ip in config.FOREIGN_DNS_BLACKLIST:
        assert ipaddress.ip_address(ip).version == 4


def test_ncsi_dns_expectations_consistent():
    assert ipaddress.ip_address(config.NCSI_DNS_EXPECT).version == 4
    assert ipaddress.ip_address(config.NCSI_STOCK["ActiveDnsProbeContent"]).version == 4
    assert ipaddress.ip_address(config.NCSI_STOCK["ActiveDnsProbeContentV6"]).version == 6
    assert config.NCSI_DNS_EXPECT == config.NCSI_STOCK["ActiveDnsProbeContent"]
