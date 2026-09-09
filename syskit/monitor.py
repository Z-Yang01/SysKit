"""后台监控线程：周期探测 + 保守自动修复梯度。只执行安全类动作，改配置类仅建议。"""
import threading
import time
import urllib.request

from . import config, netcheck, repairs, settings
from .logger import LOGGER

# 状态码
ST_OK, ST_NO_NET, ST_NCSI_FALSE, ST_OFFLINE = "ok", "no_net", "ncsi_false", "offline"


def probe_quick(timeout: float = 4.0) -> bool:
    """轻量探测：任一 204 端点可达即有网。"""
    for url in config.PROBE_204_URLS[:1]:
        try:
            req = urllib.request.Request(url, headers={"User-Agent": "SysKit/1.0"})
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                if resp.status == 204:
                    return True
        except Exception:
            continue
    return False


class Monitor(threading.Thread):
    """emit(status, detail) 通过回调通知 UI/托盘。回调在监控线程被调用，UI 需自行调度。"""

    def __init__(self, on_status=None, on_action=None, on_suggest=None):
        super().__init__(daemon=True)
        self.on_status = on_status or (lambda *a: None)
        self.on_action = on_action or (lambda *a: None)     # (action_id, result)
        self.on_suggest = on_suggest or (lambda *a: None)   # (fix_id, reason)
        self._stop = threading.Event()
        self._force_check = threading.Event()
        self.last_results = []
        self._fail_streak = 0
        self._last_repair_ts = 0.0
        self._last_l3_ts = 0.0

    def stop(self):
        self._stop.set()

    def force_check(self):
        self._force_check.set()

    def run(self):
        from .logger import sweep_old_logs
        last_sweep_day = ""
        while not self._stop.is_set():
            st = self.settings()
            try:
                today = time.strftime("%Y-%m-%d")
                if today != last_sweep_day:
                    sweep_old_logs()
                    last_sweep_day = today
                self._once(st)
            except Exception:
                LOGGER.exception("监控循环异常")
            self._stop.wait(st["poll_interval_sec"])
            if self._force_check.is_set():
                self._force_check.clear()
                self._stop.wait(1.0)

    def settings(self):
        s = settings.load()
        return {
            "poll_interval_sec": max(10, int(s.get("poll_interval_sec", 30))),
            "fail_threshold": max(1, int(s.get("fail_threshold", 2))),
            "max_level": int(s.get("auto_repair_max_level", 3)),
            "enabled": bool(s.get("auto_repair_enabled", True)),
            "cooldown_sec": int(s.get("cooldown_sec", 180)),
            "level3_cooldown_sec": int(s.get("level3_cooldown_sec", 300)),
        }

    def _once(self, st):
        from .snapshot import take
        snap = take()
        ups = snap.physical_up_adapters()
        if not ups:
            self.on_status(ST_OFFLINE, "无活动网络连接")
            self._fail_streak = 0
            return

        net_ok = probe_quick()
        if net_ok:
            self._fail_streak = 0
            self.on_status(ST_OK, "")
            return

        self._fail_streak += 1
        # 区分 NCSI 误报：真实探测（多端点+HTTPS）通过而 204 快探失败少见，直接视为断网
        self.on_status(ST_NO_NET, f"连通性探测失败 ×{self._fail_streak}")
        if not st["enabled"] or self._fail_streak < st["fail_threshold"]:
            return
        now = time.time()
        if now - self._last_repair_ts < st["cooldown_sec"]:
            return

        self._fail_streak = 0
        self._last_repair_ts = now
        level = self._repair_level(st)
        LOGGER.info("自动修复触发, 级别 L%d", level)
        self._run_ladder(level)

    def _repair_level(self, st) -> int:
        """根据距上次 L3 的时间决定本轮修复上限，避免频繁重启网卡。"""
        if time.time() - self._last_l3_ts < st["level3_cooldown_sec"]:
            return 2
        return st["max_level"]

    def _run_ladder(self, level: int):
        ladder = ["flush_dns"]
        if level >= 2:
            ladder += ["start_nlasvc", "renew_dhcp"]
        if level >= 3:
            ladder += ["restart_wlan", "toggle_adapter"]
        for fid in ladder:
            if self._stop.is_set():
                return
            act = repairs.get_action(fid)
            if not act:
                continue
            result = act.execute({})
            LOGGER.info("自动修复 [{}] -> {}: {}", fid, result.success, result.message[:120])
            self.on_action(fid, result)
            if fid == "toggle_adapter":
                self._last_l3_ts = time.time()
            time.sleep(3)
            if probe_quick():
                self.on_status(ST_OK, f"自动修复成功（{act.title}）")
                return
        self.on_status(ST_NO_NET, "自动修复梯度执行完毕仍未恢复")
        # 保守模式：建议（不自动执行）改配置类修复
        self.on_suggest("dns_auto", "多次自动修复无效，建议检查 DNS 配置")
        self.on_suggest("ncsi_restore", "建议核对 NCSI 探测配置")
