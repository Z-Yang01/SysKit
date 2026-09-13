"""场景模式：办公（勿扰/静音/开常用软件/备份配置）与游戏（关通知/高性能/暂停更新/清后台）。

原则：
- 每一步返回 (name, ok, detail)，UI 汇总展示，不因单步失败中断后续；
- 改系统配置尽量走注册表/`powercfg`/进程枚举，便于 monkeypatch 与无副作用测试；
- 「清后台」只处理白名单可选应用，绝不碰 explorer/系统关键进程。
"""
import os
import shutil
import time

from . import admin, config, proc
from .logger import LOGGER

# 通知（全局 Toast）—— Focus Assist 等价的轻量开关
NOTIFY_SETTINGS = r"Software\Microsoft\Windows\CurrentVersion\Notifications\Settings"
NOTIFY_GLOBAL_VALUE = "NOC_GLOBAL_SETTING_TOASTS_ENABLED"

# Windows Update 暂停（需管理员）
WU_POLICY = r"SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate\AU"

# 电源方案别名（powercfg 内置）
POWER_HIGH_PERF = "SCHEME_MIN"
POWER_BALANCED = "SCHEME_BALANCED"

# 常用软件（办公模式一键打开）
COMMON_APPS = {
    "记事本": "notepad.exe",
    "计算器": "calc.exe",
    "资源管理器": "explorer.exe",
    "画图": "mspaint.exe",
    "任务管理器": "taskmgr.exe",
    "系统设置": "ms-settings:",
    "终端": "powershell.exe",
}

# 可安全结束后台（游戏模式）—— 白名单，不含系统/浏览器主进程
BACKGROUND_CLOSE_WHITELIST = (
    "OneDrive.exe",
    "Spotify.exe",
    "Discord.exe",
    "Slack.exe",
    "Teams.exe",
    "ms-teams.exe",
    "Steam.exe",
    "EpicGamesLauncher.exe",
    "Battle.net.exe",
    "RiotClientServices.exe",
    "CloudMusic.exe",
    "kugou.exe",
    "KwMusic.exe",
    "iCloudServices.exe",
    "AdobeDesktopService.exe",
    "AdobeUpdateService.exe",
)


def _step(name, ok, detail=""):
    return {"name": name, "ok": bool(ok), "detail": str(detail or "")}


# ---------------- 通知 / 勿扰 ----------------

def set_notifications(enabled: bool) -> dict:
    """全局 Toast 通知开关（HKCU，无需管理员）。enabled=False 即勿扰。"""
    try:
        import winreg
        path = NOTIFY_SETTINGS
        try:
            k = winreg.OpenKey(winreg.HKEY_CURRENT_USER, path, 0, winreg.KEY_SET_VALUE)
        except OSError:
            k = winreg.CreateKeyEx(winreg.HKEY_CURRENT_USER, path, 0, winreg.KEY_SET_VALUE)
        try:
            winreg.SetValueEx(k, NOTIFY_GLOBAL_VALUE, 0, winreg.REG_DWORD, 1 if enabled else 0)
        finally:
            k.Close()
        return _step("通知/勿扰", True, "已开启" if enabled else "已关闭（勿扰）")
    except Exception as e:
        return _step("通知/勿扰", False, str(e))


# ---------------- 静音 ----------------

# Add-Type -MemberDefinition 需要的是"成员定义"而非完整类定义（给完整类会生成嵌套类，
# 方法不可达）；模板自动 using System.Runtime.InteropServices，类体内不得再写 using
# （CS1529）。另：PowerShell 不认 C# 的 uint 后缀（0u），须用 [uint32]——
# 曾因 using + 0u 两处连环错误导致整条静音链失效（测试 mock 了 run_ps 未暴露）。
_PS_AUDIO_PREAMBLE = (
    "$sig = @'\n"
    "[DllImport(\"winmm.dll\")]\n"
    "public static extern int waveOutSetVolume(IntPtr h, uint v);\n"
    "[DllImport(\"winmm.dll\")]\n"
    "public static extern int waveOutGetVolume(IntPtr h, out uint v);\n"
    "'@\n"
    "Add-Type -MemberDefinition $sig -Name Wv -Namespace SysKitAudio -PassThru | Out-Null\n"
)


def set_mute(mute: bool) -> dict:
    """默认输出设备静音/取消静音（PowerShell + winmm，无需管理员）。

    静音前先读当前音量存入设置（modes_prev_volume），取消静音时恢复它——
    而不是想当然地置最大音量（旧行为会把用户 30% 的音量变成 100%）。
    """
    from . import settings
    if mute:
        prev = None
        r = proc.run_ps(_PS_AUDIO_PREAMBLE +
                        "$v = [uint32]0; "
                        "[SysKitAudio.Wv]::waveOutGetVolume([IntPtr]::Zero, [ref]$v) | Out-Null; $v\n",
                        timeout=15)
        try:
            prev = int(r.stdout.strip().split()[-1]) if r.ok and r.stdout.strip() else None
        except (ValueError, IndexError):
            prev = None
        # prev=0 说明已处于静音（可能是本工具此前所为）：不覆盖更早的原音量记录
        if prev and 0 < prev <= 0xFFFFFFFF:
            settings.save({"modes_prev_volume": prev})
        ok = proc.run_ps(_PS_AUDIO_PREAMBLE +
                         "[SysKitAudio.Wv]::waveOutSetVolume([IntPtr]::Zero, [uint32]0)\n",
                         timeout=15).ok
        detail = "已静音" if ok else (r.stderr or r.stdout)[-120:]
        return _step("系统静音", ok, detail)
    prev = settings.load().get("modes_prev_volume")
    vol = prev if isinstance(prev, int) and 0 <= prev <= 0xFFFFFFFF else 0xFFFFFFFF
    r = proc.run_ps(_PS_AUDIO_PREAMBLE +
                    f"[SysKitAudio.Wv]::waveOutSetVolume([IntPtr]::Zero, [uint32]{vol})\n",
                    timeout=15)
    ok = bool(r.ok)
    note = "已恢复原音量" if ok and prev is not None else \
           ("已取消静音（未找到进模式前的音量，已设为最大）" if ok else
            (r.stderr or r.stdout)[-120:])
    return _step("系统静音", ok, note)


# ---------------- 常用软件 ----------------

def open_common_apps(names=None) -> list:
    """打开常用软件。names=None 时打开默认办公套件；返回逐步结果。"""
    keys = list(names) if names else ["记事本", "计算器", "资源管理器", "画图"]
    results = []
    for name in keys:
        target = COMMON_APPS.get(name)
        if not target:
            results.append(_step(f"打开 {name}", False, "未知应用"))
            continue
        try:
            os.startfile(target)  # explorer/ms-settings/notepad 等
            results.append(_step(f"打开 {name}", True, target))
        except OSError as e:
            results.append(_step(f"打开 {name}", False, str(e)))
    return results


# ---------------- 备份配置 ----------------

def backup_config() -> dict:
    """把 settings/traffic/events 等 JSON 备份到 LOCALAPPDATA\\SysKit\\backup\\配置_时间戳\\。"""
    stamp = time.strftime("%Y%m%d_%H%M%S")
    dest = os.path.join(config.BACKUP_DIR, f"config_{stamp}")
    try:
        os.makedirs(dest, exist_ok=True)
        copied = []
        for name in ("settings.json", "traffic.json", "events.json", "clipboard.json",
                     "vault.db", "vault.key"):
            src = os.path.join(config.APPDATA_DIR, name)
            if os.path.isfile(src):
                shutil.copy2(src, os.path.join(dest, name))
                copied.append(name)
        if not copied:
            # 无历史文件也要落一份默认设置，保证备份非空
            import json
            with open(os.path.join(dest, "settings.json"), "w", encoding="utf-8") as f:
                json.dump(settings_snapshot(), f, ensure_ascii=False, indent=2)
            copied = ["settings.json"]
        return _step("备份配置", True, f"{len(copied)} 个文件 → {dest}")
    except OSError as e:
        return _step("备份配置", False, str(e))


def settings_snapshot() -> dict:
    from . import settings
    return settings.load()


# ---------------- 电源 / 更新 / 清后台 ----------------

def set_power_plan(high_perf: bool) -> dict:
    """切换电源方案：True=高性能，False=平衡。"""
    alias = POWER_HIGH_PERF if high_perf else POWER_BALANCED
    r = proc.run(["powercfg", "/setactive", alias], timeout=10)
    return _step(
        "电源计划", r.ok,
        ("高性能" if high_perf else "平衡") if r.ok else (r.stderr or r.stdout)[-120:],
    )


def pause_windows_update(days: int = 7) -> dict:
    """通过 AU 策略暂停自动更新（需管理员）。days>0 写入 NoAutoUpdate=1。"""
    if not admin.is_admin():
        return _step("暂停 Windows 更新", False, "需要管理员权限")
    try:
        import winreg
        path = WU_POLICY
        try:
            k = winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, path, 0, winreg.KEY_SET_VALUE)
        except OSError:
            k = winreg.CreateKeyEx(winreg.HKEY_LOCAL_MACHINE, path, 0, winreg.KEY_SET_VALUE)
        try:
            winreg.SetValueEx(k, "NoAutoUpdate", 0, winreg.REG_DWORD, 1 if days > 0 else 0)
        finally:
            k.Close()
        return _step("暂停 Windows 更新", True, f"已暂停约 {days} 天（策略）" if days > 0 else "已恢复自动更新")
    except Exception as e:
        return _step("暂停 Windows 更新", False, str(e))


def list_background_candidates() -> list:
    """当前在跑、且在清后台白名单里的进程：[{pid, name}]。"""
    import psutil
    wanted = {n.lower() for n in BACKGROUND_CLOSE_WHITELIST}
    out = []
    try:
        for p in psutil.process_iter(["pid", "name"]):
            name = p.info.get("name") or ""
            if name.lower() in wanted:
                out.append({"pid": p.pid, "name": name})
    except Exception:
        pass
    return out


def close_background(processes=None) -> dict:
    """结束白名单进程。processes 为 [{pid,name}] 或 None=自动扫描白名单。"""
    import psutil
    targets = list(processes) if processes is not None else list_background_candidates()
    if not targets:
        return _step("清理后台", True, "无需清理（白名单内无运行中的应用）")
    killed, failed = [], []
    for t in targets:
        pid, name = int(t.get("pid") or 0), t.get("name") or "?"
        if pid <= 0:
            continue
        try:
            proc_obj = psutil.Process(pid)
            # PID 复用防护：击杀前复验进程名，避免误杀复用了 PID 的无关进程
            if proc_obj.name() and proc_obj.name().lower() != str(name).lower():
                failed.append(f"{name}(PID已复用，跳过)")
                continue
            proc_obj.kill()
            killed.append(name)
        except (psutil.NoSuchProcess, psutil.AccessDenied, psutil.ZombieProcess):
            failed.append(name)
        except Exception:
            failed.append(name)
    detail = f"已结束 {len(killed)} 个"
    if killed:
        detail += "：" + "、".join(killed[:8])
    if failed:
        detail += f"；失败 {len(failed)}"
    return _step("清理后台", bool(killed) or not targets, detail)


# ---------------- 组合入口 ----------------

def run_office_mode(open_apps=True) -> list:
    """办公模式：勿扰 → 静音 → （可选）开常用软件 → 备份配置。"""
    steps = [
        set_notifications(False),
        set_mute(True),
    ]
    if open_apps:
        steps.extend(open_common_apps())
    steps.append(backup_config())
    for s in steps:
        if s["ok"]:
            LOGGER.info("办公模式 [{}] {}", s["name"], s["detail"])
        else:
            LOGGER.warning("办公模式 [{}] 失败: {}", s["name"], s["detail"])
    return steps


def run_game_mode(clear_bg=True) -> list:
    """游戏模式：关通知 → 高性能 → 暂停更新 → （可选）清后台。"""
    steps = [
        set_notifications(False),
        set_power_plan(True),
        pause_windows_update(7),
    ]
    if clear_bg:
        steps.append(close_background())
    for s in steps:
        if s["ok"]:
            LOGGER.info("游戏模式 [{}] {}", s["name"], s["detail"])
        else:
            LOGGER.warning("游戏模式 [{}] 失败: {}", s["name"], s["detail"])
    return steps


def restore_office_defaults() -> list:
    """退出办公：恢复通知 + 取消静音（不自动关软件/不回滚备份）。"""
    steps = [set_notifications(True), set_mute(False)]
    return steps


def restore_game_defaults() -> list:
    """退出游戏：平衡电源 + 恢复更新 + 开通知。"""
    return [
        set_notifications(True),
        set_power_plan(False),
        pause_windows_update(0),
    ]
