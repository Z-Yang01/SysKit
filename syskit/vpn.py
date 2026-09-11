"""VPN / 代理隧道安全：检测在用隧道，护栏自动修复，并提供一键排查清单。

设计目标：VPN 或 Clash 类 TUN 在用时，自动修复不得做「续租 / 重启 WLAN /
关系统代理 / 删默认持久路由」等高危动作；用户可一键看排查结论。
"""
import re

from . import config, proxy
from .logger import LOGGER

# 与 netcheck.TUNNEL_PAT 对齐，并补充常见客户端特征
TUNNEL_PAT = re.compile(
    r"wintun|tap-windows|tap9|tun|utun|wireguard|openvpn|easyconnect|"
    r"sangfor|clash|v2ray|sing-box|nekoray|hysteria|uu加速|uu |"
    r"hamachi|zerotier|tailscale|oray|pgyer",
    re.I,
)


def _snap():
    from .snapshot import take
    return take()


def list_tunnels(snap=None) -> list:
    """当前 Up 的疑似隧道/VPN/代理网卡：[{name, desc, if_index, ipv4}]。"""
    snap = snap if snap is not None else _snap()
    out = []
    for a in snap.adapters:
        desc = a.desc or a.name or ""
        if a.status.lower() != "up":
            continue
        if TUNNEL_PAT.search(desc) or TUNNEL_PAT.search(a.name or ""):
            out.append({
                "name": a.name,
                "desc": desc,
                "if_index": a.if_index,
                "ipv4": list(a.ipv4 or []),
            })
    return out


def is_tunnel_up(snap=None) -> bool:
    """是否存在在用的隧道网卡（VPN / Clash TUN 等）。"""
    return bool(list_tunnels(snap))


def should_skip_auto_repair(snap=None) -> bool:
    """True = 自动修复应跳过易断隧道的动作（仍允许 flush_dns / start_nlasvc）。"""
    return is_tunnel_up(snap)


def auto_repair_guard_note() -> str:
    return "检测到 VPN/隧道网卡在线，自动修复已跳过续租/重启 WLAN 等高危步骤"


def vpn_checklist(snap=None) -> list:
    """一键排查：返回 [{name, ok, detail, level}]，level: ok/warn/fail/info。

    只读检查，不改系统；供 UI 与 CLI 展示。
    """
    snap = snap if snap is not None else _snap()
    steps = []

    # 1) 隧道网卡
    tunnels = list_tunnels(snap)
    if tunnels:
        names = "、".join(f"{t['name']}({t['desc'][:24]})" for t in tunnels[:4])
        steps.append({
            "name": "VPN/隧道网卡",
            "ok": True,
            "level": "ok",
            "detail": f"检测到在用隧道: {names}" + ("…" if len(tunnels) > 4 else ""),
        })
    else:
        steps.append({
            "name": "VPN/隧道网卡",
            "ok": False,
            "level": "info",
            "detail": "未检测到典型 VPN/TUN 网卡（客户端可能用系统代理模式，或未连接）",
        })

    # 2) 系统代理
    st = proxy.get_proxy_status()
    if st["enabled"]:
        # 有隧道在用 → 多半是客户端正常模式；无隧道却开着代理 → 更可能是残留，需重点排查
        if tunnels:
            owner_hint = "—— 已有隧道网卡在用，属客户端正常模式；仅当代理软件已退出仍显示开启才是故障"
        else:
            owner_hint = "—— 未检测到隧道网卡，可能是代理软件残留（已退出但未清理系统代理）"
        steps.append({
            "name": "系统代理",
            "ok": True,
            "level": "warn",
            "detail": f"已开启: {st['server'] or st['pac_url'][:50]} {owner_hint}"
                      + "。若 VPN 用不了且浏览器也异常，先在代理客户端点「重启/修复系统代理」",
        })
    elif st["autoconfig"]:
        steps.append({
            "name": "系统代理",
            "ok": True,
            "level": "warn",
            "detail": f"PAC: {st['pac_url'][:60]} —— 检查 PAC 指向的端口是否有进程监听",
        })
    else:
        steps.append({
            "name": "系统代理",
            "ok": True,
            "level": "ok",
            "detail": "未启用系统代理（TUN/全局路由模式的 VPN 不依赖此项）",
        })

    # 3) 持久默认路由
    defaults = [p for p in snap.persistent_routes if p.get("dest") == "0.0.0.0/0"]
    if defaults:
        gws = ", ".join(d.get("nextHop") or "?" for d in defaults)
        steps.append({
            "name": "持久默认路由",
            "ok": False,
            "level": "warn",
            "detail": f"存在持久默认路由 → {gws}。连接公寓/酒店 WiFi 时可能被 SysKit 视为幽灵路由；"
                      "删除前请确认不是 VPN 需要的（本页不会自动删）",
        })
    else:
        steps.append({
            "name": "持久默认路由",
            "ok": True,
            "level": "ok",
            "detail": "无持久默认路由",
        })

    # 4) 多默认网关 / 出口漂移
    idx_up = {a.if_index: a.status.lower() for a in snap.adapters}
    active = [d for d in snap.default_routes if idx_up.get(d["if_index"]) == "up"]
    if len(active) > 1:
        parts = []
        for d in active:
            ad = next((a for a in snap.adapters if a.if_index == d["if_index"]), None)
            label = ad.name if ad else f"if{d['if_index']}"
            parts.append(f"{label}→{d.get('nextHop') or '?'}(跃点{(d.get('metric') or 0)+(d.get('if_metric') or 0)})")
        steps.append({
            "name": "默认路由出口",
            "ok": False,
            "level": "warn",
            "detail": "存在多条活动默认路由，流量可能漂移: " + "; ".join(parts),
        })
    elif active:
        d = active[0]
        ad = next((a for a in snap.adapters if a.if_index == d["if_index"]), None)
        label = ad.name if ad else f"if{d['if_index']}"
        is_tun = bool(ad and TUNNEL_PAT.search((ad.desc or "") + (ad.name or "")))
        steps.append({
            "name": "默认路由出口",
            "ok": True,
            "level": "ok",
            "detail": f"当前主出口: {label} → {d.get('nextHop') or '?'}"
                      + ("（隧道在用，属正常 VPN 模式）" if is_tun else ""),
        })
    else:
        steps.append({
            "name": "默认路由出口",
            "ok": False,
            "level": "fail",
            "detail": "无活动默认路由（可能未联网或驱动异常）",
        })

    # 5) SysKit 自动修复策略提示
    if tunnels:
        steps.append({
            "name": "自动修复护栏",
            "ok": True,
            "level": "info",
            "detail": "SysKit 在隧道在线时不会自动续租/重启 WLAN/关代理/删路由；"
                      "若需强制修复请手动点对应按钮并确认",
        })

    # 6) 操作建议
    steps.append({
        "name": "下一步建议",
        "ok": True,
        "level": "info",
        "detail": (
            "1) 先重启 VPN/Clash 客户端；2) 确认客户端为 TUN/系统代理且端口监听正常；"
            "3) 退出 SysKit 再连一次 VPN 做 A/B；4) 仍不行再查日志 %LOCALAPPDATA%\\SysKit\\logs"
        ),
    })
    return steps


def summarize(checklist: list) -> str:
    fails = [s for s in checklist if s.get("level") == "fail"]
    warns = [s for s in checklist if s.get("level") == "warn"]
    if fails:
        return f"发现 {len(fails)} 项异常、{len(warns)} 项提醒"
    if warns:
        return f"基本可用，有 {len(warns)} 项需注意"
    return "未发现明显 VPN 相关冲突"
