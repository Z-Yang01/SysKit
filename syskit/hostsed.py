"""Hosts 文件编辑器逻辑：解析/序列化/增删改 + 备份写入（无 Qt 依赖）。

条目 dict：{"raw", "kind": "entry|disabled|comment", "ip", "host",
            "aliases": [str], "comment", "enabled"}
- kind=entry：启用条目；kind=disabled：被 # 注释掉的主机映射（可一键启用）；
  kind=comment：纯注释/空行（保留原文）。
- 解析支持行内注释（# 之后视为备注）；序列化对未修改条目保留原始文本。
- 写入前把原文件拷贝到 config.BACKUP_DIR/hosts_时间戳（沿 repairs.py 惯例）。
"""
import ipaddress
import os
import shutil
import time

from . import config

HOSTS_PATH = r"C:\Windows\System32\drivers\etc\hosts"


def _looks_like_entry(parts: list) -> bool:
    """首 token 是合法 IP 才视为主机映射行，避免把普通注释误判为禁用条目。"""
    if len(parts) < 2:
        return False
    try:
        ipaddress.ip_address(parts[0])
        return True
    except ValueError:
        return False


# ---------------- 解析 / 序列化 ----------------

def parse(text: str) -> list:
    """hosts 文本 → 条目列表。畸形行保留 raw、kind=comment。"""
    entries = []
    for raw in (text or "").splitlines():
        stripped = raw.strip()
        if not stripped:
            entries.append({"raw": raw, "kind": "comment", "ip": "", "host": "",
                            "aliases": [], "comment": "", "enabled": False})
            continue
        disabled = stripped.startswith("#")
        body = stripped.lstrip("#").strip() if disabled else stripped
        # 行内注释
        main, _, inline = body.partition("#")
        parts = main.split()
        if _looks_like_entry(parts):
            entries.append({
                "raw": raw, "kind": "disabled" if disabled else "entry",
                "ip": parts[0], "host": parts[1], "aliases": parts[2:],
                "comment": inline.strip(), "enabled": not disabled,
            })
        elif disabled and not main:
            # 纯注释行
            entries.append({"raw": raw, "kind": "comment", "ip": "", "host": "",
                            "aliases": [], "comment": stripped.lstrip("#").strip(),
                            "enabled": False})
        else:
            entries.append({"raw": raw, "kind": "comment", "ip": "", "host": "",
                            "aliases": [], "comment": "", "enabled": False})
    return entries


def serialize(entries: list) -> str:
    """条目列表 → hosts 文本。entry/disabled 重建规范行，comment 保留原文。"""
    lines = []
    for e in entries:
        if e.get("kind") == "comment" or not e.get("host"):
            lines.append(e.get("raw", ""))
            continue
        prefix = "# " if not e.get("enabled", True) else ""
        parts = [e["ip"], e["host"]] + list(e.get("aliases") or [])
        line = prefix + " ".join(parts)
        if e.get("comment"):
            line += f"    # {e['comment']}"
        lines.append(line)
    return "\n".join(lines) + ("\n" if lines else "")


def find_entry(entries: list, host: str, ip: str = None) -> int:
    """按主机名（不区分大小写）查找条目索引；给定 ip 时需同时匹配。返回 -1 无。"""
    host = (host or "").lower()
    for i, e in enumerate(entries):
        if e.get("kind") == "comment" or e.get("host", "").lower() != host:
            continue
        if ip and e.get("ip") != ip:
            continue
        return i
    return -1


# ---------------- 增 / 删 / 切换（纯内存操作） ----------------

def add_entry(entries: list, ip: str, host: str, comment: str = "",
              aliases=None) -> list:
    entries.append({"raw": "", "kind": "entry", "ip": ip.strip(), "host": host.strip(),
                    "aliases": [a.strip() for a in (aliases or []) if a.strip()],
                    "comment": comment.strip(), "enabled": True})
    return entries


def remove_entry(entries: list, index: int) -> list:
    if 0 <= index < len(entries):
        del entries[index]
    return entries


def toggle_entry(entries: list, index: int) -> list:
    """启用 ↔ 禁用（注释/取消注释）。comment 行无效果。"""
    if 0 <= index < len(entries):
        e = entries[index]
        if e.get("kind") in ("entry", "disabled"):
            e["enabled"] = not e.get("enabled", True)
            e["kind"] = "entry" if e["enabled"] else "disabled"
    return entries


# ---------------- IO ----------------

def read_entries(path: str = None) -> list:
    path = path or HOSTS_PATH
    try:
        with open(path, "r", encoding="utf-8", errors="replace") as f:
            return parse(f.read())
    except OSError:
        return []


def backup_hosts(path: str = None) -> str:
    """原文件拷贝到 BACKUP_DIR/hosts_时间戳；源不存在返回 ''。"""
    path = path or HOSTS_PATH
    if not os.path.exists(path):
        return ""
    dest_dir = os.path.join(config.BACKUP_DIR, f"hosts_{time.strftime('%Y%m%d_%H%M%S')}")
    os.makedirs(dest_dir, exist_ok=True)
    dest = os.path.join(dest_dir, "hosts")
    shutil.copy2(path, dest)
    return dest


def write_entries(entries: list, path: str = None, backup: bool = True) -> str:
    """序列化并写回。备份目录返回；备份失败不阻断写入。"""
    path = path or HOSTS_PATH
    dest = backup_hosts(path) if backup else ""
    text = serialize(entries)
    tmp = path + ".syskit.tmp"
    with open(tmp, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)
    os.replace(tmp, path)
    return dest
