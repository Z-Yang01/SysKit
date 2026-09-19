"""13 项网络诊断。每项返回 CheckResult(status/detail/fix_ids)。"""
import ipaddress
import re
import urllib.request
import urllib.error

from . import config, dnsquery, proc, winregread
from .snapshot import Snapshot, current_ssid

PASS, WARN, FAIL, SKIP, INFO = "pass", "warn", "fail", "skip", "info"

STATUS_MARK = {PASS: "√", WARN: "!", FAIL: "×", SKIP: "-", INFO: "·"}


class CheckResult:
    def __init__(self, cid: str, name: str, status: str, detail: str = "", fix_ids=None):
        self.id = cid
        self.name = name
        self.status = status
        self.detail = detail
        self.fix_ids = fix_ids or []

    def line(self) -> str:
        return f"[{STATUS_MARK.get(self.status, '?')}] {self.name}\n      {self.detail}"


# ---------------- HTTP 探测 ----------------

# 直连 opener：urlopen 默认走系统代理——被检测对象（残留代理）会劫持全部
# 探测，把「代理挂了」误判成「断网」，进而驱动重启网卡等无效修复
_OPENER_DIRECT = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def _http_get(url: str, timeout: float = 6.0, need_body: bool = False):
    req = urllib.request.Request(url, headers={"User-Agent": "SysKit/1.0"})
    try:
        with _OPENER_DIRECT.open(req, timeout=timeout) as resp:
            body = resp.read(512).decode("utf-8", errors="replace") if need_body else ""
            return resp.status, body, None
    except urllib.error.HTTPError as e:
        return e.code, "", None
    except Exception as e:
        return None, "", str(e)


def probe_204() -> tuple:
    """任一 204 端点可达即算通。返回 (ok, detail)。"""
    for url in config.PROBE_204_URLS:
        code, _, err = _http_get(url)
        if code == 204:
            return True, url
    return False, err or "all failed"


def probe_real_internet() -> tuple:
    """(真实连通 bool, 详情 str)"""
    ok204, d1 = probe_204()
    code, _, err = _http_get(config.PROBE_HTTPS_URL)
    https_ok = code in (200, 302)
    detail = f"204探测:{'OK' if ok204 else 'FAIL'}({d1}) HTTPS({config.PROBE_HTTPS_URL}):{code if code else 'FAIL'}"
    return (ok204 or https_ok), detail


def probe_ncsi() -> tuple:
    """模拟 Windows NCSI 主动探测。返回 (bool, detail)"""
    code, body, err = _http_get(config.NCSI_WEB_URL, need_body=True)
    web_ok = code == 200 and config.NCSI_EXPECT_CONTENT in body
    web_detail = f"HTTP {code}" + ("" if web_ok else f" 内容不符({(body or '')[:40]!r})" if code else f" {err}")
    ips = dnsquery.resolve_system(config.NCSI_DNS_HOST)
    dns_ok = config.NCSI_DNS_EXPECT in ips
    detail = f"Web探测: {web_detail}; DNS({config.NCSI_DNS_HOST}): {'OK' if dns_ok else 'FAIL->' + (','.join(ips) or '超时')}"
    return (web_ok and dns_ok), detail


# ---------------- 各项检查 ----------------

def _v4_dns(dns_list: list) -> str:
    """取第一个可用 IPv4 DNS，过滤 Windows 占位符 fec0::/10。"""
    for d in dns_list:
        if ":" not in d and not d.lower().startswith("fec"):
            return d
    return ""


def chk_nlasvc(snap: Snapshot) -> CheckResult:
    r = CheckResult("nlasvc", "NlaSvc 网络位置感知服务（画'有网/无网'标记的服务）", PASS)
    svc = snap.service("NlaSvc")
    if not svc:
        r.status, r.detail, r.fix_ids = FAIL, "服务不存在", ["fix_nlasvc"]
        return r
    running = svc["status"] == "Running"
    start_type = svc["startType"]
    trigger_info = winregread.subkeys(r"HKLM\SYSTEM\CurrentControlSet\Services\NlaSvc\TriggerInfo")
    if running:
        r.detail = f"运行中; 启动类型={start_type}; 触发器={'有' if trigger_info else '无'}"
        if start_type in ("Manual",) and not trigger_info:
            # 三目标解包（曾写成二元组 → ValueError，被 run_all 兜底降级为
            # 原始异常文本、丢失 fix_nlasvc 建议——恰是本工具的核心目标场景）
            r.status, r.detail, r.fix_ids = WARN, r.detail + "；本机被改为手动启动且无触发器（非原版配置，重启后仍不会自启）", ["fix_nlasvc"]
        return r
    # stopped
    if start_type in ("Disabled",):
        r.status, r.detail, r.fix_ids = FAIL, "已禁用 —— NCSI 探测无人执行，必然误报'无网络'", ["fix_nlasvc"]
    elif start_type in ("Manual",) and not trigger_info:
        r.status, r.detail, r.fix_ids = FAIL, (
            "已停止，且启动类型=手动 + 无任何触发器 —— 无人拉起它，自开机起从未运行。"
            "这是'已连接却显示无网络'的直接原因之一"), ["fix_nlasvc"]
    else:
        r.status, r.detail, r.fix_ids = FAIL, f"已停止(启动类型={start_type})", ["start_nlasvc"]
    return r


def chk_persistent_route(snap: Snapshot) -> CheckResult:
    r = CheckResult("persistent_route", "持久路由（重启不消失的手工路由，休眠时会劫持流量）", PASS)
    if not snap.persistent_routes:
        r.detail = "无持久路由"
        return r
    defaults = [p for p in snap.persistent_routes if p["dest"] == "0.0.0.0/0"]
    others = [p for p in snap.persistent_routes if p["dest"] != "0.0.0.0/0"]
    if defaults:
        gws = ", ".join(f"{d['nextHop']}(跃点{d['metric']})" for d in defaults)
        r.status = FAIL
        r.detail = f"存在持久默认路由 → {gws}。连上同网段 WiFi 时它会以更低跃点抢占全部流量，若该网关不可达则流量黑洞（热点/USB 不受影响）"
        r.fix_ids = ["del_persistent_route"]
    else:
        # 走到这里 others 必非空（空表已在函数开头 return），故只需一个分支
        r.status = WARN
        r.detail = "存在非默认持久路由: " + ", ".join(f"{p['dest']}→{p['nextHop']}" for p in others[:4])
    return r


def chk_static_dns(snap: Snapshot) -> CheckResult:
    r = CheckResult("static_dns", "网卡静态 DNS 审计（残留的静态 DNS 只影响对应网卡）", PASS)
    problems, notes = [], []
    for a in snap.adapters:
        if not a.dns or a.virtual or "WAN Miniport" in a.desc:
            continue
        v4 = _v4_dns(a.dns)
        if not v4:
            continue
        static, _src = _dns_is_static(a.name)
        foreign = [d for d in a.dns if d in config.FOREIGN_DNS_BLACKLIST]
        tag = "" if a.status.lower() == "up" else "(未连接)"
        if foreign:
            problems.append(f"{a.name}{tag}: {','.join(a.dns)}（{'静态' if static else 'DHCP'}，含国内不可达的 {','.join(foreign)}）")
            r.fix_ids = ["dns_auto", "dns_preset"]
        elif static:
            notes.append(f"{a.name}{tag}: 静态 {','.join(a.dns)}")
    if problems:
        r.status = WARN
        r.detail = "；".join(problems + notes)
    elif notes:
        r.status = INFO
        r.detail = "；".join(notes)
    else:
        r.detail = "各网卡 DNS 均为 DHCP 自动获取，无国外黑名单 DNS"
    return r


def _dns_is_static(adapter_name: str) -> tuple:
    """(是否静态, 来源描述)。解析 netsh 输出的中英文标记。"""
    r = proc.run(["netsh", "interface", "ipv4", "show", "dnsservers", adapter_name], timeout=10)
    text = r.stdout
    if re.search(r"静态配置|Statically configured", text, re.I):
        return True, "static"
    if re.search(r"通过 DHCP 配置|DHCP configured|Initiated by Dhcp", text, re.I):
        return False, "dhcp"
    return (False, "unknown")


def chk_gateway(snap: Snapshot) -> CheckResult:
    r = CheckResult("gateway", "网关连通（Ping 默认网关 ×3）", SKIP)
    ups = snap.physical_up_adapters()
    if not ups:
        r.detail = "无已连接的物理网卡，跳过"
        return r
    lines, any_ok, any_fail = [], False, False
    for a in ups:
        for gw in a.gw4[:1]:
            p = proc.run(["ping", "-n", "3", "-w", "1000", gw], timeout=12)
            loss = _ping_loss(p.stdout)
            ok = loss is not None and loss < 100
            any_ok |= ok
            any_fail |= not ok
            lines.append(f"{a.name} 网关 {gw}: 丢包 {loss if loss is not None else '??'}%")
    if not lines:
        r.detail = "已连接网卡均无默认网关"
        r.status = WARN
        return r
    r.detail = "；".join(lines)
    r.status = FAIL if any_fail and not any_ok else (WARN if any_fail else PASS)
    return r


def _ping_loss(ping_out: str) -> int:
    m = re.search(r"(\d+)%", ping_out)
    return int(m.group(1)) if m else None


def chk_dns_compare(snap: Snapshot) -> CheckResult:
    r = CheckResult("dns_compare", "DNS 解析（系统 DNS vs 公共 DNS 对比）", SKIP)
    ups = snap.physical_up_adapters()
    sys_dns = next((_v4_dns(a.dns) for a in ups if _v4_dns(a.dns)), None)
    if not sys_dns:
        r.detail = "无系统 DNS 可测"
        return r
    sys_ips = dnsquery.query_safe("www.baidu.com", sys_dns, 3.0)
    ref_ips = dnsquery.query_safe("www.baidu.com", config.PUBLIC_DNS_TEST[0], 3.0)
    if not ref_ips:
        r.status, r.detail = WARN, f"公共 DNS({config.PUBLIC_DNS_TEST[0]}) 也不可达，无法建立参照"
        return r
    if sys_ips and set(sys_ips) & set(ref_ips):
        r.status, r.detail = PASS, f"系统 DNS({sys_dns}) 解析正常"
        return r
    if not sys_ips:
        r.status, r.detail = FAIL, f"系统 DNS({sys_dns}) 解析 www.baidu.com 超时/失败；公共 DNS 正常 → DNS 问题，建议改用公共 DNS"
        r.fix_ids = ["dns_auto", "dns_preset"]
    else:
        r.status, r.detail = WARN, f"系统 DNS({sys_dns}) 与公共 DNS 结果不一致: {sys_ips[:2]} vs {ref_ips[:2]}"
    return r


def chk_dns_pollution(snap: Snapshot) -> CheckResult:
    r = CheckResult("dns_pollution", "NCSI 域名解析对比（污染检测）", INFO)
    sys_ips = dnsquery.resolve_system("www.msftconnecttest.com")
    ref = dnsquery.query_safe("www.msftconnecttest.com", config.PUBLIC_DNS_TEST[0], 3.0)
    if not ref:
        r.detail = "参照 DNS 不可达"
        return r
    if not sys_ips:
        r.status, r.detail = FAIL, f"系统解析 www.msftconnecttest.com 失败（公共 DNS 可解析 {ref[:2]}）→ 会直接导致 NCSI 探测失败"
        r.fix_ids = ["dns_auto", "dns_preset"]
    elif _bogus(sys_ips):
        # 污染特征判定必须先于交集 PASS：系统应答可同时含污染地址和与公共
        # DNS 重叠的真实地址，先交集会把污染掩盖成「解析一致」
        r.status, r.detail = WARN, f"系统结果 {sys_ips[:2]} 是无效地址（污染特征），公共 DNS 为 {ref[:2]}"
    elif set(sys_ips) & set(ref):
        r.status, r.detail = PASS, f"解析一致: {sys_ips[:2]}"
    else:
        r.status, r.detail = INFO, f"结果不同但均为有效公网地址（CDN 分流，非污染）: {sys_ips[:2]} vs {ref[:2]}"
    return r


def _bogus(ips: list) -> bool:
    for ip in ips:
        try:
            addr = ipaddress.ip_address(ip)
        except ValueError:
            return True
        if addr.is_private or addr.is_reserved or addr.is_loopback or addr.is_unspecified or addr.is_link_local:
            return True
    return False


def chk_ncsi_probe(snap: Snapshot) -> CheckResult:
    ok, detail = probe_ncsi()
    r = CheckResult("ncsi_probe", "NCSI 探测模拟（Windows 判定'有网'的依据）", PASS if ok else FAIL, detail)
    if not ok:
        r.fix_ids = ["dns_auto", "dns_preset", "ncsi_restore"]
    return r


def chk_real(snap: Snapshot) -> CheckResult:
    ok, detail = probe_real_internet()
    r = CheckResult("real_conn", "真实连通性（204 端点 + HTTPS）", PASS if ok else FAIL, detail)
    return r


def chk_ncsi_registry(snap: Snapshot) -> CheckResult:
    r = CheckResult("ncsi_reg", "NCSI 注册表配置核对", PASS)
    vals = winregread.values(rf"HKLM\{config.NCSI_REG_PATH}")
    if not vals:
        r.status, r.detail = FAIL, "无法读取 NCSI 注册表（需管理员？）"
        return r
    issues = []
    probing = vals.get("EnableActiveProbing")
    if probing == 0:
        issues.append("EnableActiveProbing=0（主动探测被关闭）")
    for k, want in config.NCSI_STOCK.items():
        got = vals.get(k)
        if k == "EnableActiveProbing":
            continue
        if got is None:
            issues.append(f"{k} 缺失")
        elif str(got) != str(want):
            issues.append(f"{k}={got}(默认{want})")
    policy = winregread.values(rf"HKLM\{config.NCSI_POLICY_KEY}")
    if policy.get("NoActiveProbe") == 1:
        issues.append("组策略 NoActiveProbe=1")
    if issues:
        r.status, r.detail, r.fix_ids = WARN, "；".join(issues), ["ncsi_restore"]
    else:
        r.detail = f"与原版默认一致（EnableActiveProbing={probing}, 探测 {vals.get('ActiveWebProbeHost')}/{vals.get('ActiveWebProbePath')}）"
    return r


def chk_power(snap: Snapshot) -> CheckResult:
    r = CheckResult("power_mgmt", "网卡节电（允许系统关闭设备以节约电源）", INFO)
    items = []
    for a in snap.adapters:
        if a.virtual or a.desc == "":
            continue
        state = winregread.pnpcapabilities(a.desc)
        if state == "enabled_off":
            items.append(f"{a.name}({a.desc[:24]}): 允许断电")
        elif state is None:
            continue
        else:
            items.append(f"{a.name}({a.desc[:24]}): 已禁止断电")
    caps = winregread.subkeys(rf"HKLM\{config.NIC_CLASS_KEY}")
    if not caps and not items:
        r.status, r.detail = SKIP, "无法读取网卡注册表"
        return r
    risky = [i for i in items if "允许断电" in i]
    if risky:
        r.status, r.detail, r.fix_ids = WARN, "；".join(items) + " —— 无线/USB 网卡被允许休眠省电，是 WiFi/USB 共享频繁掉线的常见原因", ["power_off"]
    else:
        r.status, r.detail = PASS, "；".join(items) if items else "未发现可断电网卡"
    return r


def chk_services(snap: Snapshot) -> CheckResult:
    r = CheckResult("services", "关键网络服务状态", PASS)
    lines, bad = [], []
    for name in ("Dhcp", "Dnscache", "netprofm", "WlanSvc"):
        s = snap.service(name)
        if not s:
            continue
        ok = s["status"] == "Running"
        lines.append(f"{name}={s['status']}/{s['startType']}")
        if not ok and name not in ("WlanSvc",):  # WlanSvc 停止在无 WiFi 使用场景可接受
            bad.append(name)
    r.detail = "；".join(lines) or "无数据"
    if bad:
        r.status, r.fix_ids = WARN, ["restart_services"]
    return r


# 与 vpn.should_skip_auto_repair 共用同一份隧道识别（vpn 版覆盖更多客户端）：
# 两套正则曾分叉——同一 TUN 网卡在护栏处被跳过、在 chk_proxy 处不被认为是
# 隧道，反而对 TUN 客户端误报「代理残留」并建议 proxy_off
from .vpn import TUNNEL_PAT


def chk_proxy(snap: Snapshot) -> CheckResult:
    """系统代理检测：代理软件残留是'已连接但无网络'的经典元凶。"""
    from . import proxy
    r = CheckResult("proxy", "系统代理（代理软件残留会让全网流量进死端口）", PASS)
    st = proxy.get_proxy_status()
    tunnels_up = [a for a in snap.adapters
                  if a.status.lower() == "up" and TUNNEL_PAT.search(a.desc or "")]
    if st["enabled"] and st["server"]:
        if tunnels_up:
            # VPN/Clash 在线时系统代理多为预期，降为 INFO 且不建议 proxy_off
            r.status, r.fix_ids = INFO, []
            r.detail = (f"系统代理已开启: {st['server']}；同时检测到隧道网卡在线"
                        f"（{tunnels_up[0].name}）—— 多半是客户端正常模式，勿盲目关闭")
        else:
            r.status, r.fix_ids = WARN, ["proxy_off"]
            r.detail = f"系统代理已开启: {st['server']} —— 若代理软件未运行，所有应用将无法上网"
    elif st["autoconfig"] and st["pac_url"]:
        r.status, r.fix_ids = WARN, ["proxy_off"]
        r.detail = f"配置了 PAC 自动代理脚本: {st['pac_url'][:60]}"
    else:
        r.detail = "未启用系统代理"
    return r


def chk_routes(snap: Snapshot) -> CheckResult:
    r = CheckResult("routes", "路由健康（幽灵路由 / 多默认网关 / 隧道抢占）", PASS)
    idx_status = {a.if_index: a.status.lower() for a in snap.adapters}
    drs = snap.default_routes
    phantom = [d for d in drs if idx_status.get(d["if_index"]) != "up"]
    active = [d for d in drs if idx_status.get(d["if_index"]) == "up"]

    def fmt(d):
        return f"{d['nextHop']}(if{d['if_index']}, 跃点{(d['metric'] or 0) + (d['if_metric'] or 0)})"

    lines = [f"活动默认路由: {fmt(d)}" for d in active]
    if phantom:
        persist_gw = {p["nextHop"] for p in snap.persistent_routes if p["dest"] == "0.0.0.0/0"}
        r.status = FAIL
        r.detail = (f"幽灵默认路由（接口已断开却占据活动路由表）: {'; '.join(fmt(d) for d in phantom)}"
                    + ("；来自持久路由 —— 连上同网段网络时会劫持全部流量，且已扰乱 DNS 解析排序" if any(d['nextHop'] in persist_gw for d in phantom) else ""))
        r.fix_ids = ["del_persistent_route"]
        return r
    if len(active) > 1:
        r.status = WARN
        r.detail = "；".join(lines) + " —— 存在多个默认网关，流量出口可能漂移"
        return r
    tunnels = [a for a in snap.adapters if a.status.lower() == "up" and TUNNEL_PAT.search(a.desc)]
    tun_with_gw = {d["if_index"] for d in snap.default_routes if idx_status.get(d["if_index"]) == "up"}
    low_metric_tun = []
    for t in tunnels:
        if t.if_index not in tun_with_gw:  # 无默认路由的隧道不参与出口竞争
            continue
        m = snap.if_metrics.get(t.if_index)
        phys = [snap.if_metrics.get(a.if_index, 9999) for a in snap.adapters
                if a.status.lower() == "up" and not TUNNEL_PAT.search(a.desc) and not a.virtual]
        if m is not None and phys and m <= min(phys):
            low_metric_tun.append(f"{t.name}({t.desc[:20]}, 跃点{m})")
    if low_metric_tun:
        r.status = WARN
        r.detail = "；".join(lines) + f"；隧道网卡跃点不高于物理网卡: {', '.join(low_metric_tun)}"
    else:
        r.detail = "；".join(lines) if lines else "无默认路由（可能未连接）"
    return r


def chk_ipv6(snap: Snapshot) -> CheckResult:
    r = CheckResult("ipv6", "IPv6 状态（V6 探测值缺失会导致 v6 网络误判）", INFO)
    vals = winregread.values(rf"HKLM\{config.NCSI_REG_PATH}")
    has_v6_vals = bool(vals) and "ActiveWebProbeHostV6" in vals
    r.detail = f"NCSI V6 探测值: {'有' if has_v6_vals else '无'}"
    return r


def chk_wlan(snap: Snapshot) -> CheckResult:
    r = CheckResult("wlan", "WLAN 配置（MAC 随机化 / 连接状态）", INFO)
    text = snap.wlan_text
    ssid = current_ssid(text)
    lines = []
    if "已断开" in text or "disconnected" in text.lower():
        lines.append("当前未连接 WiFi")
    elif ssid:
        lines.append(f"已连接: {ssid}")
    rr = proc.run(["netsh", "wlan", "show", "settings"], timeout=10)
    # 真实输出形如「MAC randomization for all networks: ENABLED」/「MAC 随机化(全部网络): 已启用」：
    # 词组间有说明文字与冒号，旧正则的紧邻匹配永不命中 → 该 WARN 从未产生过
    if re.search(r"MAC\s*随机化[^\r\n]*(已启用|ENABLED)|MAC\s*randomization[^\r\n]*ENABLED",
                 rr.stdout, re.I):
        lines.append("接口级 MAC 随机化已启用（部分路由器/公寓网对此不友好，连不上或频繁掉线时可尝试关闭）")
        r.status = WARN
    r.detail = "；".join(lines) if lines else "无法读取 WLAN 设置"
    return r


ALL_CHECKS = [
    chk_nlasvc, chk_persistent_route, chk_static_dns, chk_proxy, chk_gateway, chk_dns_compare,
    chk_dns_pollution, chk_ncsi_probe, chk_real, chk_ncsi_registry, chk_power,
    chk_services, chk_routes, chk_ipv6, chk_wlan,
]


def run_all(progress=None, quick=False, should_stop=None) -> list:
    """quick=True 时跳过慢速外网探测（供监控轮询）。

    should_stop: 可选回调，返回 True 时停止后续检查（中止按钮）。已完成
    的检查照常返回，末尾追加一条 skip 状态的「诊断中止」记录。
    """
    checks = list(ALL_CHECKS)
    if quick:
        checks = [c for c in checks if c not in (chk_dns_pollution, chk_ncsi_registry, chk_power, chk_ipv6, chk_wlan)]
    from .snapshot import take as take_snap
    snap = take_snap()
    results = []
    for fn in checks:
        if should_stop is not None and should_stop():
            aborted = CheckResult("diag_aborted", "诊断中止", SKIP,
                                  "已被用户中止，剩余检查未执行")
            results.append(aborted)
            if progress:
                progress(aborted)
            break
        try:
            res = fn(snap)
        except Exception as e:
            res = CheckResult(fn.__name__, fn.__name__, WARN, f"检查执行异常: {e}")
        results.append(res)
        if progress:
            progress(res)
    return results


def verdict(results: list) -> tuple:
    """综合判定 → (状态码, 一句话)。状态码: normal/ncsi_false_alarm/dns_broken/no_internet。

    （config_issues 曾是声明的第五状态，但其分支不可达——前四个分支已穷尽
    real_ok 的真假组合；假承诺已于 2026-09-19 清理。）
    """
    by = {r.id: r for r in results}
    real = by.get("real_conn")
    ncsi = by.get("ncsi_probe")
    dns_cmp = by.get("dns_compare")
    real_ok = bool(real and real.status == PASS)
    ncsi_ok = bool(ncsi and ncsi.status == PASS)
    dns_bad = bool(dns_cmp and dns_cmp.status == FAIL)
    if real_ok and ncsi_ok and not dns_bad:
        base, text = "normal", "网络正常"
    elif real_ok and not ncsi_ok:
        base, text = "ncsi_false_alarm", "实际有网，但 Windows 判定'无网络'（NCSI 误报）"
    elif real_ok and dns_bad:
        base, text = "dns_broken", "能连上但 DNS 解析异常/缓慢（系统 DNS 排序被幽灵路由或残废 DNS 带偏）"
    else:
        base, text = "no_internet", "外网不可达"
    fails = [r.name for r in results if r.status == FAIL]
    if fails:
        text += f"；失败项: {len(fails)} 个"
    return base, text
