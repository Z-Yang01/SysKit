"""多子代理查 bug 第六批回归（2026-09-19）：密码库安全 / 网络逻辑 / UI 交互。

对应修复（均先经总控核实代码证据）：
- vault_crypto：KDF 迭代次数按落盘值读回（曾恒用当前常量——调参升级会锁死全部存量库）；
- vault：search_sites/list_accounts LIKE 通配符转义、find_by_domain 别名精确匹配、
  totp_code 输入容错（预 padded/小写/空格，非法字符返回空串而非抛 binascii.Error）；
- netcheck：chk_nlasvc 三元组解包、chk_wlan MAC 随机化正则对齐真实 netsh 输出、
  污染特征判定先于交集 PASS、探测强制直连（残留代理不再劫持判定）；
- dnsquery：UDP 丢包重发 + TC 截断显式报错、resolve_system 改 daemon 线程（不再拖住退出）；
- repairs：PowerOff 写 REG_DWORD（REG_SZ 不被电源策略识别）、RestartServices 用
  flushdns 替代受保护的 Dnscache、_pick_adapter 识别英文无线网卡名并优先在线物理网卡；
- snapshot：physical_up_adapters 排除虚拟网卡（TUN 假象不再污染网关/DNS 判定）；
- icmp：default_gateway 限定 IPv4 地址族；
- ui：wifi 二维码防错配、启动文件夹 .disabled 状态反转、性能表选中按 PID 跟随、
  持续 ping 按钮文案同步、复制报告空/中止守卫。

红线：真实 AESGCM/DPAPI 仅本地计算；网络/注册表写/剪贴板全程 mock 或哨兵拦截。
"""
import base64
import os
import socket as socket_mod

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

import pytest
from PySide6.QtWidgets import QApplication, QSystemTrayIcon

from test_repairs import _patch_snapshot, _snap
from test_ui_smoke import (  # noqa: E402,F401
    main_win, make_main_window, ui_offline_mocks)

QSystemTrayIcon.showMessage = lambda self, *a, **k: None  # noqa: E305 离屏无托盘实现

from syskit import vault  # noqa: E402


# ---------------- vault_crypto：KDF 迭代读回 ----------------

def test_kdf_iter_read_back_keeps_old_vaults_unlockable(tmp_path, monkeypatch):
    """iter 按落盘值派生：调参升级（KDF_ITER 上调）后老库必须仍可解锁。

    回归：unlock/verify 曾恒用当前模块常量——一旦上调迭代数，全部存量库
    verifier 解不开，用户会得到「主密码错误」并可能误删整个库。
    """
    from syskit import vault_crypto as vc

    key_file = str(tmp_path / "vault.key")
    monkeypatch.setattr(vc, "KDF_ITER", 1000)
    c1 = vc.VaultCrypto(key_file=key_file)
    c1.setup("pw-old")
    assert c1.unlock("pw-old") is True
    c1.lock()

    monkeypatch.setattr(vc, "KDF_ITER", 20_000)      # 模拟未来版本调参升级
    c2 = vc.VaultCrypto(key_file=key_file)
    assert c2.has_master_password() is True
    assert c2.unlock("pw-old") is True               # 按落盘 iter=1000 派生 → 成功
    c2.lock()

    c3 = vc.VaultCrypto(key_file=key_file)
    assert c3.unlock("wrong-password") is False
    assert c3._meta()["kdf"]["iter"] == 1000         # 落盘值未被读回逻辑改动


# ---------------- vault：搜索转义 / 域名精确匹配 / TOTP 容错 ----------------

def test_search_sites_escapes_like_wildcards():
    vault.init_db()
    s1 = vault.add_site("100%")
    vault.add_site("100X")
    s3 = vault.add_site("a_b")
    vault.add_site("axb")
    assert [r["id"] for r in vault.search_sites("100%")] == [s1]   # 字面 %，不通配
    assert [r["id"] for r in vault.search_sites("a_b")] == [s3]    # 字面 _


def test_find_by_domain_alias_exact_not_substring():
    vault.init_db()
    vault.add_site("Gmail Mirror", aliases="gmail.com")
    assert vault.find_by_domain("mail.com") is None               # 子串不再命中
    assert vault.find_by_domain("gmail.com")["name"] == "Gmail Mirror"
    vault.add_site("Example", domain="example.com")
    assert vault.find_by_domain("EXAMPLE.com")["name"] == "Example"


RFC_SECRET = base64.b32encode(b"12345678901234567890").decode("ascii")


def test_totp_code_input_robustness():
    """兼容常见粘贴格式；字母表外字符（0/1/8/9）返回空串而非抛异常。"""
    at = 1_000_000
    base = vault.totp_code(RFC_SECRET, at=at)
    assert base == vault.totp_code(RFC_SECRET + "=", at=at)            # 已带 padding
    assert base == vault.totp_code(RFC_SECRET.lower(), at=at)          # 小写
    assert base == vault.totp_code(" ".join(RFC_SECRET), at=at)        # 空格分组
    assert vault.totp_code("AB01", at=at) == ""                        # 非法字符 → ""
    assert vault.totp_code("", at=at) == ""


# ---------------- netcheck / dnsquery / icmp ----------------

def test_dns_query_retries_on_timeout(monkeypatch):
    """单次 UDP 丢包不再直接判失败：超时后重发（attempts 次）。"""
    from syskit import dnsquery as dq

    class FakeSock:
        def __init__(self):
            self.sends = 0

        def settimeout(self, t):
            pass

        def sendto(self, data, addr):
            self.sends += 1

        def recvfrom(self, n):
            raise socket_mod.timeout("timed out")

        def close(self):
            pass

    fake = FakeSock()
    monkeypatch.setattr(dq.socket, "socket", lambda *a, **k: fake)
    with pytest.raises(socket_mod.timeout):
        dq.query("www.baidu.com", "223.5.5.5", timeout=0.01, attempts=3)
    assert fake.sends == 3


def test_dns_query_tc_truncated_raises(monkeypatch):
    """TC=1（截断应答）显式报错，不再当半截报文解析。"""
    import struct as _s
    from syskit import dnsquery as dq

    class FakeSock:
        def settimeout(self, t):
            pass

        def sendto(self, data, addr):
            pass

        def recvfrom(self, n):
            # TC 位（0x0200）置位；an=1 越过 an==0 检查
            return _s.pack(">HHHHHH", 0x1234, 0x8380 | 0x0200, 1, 1, 0, 0), ("x", 53)

        def close(self):
            pass

    monkeypatch.setattr(dq.socket, "socket", lambda *a, **k: FakeSock())
    monkeypatch.setattr(dq.os, "urandom", lambda n: b"\x12\x34")
    with pytest.raises(RuntimeError, match="截断"):
        dq.query("www.baidu.com", "223.5.5.5")


def test_http_get_bypasses_system_proxy():
    """诊断探测必须直连：经系统代理转发会把「代理挂了」误判成断网。

    build_opener(ProxyHandler({})) 的 opener 不做任何系统代理查找
    （无 ProxyHandler 或空代理表均等价于直连）。
    """
    import urllib.request as ur
    from syskit import netcheck

    ph = [h for h in netcheck._OPENER_DIRECT.handlers
          if type(h).__name__ == "ProxyHandler"]
    assert ph == [] or ph[0].proxies == {}
    # 对照：默认 opener 的 ProxyHandler 读 getproxies()（注册表/环境）
    default_ph = [h for h in ur.build_opener().handlers
                  if type(h).__name__ == "ProxyHandler"]
    assert default_ph and default_ph[0].proxies is not None


def test_gateway_query_pins_ipv4_family():
    """default_gateway 的 PowerShell 必须 -AddressFamily IPv4（否则 v6 默认路由抢排序）。"""
    from syskit import icmp
    # 直接读文件：运行期属性可能被离线夹具替换，getsource 不可靠
    src = open(os.path.join(os.path.dirname(icmp.__file__), "icmp.py"),
               encoding="utf-8").read()
    assert "-AddressFamily IPv4" in src


# ---------------- snapshot / repairs ----------------

def test_physical_up_adapters_excludes_virtual():
    from syskit.snapshot import Adapter, Snapshot
    physical = Adapter(name="以太网", desc="Realtek", virtual=False,
                       status="Up", ipv4=["192.168.1.2"])
    tun = Adapter(name="Clash", desc="Mihomo TUN", virtual=True,
                  status="Up", ipv4=["198.18.0.1"])
    snap = Snapshot(adapters=[physical, tun])
    assert snap.physical_up_adapters() == [physical]   # TUN 假象不进网关/DNS 判定


def test_pick_adapter_recognizes_english_wifi_name(monkeypatch):
    """英文系统无线网卡名 "Wi-Fi" 参与优先选择（曾只认 WLAN → 选到未接线网卡）。"""
    from syskit import repairs
    from syskit.snapshot import Adapter

    adapters = [Adapter(name="Wi-Fi", desc="Intel AX201", virtual=False, status="Up"),
                Adapter(name="Ethernet", desc="Realtek", virtual=False, status="Down")]
    _patch_snapshot(monkeypatch, _snap(adapters=adapters))
    assert repairs._pick_adapter() == "Wi-Fi"


# ---------------- UI：二维码防错配 / 启动文件夹禁用态 / 性能表选中 / 复制报告 ----------------

def test_qr_ready_rejects_profile_mismatch(qtbot):
    """后台取密码期间切走下拉 → 拒绝渲染（曾生成「A 的 SSID + B 的密码」错配二维码）。"""
    import syskit.ui.wifi_page as wp

    page = wp.WifiPage()
    qtbot.addWidget(page)
    page.cmb_share.addItem("Office")
    page.cmb_share.setCurrentText("Office")
    page._qr_ready("HomeWiFi", "WPA2-PSK", "s3cret")
    assert "过期" in page.lbl_qr.text() and not page.btn_save_qr.isEnabled()

    page.cmb_share.addItem("HomeWiFi")
    page.cmb_share.setCurrentText("HomeWiFi")
    page._qr_ready("HomeWiFi", "WPA2-PSK", "s3cret")   # 匹配 → 正常渲染
    assert page.btn_save_qr.isEnabled()
    assert page.lbl_qr.pixmap() is not None and not page.lbl_qr.pixmap().isNull()


def test_startup_tab_lists_disabled_shortcut_correctly(qtbot, tmp_path, monkeypatch):
    """启动文件夹已禁用项：以原名 + 已禁用态列出（曾显示「已启用」且无法恢复）。"""
    import syskit.ui.system_page as sp

    (tmp_path / "app.lnk").write_bytes(b"")
    (tmp_path / "game.lnk.disabled").write_bytes(b"")
    monkeypatch.setattr(sp, "USER_STARTUP", str(tmp_path))

    tab = sp.StartupTab()
    qtbot.addWidget(tab)
    tab.reload()
    names = {tab.tbl.item(r, 0).text(): tab.tbl.item(r, 3).text()
             for r in range(tab.tbl.rowCount())
             if tab.tbl.item(r, 1).text() == "启动文件夹"}
    assert names == {"app.lnk": "已启用", "game.lnk": "已禁用"}

    # 禁用/启用即改名（round-trip 后缀无残留、无连锁）
    tab._enable_one("启动文件夹", "game.lnk")
    assert (tmp_path / "game.lnk").exists() and not (tmp_path / "game.lnk.disabled").exists()
    tab._disable_one("启动文件夹", "game.lnk")
    assert (tmp_path / "game.lnk.disabled").exists()


def test_perf_table_selection_follows_pid(qtbot):
    """性能表 2s 重建+重排序：选中按 PID 跟随（曾停在行号上漂移到别的进程）。"""
    from syskit.ui.system_page import PerfTab

    tab = PerfTab()
    qtbot.addWidget(tab)
    tab.update_tick({"cpu": 5, "mem": 30, "top_procs": [
        {"pid": 11, "name": "a.exe", "cpu": 9.0, "mem": 1024},
        {"pid": 22, "name": "b.exe", "cpu": 5.0, "mem": 2048}]})
    rows = {tab.tbl.item(r, 0).text(): r for r in range(tab.tbl.rowCount())}
    tab.tbl.setCurrentCell(rows["22"], 1)

    # 下一跳：a.exe 的行序变化后，选中仍应停在 pid=22 上
    tab.update_tick({"cpu": 5, "mem": 30, "top_procs": [
        {"pid": 22, "name": "b.exe", "cpu": 5.0, "mem": 2048},
        {"pid": 11, "name": "a.exe", "cpu": 9.0, "mem": 1024}]})
    assert tab.tbl.item(tab.tbl.currentRow(), 0).text() == "22"


def test_copy_report_guards_empty_and_aborted(main_win):
    """空结果不复制「外网不可达」；中止残片明示「已中止」而非调用 verdict。"""
    from syskit.netcheck import CheckResult

    win = main_win
    win._diag_results.clear()
    win._copy_report()
    assert "暂无诊断结果" in win.statusBar().currentMessage()

    win._diag_results[:] = [CheckResult("diag_aborted", "诊断中止", "skip", "用户中止")]
    win._copy_report()
    report = QApplication.clipboard().text()
    assert "诊断已中止" in report and "外网不可达" not in report
