"""路径常量、探测端点、DNS/NCSI 预设。"""
import os
import sys

APP_NAME = "SysKit"          # 数据目录/日志/互斥锁等内部标识（保持稳定以保证升级兼容）
DISPLAY_NAME = "SysKit 系统工具箱"   # 面向用户的显示名（窗口/托盘/exe）
VERSION = "1.2.0"            # 显示版本；打包安装脚本可用其生成快捷方式说明

if sys.platform == "win32":
    APPDATA_DIR = os.path.join(os.environ.get("APPDATA", os.path.expanduser("~")), APP_NAME)
    LOCALAPPDATA_DIR = os.path.join(os.environ.get("LOCALAPPDATA", os.path.expanduser("~")), APP_NAME)
else:
    APPDATA_DIR = os.path.join(os.path.expanduser("~"), "." + APP_NAME)
    LOCALAPPDATA_DIR = os.path.join(os.path.expanduser("~"), "." + APP_NAME)

SETTINGS_FILE = os.path.join(APPDATA_DIR, "settings.json")
LOG_DIR = os.path.join(LOCALAPPDATA_DIR, "logs")
BACKUP_DIR = os.path.join(LOCALAPPDATA_DIR, "backup")

# 连通性探测端点：期望 204 无内容响应，国内可达
PROBE_204_URLS = [
    "http://connect.rom.miui.com/generate_204",
    "http://wifi.vivo.com.cn/generate_204",
]

# 真实 HTTPS 连通性
PROBE_HTTPS_URL = "https://www.baidu.com"

# NCSI 主动探测模拟（Windows 原生探测的等价请求）
NCSI_WEB_URL = "http://www.msftconnecttest.com/connecttest.txt"
NCSI_EXPECT_CONTENT = "Microsoft Connect Test"
NCSI_DNS_HOST = "dns.msftncsi.com"
NCSI_DNS_EXPECT = "131.107.255.255"

# NCSI 注册表（Win11 实测键名，含 V6 变体）
NCSI_REG_PATH = r"SYSTEM\CurrentControlSet\Services\NlaSvc\Parameters\Internet"
NCSI_REG_FULL = "HKLM\\" + NCSI_REG_PATH
NCSI_POLICY_KEY = r"SOFTWARE\Policies\Microsoft\Windows\NetworkConnectivityStatusIndicator"

# 原版默认值（用于"恢复微软默认"）
NCSI_STOCK = {
    "EnableActiveProbing": 1,
    "ActiveWebProbeHost": "www.msftconnecttest.com",
    "ActiveWebProbePath": "connecttest.txt",
    "ActiveWebProbeContent": "Microsoft Connect Test",
    "ActiveWebProbeHostV6": "ipv6.msftconnecttest.com",
    "ActiveWebProbePathV6": "connecttest.txt",
    "ActiveWebProbeContentV6": "Microsoft Connect Test",
    "ActiveDnsProbeHost": "dns.msftncsi.com",
    "ActiveDnsProbeContent": "131.107.255.255",
    "ActiveDnsProbeHostV6": "dns.msftncsi.com",
    "ActiveDnsProbeContentV6": "fd3e:4f5a:5b81::1",
}

# 国内公共 DNS 预设
DNS_PRESETS = {
    "阿里": ("223.5.5.5", "223.6.6.6"),
    "腾讯": ("119.29.29.29", "182.254.116.116"),
    "114": ("114.114.114.114", "114.114.115.115"),
    "360": ("101.226.4.6", "218.30.118.6"),
}

# 对比测试用公共 DNS
PUBLIC_DNS_TEST = ("223.5.5.5", "119.29.29.29")

# 国内不可靠、常被列为"地雷"的国外 DNS
FOREIGN_DNS_BLACKLIST = {"8.8.8.8", "8.8.4.4", "1.1.1.1", "1.0.0.1", "9.9.9.9", "208.67.222.222"}

# 网卡驱动类 GUID（PnPCapabilities 关节电用）
NIC_CLASS_KEY = r"SYSTEM\CurrentControlSet\Control\Class\{4d36e972-e325-11ce-bfc1-08002be10318}"

# 监控默认参数
DEFAULTS = {
    "auto_repair_enabled": True,
    "poll_interval_sec": 30,
    "auto_repair_max_level": 3,       # 1=刷缓存 2=+服务/续租 3=+重启网卡/WLAN服务
    "fail_threshold": 2,              # 连续失败几次触发修复
    "cooldown_sec": 180,              # 修复梯度最小间隔
    "level3_cooldown_sec": 300,       # L3 之后冷却
    "start_with_windows": False,
    "minimize_to_tray": True,
    "tray_rate_enabled": True,
    "sound_alert_enabled": True,
    "dark_mode": False,
    "dns_preset": "阿里",
    "clipboard_max": 50,               # 剪贴板历史上限条数（非置顶）
    "clip_process_blacklist": "keepass,bitwarden,1password,enpass",  # 前台为这些进程时不记录剪贴板
    "clip_sensitive_ttl": 25,          # 敏感复制在剪贴板中的存活秒数（0=不清空）
    "clip_capture_images": True,       # 剪贴板监听是否记录图片
}
