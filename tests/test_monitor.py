"""syskit/monitor.py 单元测试：探测快探、修复梯度状态机、后台循环（全程无真实网络/命令/线程睡眠）。

Mock 边界（配合 conftest 哨兵双保险）：
- urllib.request.urlopen 打桩返回假 204 响应（conftest 哨兵兜底漏网连接）；
- snapshot.take 打桩返回假 Snapshot（不跑 PowerShell）；
- repairs.get_action 打桩返回假动作对象（不执行任何修复命令）；
- probe_quick 在模块级替换；time.sleep 置空；monitor 的 stop/force 事件换成假 Event
  （is_set 按脚本返回、wait 只记录不阻塞），让 run() 循环确定性退出。

已修复行为（原 PROGRESS 疑似 bug，现已改源码）：
1. probe_quick 遍历全部 PROBE_204_URLS，首端点失败时继续尝试后续端点。
2. Monitor 停止事件命名为 _stop_evt（不可覆盖 Thread._stop）。
3. force_check 可中断等待（_force_check.wait）——置位立即醒来（1s 节流），
   不再等满 poll_interval；见 test_force_check_wakes_early_and_rechecks。
"""
import threading
import time

import pytest

from syskit import config, monitor as monitor_mod, settings as settings_mod
from syskit.monitor import (Monitor, ST_NO_NET, ST_OFFLINE, ST_OK, probe_quick)
from syskit.repairs import RepairResult
from syskit.snapshot import Adapter, Snapshot


# ---------------- 假数据工厂 ----------------

def make_adapter(**kw):
    defaults = dict(name="WLAN", desc="Intel(R) Wi-Fi 6 AX201", status="Up", if_index=5,
                    ipv4=["192.168.1.10"])
    defaults.update(kw)
    return Adapter(**defaults)


def make_snap(adapters=None):
    snap = Snapshot()
    snap.adapters = adapters if adapters is not None else []
    snap.ok = True
    return snap


def patch_take(monkeypatch, snap):
    monkeypatch.setattr("syskit.snapshot.take", lambda use_ps=True: snap)


def make_mon():
    seen = {"status": [], "action": [], "suggest": []}
    m = Monitor(on_status=lambda s, d: seen["status"].append((s, d)),
                on_action=lambda fid, r: seen["action"].append((fid, r)),
                on_suggest=lambda fid, why: seen["suggest"].append((fid, why)))
    return m, seen


def patch_sleep(monkeypatch):
    monkeypatch.setattr(monitor_mod.time, "sleep", lambda s: None)


class FakeEvent:
    """is_set 按 seq 返回；wait 按 wait_seq 返回（默认全 False=超时）。

    两者脚本分离：run() 里 stop.is_set 控制循环退出，force.wait 控制是否
    提前唤醒；若共用同一序列会互相抢消费导致轮次错乱。
    """

    def __init__(self, seq, wait_seq=None):
        self.seq = list(seq)
        self._wait_seq = list(wait_seq) if wait_seq is not None else [False]
        self.waits = []
        self.cleared = False
        self._flag = False

    def _next(self, pool):
        if len(pool) > 1:
            return pool.pop(0)
        return bool(pool[0]) if pool else False

    def is_set(self):
        if self._flag:
            return True
        return self._next(self.seq)

    def set(self):
        self._flag = True

    def clear(self):
        self.cleared = True
        self._flag = False

    def wait(self, timeout=None):
        self.waits.append(timeout)
        if self._flag:
            return True
        return self._next(self._wait_seq)


class FakeAction:
    """假修复动作：execute 记录入参并返回预设 RepairResult。"""

    def __init__(self, fid, title, result=None):
        self.id, self.title = fid, title
        self.result = result or RepairResult(True, "ok")
        self.executed_with = []

    def execute(self, param):
        self.executed_with.append(param)
        return self.result


class _FakeResp:
    def __init__(self, status):
        self.status = status

    def __enter__(self):
        return self

    def __exit__(self, *a):
        return False


# ---------------- probe_quick ----------------

class TestProbeQuick:
    def patch_urlopen(self, monkeypatch, behavior):
        seen = []

        def fake_open(req, timeout=None):
            seen.append((req, timeout))
            return behavior(req, timeout)
        # 探测走直连 opener（不经系统代理）：patch 的是 opener.open
        monkeypatch.setattr(monitor_mod._PROBE_OPENER, "open", fake_open)
        return seen

    def test_204_ok(self, monkeypatch):
        seen = self.patch_urlopen(monkeypatch, lambda req, t: _FakeResp(204))
        assert probe_quick(timeout=2.5) is True
        req, timeout = seen[0]
        assert req.full_url == config.PROBE_204_URLS[0]
        assert req.headers.get("User-agent") == "SysKit/1.0"   # Request 规范化首字母
        assert timeout == 2.5

    def test_non_204_false(self, monkeypatch):
        self.patch_urlopen(monkeypatch, lambda req, t: _FakeResp(200))
        assert probe_quick() is False

    def test_request_error_false(self, monkeypatch):
        def boom(req, timeout=None):
            raise OSError("unreachable")
        self.patch_urlopen(monkeypatch, boom)
        assert probe_quick() is False

    def test_fallback_to_second_endpoint(self, monkeypatch):
        """首端点故障时必须尝试后续端点，第二端点 204 即视为有网。"""
        urls = ["http://a.example/generate_204", "http://b.example/generate_204"]
        monkeypatch.setattr(config, "PROBE_204_URLS", urls)
        asked = []

        def fake(req, timeout=None):
            asked.append(req.full_url)
            if req.full_url == urls[0]:
                raise OSError("a down")
            return _FakeResp(204)
        self.patch_urlopen(monkeypatch, fake)
        assert probe_quick() is True
        assert asked == [urls[0], urls[1]]

    def test_all_endpoints_fail_false(self, monkeypatch):
        urls = ["http://a.example/generate_204", "http://b.example/generate_204"]
        monkeypatch.setattr(config, "PROBE_204_URLS", urls)
        asked = []

        def fake(req, timeout=None):
            asked.append(req.full_url)
            raise OSError("down")
        self.patch_urlopen(monkeypatch, fake)
        assert probe_quick() is False
        assert asked == urls


# ---------------- settings 映射 ----------------

class TestMonitorSettings:
    def test_defaults_from_settings_load(self):
        m, _ = make_mon()
        st = m.settings()
        assert st == {"poll_interval_sec": 30, "fail_threshold": 2, "max_level": 3,
                      "enabled": True, "cooldown_sec": 180, "level3_cooldown_sec": 300}

    def test_clamps_and_coercion(self, monkeypatch):
        """下限钳制 + 字符串/浮点安全转 int。"""
        monkeypatch.setattr(settings_mod, "load", lambda: {
            "poll_interval_sec": 5, "fail_threshold": 0, "auto_repair_max_level": "2",
            "auto_repair_enabled": 0, "cooldown_sec": "60", "level3_cooldown_sec": 45.9})
        m, _ = make_mon()
        assert m.settings() == {"poll_interval_sec": 10, "fail_threshold": 1,
                                "max_level": 2, "enabled": False,
                                "cooldown_sec": 60, "level3_cooldown_sec": 45}


# ---------------- Monitor 基础 ----------------

class TestMonitorBasics:
    def test_default_callbacks_are_noop(self):
        m = Monitor()                                  # 无回调也不应报错
        m.on_status(ST_OK, "")
        m.on_action("x", None)
        m.on_suggest("y", "z")

    def test_stop_and_force_check_set_events(self):
        m, _ = make_mon()
        assert not m._stop_evt.is_set() and not m._force_check.is_set()
        m.stop()
        m.force_check()
        assert m._stop_evt.is_set() and m._force_check.is_set()

    def test_is_daemon_thread(self):
        m, _ = make_mon()
        assert isinstance(m, threading.Thread) and m.daemon is True


ST = {"enabled": True, "fail_threshold": 2, "cooldown_sec": 180,
      "level3_cooldown_sec": 300, "max_level": 3, "poll_interval_sec": 30}


# ---------------- _once 状态机 ----------------

class TestOnce:
    def _recording_take(self, monkeypatch, snap):
        """统计 snapshot.take 调用次数：健康路径应当一次都不付那 6.4s 的 PowerShell。"""
        calls = []

        def take(use_ps=True):
            calls.append(1)
            return snap

        monkeypatch.setattr("syskit.snapshot.take", take)
        return calls

    def test_offline_no_up_adapters(self, monkeypatch):
        # 探测先于快照：本例要先让探测失败，才会走到「无活动网卡 → offline」
        monkeypatch.setattr(monitor_mod, "probe_quick", lambda: False)
        calls = self._recording_take(monkeypatch, make_snap([make_adapter(status="Down")]))
        m, seen = make_mon()
        m._fail_streak = 3
        m._once(ST)
        assert seen["status"] == [(ST_OFFLINE, "无活动网络连接")]
        assert m._fail_streak == 0                     # 无网卡视为基线，清零
        assert calls == [1]

    def test_probe_ok_skips_snapshot(self, monkeypatch):
        """探测通过即报 ok，不再抓快照：省掉每轮 6.4s PowerShell，且纯虚拟网卡（Clash
        TUN）上网时不会被误报成 offline（netstats 会把它计入断网时长）。"""
        monkeypatch.setattr(monitor_mod, "probe_quick", lambda: True)
        calls = self._recording_take(monkeypatch, make_snap([make_adapter(status="Down")]))
        m, seen = make_mon()
        m._fail_streak = 5
        m._once(ST)
        assert seen["status"] == [(ST_OK, "")]
        assert m._fail_streak == 0
        assert calls == []

    def test_ok_resets_streak(self, monkeypatch):
        patch_take(monkeypatch, make_snap([make_adapter()]))
        monkeypatch.setattr(monitor_mod, "probe_quick", lambda: True)
        m, seen = make_mon()
        m._fail_streak = 5
        m._once(ST)
        assert seen["status"] == [(ST_OK, "")]
        assert m._fail_streak == 0

    def test_fail_increments_streak_without_repair(self, monkeypatch):
        patch_take(monkeypatch, make_snap([make_adapter()]))
        monkeypatch.setattr(monitor_mod, "probe_quick", lambda: False)
        m, seen = make_mon()
        st = dict(ST, fail_threshold=5)               # 阈值内只累计不修复
        m._once(st)
        m._once(st)
        assert seen["status"] == [(ST_NO_NET, "连通性探测失败 ×1"),
                                  (ST_NO_NET, "连通性探测失败 ×2")]
        assert seen["action"] == [] and seen["suggest"] == []

    def test_streak_resets_after_recovery(self, monkeypatch):
        patch_take(monkeypatch, make_snap([make_adapter()]))
        states = iter([False, True, False])
        monkeypatch.setattr(monitor_mod, "probe_quick", lambda: next(states))
        m, seen = make_mon()
        m._once(ST)
        m._once(ST)
        m._once(ST)
        assert [s for s, _ in seen["status"]] == [ST_NO_NET, ST_OK, ST_NO_NET]
        assert seen["status"][2] == (ST_NO_NET, "连通性探测失败 ×1")

    def test_repair_triggers_at_threshold(self, monkeypatch):
        patch_take(monkeypatch, make_snap([make_adapter()]))
        monkeypatch.setattr(monitor_mod, "probe_quick", lambda: False)
        ladder_calls = []
        m, seen = make_mon()
        monkeypatch.setattr(m, "_run_ladder", lambda level: ladder_calls.append(level))
        m._once(ST)                                    # streak=1 < 2
        assert ladder_calls == []
        m._once(ST)                                    # streak=2 → 触发
        assert ladder_calls == [3]                     # _last_l3_ts=0 → 上限 max_level
        assert m._fail_streak == 0
        assert m._last_repair_ts > 0

    def test_disabled_never_repairs(self, monkeypatch):
        patch_take(monkeypatch, make_snap([make_adapter()]))
        monkeypatch.setattr(monitor_mod, "probe_quick", lambda: False)
        m, seen = make_mon()
        monkeypatch.setattr(m, "_run_ladder", lambda level: pytest.fail("不应触发修复"))
        st = dict(ST, enabled=False)
        for _ in range(5):
            m._once(st)
        assert len(seen["status"]) == 5 and seen["action"] == []

    def test_cooldown_blocks_repair(self, monkeypatch):
        patch_take(monkeypatch, make_snap([make_adapter()]))
        monkeypatch.setattr(monitor_mod, "probe_quick", lambda: False)
        m, seen = make_mon()
        monkeypatch.setattr(m, "_run_ladder", lambda level: pytest.fail("冷却中不应修复"))
        m._last_repair_ts = time.monotonic()           # 刚修过（生产为 monotonic 轴）
        m._once(ST)                                    # streak=1
        m._once(ST)                                    # 达阈值但冷却中
        assert m._fail_streak == 2                     # 未重置
        assert seen["action"] == []


# ---------------- _repair_level ----------------

class TestRepairLevel:
    def test_within_level3_cooldown_caps_at_2(self):
        m, _ = make_mon()
        # 必须种入 monotonic：种 epoch 会让断言对「轴写错」这个回归零检出
        m._last_l3_ts = time.monotonic()               # 刚做过 L3
        assert m._repair_level(dict(ST, max_level=3)) == 2

    def test_outside_cooldown_uses_max_level(self):
        m, _ = make_mon()
        m._last_l3_ts = 0.0
        assert m._repair_level(dict(ST, max_level=3)) == 3
        assert m._repair_level(dict(ST, max_level=1)) == 1


# ---------------- _run_ladder ----------------

class TestRunLadder:
    def _patch_actions(self, monkeypatch, m, seen):
        """get_action 返回按 fid 构造的假动作；返回 {fid: FakeAction}。"""
        actions = {}

        def fake_get(fid):
            if fid not in actions:
                actions[fid] = FakeAction(fid, f"标题-{fid}")
            return actions[fid]
        monkeypatch.setattr(monitor_mod.repairs, "get_action", fake_get)
        # 默认无隧道；VPN 相关用例在 test_vpn.py 单独覆盖
        from syskit import vpn as vpn_mod
        monkeypatch.setattr(vpn_mod, "should_skip_auto_repair", lambda snap=None: False)
        patch_sleep(monkeypatch)
        return actions

    def test_level1_single_action_recovers(self, monkeypatch):
        m, seen = make_mon()
        self._patch_actions(monkeypatch, m, seen)
        probes = iter([True])
        monkeypatch.setattr(monitor_mod, "probe_quick", lambda: next(probes))
        m._run_ladder(1)
        assert [fid for fid, _ in seen["action"]] == ["flush_dns"]
        assert seen["action"][0][1].success is True
        assert seen["status"] == [(ST_OK, "自动修复成功（标题-flush_dns）")]
        assert seen["suggest"] == []

    def test_level3_full_ladder_no_recovery(self, monkeypatch):
        m, seen = make_mon()
        self._patch_actions(monkeypatch, m, seen)
        monkeypatch.setattr(monitor_mod, "probe_quick", lambda: False)
        m._run_ladder(3)
        assert [fid for fid, _ in seen["action"]] == [
            "flush_dns", "start_nlasvc", "renew_dhcp", "restart_wlan", "toggle_adapter"]
        assert seen["status"] == [(ST_NO_NET, "自动修复梯度执行完毕仍未恢复")]
        assert [f for f, _ in seen["suggest"]] == ["dns_auto", "ncsi_restore"]

    def test_level2_skips_ladder3_actions(self, monkeypatch):
        m, seen = make_mon()
        self._patch_actions(monkeypatch, m, seen)
        monkeypatch.setattr(monitor_mod, "probe_quick", lambda: False)
        m._run_ladder(2)
        assert [fid for fid, _ in seen["action"]] == ["flush_dns", "start_nlasvc", "renew_dhcp"]

    def test_unknown_action_id_skipped(self, monkeypatch):
        """get_action 返回 None（动作不存在）→ continue 而非崩溃。"""
        m, seen = make_mon()
        monkeypatch.setattr(monitor_mod.repairs, "get_action", lambda fid: None)
        patch_sleep(monkeypatch)
        monkeypatch.setattr(monitor_mod, "probe_quick", lambda: False)
        m._run_ladder(3)
        assert seen["action"] == []
        assert seen["status"] == [(ST_NO_NET, "自动修复梯度执行完毕仍未恢复")]

    def test_stop_mid_ladder_aborts_silently(self, monkeypatch):
        m, seen = make_mon()
        m.stop()
        executed = []
        monkeypatch.setattr(monitor_mod.repairs, "get_action",
                            lambda fid: executed.append(fid) or FakeAction(fid, fid))
        patch_sleep(monkeypatch)
        m._run_ladder(3)
        assert executed == []                          # 停止后一条都不执行
        assert seen["status"] == [] and seen["suggest"] == []

    def test_recovery_mid_ladder_stops_remaining(self, monkeypatch):
        m, seen = make_mon()
        self._patch_actions(monkeypatch, m, seen)
        n = {"i": 0}

        def probe():
            n["i"] += 1
            return n["i"] >= 2                         # 第 2 个动作修好后恢复
        monkeypatch.setattr(monitor_mod, "probe_quick", probe)
        m._run_ladder(3)
        assert [fid for fid, _ in seen["action"]] == ["flush_dns", "start_nlasvc"]
        assert seen["status"] == [(ST_OK, "自动修复成功（标题-start_nlasvc）")]

    def test_toggle_adapter_sets_l3_timestamp(self, monkeypatch):
        m, seen = make_mon()
        actions = self._patch_actions(monkeypatch, m, seen)
        monkeypatch.setattr(monitor_mod, "probe_quick", lambda: False)
        assert m._last_l3_ts == 0.0
        m._run_ladder(3)
        assert "toggle_adapter" in actions
        assert m._last_l3_ts > 0
        # 之后 _repair_level 进入 L3 冷却 → 上限 2
        assert m._repair_level(dict(ST, max_level=3)) == 2

    def test_toggle_adapter_l3_timestamp_is_monotonic(self, monkeypatch):
        """L3 时间戳必须写 monotonic 轴：曾误写 epoch 秒，(monotonic - ts) 变
        巨负 → 冷却恒命中，本进程内 L3 永久锁死（自动修复静默降级 L2）。"""
        class FakeTime:
            mono = 5000.0
            epoch = 1_789_000_000.0

            @classmethod
            def monotonic(cls):
                return cls.mono

            @classmethod
            def time(cls):
                return cls.epoch

            @staticmethod
            def sleep(_s):
                pass

        monkeypatch.setattr(monitor_mod, "time", FakeTime)
        m, seen = make_mon()
        self._patch_actions(monkeypatch, m, seen)
        monkeypatch.setattr(monitor_mod, "probe_quick", lambda: False)
        m._run_ladder(3)
        assert m._last_l3_ts == FakeTime.mono       # 是 monotonic 值，不是 epoch

    def test_execute_receives_empty_param(self, monkeypatch):
        m, seen = make_mon()
        actions = self._patch_actions(monkeypatch, m, seen)
        monkeypatch.setattr(monitor_mod, "probe_quick", lambda: True)
        m._run_ladder(1)
        assert actions["flush_dns"].executed_with == [{}]

    def test_failed_action_still_reported(self, monkeypatch):
        m, seen = make_mon()
        acts = {}
        acts["flush_dns"] = FakeAction("flush_dns", "刷DNS",
                                       RepairResult(False, "拒绝访问"))
        monkeypatch.setattr(monitor_mod.repairs, "get_action",
                            lambda fid: acts.get(fid))
        patch_sleep(monkeypatch)
        monkeypatch.setattr(monitor_mod, "probe_quick", lambda: False)
        m._run_ladder(1)
        fid, result = seen["action"][0]
        assert fid == "flush_dns" and result.success is False


# ---------------- run 主循环 ----------------

class TestRun:
    def _prepare(self, monkeypatch, m, stop_seq, force_wait_seq,
                 stop_wait_seq=None):
        """stop_seq → is_set 脚本；force_wait_seq → force.wait 返回值脚本。"""
        sweep_calls = []
        once_calls = []
        m._stop_evt = FakeEvent(stop_seq, wait_seq=stop_wait_seq or [False])
        m._force_check = FakeEvent([False], wait_seq=force_wait_seq)
        import syskit.logger as logger_mod
        monkeypatch.setattr(logger_mod, "sweep_old_logs",
                            lambda *a, **kw: sweep_calls.append(1) or 0)
        monkeypatch.setattr(m, "_once", lambda st: once_calls.append(st))
        return sweep_calls, once_calls

    def test_single_pass_then_stop(self, monkeypatch):
        m, _ = make_mon()
        # force.wait 超时(False)，随后 stop.is_set 置位退出
        sweep, once = self._prepare(monkeypatch, m, [False, True], [False])
        m.run()
        assert len(once) == 1
        assert sweep == [1]                            # 每天首圈清扫
        assert m._force_check.waits == [30]            # 以 poll_interval 作可中断等待
        assert m._stop_evt.waits == []

    def test_force_check_wakes_early_and_rechecks(self, monkeypatch):
        """force 置位时不必等满剩余 poll：wait 提前返回 True，短暂节流后再探测。"""
        m, _ = make_mon()
        # force.wait: 第一次 True（提前唤醒），第二次 False（超时）
        # stop.is_set: 两轮 _once 后退出；stop.wait(1.0) 未停
        sweep, once = self._prepare(
            monkeypatch, m,
            stop_seq=[False, False, True],
            force_wait_seq=[True, False],
            stop_wait_seq=[False],
        )
        m.run()
        assert len(once) == 2
        assert m._force_check.waits == [30, 30]        # 两次都以 poll 为超时上限
        assert m._stop_evt.waits == [1.0]              # 唤醒后仅节流 1s，非再等满 poll
        assert m._force_check.cleared is True
        assert sweep == [1]                            # 同一天只清扫一次

    def test_force_then_stop_during_throttle(self, monkeypatch):
        """force 唤醒后 1s 节流期间收到 stop → 不再探测下一轮。"""
        m, _ = make_mon()
        sweep, once = self._prepare(
            monkeypatch, m,
            stop_seq=[False],
            force_wait_seq=[True],
            stop_wait_seq=[True],                      # 节流 wait 立即“停”
        )
        m.run()
        assert len(once) == 1
        assert m._stop_evt.waits == [1.0]

    def test_once_exception_does_not_kill_loop(self, monkeypatch):
        """_once 抛异常被兜底记录，循环继续到 stop。"""
        m, _ = make_mon()
        sweep, once = self._prepare(monkeypatch, m, [False, False, True], [False])
        calls = {"n": 0}

        def flaky(st):
            calls["n"] += 1
            if calls["n"] == 1:
                raise RuntimeError("mock boom")
            once.append(st)
        monkeypatch.setattr(m, "_once", flaky)
        m.run()                                        # 不应向外抛异常
        assert calls["n"] == 2

    def test_settings_dict_passed_to_once(self, monkeypatch):
        m, _ = make_mon()
        once = self._prepare(monkeypatch, m, [False, True], [False])[1]
        m.run()
        assert once == [m.settings()]
