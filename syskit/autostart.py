"""开机自启：schtasks 最高权限计划任务（登录静默运行，免 UAC）。"""
import os
import sys

from . import proc, admin, config
from .logger import LOGGER

TASK_NAME = "SysKitAutostart"
LEGACY_TASK_NAME = "NetToolAutostart"   # 旧版任务名，enable 时自动清理


def _command() -> str:
    # 开机自启仅对打包后的 exe 有意义（源码模式无 pythonw 工作目录保证）
    return f'"{sys.executable}" --minimized'


def enable() -> bool:
    if not admin.is_admin():
        return False
    if not getattr(sys, "frozen", False):
        LOGGER.warning("源码模式不支持开机自启，请使用打包后的 {}。exe", config.DISPLAY_NAME)
        return False
    # 清理旧版任务（改名遗留）
    proc.run(["schtasks", "/Delete", "/TN", LEGACY_TASK_NAME, "/F"], timeout=15)
    r = proc.run(["schtasks", "/Create", "/TN", TASK_NAME, "/TR", _command(),
                  "/SC", "ONLOGON", "/RL", "HIGHEST", "/F"], timeout=20)
    if r.ok:
        LOGGER.info("已创建开机自启计划任务")
    else:
        LOGGER.warning("创建计划任务失败: {}", (r.stdout + r.stderr)[-200:])
    return r.ok


def disable() -> bool:
    ok = proc.run(["schtasks", "/Delete", "/TN", TASK_NAME, "/F"], timeout=20).ok
    proc.run(["schtasks", "/Delete", "/TN", LEGACY_TASK_NAME, "/F"], timeout=15)
    return ok


def enabled() -> bool:
    if proc.run(["schtasks", "/Query", "/TN", TASK_NAME], timeout=15).ok:
        return True
    # 旧任务存在也视为已启用（引导用户保存设置时迁移到新任务）
    return proc.run(["schtasks", "/Query", "/TN", LEGACY_TASK_NAME], timeout=15).ok
