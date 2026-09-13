"""修复动作库：风险分级、改前备份、还原提示。所有动作 execute(param) -> RepairResult。"""
import json
import os
import re
import time
from dataclasses import dataclass, field

from . import config, proc, winregread
from .logger import LOGGER

RISK_SAFE, RISK_CONFIG, RISK_AGGRESSIVE = "safe", "config", "aggressive"
RISK_LABEL = {RISK_SAFE: "安全", RISK_CONFIG: "改配置", RISK_AGGRESSIVE: "强力(需重启)"}


@dataclass
class RepairResult:
    success: bool
    message: str
    need_reboot: bool = False
    output: str = ""


def _backup_dir() -> str:
    d = os.path.join(config.BACKUP_DIR, time.strftime("%Y%m%d_%H%M%S"))
    os.makedirs(d, exist_ok=True)
    return d


def _reg_export(reg_path_hklm: str) -> str:
    d = _backup_dir()
    f = os.path.join(d, re.sub(r"[^A-Za-z0-9]+", "_", reg_path_hklm)[:60] + ".reg")
    r = proc.run(["reg", "export", reg_path_hklm, f, "/y"], timeout=15)
    return f if r.ok else ""


def _save_note(name: str, content: str) -> str:
    d = _backup_dir()
    f = os.path.join(d, name)
    with open(f, "w", encoding="utf-8") as fh:
        fh.write(content)
    return f


class RepairAction:
    def __init__(self, aid, title, risk, description, restore_hint,
                 brief="", need_reboot=False, params=None):
        self.id = aid
        self.title = title
        self.risk = risk
        self.description = description
        self.restore_hint = restore_hint or "无"
        self.brief = brief or description[:60]
        self.need_reboot = need_reboot
        self.params = params or {}   # name -> {"desc": str, "required": bool}

    def execute(self, param: dict = None) -> RepairResult:
        try:
            return self._run(param or {})
        except Exception as e:
            LOGGER.exception("修复动作异常 {}", self.id)
            return RepairResult(False, f"执行异常: {e}")

    def _run(self, param: dict) -> RepairResult:
        raise NotImplementedError


# ---------------- 安全类 ----------------

class FlushDns(RepairAction):
    def __init__(self):
        super().__init__("flush_dns", "刷新 DNS 解析缓存", RISK_SAFE,
                         "清空系统 DNS 缓存。无副作用，可反复执行。",
                         "无需还原（缓存会自动重建）")

    def _run(self, param):
        r = proc.run(["ipconfig", "/flushdns"], timeout=15)
        return RepairResult(r.ok, r.stdout.strip() or r.stderr.strip())


class RenewDhcp(RepairAction):
    def __init__(self):
        super().__init__("renew_dhcp", "重新获取 IP（DHCP 续租）", RISK_SAFE,
                         "释放并重新获取所有 DHCP 网卡的 IP。执行期间网络会瞬断几秒，隧道/VPN 不受影响（其非 DHCP）。",
                         "无需还原（DHCP 自动分配）")

    def _run(self, param):
        proc.run(["ipconfig", "/release"], timeout=30)
        r = proc.run(["ipconfig", "/renew"], timeout=90)
        return RepairResult(r.ok, "已重新获取 IP" if r.ok else r.stdout[-200:] + r.stderr[-100:])


class StartNlaSvc(RepairAction):
    def __init__(self):
        super().__init__("start_nlasvc", "启动 NlaSvc（网络位置感知）", RISK_SAFE,
                         "立即启动 NlaSvc，使 NCSI 探测恢复运行。若启动类型是'手动+无触发器'，重启后还会复发，建议配合 fix_nlasvc。",
                         "无需还原")

    def _run(self, param):
        r = proc.run(["sc", "start", "NlaSvc"], timeout=30)
        ok = r.ok or "1056" in r.stdout + r.stderr  # 1056=已在运行
        q = proc.run(["sc", "query", "NlaSvc"], timeout=10)
        running = "RUNNING" in q.stdout or "4" in q.stdout
        return RepairResult(ok and running, f"NlaSvc 当前状态: {'运行中' if running else '未运行'} " + (r.stdout[:100] if not ok else ""))


class RestartWlan(RepairAction):
    def __init__(self):
        super().__init__("restart_wlan", "重启 WLAN 服务并重连 WiFi", RISK_SAFE,
                         "重启 WlanSvc（WiFi 自动配置服务），并自动重连之前连接的 WiFi。WiFi 会断开几秒。",
                         "无需还原")

    def _run(self, param):
        cur = proc.run(["netsh", "wlan", "show", "interfaces"], timeout=10)
        ssid = ""
        m = re.search(r"\bSSID\s*[:：]\s*(\S.*)", cur.stdout)
        if m and m.group(1).strip().strip('"'):
            ssid = m.group(1).strip().strip('"')
        proc.run(["net", "stop", "WlanSvc"], timeout=40)
        time.sleep(1)
        r = proc.run(["net", "start", "WlanSvc"], timeout=40)
        if not r.ok:
            return RepairResult(False, "WlanSvc 启动失败: " + (r.stdout + r.stderr)[-150:])
        msg = "WlanSvc 已重启"
        if ssid:
            time.sleep(2)
            c = proc.run(["netsh", "wlan", "connect", f"name={ssid}"], timeout=15)
            msg += f"，已发起重连 {ssid}" + ("成功" if c.ok else "（请手动选择网络）")
        return RepairResult(True, msg)


class ToggleAdapter(RepairAction):
    def __init__(self):
        super().__init__("toggle_adapter", "禁用并重新启用网卡", RISK_SAFE,
                         "对指定网卡执行 disable→enable，重建链路。WiFi/USB 网卡'假连接'时的经典修复。",
                         "无需还原",
                         params={"adapter": {"desc": "网卡名称，如 WLAN / 以太网 3", "required": False}})

    def _run(self, param):
        name = param.get("adapter") or _pick_adapter()
        if not name:
            return RepairResult(False, "未指定网卡且无法自动选择")
        d = proc.run(["netsh", "interface", "set", "interface", f"name={name}", "disable"], timeout=20)
        time.sleep(2)
        e = proc.run(["netsh", "interface", "set", "interface", f"name={name}", "enable"], timeout=25)
        return RepairResult(e.ok, f"{name} 已重新启用" if e.ok else f"启用失败: {e.stdout[-100:]}{e.stderr[-100:]}")


def _pick_adapter() -> str:
    """优先选 WLAN，其次第一个有连接意图的物理网卡。"""
    from .snapshot import take
    snap = take()
    for a in snap.adapters:
        if a.name.upper() == "WLAN":
            return a.name
    for a in snap.adapters:
        if not a.virtual and a.status.lower() != "up" and "WAN Miniport" not in a.desc:
            return a.name
    return ""


class RestartServices(RepairAction):
    def __init__(self):
        super().__init__("restart_services", "重启 DHCP/DNS 缓存服务", RISK_SAFE,
                         "重启 Dhcp 与 Dnscache 服务，清理卡死的服务状态。",
                         "无需还原")

    def _run(self, param):
        msgs = []
        for svc in ("Dhcp", "Dnscache"):
            proc.run(["net", "stop", svc], timeout=30)
            r = proc.run(["net", "start", svc], timeout=30)
            msgs.append(f"{svc}:{'OK' if r.ok else '失败'}")
        ok = all("OK" in m for m in msgs)
        return RepairResult(ok, "，".join(msgs))


# ---------------- 改配置类 ----------------

class FixNlaSvc(RepairAction):
    def __init__(self):
        super().__init__("fix_nlasvc", "修复 NlaSvc 启动配置（设为自动并立即启动）", RISK_CONFIG,
                         "本机 NlaSvc 被改为'手动启动+无触发器'（原版为触发启动），导致它自开机起从未运行、NCSI 探测无人执行。"
                         "此动作把启动类型改为'自动'并立即启动。这是对误报'无网络'的直接修复。",
                         "以管理员执行: sc config NlaSvc start= demand",
                         params={})

    def _run(self, param):
        # sc 解析原始命令行，"start= auto" 必须是两个裸参数（不能带引号）
        c = proc.run(["sc", "config", "NlaSvc", "start=", "auto"], timeout=15)
        if not c.ok:
            return RepairResult(False, "sc config 失败: " + (c.stdout + c.stderr)[-120:])
        s = proc.run(["sc", "start", "NlaSvc"], timeout=30)
        q = proc.run(["sc", "query", "NlaSvc"], timeout=10)
        running = "RUNNING" in q.stdout
        return RepairResult(running, "NlaSvc 已设为自动启动" + ("，且已成功启动" if running else "，但启动失败，请重启后再试"))


class DnsAuto(RepairAction):
    def __init__(self):
        super().__init__("dns_auto", "DNS 恢复为 DHCP 自动获取", RISK_CONFIG,
                         "清除指定网卡上残留的静态 DNS（如 8.8.8.8），恢复由路由器/热点下发。改前自动备份原 DNS。",
                         "用 dns_preset 动作把原值设回，或查看备份 JSON",
                         params={"adapter": {"desc": "网卡名称，默认自动选静态 DNS 的网卡", "required": False}})

    def _run(self, param):
        name = param.get("adapter") or _static_dns_adapter()
        if not name:
            return RepairResult(False, "未找到配置了静态 DNS 的网卡")
        _backup_dns(name)
        r = proc.run(["netsh", "interface", "ipv4", "set", "dnsservers", f"name={name}", "source=dhcp"], timeout=15)
        v6 = proc.run(["netsh", "interface", "ipv6", "set", "dnsservers", f"name={name}", "source=dhcp"], timeout=15)
        ok = r.ok
        proc.run(["ipconfig", "/flushdns"], timeout=10)
        return RepairResult(ok, f"{name} 的 DNS 已恢复自动获取" + ("" if ok else ": " + (r.stdout + r.stderr)[-100:]))


def _static_dns_adapter() -> str:
    from .snapshot import take
    snap = take()
    for a in snap.adapters:
        if a.dns and not a.virtual and a.status.lower() == "up":
            r = proc.run(["netsh", "interface", "ipv4", "show", "dnsservers", a.name], timeout=10)
            if re.search(r"静态配置|Statically configured", r.stdout, re.I):
                return a.name
    for a in snap.adapters:  # WLAN 优先兜底（未连接时也有静态配置）
        if a.name.upper() == "WLAN" and a.dns:
            return a.name
    return ""


def _backup_dns(adapter: str) -> str:
    """备份 v4+v6 DNS（dns_auto 会同时清两者的静态配置，只备 v4 会让 v6 静态值
    被永久清除且不可恢复）。任一段失败则返回 ""（调用方应视为备份未完成）。"""
    try:
        out = []
        for ip in ("ipv4", "ipv6"):
            r = proc.run(["netsh", "interface", ip, "show", "dnsservers", adapter],
                         timeout=10)
            out.append("===== {} =====\n{}".format(ip, r.stdout))
        return _save_note(f"dns_backup_{re.sub(r'[^A-Za-z0-9]+', '_', adapter)}.txt",
                          "\n".join(out))
    except Exception:
        return ""


class DnsPreset(RepairAction):
    def __init__(self):
        super().__init__("dns_preset", "设置静态公共 DNS", RISK_CONFIG,
                         "把指定网卡 DNS 设为国内公共 DNS（阿里/腾讯/114 等）。改前自动备份原值。",
                         "运行 dns_auto 恢复自动获取",
                         params={"adapter": {"desc": "网卡名称", "required": False},
                                 "preset": {"desc": "预设名: " + "/".join(config.DNS_PRESETS), "required": False}})

    def _run(self, param):
        name = param.get("adapter") or _pick_adapter()
        preset = param.get("preset") or settings_default_preset()
        if preset not in config.DNS_PRESETS:
            return RepairResult(False, f"未知 DNS 预设: {preset}")
        if not name:
            return RepairResult(False, "未指定网卡且无法自动选择")
        primary, secondary = config.DNS_PRESETS[preset]
        _backup_dns(name)
        r1 = proc.run(["netsh", "interface", "ipv4", "set", "dnsservers", f"name={name}", "static", primary, "primary"], timeout=15)
        r2 = proc.run(["netsh", "interface", "ipv4", "add", "dnsservers", f"name={name}", secondary, "index=2"], timeout=15)
        proc.run(["ipconfig", "/flushdns"], timeout=10)
        ok = r1.ok
        return RepairResult(ok, f"{name} DNS → {primary}, {secondary}（{preset}）" + ("" if ok else ": " + (r1.stdout + r1.stderr)[-100:]))


def settings_default_preset() -> str:
    from . import settings
    return settings.load().get("dns_preset", "阿里")


class NcsiRestore(RepairAction):
    def __init__(self):
        super().__init__("ncsi_restore", "NCSI 探测恢复微软默认并启用主动探测", RISK_CONFIG,
                         "把 NCSI 注册表恢复为原版默认值（探测 www.msftconnecttest.com/connecttest.txt），"
                         "并确保 EnableActiveProbing=1。改前 reg export 全量备份。",
                         "双击备份目录中的 .reg 文件导入即可还原")

    def _run(self, param):
        backup = _reg_export(rf"HKLM\{config.NCSI_REG_PATH}")
        try:
            import winreg
            key = winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, config.NCSI_REG_PATH, 0, winreg.KEY_SET_VALUE)
            try:
                winreg.SetValueEx(key, "EnableActiveProbing", 0, winreg.REG_DWORD, 1)
                for name, val in config.NCSI_STOCK.items():
                    if name == "EnableActiveProbing":
                        continue
                    winreg.SetValueEx(key, name, 0, winreg.REG_SZ, str(val))
            finally:
                key.Close()
        except OSError as e:
            return RepairResult(False, f"写注册表失败（需要管理员）: {e}")
        proc.run(["sc", "start", "NlaSvc"], timeout=20)
        return RepairResult(True, "NCSI 已恢复默认并启用主动探测" + (f"；备份: {backup}" if backup else "；备份失败但修改成功"))


class NcsiDisable(RepairAction):
    def __init__(self):
        super().__init__("ncsi_disable", "关闭 NCSI 主动探测（最后手段）", RISK_CONFIG,
                         "EnableActiveProbing=0：Windows 不再探测，状态永远显示'已连接'。代价：真正断网时不再显示'无Internet'提醒。仅在探测源彻底不可用且更换 DNS 无效时使用。",
                         "运行 ncsi_restore 恢复探测")

    def _run(self, param):
        backup = _reg_export(rf"HKLM\{config.NCSI_REG_PATH}")
        try:
            import winreg
            key = winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, config.NCSI_REG_PATH, 0, winreg.KEY_SET_VALUE)
            try:
                winreg.SetValueEx(key, "EnableActiveProbing", 0, winreg.REG_DWORD, 0)
            finally:
                key.Close()
        except OSError as e:
            return RepairResult(False, f"写注册表失败（需要管理员）: {e}")
        return RepairResult(True, "已关闭主动探测" + (f"；备份: {backup}" if backup else ""))


class PowerOff(RepairAction):
    def __init__(self):
        super().__init__("power_off", "禁止系统关闭网卡省电（关节电）", RISK_CONFIG,
                         "对各物理网卡驱动键写入 PnPCapabilities=24（本机电源管理 WMI 已损坏，走注册表方案），"
                         "并关闭 USB 选择性暂停。修复 WiFi/USB 共享频繁掉线。设置后需重新启用网卡或重启生效。",
                         "删除对应注册表值的 PnPCapabilities（备份 JSON 中有记录）",
                         need_reboot=False)

    def _run(self, param):
        import winreg
        base = config.NIC_CLASS_KEY
        changed, skipped = [], []
        full_base = rf"HKLM\{base}"
        for sub in winregread.subkeys(full_base):
            if not sub.startswith("0"):
                continue
            kpath = f"{base}\\{sub}"
            vals = winregread.values(full_base + "\\" + sub)
            desc = str(vals.get("DriverDesc", ""))
            if not desc:
                continue
            if any(x in desc.lower() for x in ("wan miniport", "microsoft kernel", "bluetooth", "virtual")):
                continue
            if str(vals.get("PnPCapabilities")) == "24":
                continue
            backup = _save_note(f"pnp_backup_{sub}.txt", f"{kpath}\nDriverDesc={desc}\nPnPCapabilities={vals.get('PnPCapabilities', '<缺省=允许断电>')}")
            try:
                key = winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, kpath, 0, winreg.KEY_SET_VALUE)
                try:
                    winreg.SetValueEx(key, "PnPCapabilities", 0, winreg.REG_SZ, "24")
                finally:
                    key.Close()
                changed.append(f"{desc[:30]}")
            except OSError as e:
                skipped.append(f"{desc[:30]}: {e}")
        usb = proc.run(["powercfg", "/setacvalueindex", "SCHEME_CURRENT",
                        "2a737441-1930-4402-8d77-b2bebba308a3", "48e6b7a6-50f5-4782-a5d4-53bb8f07e226", "0"], timeout=15)
        proc.run(["powercfg", "/setactive", "SCHEME_CURRENT"], timeout=15)
        if not changed:
            return RepairResult(False, "没有需要修改的网卡" + ("；USB选择性暂停:" + ("OK" if usb.ok else "FAIL")))
        msg = (f"已对 {len(changed)} 个网卡禁止断电: {', '.join(changed)}"
               f"；USB 选择性暂停{'已关闭' if usb.ok else '关闭失败'}")
        if skipped:
            msg += f"；跳过: {', '.join(skipped)}"
        msg += "。重新启用网卡或重启后生效。"
        return RepairResult(True, msg)


class DelPersistentRoute(RepairAction):
    def __init__(self):
        super().__init__("del_persistent_route", "删除劫持流量的持久路由", RISK_CONFIG,
                         "删除持久化存储中的默认路由（如 0.0.0.0/0 → 172.16.2.1）。这类路由会在连上同网段 WiFi 时以最低跃点抢走全部流量。"
                         "删除前把完整路由表存档，还原命令会打印到日志。",
                         "route -p add <目标> mask <掩码> <网关> metric <跃点>（详见备份文件）",
                         params={"dest": {"desc": "目标网段，默认自动检测持久默认路由", "required": False},
                                 "gw": {"desc": "网关", "required": False}})

    def _run(self, param):
        from .snapshot import take
        snap = take()
        target = None
        if param.get("dest") and param.get("gw"):
            target = {"dest": param["dest"], "nextHop": param["gw"], "metric": 1}
        else:
            defaults = [p for p in snap.persistent_routes if p["dest"] == "0.0.0.0/0"]
            target = defaults[0] if defaults else None
        if not target:
            return RepairResult(True, "未发现持久默认路由，无需处理")
        gw = target["nextHop"]
        dest = target["dest"]
        dest_ip = dest.split("/")[0]
        mask = "0.0.0.0" if dest.endswith("/0") else _prefix_to_mask(dest)
        backup = _save_note("route_backup.txt",
                            "还原命令:\n"
                            f"  route -p add {dest_ip} mask {mask} {gw} metric {target.get('metric') or 1}\n"
                            "\n删除前完整路由表:\n" + proc.run(["route", "print", "-4"], timeout=10).stdout)
        proc.run(["route", "delete", dest_ip, "mask", mask, gw], timeout=15)
        ps = (f"Remove-NetRoute -DestinationPrefix {dest} -NextHop {gw} "
              f"-PolicyStore PersistentStore -Confirm:$false -ErrorAction SilentlyContinue; "
              f"'done'")
        proc.run_ps(ps, timeout=20)
        snap2 = take()
        gone_p = not any(p["dest"] == target["dest"] and p["nextHop"] == gw for p in snap2.persistent_routes)
        gone_a = not any(d["nextHop"] == gw for d in snap2.default_routes)
        if gone_p and gone_a:
            return RepairResult(True, f"已删除持久路由 {dest} → {gw}（活动表+持久表）；备份/还原命令: {backup}")
        return RepairResult(False,
                            f"删除未完全（活动表残留:{not gone_a} 持久表残留:{not gone_p}），请手动执行: "
                            f"route delete {dest_ip} mask {mask} {gw} 及 "
                            f"Remove-NetRoute -DestinationPrefix {dest} -NextHop {gw} -PolicyStore PersistentStore")


def _prefix_to_mask(prefix: str) -> str:
    try:
        bits = int(prefix.split("/")[1])
        import struct, socket
        return socket.inet_ntoa(struct.pack(">I", (0xFFFFFFFF << (32 - bits)) & 0xFFFFFFFF))
    except Exception:
        return "255.255.255.0"


def _set_mac_random(profile: str, enable: bool) -> tuple:
    """改指定配置文件的 EnableRandomization。返回 (改动数, 失败信息, 备份目录)。"""
    import glob
    if not profile:
        return 0, "未指定 WiFi 配置文件", ""
    root = r"C:\ProgramData\Microsoft\Wlansvc\Profiles\Interfaces"
    xml_files = glob.glob(os.path.join(root, "*", "*.xml"))
    hits = []
    for f in xml_files:
        try:
            with open(f, "r", encoding="utf-8") as fh:
                content = fh.read()
        except OSError:
            continue
        m = re.search(r"<name>([^<]+)</name>", content)
        if m and m.group(1).strip().lower() == profile.lower():
            hits.append((f, content))
    if not hits:
        return 0, f"未找到名为 {profile} 的配置文件", ""
    d = _backup_dir()
    changed = 0
    want = "true" if enable else "false"
    for f, content in hits:
        with open(os.path.join(d, os.path.basename(f)), "w", encoding="utf-8") as fh:
            fh.write(content)
        new = re.sub(
            r"(<MacRandomization[^>]*>\s*<EnableRandomization>\s*)(true|false)(\s*</EnableRandomization>)",
            r"\g<1>" + want + r"\g<3>", content, flags=re.I)
        if new == content and "<MacRandomization" not in content:
            new = content.replace("</WLANProfile>",
                f'<MacRandomization xmlns="http://www.microsoft.com/networking/WLAN/profile/v3">'
                f"<EnableRandomization>{want}</EnableRandomization></MacRandomization></WLANProfile>")
        if new != content:
            with open(f, "w", encoding="utf-8") as fh:
                fh.write(new)
            changed += 1
    return changed, "", d


class ProxyOff(RepairAction):
    def __init__(self):
        super().__init__("proxy_off", "关闭系统代理（清除代理软件残留）", RISK_CONFIG,
                         "把 WinINET 系统代理（ProxyEnable）关闭并立即通知系统刷新。"
                         "代理软件异常退出后残留的代理会让所有流量发往已失效的本地端口，"
                         "表现为'已连接但所有应用都无法上网'。",
                         "在代理软件或 Windows 设置中重新开启代理即可")

    def _run(self, param):
        from . import proxy
        ok = proxy.disable_proxy()
        return RepairResult(ok, "系统代理已关闭" if ok else "关闭失败，请手动在 设置→网络→代理 中关闭")


class MacRandomOff(RepairAction):
    def __init__(self):
        super().__init__("mac_random_off", "关闭 WiFi 的 MAC 随机化（用真实地址连接）", RISK_CONFIG,
                         "Win11 默认对每个 WiFi 用固定随机 MAC。部分公寓/企业网络按 MAC 授权，关掉随机化用真实地址便于注册设备；"
                         "但若网络只认识旧的随机地址，关闭后反而可能拿不到 IP——工具会先备份，随时可切回。",
                         "运行 mac_random_on 重新开启（备份 XML 也在备份目录）",
                         params={"profile": {"desc": "WiFi 配置文件名（SSID），默认当前/最近使用的", "required": False}})

    def _run(self, param):
        profile = param.get("profile") or _current_or_first_profile()
        changed, err, d = _set_mac_random(profile, False)
        if err:
            return RepairResult(False, err)
        if not changed:
            return RepairResult(True, f"{profile} 的 MAC 随机化本来就是关闭的")
        r = RestartWlan().execute()
        return RepairResult(True, f"已关闭 {profile} 的 MAC 随机化（{changed} 个文件）；{r.message}")


class MacRandomOn(RepairAction):
    def __init__(self):
        super().__init__("mac_random_on", "开启 WiFi 的 MAC 随机化（恢复 Win11 默认）", RISK_CONFIG,
                         "恢复对指定 WiFi 使用固定随机 MAC。若公寓网络此前只授权了旧随机地址，开启后应能恢复拿 IP。",
                         "运行 mac_random_off 再次关闭（备份 XML 也在备份目录）",
                         params={"profile": {"desc": "WiFi 配置文件名（SSID）", "required": False}})

    def _run(self, param):
        profile = param.get("profile") or _current_or_first_profile()
        changed, err, d = _set_mac_random(profile, True)
        if err:
            return RepairResult(False, err)
        if not changed:
            return RepairResult(True, f"{profile} 的 MAC 随机化本来就是开启的")
        r = RestartWlan().execute()
        return RepairResult(True, f"已开启 {profile} 的 MAC 随机化（{changed} 个文件）；{r.message}")


def wlan_profiles() -> list:
    """列出已保存的 WiFi 配置文件名。"""
    r = proc.run(["netsh", "wlan", "show", "profiles"], timeout=10)
    names = re.findall(r"所有用户配置文件\s*[:：]\s*(.+)|All User Profile\s*[:：]\s*(.+)", r.stdout)
    return [a or b for a, b in names if (a or b).strip()]


def mac_random_state(profile: str):
    """读取指定配置文件 MAC 随机化状态: True/False/None(未知)。"""
    import glob
    root = r"C:\ProgramData\Microsoft\Wlansvc\Profiles\Interfaces"
    for f in glob.glob(os.path.join(root, "*", "*.xml")):
        try:
            with open(f, "r", encoding="utf-8") as fh:
                content = fh.read()
        except OSError:
            continue
        m = re.search(r"<name>([^<]+)</name>", content)
        if not m or m.group(1).strip().lower() != profile.lower():
            continue
        rm = re.search(r"<MacRandomization[^>]*>\s*<EnableRandomization>\s*(true|false)", content, re.I)
        return rm.group(1).lower() == "true" if rm else None
    return None


def _current_or_first_profile() -> str:
    """当前连接的 SSID 优先，否则第一个已保存配置。"""
    cur = proc.run(["netsh", "wlan", "show", "interfaces"], timeout=10)
    m = re.search(r"\bSSID\s*[:：]\s*(\S[^\n]*)", cur.stdout)
    if m and m.group(1).strip().strip('"'):
        return m.group(1).strip().strip('"')
    profiles = wlan_profiles()
    return profiles[0] if profiles else ""


# ---------------- 强力类 ----------------

class WinsockReset(RepairAction):
    def __init__(self):
        super().__init__("winsock_reset", "重置 Winsock 目录", RISK_AGGRESSIVE,
                         "netsh winsock reset。清除损坏的 LSP（装过 VPN/代理/加速器后常见）。需要重启电脑生效，重置后部分网络软件需重新配置。",
                         "不可直接还原（系统会重建干净目录）", need_reboot=True)

    def _run(self, param):
        r = proc.run(["netsh", "winsock", "reset"], timeout=20)
        return RepairResult(r.ok, (r.stdout or r.stderr).strip(), need_reboot=True)


class IpStackReset(RepairAction):
    def __init__(self):
        super().__init__("ipstack_reset", "重置 TCP/IP 与 IPv6 协议栈", RISK_AGGRESSIVE,
                         "netsh int ip reset + netsh int ipv6 reset。重建 TCP/IP 注册表配置。需要重启电脑生效；静态 IP/DNS 配置会被清除。",
                         "不可直接还原（需手动重配静态 IP/DNS）", need_reboot=True)

    def _run(self, param):
        r1 = proc.run(["netsh", "int", "ip", "reset"], timeout=20)
        r2 = proc.run(["netsh", "int", "ipv6", "reset"], timeout=20)
        ok = r1.ok and r2.ok
        return RepairResult(ok, ((r1.stdout or "") + (r2.stdout or "")).strip()[-200:], need_reboot=True)


# 组合修复（一键按钮）
COMBOS = {
    "fast": ("快速修复", ["flush_dns", "renew_dhcp", "start_nlasvc", "restart_wlan"]),
    "full": ("深度修复", ["flush_dns", "renew_dhcp", "start_nlasvc", "restart_wlan",
                          "power_off", "dns_auto", "ncsi_restore"]),
    "stack": ("重置网络栈（需重启）", ["winsock_reset", "ipstack_reset"]),
}

# ---------------- 注册表 ----------------

ACTIONS = [
    FlushDns(), RenewDhcp(), StartNlaSvc(), FixNlaSvc(), RestartWlan(), ToggleAdapter(), RestartServices(),
    DnsAuto(), DnsPreset(), NcsiRestore(), NcsiDisable(), PowerOff(), DelPersistentRoute(),
    ProxyOff(), MacRandomOff(), MacRandomOn(), WinsockReset(), IpStackReset(),
]
ACTION_MAP = {a.id: a for a in ACTIONS}
ACTION_IDS = [a.id for a in ACTIONS]


def get_action(aid: str):
    return ACTION_MAP.get(aid)
