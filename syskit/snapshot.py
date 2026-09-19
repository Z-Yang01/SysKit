"""系统网络数据采集：一次 PowerShell 调用抓全量 JSON，避免解析本地化文本。"""
import json
import re
from dataclasses import dataclass, field

from . import proc

_PS_SCRIPT = r"""
$ErrorActionPreference = 'SilentlyContinue'
$out = [ordered]@{}

$out.adapters = Get-NetAdapter | ForEach-Object {
    [ordered]@{ name=$_.Name; desc=$_.InterfaceDescription; status=[string]$_.Status;
                ifIndex=$_.ifIndex; mac=$_.MacAddress; speed=$_.LinkSpeed;
                admin=[string]$_.AdminStatus; virtual=[bool]$_.Virtual } }

# 默认路由与地址/DNS 先各取一次，下面 ipcfg 与 routes4 共用。旧口径的
# Get-NetIPConfiguration 让整脚本从 4.2s 涨到 6.8s（同机同会话实测，-39%）：它内部会
# 再解析一遍连通性。换掉后逐网卡 ipv4/gw4/dns 与默认路由和旧输出逐字段等价。
$defRoutes4 = Get-NetRoute -AddressFamily IPv4 | Where-Object { $_.DestinationPrefix -eq '0.0.0.0/0' }
$ifAddrs = Get-NetIPAddress -AddressFamily IPv4
# 不带 -AddressFamily：旧口径里未显式配 DNS 的网卡会带 IPv6 链路项（fec0:...），
# 只查 IPv4 会让这些网卡的 dns 变空，静态 IP 检查的判定对象随之漂移。
$dnsAddrs = Get-DnsClientServerAddress

$out.ipcfg = $ifAddrs | Group-Object InterfaceIndex | ForEach-Object {
    $idx = [int]$_.Name; $g = $_.Group
    [ordered]@{ ifIndex=$idx; name=$g[0].InterfaceAlias;
        ipv4=@($g | ForEach-Object { $_.IPAddress });
        gw4=@($defRoutes4 | Where-Object { $_.ifIndex -eq $idx } | ForEach-Object { $_.NextHop });
        dns=@($dnsAddrs | Where-Object { $_.InterfaceIndex -eq $idx } | ForEach-Object { $_.ServerAddresses }) } }

$out.routes4 = $defRoutes4 |
    ForEach-Object { [ordered]@{ ifIndex=$_.ifIndex; nextHop=$_.NextHop;
        routeMetric=$_.RouteMetric; ifMetric=$_.InterfaceMetric; store=[string]$_.Store } }

$out.routesPersistent = Get-NetRoute -AddressFamily IPv4 -PolicyStore PersistentStore |
    ForEach-Object { [ordered]@{ dest=$_.DestinationPrefix; nextHop=$_.NextHop;
        metric=$_.RouteMetric; ifIndex=$_.ifIndex } }

$out.connProfiles = Get-NetConnectionProfile | ForEach-Object {
    [ordered]@{ name=$_.Name; ifIndex=$_.InterfaceIndex; category=[string]$_.Category;
        v4=[string]$_.IPv4Connectivity; v6=[string]$_.IPv6Connectivity } }

$out.services = 'NlaSvc','netprofm','Dhcp','Dnscache','WlanSvc','dot3svc' | ForEach-Object {
    $s = Get-Service -Name $_
    if ($s) { [ordered]@{ name=$s.Name; status=[string]$s.Status; startType=[string]$s.StartType } } }

$out.wlanIfs = netsh wlan show interfaces 2>$null | Out-String
$out.ifMetrics = Get-NetIPInterface -AddressFamily IPv4 | ForEach-Object {
    [ordered]@{ ifIndex=$_.InterfaceIndex; name=$_.InterfaceAlias; metric=$_.InterfaceMetric } }

ConvertTo-Json -InputObject $out -Depth 4 -Compress
"""


@dataclass
class Adapter:
    name: str = ""
    desc: str = ""
    status: str = ""
    if_index: int = 0
    mac: str = ""
    speed: str = ""
    virtual: bool = False
    ipv4: list = field(default_factory=list)
    gw4: list = field(default_factory=list)
    dns: list = field(default_factory=list)
    conn_name: str = ""
    conn_v4: str = ""          # NLM 连接性判定: Internet / LocalNetwork / NoTraffic ...
    conn_category: str = ""    # Public / Private


@dataclass
class Snapshot:
    adapters: list = field(default_factory=list)
    default_routes: list = field(default_factory=list)     # 活动默认路由
    persistent_routes: list = field(default_factory=list)  # 持久路由（全部）
    conn_profiles: list = field(default_factory=list)
    services: dict = field(default_factory=dict)           # name -> {status, startType}
    wlan_text: str = ""                                    # netsh wlan show interfaces 原文
    if_metrics: dict = field(default_factory=dict)         # ifIndex -> metric
    ok: bool = False
    error: str = ""

    def physical_up_adapters(self) -> list:
        """状态 Up 且持有 IPv4 的非虚拟网卡（网关/DNS 对比检查的判定对象）。

        排除虚拟网卡：Clash TUN（Up, 198.18.0.1, fake-ip DNS）参与网关连通与
        DNS 对比会让两项核心判定的对象变成隧道假象。
        """
        return [a for a in self.adapters
                if a.status.lower() == "up" and a.ipv4 and not a.virtual]

    def service(self, name: str):
        return self.services.get(name)


def _clean(s):
    return str(s).strip() if s is not None else ""


def _parse_json_loose(text: str):
    """PowerShell 偶发在 JSON 前后带杂讯，截取第一个 { 到最后一个 }。"""
    start, end = text.find("{"), text.rfind("}")
    if start < 0 or end <= start:
        return None
    return json.loads(text[start:end + 1])


def take(use_ps: bool = True) -> Snapshot:
    snap = Snapshot()
    r = proc.run_ps(_PS_SCRIPT)
    if not r.stdout.strip():
        snap.error = r.stderr.strip() or "PowerShell 无输出"
        return snap
    try:
        data = _parse_json_loose(r.stdout)
    except Exception as e:
        snap.error = f"JSON 解析失败: {e}"
        return snap
    if not data:
        snap.error = "PowerShell 返回空数据"
        return snap

    for item in _as_list(data.get("adapters")):
        snap.adapters.append(Adapter(
            name=_clean(item.get("name")), desc=_clean(item.get("desc")),
            status=_clean(item.get("status")), if_index=int(item.get("ifIndex") or 0),
            mac=_clean(item.get("mac")), speed=_clean(item.get("speed")),
            virtual=bool(item.get("virtual")),
        ))

    ip_by_idx = {int(i.get("ifIndex") or 0): i for i in _as_list(data.get("ipcfg"))}
    prof_by_idx = {int(i.get("ifIndex") or 0): i for i in _as_list(data.get("connProfiles"))}
    for a in snap.adapters:
        ipc = ip_by_idx.get(a.if_index, {})
        a.ipv4 = [x for x in (_clean(v) for v in _as_list(ipc.get("ipv4"))) if x]
        a.gw4 = [x for x in (_clean(v) for v in _as_list(ipc.get("gw4"))) if x]
        a.dns = [x for x in (_clean(v) for v in _as_list(ipc.get("dns"))) if x]
        p = prof_by_idx.get(a.if_index)
        if p:
            a.conn_name = _clean(p.get("name"))
            a.conn_v4 = _clean(p.get("v4"))
            a.conn_category = _clean(p.get("category"))

    snap.default_routes = [dict(dest="0.0.0.0/0", nextHop=_clean(i.get("nextHop")),
                                metric=i.get("routeMetric"), if_metric=i.get("ifMetric"),
                                if_index=int(i.get("ifIndex") or 0), store=_clean(i.get("store")))
                           for i in _as_list(data.get("routes4"))]
    snap.persistent_routes = [dict(dest=_clean(i.get("dest")), nextHop=_clean(i.get("nextHop")),
                                   metric=i.get("metric"), if_index=int(i.get("ifIndex") or 0))
                              for i in _as_list(data.get("routesPersistent"))]
    snap.conn_profiles = [dict(name=_clean(i.get("name")), if_index=int(i.get("ifIndex") or 0),
                               v4=_clean(i.get("v4")), v6=_clean(i.get("v6")))
                          for i in _as_list(data.get("connProfiles"))]
    for s in _as_list(data.get("services")):
        snap.services[_clean(s.get("name"))] = {
            "status": _clean(s.get("status")), "startType": _clean(s.get("startType")),
        }
    snap.wlan_text = _clean(data.get("wlanIfs"))
    snap.if_metrics = {int(i.get("ifIndex") or 0): i.get("metric")
                       for i in _as_list(data.get("ifMetrics"))}
    snap.ok = True
    return snap


def _as_list(x):
    if x is None:
        return []
    return x if isinstance(x, list) else [x]


def current_ssid(wlan_text: str = "") -> str:
    """从 netsh wlan show interfaces 文本提取当前 SSID（中英文环境均适用）。"""
    text = wlan_text
    m = re.search(r"\bSSID\s*[:：]\s*(.+)", text)
    if m:
        ssid = m.group(1).strip()
        if ssid and ssid != '""':
            return ssid
    return ""
