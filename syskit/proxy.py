"""系统代理状态检测与关闭。

代理软件（Clash/v2ray 等）异常退出后残留的系统代理是"已连接但无网络"
的经典元凶：所有 HTTP 流量发往一个已经没人监听的本地端口。
"""
import ctypes
import winreg

from . import winregread
from .logger import LOGGER

_INTERNET_SETTINGS = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings"


def get_proxy_status() -> dict:
    """{enabled, server, autoconfig, pac_url}"""
    vals = winregread.values(_INTERNET_SETTINGS)
    return {
        "enabled": vals.get("ProxyEnable") == 1,
        "server": str(vals.get("ProxyServer") or ""),
        "autoconfig": bool(vals.get("AutoConfigURL")),
        "pac_url": str(vals.get("AutoConfigURL") or ""),
    }


def _backup_values(vals: dict) -> str:
    """把关闭前的代理相关原值落盘到备份目录，便于人工还原。失败不抛。"""
    try:
        import json
        import os
        import time
        from . import config
        d = os.path.join(config.BACKUP_DIR, time.strftime("%Y%m%d_%H%M%S"))
        os.makedirs(d, exist_ok=True)
        path = os.path.join(d, "proxy_backup.json")
        with open(path, "w", encoding="utf-8") as f:
            json.dump({k: vals.get(k) for k in ("ProxyEnable", "ProxyServer", "AutoConfigURL")},
                      f, ensure_ascii=False, indent=2)
        return path
    except Exception:
        return ""


def disable_proxy() -> bool:
    """关闭系统代理：ProxyEnable=0，并清除 PAC（AutoConfigURL）。

    诊断里对「PAC 自动代理脚本」项给出的建议修复也是 proxy_off——若这里只关
    ProxyEnable 而不清 PAC，用户点了建议按钮却毫无效果。因此两者一并处理，
    关闭前把原值写入备份目录（还原方式：在代理软件或 Windows 设置里重新开启）。
    """
    try:
        original = winregread.values(_INTERNET_SETTINGS)
    except Exception:
        original = {}
    try:
        key = winreg.OpenKey(winreg.HKEY_CURRENT_USER,
                             _INTERNET_SETTINGS.split("\\", 1)[1], 0, winreg.KEY_SET_VALUE)
        try:
            winreg.SetValueEx(key, "ProxyEnable", 0, winreg.REG_DWORD, 0)
            if original.get("AutoConfigURL"):
                try:
                    winreg.DeleteValue(key, "AutoConfigURL")
                except OSError:
                    winreg.SetValueEx(key, "AutoConfigURL", 0, winreg.REG_SZ, "")
        finally:
            key.Close()
    except OSError as e:
        LOGGER.error("关闭系统代理失败: {}", e)
        return False
    backup = _backup_values(original) if original else ""
    # 通知系统设置已变更并刷新（WinINET 系列选项 39/37）
    wininet = ctypes.windll.wininet
    wininet.InternetSetOptionW(None, 39, None, 0)   # INTERNET_OPTION_SETTINGS_CHANGED
    wininet.InternetSetOptionW(None, 37, None, 0)   # INTERNET_OPTION_REFRESH
    LOGGER.info("系统代理已关闭（含 PAC 清理）{}", f"；备份: {backup}" if backup else "")
    return True
