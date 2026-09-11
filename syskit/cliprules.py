"""剪贴板内容规则引擎：检测文本类型并给出可执行动作（纯逻辑，无 Qt、无网络）。

detect() 返回动作列表，每项 {"id", "label", "arg", "online"}：
- arg 为动作执行所需参数（格式化后的 JSON / URL / 路径 / 主机名…）
- online=True 的动作需要联网（如 IP 归属地查询），UI 可据此提示
动作的实际执行（DNS 查询、ping、二维码渲染等）由 UI 层接线到 dnsquery/icmp 等模块。
"""
import ipaddress
import json
import os
import re

URL_RE = re.compile(r"https?://[^\s<>\"']+", re.I)
COLOR_RE = re.compile(r"^#(?:[0-9a-fA-F]{3}|[0-9a-fA-F]{6})$")
WIN_PATH_RE = re.compile(r"^[A-Za-z]:\\[^<>:\"|?*\r\n]+$")
DOMAIN_RE = re.compile(
    r"^(?=.{4,253}$)([a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,}$", re.I)


def detect(text) -> list:
    """按优先级返回可执行动作；不匹配返回 []。"""
    hits = []
    t = (text or "").strip()
    if not t or len(t) > 1_000_000:
        return hits

    # JSON：以 { 或 [ 开头且可解析
    if t[0] in "{[":
        try:
            obj = json.loads(t)
            hits.append({"id": "json_format", "label": "格式化 JSON",
                         "arg": json.dumps(obj, ensure_ascii=False, indent=2),
                         "online": False})
        except Exception:
            pass

    # URL
    m = URL_RE.search(t)
    if m:
        url = m.group(0).rstrip(".,;)")
        hits.append({"id": "url_qr", "label": "生成二维码", "arg": url, "online": False})
        hits.append({"id": "url_open", "label": "打开链接", "arg": url, "online": False})

    # 颜色值
    if COLOR_RE.match(t):
        hits.append({"id": "color_show", "label": "查看色块", "arg": t.upper(), "online": False})

    # Windows 文件路径（路径需真实存在，避免把普通句子误判）
    if WIN_PATH_RE.match(t) and os.path.exists(t):
        hits.append({"id": "file_reveal", "label": "打开所在目录", "arg": t, "online": False})
        hits.append({"id": "file_open", "label": "打开文件", "arg": t, "online": False})

    # IP / 域名
    try:
        ip = ipaddress.ip_address(t)
        hits.append({"id": "ip_ping", "label": "Ping", "arg": str(ip), "online": True})
        if ip.version == 4:
            hits.append({"id": "ip_geo", "label": "查询归属地", "arg": str(ip), "online": True})
    except ValueError:
        if DOMAIN_RE.match(t):
            hits.append({"id": "domain_dns", "label": "DNS 解析", "arg": t.lower(), "online": True})
            hits.append({"id": "domain_ping", "label": "Ping", "arg": t.lower(), "online": True})
    return hits
