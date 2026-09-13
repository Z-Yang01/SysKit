"""后台监控线程：周期探测 + 保守自动修复梯度。只执行安全类动作，改配置类仅建议。"""
import threading
import time
import urllib.request

from . import config, netcheck, repairs, settings
from .logger import LOGGER

# 状态码
ST_OK, ST_NO_NET, ST_NCSI_FALSE, ST_OFFLINE = "ok", "no_net", "ncsi_false", "offline"


def probe_quick(timeout: float = 4.0) -> bool:
    """轻量探测：任一 204 端点可达即有网。遍历全部端点，避免单点故障误报断网。"""
    for url in config.PROBE_204_URLS:
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
        # 不可命名为 _stop：会覆盖 threading.Thread._stop()，导致 join() 抛 TypeError
        self._stop_evt = threading.Event()
        self._force_check = threading.Event()
        self.last_results = []
        self._fail_streak = 0
        self._last_repair_ts = 0.0
        self._last_l3_ts = 0.0

    def stop(self):
        # 同时置 force：run() 的可中断等待只监听 _force_check，
        # 否则 stop 后仍要等满 poll_interval（默认 30s）才能醒来
        self._stop_evt.set()
        self._force_check.set()

    def force_check(self):
        self._force_check.set()

    def run(self):
        from .logger import sweep_old_logs
        last_sweep_day = ""
        while not self._stop_evt.is_set():
            st = self.settings()
            try:
                today = time.strftime("%Y-%m-%d")
                if today != last_sweep_day:
                    sweep_old_logs()
                    last_sweep_day = today
                self._once(st)
            except Exception:
                LOGGER.exception("监控循环异常")
            # 用 force 事件做可中断等待：force 置位立即醒来（跳过剩余 poll），
            # 再短暂节流后进入下一轮 _once——手动刷新不必等满 poll_interval。
            if self._force_check.wait(timeout=st["poll_interval_sec"]):
                self._force_check.clear()
                if self._stop_evt.wait(1.0):
                    break
                continue
            # 超时：回到循环顶部再查 stop / 再次探测

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
        now = time.monotonic()
        if now - self._last_repair_ts < st["cooldown_sec"]:
            return

        self._fail_streak = 0
        self._last_repair_ts = now
        level = self._repair_level(st)
        if level >= 3:
            # L3 限频记在梯度级别：restart_wlan 修复后探测恢复（早退）也计入冷却，
            # 避免反复全量 L3（重启 WlanSvc + 断合网卡）
            self._last_l3_ts = now
        LOGGER.info("自动修复触发, 级别 L%d", level)
        self._run_ladder(level)

    def _repair_level(self, st) -> int:
        """根据距上次 L3 的时间决定本轮修复上限，避免频繁重启网卡。"""
        if time.monotonic() - self._last_l3_ts < st["level3_cooldown_sec"]:
            return 2
        return st["max_level"]

    def _run_ladder(self, level: int):
        from . import vpn as vpn_mod
        ladder = ["flush_dns"]
        if level >= 2:
            ladder += ["start_nlasvc", "renew_dhcp"]
        if level >= 3:
            ladder += ["restart_wlan", "toggle_adapter"]
        # VPN/TUN 在用：跳过易断隧道的动作（续租/重启 WLAN/网卡开关）
        try:
            skip_vpn = vpn_mod.should_skip_auto_repair()
        except Exception:
            skip_vpn = False  # 快照失败时不停用自动修复，仅记日志
            LOGGER.warning("VPN 护栏检测失败，本轮按无隧道处理", exc_info=True)
        if skip_vpn:
            skipped = [x for x in ladder if x in ("renew_dhcp", "restart_wlan", "toggle_adapter")]
            ladder = [x for x in ladder if x not in skipped]
            LOGGER.warning("检测到 VPN/隧道在线，自动修复跳过: {}", ", ".join(skipped) or "无")
            self.on_suggest("flush_dns", vpn_mod.auto_repair_guard_note())
        for fid in ladder:
            if self._stop_evt.is_set():
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
