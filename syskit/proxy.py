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


def disable_proxy() -> bool:
    """关闭系统代理并通知 WinINET 立即生效（无需管理员，HKCU 用户级配置）。"""
    try:
        key = winreg.OpenKey(winreg.HKEY_CURRENT_USER,
                             _INTERNET_SETTINGS.split("\\", 1)[1], 0, winreg.KEY_SET_VALUE)
        try:
            winreg.SetValueEx(key, "ProxyEnable", 0, winreg.REG_DWORD, 0)
        finally:
            key.Close()
    except OSError as e:
        LOGGER.error("关闭系统代理失败: {}", e)
        return False
    # 通知系统设置已变更并刷新（WinINET 系列选项 39/37）
    wininet = ctypes.windll.wininet
    wininet.InternetSetOptionW(None, 39, None, 0)   # INTERNET_OPTION_SETTINGS_CHANGED
    wininet.InternetSetOptionW(None, 37, None, 0)   # INTERNET_OPTION_REFRESH
    LOGGER.info("系统代理已关闭")
    return True
