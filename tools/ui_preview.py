"""离屏渲染 SysKit 各页面截图（浅色 / 深色），用于界面验收与改动前后对比。

特点：**全程 mock 所有系统调用与持久化路径**（重定向到临时目录），
不触真实网络 / 注册表 / %APPDATA%，不启真实监控线程，可安全反复运行。

用法：
    python tools/ui_preview.py [输出目录]
默认输出目录：<临时目录>/syskit_preview
"""
import os
import re
import sys
import tempfile
import types

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def _norm_out(arg: str) -> str:
    """把 Git-Bash 风格路径 /e/Code/... 归一化为 Windows 路径。

    否则 os.path.abspath("/e/Code/...") 会得到 E:\\e\\Code\\...（盘内相对目录），
    在盘根误建一棵目录树。
    """
    m = re.match(r"^/([A-Za-z])/(.*)$", arg)
    if m:
        return m.group(1).upper() + ":\\" + m.group(2).replace("/", "\\")
    return arg


OUT = os.path.abspath(_norm_out(sys.argv[1]) if len(sys.argv) > 1
                      else os.path.join(tempfile.gettempdir(), "syskit_preview"))
os.makedirs(OUT, exist_ok=True)

# ---------------- 1) 持久化路径重定向到临时目录 ----------------
TMP = tempfile.mkdtemp(prefix="syskit_preview_")
from syskit import config                      # noqa: E402

config.APPDATA_DIR = os.path.join(TMP, "appdata")
config.LOCALAPPDATA_DIR = os.path.join(TMP, "localappdata")
config.SETTINGS_FILE = os.path.join(config.APPDATA_DIR, "settings.json")
config.LOG_DIR = os.path.join(config.LOCALAPPDATA_DIR, "logs")
config.BACKUP_DIR = os.path.join(config.LOCALAPPDATA_DIR, "backup")

# ---------------- 2) 系统调用全部换成假数据 ----------------
from syskit import admin, icmp, netcheck, proc, proxy, repairs  # noqa: E402
from syskit import snapshot as snap_mod                        # noqa: E402
from syskit.netcheck import CheckResult                        # noqa: E402
from syskit.proc import CmdResult                              # noqa: E402
from syskit.snapshot import Adapter, Snapshot                  # noqa: E402

admin.is_admin = lambda: True          # 不显示「未以管理员运行」横幅

_FAKE_CMDS = {
    "tracert": "  1     1 ms     2 ms     1 ms  192.168.1.1\n"
               "  2    12 ms    11 ms    13 ms  10.0.0.1\n"
               "  3    28 ms    27 ms    29 ms  223.5.5.5\n",
    "arp": "  192.168.1.1          3c-84-6a-11-22-33     dynamic\n"
           "  192.168.1.12         a4-5e-60-ab-cd-ef     dynamic\n",
    "wlan show interfaces": "    名称                   : WLAN\n"
                            "    状态                   : 已连接\n"
                            "    SSID                   : HomeWiFi\n"
                            "    信号                   : 82%\n",
    "wlan show profiles": "所有用户配置文件 : HomeWiFi\n所有用户配置文件 : Office-5G\n",
}


def _fake_run(cmd, timeout=20):
    key = " ".join(cmd) if isinstance(cmd, list) else str(cmd)
    for pat, out in _FAKE_CMDS.items():
        if pat in key:
            return CmdResult(0, out, "")
    return CmdResult(0, "", "")


proc.run = _fake_run
proc.run_ps = lambda script, timeout=40: CmdResult(0, "", "")

_APS = [
    {"ssid": "HomeWiFi", "bssid": "3c:84:6a:11:22:33", "signal": 82, "channel": 6,
     "radio": "802.11n", "auth": "WPA2-Personal", "band": "2.4G"},
    {"ssid": "HomeWiFi-5G", "bssid": "3c:84:6a:11:22:34", "signal": 74, "channel": 44,
     "radio": "802.11ac", "auth": "WPA2-Personal", "band": "5G"},
    {"ssid": "CMCC-8F2A", "bssid": "a0:4e:cf:8f:2a:10", "signal": 41, "channel": 11,
     "radio": "802.11n", "auth": "WPA2-Personal", "band": "2.4G"},
    {"ssid": "TP-LINK_2.4G", "bssid": "e8:de:27:00:11:22", "signal": 30, "channel": 1,
     "radio": "802.11n", "auth": "WPA2-Personal", "band": "2.4G"},
]


def _fake_snapshot():
    s = Snapshot(ok=True)
    s.adapters = [
        Adapter(name="WLAN", desc="Intel(R) Wi-Fi 6 AX201 160MHz", status="Up", if_index=12,
                mac="3C-84-6A-11-22-33", speed="866 Mbps",
                ipv4=["192.168.1.20"], gw4=["192.168.1.1"], dns=["223.5.5.5", "223.6.6.6"],
                conn_name="HomeWiFi", conn_v4="Internet", conn_category="Private"),
        Adapter(name="以太网", desc="Realtek PCIe GbE Family Controller", status="Disconnected",
                if_index=7, mac="F0-2F-74-AA-BB-CC", conn_v4="NoTraffic"),
        Adapter(name="vEthernet (Default Switch)", desc="Hyper-V Virtual Ethernet Adapter",
                status="Up", if_index=22, ipv4=["172.24.96.1"], virtual=True,
                conn_v4="LocalNetwork"),
    ]
    s.default_routes = [{"dest": "0.0.0.0/0", "nextHop": "192.168.1.1", "metric": 0,
                         "if_metric": 45, "if_index": 12, "store": "ActiveStore"}]
    s.persistent_routes = []
    s.services = {"NlaSvc": {"status": "Running", "startType": "Manual"},
                  "netprofm": {"status": "Running", "startType": "Manual"},
                  "Dhcp": {"status": "Running", "startType": "Auto"},
                  "Dnscache": {"status": "Running", "startType": "Auto"},
                  "WlanSvc": {"status": "Running", "startType": "Auto"}}
    s.wlan_text = _FAKE_CMDS["wlan show interfaces"]
    s.if_metrics = {12: 45, 7: 25, 22: 5}
    return s


snap_mod.take = lambda use_ps=True: _fake_snapshot()

_DIAG = [
    CheckResult("real_conn", "真实连通性（204 端点 + HTTPS）", "pass",
                "204探测:OK(http://connect.rom.miui.com/generate_204) HTTPS(https://www.baidu.com):200"),
    CheckResult("ncsi_probe", "NCSI 探测模拟（Windows 判定'有网'的依据）", "pass",
                "Web探测: HTTP 200; DNS(dns.msftncsi.com): OK"),
    CheckResult("nlasvc", "NlaSvc 网络位置感知服务（画'有网/无网'标记的服务）", "warn",
                "运行中; 启动类型=Manual; 触发器=无；本机被改为手动启动且无触发器（非原版配置，重启后仍不会自启）",
                fix_ids=["fix_nlasvc"]),
    CheckResult("proxy", "系统代理（代理软件残留会让全网流量进死端口）", "warn",
                "系统代理已开启: 127.0.0.1:7890 —— 若代理软件未运行，所有应用将无法上网",
                fix_ids=["proxy_off"]),
    CheckResult("static_dns", "网卡静态 DNS 审计（残留的静态 DNS 只影响对应网卡）", "info",
                "WLAN: 静态 223.5.5.5,223.6.6.6"),
    CheckResult("gateway", "网关连通（Ping 默认网关 ×3）", "pass",
                "WLAN 网关 192.168.1.1: 丢包 0%"),
    CheckResult("dns_compare", "DNS 解析（系统 DNS vs 公共 DNS 对比）", "pass",
                "系统 DNS(223.5.5.5) 解析正常"),
    CheckResult("routes", "路由健康（幽灵路由 / 多默认网关 / 隧道抢占）", "pass",
                "活动默认路由: 192.168.1.1(if12, 跃点45)"),
    CheckResult("power_mgmt", "网卡节电（允许系统关闭设备以节约电源）", "warn",
                "以太网(Realtek PCIe GbE Family Co): 允许断电 —— 无线/USB 网卡被允许休眠省电，是 WiFi/USB 共享频繁掉线的常见原因",
                fix_ids=["power_off"]),
]
netcheck.run_all = lambda progress=None, quick=False: list(_DIAG)
netcheck.verdict = lambda results: ("config_issues", "存在配置隐患；失败项: 1 个")
icmp.ping = lambda host, timeout_ms=1500: 12.0
icmp.default_gateway = lambda: "192.168.1.1"
proxy.get_proxy_status = lambda: {"enabled": True, "server": "127.0.0.1:7890",
                                  "autoconfig": False, "pac_url": ""}
repairs.wlan_profiles = lambda: ["HomeWiFi", "Office-5G"]
repairs.mac_random_state = lambda profile: False
os.startfile = lambda *a, **k: None

# ---------------- 3) 假 psutil ----------------
import psutil                                                  # noqa: E402

_NS = types.SimpleNamespace
psutil.net_io_counters = lambda pernic=False: {
    "WLAN": _NS(bytes_recv=812_345_678, bytes_sent=98_765_432)} if pernic else \
    _NS(bytes_recv=812_345_678, bytes_sent=98_765_432)
psutil.cpu_percent = lambda interval=None: 23.4
psutil.virtual_memory = lambda: _NS(percent=58.2, total=16 * 2 ** 30,
                                    available=6 * 2 ** 30)
psutil.disk_io_counters = lambda perdisk=False: _NS(read_bytes=1_048_576,
                                                   write_bytes=524_288)
psutil.process_iter = lambda attrs=None: iter(())
psutil.net_if_addrs = lambda: {}
psutil.net_connections = lambda kind="inet": [
    _NS(pid=4321, laddr=_NS(ip="0.0.0.0", port=7890),
        raddr=(), status="LISTEN", type=1),
    _NS(pid=1234, laddr=_NS(ip="0.0.0.0", port=135),
        raddr=(), status="LISTEN", type=1),
    _NS(pid=9876, laddr=_NS(ip="192.168.1.20", port=52341),
        raddr=_NS(ip="223.5.5.5", port=443), status="ESTABLISHED", type=1),
]
psutil.disk_partitions = lambda all=False: [
    _NS(device="C:\\", mountpoint="C:\\", fstype="NTFS", opts="rw,fixed"),
    _NS(device="D:\\", mountpoint="D:\\", fstype="NTFS", opts="rw,fixed")]
psutil.disk_usage = lambda path: _NS(total=500 * 2 ** 30, used=329 * 2 ** 30,
                                     free=171 * 2 ** 30, percent=65.8)


class _FakeProc:
    def __init__(self, pid, name):
        self.pid, self._name = pid, name
    def name(self):
        return self._name
    def exe(self):
        return ""
    def cpu_percent(self, interval=None):
        return 0.0
    def memory_info(self):
        return _NS(rss=1)


psutil.Process = lambda pid=0: _FakeProc(pid or 1, "svchost.exe")

# ---------------- 4) 阻止真实后台线程 ----------------
from PySide6.QtCore import QObject, Signal, QTimer              # noqa: E402
from PySide6.QtWidgets import QApplication, QMessageBox         # noqa: E402

from syskit import monitor as monitor_mod                        # noqa: E402
from syskit.ui import main_window as mw_mod                      # noqa: E402


class _FakeMonitor:
    def __init__(self, **kw):
        self.last_results = []
    def start(self):
        pass
    def stop(self):
        pass
    def join(self, timeout=None):
        pass
    def force_check(self):
        pass


class _FakeTelemetry(QObject):
    tick = Signal(dict)

    def __init__(self, *a, **k):
        super().__init__()
        # 假流量存储：days() 供网络质量日报聚合（预置近几天含流量、今日为 0）
        def _add(sent, recv):
            pass

        def _days():
            import time as _t
            out = {}
            for off in range(6, -1, -1):
                key = _t.strftime("%Y-%m-%d", _t.localtime(_t.time() - off * 86400))
                out[key] = {"sent": (7 - off) * 41_000_000,
                            "recv": (7 - off) * 380_000_000}
            return out

        self.traffic = types.SimpleNamespace(save_if_dirty=lambda: None,
                                             today=lambda: (0, 0),
                                             add=_add, days=_days)
    def start(self):
        pass
    def stop(self):
        pass
    def wait(self, timeout=None):
        return True


monitor_mod.Monitor = _FakeMonitor
mw_mod.TelemetryWorker = _FakeTelemetry

for _name in ("information", "warning", "question", "critical"):
    setattr(QMessageBox, _name, staticmethod(lambda *a, **k: QMessageBox.Ok))
QMessageBox.about = staticmethod(lambda *a, **k: None)

# ---------------- 5) 启动窗口并灌入假数据 ----------------
from syskit.diskusage import DirNode, ScanResult  # noqa: E402
from syskit.ui import theme                       # noqa: E402
from syskit.ui.main_window import MainWindow      # noqa: E402
from syskit.ui.system_page import (ClipboardTab, DiskTab, ModesTab,  # noqa: E402
                                   StartupTab)

app = QApplication(sys.argv)
app.setStyle("Fusion")
theme.apply(app, dark=False)
win = MainWindow()
win.resize(1180, 780)
win.show()
app.processEvents()


def _pump(ms=60):
    end = int(ms / 10)
    for _ in range(max(1, end)):
        app.processEvents()
        QTimer.singleShot(0, lambda: None)


def seed():
    # 概览页
    win._diag_done(list(_DIAG), True)
    win._fill_adapters(snap_mod.take())
    win.lbl_probe.setText("连通性探测：" + _DIAG[0].detail)
    win._refresh_proxy_label()

    # 网络监控页
    for i in range(40):
        win._on_telemetry({"down": (i % 9) * 380_000, "up": (i % 5) * 90_000,
                           "cpu": 20 + (i % 7), "mem": 55 + (i % 5),
                           "disk_r": (i % 6) * 512_000, "disk_w": (i % 4) * 256_000,
                           "lat_gw": 6 + (i % 4), "lat_pub": 24 + (i % 9),
                           "gateway": "192.168.1.1",
                           "wifi_signal": 78 + (i % 6),
                           "today_sent": 98_765_432, "today_recv": 812_345_678,
                           "top_procs": [{"pid": 1234, "name": "chrome.exe", "cpu": 18.2,
                                          "mem": 1_248_576_000},
                                         {"pid": 4321, "name": "Code.exe", "cpu": 12.7,
                                          "mem": 862_978_048},
                                         {"pid": 9876, "name": "explorer.exe", "cpu": 4.1,
                                          "mem": 214_958_080}]})
    for st, msg in (("no_net", "连通性探测失败 ×2"), ("ok", ""),
                    ("offline", "无网络连接")):
        win.eventstore.add(st, msg)
    win.page_dash.reload_events()

    # WiFi 页
    wifi = win.stack.widget(2)
    wifi._scan_done(list(_APS))

    # 系统助手：启动项 / 磁盘 / 剪贴板 / 场景模式
    startup = win.page_system.findChild(StartupTab)
    startup._enum = staticmethod(lambda: [
        ("HKCU-Run", "OneDrive", r"C:\Users\me\AppData\Local\Microsoft\OneDrive\OneDrive.exe /background", True),
        ("HKCU-Run", "WeChat", r"C:\Program Files\Tencent\WeChat\WeChat.exe", True),
        ("HKLM-Run", "SecurityHealth", r"%windir%\system32\SecurityHealthSystray.exe", True),
        ("启动文件夹", "Docker Desktop.lnk", r"C:\Users\me\...\Docker Desktop.lnk", True),
        ("HKCU-Run", "Steam", r"C:\Program Files (x86)\Steam\steam.exe -silent", False),
    ])
    startup.reload()

    disk = win.page_system.findChild(DiskTab)
    root = DirNode("C:\\", "C:\\", size=1 << 30, total=329 << 30,
                   file_count=421_337, error_count=12)
    for name, total in (("Windows", 42 << 30), ("Users", 128 << 30),
                        ("Program Files", 61 << 30), ("ProgramData", 24 << 30),
                        ("Games", 74 << 30)):
        child = DirNode(name, f"C:\\{name}", total=total, file_count=total >> 18)
        root.children.append(child)
    for name, total in (("下载", 38 << 30), ("视频", 22 << 30),
                        ("AppData", 46 << 30), ("文档", 14 << 30)):
        grand = DirNode(name, f"C:\\Users\\me\\{name}", total=total, file_count=total >> 19)
        root.children[1].children.append(grand)
    res = ScanResult(root=root,
                     ext_stats={"mp4": 46 << 30, "zip": 21 << 30, "exe": 18 << 30,
                                "iso": 12 << 30, "dll": 9 << 30, "png": 4 << 30,
                                "无后缀": 2 << 30, "log": 1 << 30},
                     top_files=[(9 << 30, r"C:\Games\big-title.iso", 1757000000),
                                (6 << 30, r"C:\Users\me\下载\win11.iso", 1756000000),
                                (4 << 30, r"C:\Users\me\视频\record.mp4", 1755000000),
                                (3 << 30, r"C:\ProgramData\cache.zip", 1754000000)],
                     file_count=421_337, error_count=12, duration=38.4)
    disk._scan_done(res)

    clip = win.page_system.findChild(ClipboardTab)
    for text in ("https://www.deepseek.com/",
                 "git commit -m \"fix(ui): 窗口状态持久化\"",
                 "npm run build && npm run preview"):
        clip.hist.add(text)
    clip.reload()

    modes = win.page_system.findChild(ModesTab)
    modes._show_steps([{"name": "通知/勿扰", "ok": True, "detail": "已关闭（勿扰）"},
                       {"name": "电源计划", "ok": True, "detail": "高性能"},
                       {"name": "暂停 Windows 更新", "ok": False,
                        "detail": "需要管理员权限"},
                       {"name": "清理后台", "ok": True, "detail": "已结束 2 个：OneDrive.exe、Steam.exe"}],
                      "进入游戏模式")

    # 密码库
    from syskit import vault as vault_mod
    vp = win.page_vault
    gh = vault_mod.add_site("GitHub", domain="github.com", aliases="gh", tags="代码")
    aws = vault_mod.add_site("AWS 控制台", domain="aws.amazon.com", tags="云")
    vault_mod.add_account(gh, "pw-demo", vp.crypto, username="alice", label="工作",
                          totp_secret="JBSWY3DPEHPK3PXP")
    vault_mod.add_account(gh, "pw-demo2", vp.crypto, username="bob")
    vault_mod.add_account(aws, "pw-demo3", vp.crypto, username="root", label="管理员")
    vp.refresh_sites()
    for i in range(vp.lst_sites.count()):           # 选中 GitHub 展示账号列表
        if "GitHub" in vp.lst_sites.item(i).text():
            vp.lst_sites.setCurrentRow(i)
            break
    vp.refresh_accounts()

    # 工具箱 · Hosts 编辑
    from syskit import hostsed as hostsed_mod
    hosts_tab = win.page_toolbox.tabs.widget(win.page_toolbox.tabs.count() - 1)
    hosts_tab._entries = hostsed_mod.parse(
        "127.0.0.1 localhost\n"
        "# 10.0.0.5 blocked.example.com\n"
        "10.0.0.6 db.local    # 数据库\n"
        "192.168.1.9 nas.home\n")
    hosts_tab._reload_table()


seed()
_pump(200)


def shoot(name):
    path = os.path.join(OUT, name)
    win.grab().save(path)
    print("saved", path)


PAGES = ["概览", "网络监控", "WiFi工具", "诊断", "修复", "工具箱", "系统助手", "密码库", "设置", "日志"]
for i, label in enumerate(PAGES):
    win.switch_page(i)
    _pump(140)
    shoot(f"01_light_{i}_{label}.png")

# 系统助手子页：磁盘空间 / 场景模式
from PySide6.QtWidgets import QTabWidget  # noqa: E402

sys_tabs = win.page_system.findChild(QTabWidget)
for idx, label in ((4, "磁盘空间"), (6, "场景模式")):
    sys_tabs.setCurrentIndex(idx)
    win.switch_page(6)
    _pump(140)
    shoot(f"02_light_system_{label}.png")

# 深色主题（与应用 _save_settings 同路径：apply + 固化色重填）
theme.apply(app, dark=True)
win._on_theme_changed()
win.switch_page(0)
_pump(140)
shoot("03_dark_0_概览.png")
win.switch_page(1)
_pump(140)
shoot("04_dark_1_网络监控.png")
theme.apply(app, dark=False)
win._on_theme_changed()

print("OUT=" + OUT)
