"""syskit/repairs.py 单元测试：只验证「构造了什么命令」，绝不真实执行。

红线遵守：
- proc.run / proc.run_ps 由本文件的 Recorder 记录器 fake 替换（覆盖 conftest 哨兵，
  只记录收到的命令并返回假 CmdResult，不产生任何真实进程）；
- winreg 写操作由 reg_spy 记录；winregread 读操作按测试需要 monkeypatch；
- snapshot.take / proxy.disable_proxy / glob.glob 等重 I/O 依赖一律 monkeypatch；
- 备份断言只看 config.BACKUP_DIR（conftest 已重定向到 pytest 临时目录）下的文件。
"""
import os
import time
import winreg
from pathlib import Path
from types import SimpleNamespace

import pytest

from syskit import config, repairs

# README 风险分级表（README.md「修复动作与风险级别」一节）声称的三个梯度
# restart_wlan / toggle_adapter 补54 起 RISK_CONFIG（与 monitor L3 定性对齐）
SAFE_IDS = {"flush_dns", "renew_dhcp", "start_nlasvc",
            "restart_services", "restore_point"}
CONFIG_IDS = {"fix_nlasvc", "dns_auto", "dns_preset", "ncsi_restore", "ncsi_disable",
              "power_off", "del_persistent_route", "proxy_off",
              "mac_random_off", "mac_random_on",
              "restart_wlan", "toggle_adapter"}
AGGRESSIVE_IDS = {"winsock_reset", "ipstack_reset"}


@pytest.fixture(autouse=True)
def _reset_mac_scan_cache():
    """_MAC_SCAN 是模块级缓存：不逐例清零会跨用例泄漏上一个用例的假 profile 表。"""
    repairs._MAC_SCAN.update(stamp=None, profiles={}, checked=0.0)
    yield


# ---------------- 记录器 fake：记录命令、按规则返回假 CmdResult ----------------

def _cmd_str(cmd):
    return " ".join(cmd) if isinstance(cmd, list) else str(cmd)


class Recorder:
    """替换 proc.run / proc.run_ps：记录每次调用，返回预设/默认的成功 CmdResult。"""

    def __init__(self, make_result, events=None):
        self._make = make_result
        self.calls = []      # (kind, cmd, timeout, CmdResult)
        self._rules = []     # (predicate(cmd) -> bool, result_kwargs)
        self.events = events if events is not None else []

    def respond(self, pred, code=0, stdout="", stderr=""):
        self._rules.append((pred, dict(code=code, stdout=stdout, stderr=stderr)))

    def respond_sub(self, *subs, **res_kwargs):
        """当命令文本（list 以空格连接）包含全部子串时返回预设结果。"""
        def pred(cmd):
            s = _cmd_str(cmd)
            return all(x in s for x in subs)
        self.respond(pred, **res_kwargs)

    def _dispatch(self, kind, cmd, timeout):
        for pred, res_kwargs in self._rules:
            if pred(cmd):
                res = self._make(**res_kwargs)
                break
        else:
            res = self._make()
        self.calls.append((kind, cmd, timeout, res))
        self.events.append((kind, _cmd_str(cmd)))
        return res

    def run(self, cmd, timeout=20):
        return self._dispatch("run", cmd, timeout)

    def run_ps(self, script, timeout=40, resident=False):
        return self._dispatch("ps", script, timeout)

    @property
    def cmds(self):
        """所有 proc.run 收到的命令列表（原始 list）。"""
        return [c[1] for c in self.calls if c[0] == "run"]

    @property
    def joined(self):
        """所有 proc.run 命令的空格连接文本，便于子串断言。"""
        return [_cmd_str(c[1]) for c in self.calls if c[0] == "run"]

    @property
    def ps_scripts(self):
        return [c[1] for c in self.calls if c[0] == "ps"]

    def find(self, substr, kind="run"):
        for c in self.calls:
            if c[0] == kind and substr in _cmd_str(c[1]):
                return c
        return None


@pytest.fixture(autouse=True)
def _no_sleep(monkeypatch):
    """动作里的 time.sleep(1~3s) 全部跳过，加速测试。"""
    monkeypatch.setattr(time, "sleep", lambda _s: None)


@pytest.fixture
def events():
    """时间线：recorder 与 reg_spy 共用，用于断言「备份先于修改」。"""
    return []


@pytest.fixture
def recorder(monkeypatch, fake_cmd_result, events):
    """覆盖 conftest 哨兵：proc.run/run_ps 改为记录器。"""
    from syskit import proc
    rec = Recorder(fake_cmd_result, events)
    monkeypatch.setattr(proc, "run", rec.run)
    monkeypatch.setattr(proc, "run_ps", rec.run_ps)
    return rec


class _FakeKey:
    def __init__(self, path):
        self.path = path

    def Close(self):
        pass


@pytest.fixture
def reg_spy(monkeypatch, events):
    """覆盖 conftest 的 winreg.SetValueEx 哨兵：记录全部写操作，不碰真实注册表。"""
    spy = SimpleNamespace(opened=[], written=[], fail_paths=[])
    hive_names = {winreg.HKEY_LOCAL_MACHINE: "HKLM", winreg.HKEY_CURRENT_USER: "HKCU"}

    def fake_open(hive, sub, reserved=0, access=winreg.KEY_READ):
        key = _FakeKey(f"{hive_names.get(hive, 'HKLM')}\\{sub}")
        spy.opened.append((key.path, access))
        events.append(("regopen", key.path))
        return key

    def fake_set(key, name, reserved, typ, data):
        for frag in spy.fail_paths:
            if frag in key.path:
                raise OSError(f"模拟写入失败: {key.path}")
        spy.written.append((key.path, name, typ, data))
        events.append(("regwrite", key.path, name, typ, data))

    monkeypatch.setattr(winreg, "OpenKey", fake_open)
    monkeypatch.setattr(winreg, "SetValueEx", fake_set)
    return spy


# ---------------- 公共小工具 ----------------

def _snap(persistent=(), defaults=(), adapters=()):
    from syskit.snapshot import Snapshot
    return Snapshot(persistent_routes=list(persistent),
                    default_routes=list(defaults),
                    adapters=list(adapters))


def _patch_snapshot(monkeypatch, *snaps):
    """把 snapshot.take 替换为依次返回给定快照（动作内延迟 import，按属性查找）。"""
    from syskit import snapshot
    queue = list(snaps)
    monkeypatch.setattr(snapshot, "take", lambda use_ps=True: queue.pop(0))


# ---------------- 注册表完整性 ----------------

def test_action_ids_unique_and_registry_consistent():
    ids = repairs.ACTION_IDS
    assert len(ids) == len(set(ids))                       # id 唯一
    assert len(ids) == len(repairs.ACTIONS) == len(repairs.ACTION_MAP)
    assert set(ids) == SAFE_IDS | CONFIG_IDS | AGGRESSIVE_IDS


@pytest.mark.parametrize("aid", repairs.ACTION_IDS)
def test_action_metadata_complete(aid):
    a = repairs.get_action(aid)
    assert a is not None
    assert a.title.strip()
    assert a.description.strip()
    assert a.restore_hint.strip()          # 空串会被兜底为 "无"，也视为非空
    assert a.brief.strip()
    assert a.risk in (repairs.RISK_SAFE, repairs.RISK_CONFIG, repairs.RISK_AGGRESSIVE)
    assert a.risk in repairs.RISK_LABEL    # 每个风险级都有中文标签


@pytest.mark.parametrize("aid", repairs.ACTION_IDS)
def test_need_reboot_only_on_stack_resets(aid):
    """need_reboot 只允许出现在 winsock_reset / ipstack_reset 这类重启生效的动作上。"""
    a = repairs.get_action(aid)
    if aid in AGGRESSIVE_IDS:
        assert a.need_reboot is True
    else:
        assert a.need_reboot is False


def test_get_action_unknown_returns_none():
    assert repairs.get_action("no_such_action") is None


def test_repair_result_defaults():
    r = repairs.RepairResult(True, "ok")
    assert r.need_reboot is False
    assert r.output == ""


def test_base_action_run_not_implemented_returns_failure():
    a = repairs.RepairAction("base", "标题", repairs.RISK_SAFE, "描述", "")
    assert a.restore_hint == "无"          # 空 restore_hint 兜底
    assert a.brief == "描述"
    r = a.execute({})
    assert r.success is False
    assert "执行异常" in r.message


def test_execute_swallows_exceptions():
    class _Boom(repairs.RepairAction):
        def __init__(self):
            super().__init__("boom", "b", repairs.RISK_SAFE, "d", "r")

        def _run(self, param):
            raise ValueError("kaboom")

    r = _Boom().execute(None)              # param=None 应兜底为 {}
    assert r.success is False
    assert "kaboom" in r.message


# ---------------- 风险分级（对照 README 安全梯度声明） ----------------

def test_risk_partition_matches_readme():
    by_risk = {}
    for a in repairs.ACTIONS:
        by_risk.setdefault(a.risk, set()).add(a.id)
    assert by_risk[repairs.RISK_SAFE] == SAFE_IDS
    assert by_risk[repairs.RISK_CONFIG] == CONFIG_IDS
    assert by_risk[repairs.RISK_AGGRESSIVE] == AGGRESSIVE_IDS


def test_key_action_risk_levels():
    assert repairs.get_action("ncsi_restore").risk == repairs.RISK_CONFIG
    assert repairs.get_action("ncsi_disable").risk == repairs.RISK_CONFIG
    assert repairs.get_action("fix_nlasvc").risk == repairs.RISK_CONFIG
    assert repairs.get_action("winsock_reset").risk == repairs.RISK_AGGRESSIVE
    assert repairs.get_action("ipstack_reset").risk == repairs.RISK_AGGRESSIVE
    for aid in ("flush_dns", "renew_dhcp", "start_nlasvc",
                "restart_services", "restore_point"):
        assert repairs.get_action(aid).risk == repairs.RISK_SAFE
    # 补54：两项断网类动作升为改配置档——与 monitor 自动修复把它们列为最高危
    # L3（300s 冷却）的定性对齐；组合内逐项确认、无管理员跳过
    for aid in ("restart_wlan", "toggle_adapter"):
        assert repairs.get_action(aid).risk == repairs.RISK_CONFIG


def test_monitor_ladder_only_uses_safe_actions(monkeypatch):
    """monitor 自动修复梯度（L3 全量）= 安全类 + 两项显式白名单的断网类动作；
    其余改配置类仅作建议。"""
    from syskit import monitor
    real_get = repairs.get_action
    used = []

    class _FakeAct:
        title = "fake"

        def execute(self, param):
            return repairs.RepairResult(True, "ok")

    monkeypatch.setattr(repairs, "get_action",
                        lambda aid: (used.append(aid), _FakeAct())[1])
    monkeypatch.setattr(monitor, "probe_quick", lambda timeout=4.0: False)
    suggests = []
    m = monitor.Monitor(on_suggest=lambda fid, reason: suggests.append(fid))
    m._run_ladder(3)
    assert used == ["flush_dns", "start_nlasvc", "renew_dhcp",
                    "restart_wlan", "toggle_adapter"]
    ladder_config_ids = {"restart_wlan", "toggle_adapter"}   # 唯一自动执行的改配置类
    for a in used:
        risk = real_get(a).risk
        if a in ladder_config_ids:
            assert risk == repairs.RISK_CONFIG
        else:
            assert risk == repairs.RISK_SAFE
    # 改配置类只出现在建议或上述白名单里，绝不越界自动执行
    assert set(suggests) == {"dns_auto", "ncsi_restore"}
    assert all(real_get(a).risk == repairs.RISK_CONFIG for a in suggests)


def test_monitor_ladder_level1_only_flush_dns(monkeypatch):
    from syskit import monitor
    used = []

    class _FakeAct:
        title = "fake"

        def execute(self, param):
            return repairs.RepairResult(True, "ok")

    monkeypatch.setattr(repairs, "get_action",
                        lambda aid: (used.append(aid), _FakeAct())[1])
    monkeypatch.setattr(monitor, "probe_quick", lambda timeout=4.0: False)
    monitor.Monitor()._run_ladder(1)
    assert used == ["flush_dns"]


def test_combos_reference_existing_ids_with_risk_split():
    for label, ids in repairs.COMBOS.values():
        assert label
        for aid in ids:
            assert repairs.get_action(aid) is not None
    # 补54 起 fast 组合含改配置档的 restart_wlan：管理员下逐项确认，无管理员跳过
    fast_risks = {repairs.get_action(a).risk for a in repairs.COMBOS["fast"][1]}
    assert fast_risks <= {repairs.RISK_SAFE, repairs.RISK_CONFIG}
    assert repairs.RISK_CONFIG in fast_risks
    # 深度/重置组合首位是系统还原点安全网（SAFE，无需确认即执行）
    assert repairs.COMBOS["full"][1][0] == "restore_point"
    assert repairs.COMBOS["stack"][1][0] == "restore_point"
    assert repairs.COMBOS["stack"][1][1:] == ["winsock_reset", "ipstack_reset"]
    assert all(repairs.get_action(a).risk == repairs.RISK_AGGRESSIVE
               for a in repairs.COMBOS["stack"][1][1:])   # 首位是 SAFE 的还原点
    # 一键组合（手动按钮）里深度修复也不含强力级动作
    assert not (set(repairs.COMBOS["full"][1]) & AGGRESSIVE_IDS)


# ---------------- 安全类：命令构造 ----------------

def test_flush_dns(recorder):
    r = repairs.get_action("flush_dns").execute({})
    assert r.success is True
    assert recorder.cmds == [["ipconfig", "/flushdns"]]


def test_renew_dhcp_release_then_renew(recorder):
    r = repairs.get_action("renew_dhcp").execute({})
    assert r.success is True
    assert recorder.cmds == [["ipconfig", "/release"], ["ipconfig", "/renew"]]
    assert "已重新获取 IP" in r.message


def test_start_nlasvc_success(recorder):
    recorder.respond_sub("sc query NlaSvc", stdout="STATE : 4 RUNNING")
    r = repairs.get_action("start_nlasvc").execute({})
    assert r.success is True
    assert recorder.cmds == [["sc", "start", "NlaSvc"], ["sc", "query", "NlaSvc"]]
    assert "运行中" in r.message


def test_start_nlasvc_already_running_1056(recorder):
    """sc start 返回 1056（服务已在运行）也应视为成功。"""
    recorder.respond_sub("sc start NlaSvc", code=1056, stdout="错误 1056: 服务已运行")
    recorder.respond_sub("sc query NlaSvc", stdout="RUNNING")
    r = repairs.get_action("start_nlasvc").execute({})
    assert r.success is True


def test_start_nlasvc_query_says_stopped(recorder):
    recorder.respond_sub("sc query NlaSvc", stdout="STATE : 1 STOPPED")
    r = repairs.get_action("start_nlasvc").execute({})
    assert r.success is False
    assert "未运行" in r.message


def test_start_nlasvc_stray_digit4_not_running(recorder):
    """sc query 输出里含数字 4 的字段（如 WIN32_EXIT_CODE）不得误判为运行中。"""
    recorder.respond_sub("sc query NlaSvc",
                         stdout="STATE : 1 STOPPED\nWIN32_EXIT_CODE : 4242\n")
    r = repairs.get_action("start_nlasvc").execute({})
    assert r.success is False and "未运行" in r.message


def test_dns_auto_backup_failure_aborts_without_changes(recorder, monkeypatch):
    """备份失败（返回 ""）→ 不清静态 DNS：原值是用户唯一的还原凭据。"""
    monkeypatch.setattr(repairs, "_backup_dns", lambda adapter: "")
    r = repairs.get_action("dns_auto").execute({"adapter": "WLAN"})
    assert r.success is False and "备份" in r.message
    assert not any("set dnsservers" in _cmd_str(c) for c in recorder.cmds)
    assert not any("flushdns" in _cmd_str(c) for c in recorder.cmds)


def test_dns_preset_backup_failure_aborts_without_changes(recorder, monkeypatch):
    monkeypatch.setattr(repairs, "_backup_dns", lambda adapter: "")
    r = repairs.get_action("dns_preset").execute({"adapter": "WLAN"})
    assert r.success is False and "备份" in r.message
    assert not any("set dnsservers" in _cmd_str(c) for c in recorder.cmds)


def test_dns_auto_v6_failure_reports_failure(recorder):
    """v6 恢复失败 → 整体失败并带 v6 详情（此前 v6 结果被丢弃、恒报成功）。"""
    recorder.respond_sub("show dnsservers", stdout="静态配置的 DNS 服务器:\n    8.8.8.8")
    recorder.respond_sub("ipv6 set dnsservers", code=1, stdout="v6失败详情")
    r = repairs.get_action("dns_auto").execute({"adapter": "WLAN"})
    assert r.success is False
    assert "v6" in r.message and "v6失败详情" in r.message


def test_restart_wlan_reconnects_current_ssid(recorder):
    recorder.respond_sub("netsh wlan show interfaces",
                         stdout="    SSID                  : HomeWifi\r\n"
                                "    BSSID                 : aa:bb:cc:dd:ee:ff")
    r = repairs.get_action("restart_wlan").execute({})
    assert r.success is True
    assert recorder.cmds == [
        ["netsh", "wlan", "show", "interfaces"],
        ["net", "stop", "WlanSvc"],
        ["net", "start", "WlanSvc"],
        ["netsh", "wlan", "connect", "name=HomeWifi"],
    ]
    assert "HomeWifi" in r.message


def test_restart_wlan_without_ssid_skips_connect(recorder):
    r = repairs.get_action("restart_wlan").execute({})
    assert r.success is True
    assert recorder.cmds == [["netsh", "wlan", "show", "interfaces"],
                             ["net", "stop", "WlanSvc"],
                             ["net", "start", "WlanSvc"]]
    assert recorder.find("wlan connect") is None


def test_restart_wlan_start_failure(recorder):
    recorder.respond_sub("net start WlanSvc", code=1, stderr="服务无法启动")
    r = repairs.get_action("restart_wlan").execute({})
    assert r.success is False
    assert "WlanSvc 启动失败" in r.message


def test_toggle_adapter_explicit_name(recorder):
    r = repairs.get_action("toggle_adapter").execute({"adapter": "以太网 3"})
    assert r.success is True
    assert recorder.cmds == [
        ["netsh", "interface", "set", "interface", "name=以太网 3", "disable"],
        ["netsh", "interface", "set", "interface", "name=以太网 3", "enable"],
    ]
    assert "以太网 3" in r.message


def test_toggle_adapter_no_candidate_fails_without_commands(recorder):
    """快照里没有可用网卡时直接失败，不构造任何 netsh 命令。"""
    r = repairs.get_action("toggle_adapter").execute({})
    assert r.success is False
    assert "未指定网卡且无法自动选择" in r.message
    assert recorder.cmds == []


def test_pick_adapter_prefers_wlan(monkeypatch):
    from syskit.snapshot import Adapter
    adapters = [Adapter(name="以太网", virtual=False, status="Up"),
                Adapter(name="WLAN", virtual=False, status="Up")]
    _patch_snapshot(monkeypatch, _snap(adapters=adapters))
    assert repairs._pick_adapter() == "WLAN"


def test_pick_adapter_falls_back_to_first_physical_down(monkeypatch):
    from syskit.snapshot import Adapter
    adapters = [Adapter(name="WAN Miniport IP", desc="WAN Miniport (IP)",
                        virtual=False, status="Up"),
                Adapter(name="以太网 2", desc="Realtek", virtual=False,
                        status="Disconnected"),
                Adapter(name="vEthernet", desc="Hyper-V Virtual",
                        virtual=True, status="Up")]
    _patch_snapshot(monkeypatch, _snap(adapters=adapters))
    assert repairs._pick_adapter() == "以太网 2"


def test_pick_adapter_prefers_connected_physical(monkeypatch):
    """无无线命名时：选「在线」的物理网卡（旧逻辑选未接线的网卡还报成功）。"""
    from syskit.snapshot import Adapter
    adapters = [Adapter(name="以太网", desc="Realtek", virtual=False, status="Up")]
    _patch_snapshot(monkeypatch, _snap(adapters=adapters))
    assert repairs._pick_adapter() == "以太网"

    # 英文系统的无线网卡名 "Wi-Fi" 同样被第一优先识别
    adapters_b = [Adapter(name="Wi-Fi", desc="Intel AX201", virtual=False, status="Up"),
                  Adapter(name="以太网", desc="Realtek", virtual=False, status="Up")]
    _patch_snapshot(monkeypatch, _snap(adapters=adapters_b))
    assert repairs._pick_adapter() == "Wi-Fi"


def test_pick_adapter_empty_when_no_physical(monkeypatch):
    from syskit.snapshot import Adapter
    adapters = [Adapter(name="vEthernet (WSL)", desc="Hyper-V Virtual",
                        virtual=True, status="Up")]
    _patch_snapshot(monkeypatch, _snap(adapters=adapters))
    assert repairs._pick_adapter() == ""


def test_restart_services(recorder):
    """Dnscache 是受保护服务（net stop 恒拒绝）：用 flushdns 达成等效语义。"""
    r = repairs.get_action("restart_services").execute({})
    assert r.success is True
    assert recorder.cmds == [["net", "stop", "Dhcp"], ["net", "start", "Dhcp"],
                             ["ipconfig", "/flushdns"]]
    assert "Dhcp:OK" in r.message and "DNS缓存:OK" in r.message


# ---------------- 改配置类：命令构造 + 改前备份 ----------------

def test_fix_nlasvc_sets_auto_with_bare_args(recorder):
    recorder.respond_sub("sc query NlaSvc", stdout="RUNNING")
    r = repairs.get_action("fix_nlasvc").execute({})
    assert r.success is True
    # "start= auto" 必须是两个独立裸参数（sc 按原始命令行解析，不能带引号）
    assert recorder.cmds[0] == ["sc", "config", "NlaSvc", "start=", "auto"]
    assert recorder.cmds[1] == ["sc", "start", "NlaSvc"]
    assert recorder.cmds[2] == ["sc", "query", "NlaSvc"]
    assert "已设为自动启动" in r.message


def test_fix_nlasvc_config_failure_aborts(recorder):
    recorder.respond_sub("sc config NlaSvc", code=1, stderr="Access is denied")
    r = repairs.get_action("fix_nlasvc").execute({})
    assert r.success is False
    assert "sc config 失败" in r.message
    assert recorder.find("sc start") is None     # 配置失败后不再尝试启动


def test_dns_auto_restores_dhcp_and_backs_up_first(recorder, events):
    recorder.respond_sub("show dnsservers", stdout="静态配置的 DNS 服务器:\n    8.8.8.8")
    r = repairs.get_action("dns_auto").execute({"adapter": "WLAN"})
    assert r.success is True
    assert recorder.cmds == [
        ["netsh", "interface", "ipv4", "show", "dnsservers", "WLAN"],   # 备份读取（v4）
        ["netsh", "interface", "ipv6", "show", "dnsservers", "WLAN"],   # 备份读取（v6，dns_auto 同清两者）
        ["netsh", "interface", "ipv4", "set", "dnsservers", "name=WLAN", "source=dhcp"],
        ["netsh", "interface", "ipv6", "set", "dnsservers", "name=WLAN", "source=dhcp"],
        ["ipconfig", "/flushdns"],
    ]
    # 备份（show）先于修改（set）
    seq = [e[1] for e in events]
    assert seq[0].startswith("netsh interface ipv4 show dnsservers")
    assert seq[1].startswith("netsh interface ipv6 show dnsservers")
    assert "set dnsservers" in seq[2]
    # 备份 note 落在重定向后的 BACKUP_DIR
    notes = list(Path(config.BACKUP_DIR).rglob("dns_backup_*.txt"))
    assert notes and "8.8.8.8" in notes[0].read_text(encoding="utf-8")


def test_dns_auto_no_static_adapter(recorder):
    r = repairs.get_action("dns_auto").execute({})
    assert r.success is False
    assert "未找到配置了静态 DNS 的网卡" in r.message
    assert recorder.cmds == []


def test_static_dns_adapter_detection(monkeypatch, recorder):
    from syskit.snapshot import Adapter
    adapters = [Adapter(name="以太网", virtual=False, status="Up", dns=["8.8.8.8"])]
    _patch_snapshot(monkeypatch, _snap(adapters=adapters))
    recorder.respond_sub("show dnsservers", stdout="静态配置的 DNS 服务器")
    assert repairs._static_dns_adapter() == "以太网"


def test_static_dns_adapter_wlan_fallback(monkeypatch, recorder):
    """无 up 且静态的网卡时，兜底选有 DNS 记录的 WLAN（未连接也可能残留静态 DNS）。"""
    from syskit.snapshot import Adapter
    adapters = [Adapter(name="以太网", virtual=False, status="Up", dns=[]),
                Adapter(name="WLAN", virtual=False, status="Down", dns=["192.168.1.1"])]
    _patch_snapshot(monkeypatch, _snap(adapters=adapters))
    assert repairs._static_dns_adapter() == "WLAN"
    assert recorder.cmds == []      # 兜底分支不需要 netsh 探测


def test_dns_preset_applies_configured_preset_with_backup(recorder):
    recorder.respond_sub("show dnsservers", stdout="静态配置的 DNS 服务器:\n    8.8.8.8")
    primary, secondary = config.DNS_PRESETS["腾讯"]
    r = repairs.get_action("dns_preset").execute({"adapter": "WLAN", "preset": "腾讯"})
    assert r.success is True
    assert (primary, secondary) == ("119.29.29.29", "182.254.116.116")   # 预设值来自 config
    assert recorder.cmds == [
        ["netsh", "interface", "ipv4", "show", "dnsservers", "WLAN"],    # 备份读取（v4）
        ["netsh", "interface", "ipv6", "show", "dnsservers", "WLAN"],    # 备份读取（v6）
        ["netsh", "interface", "ipv4", "set", "dnsservers",
         "name=WLAN", "static", primary, "primary"],
        ["netsh", "interface", "ipv4", "add", "dnsservers",
         "name=WLAN", secondary, "index=2"],
        ["netsh", "interface", "ipv6", "set", "dnsservers",
         "name=WLAN", "source=dhcp"],                                    # v6 残留同清
        ["ipconfig", "/flushdns"],
    ]
    assert f"DNS → {primary}" in r.message and "v6 已恢复自动获取" in r.message
    notes = list(Path(config.BACKUP_DIR).rglob("dns_backup_*.txt"))
    assert notes


def test_dns_preset_default_preset_is_ali_from_settings(recorder):
    """不传 preset 时用 settings 的 dns_preset（conftest 已隔离 → 默认"阿里"）。"""
    from syskit import settings
    assert settings.load()["dns_preset"] == "阿里"
    r = repairs.get_action("dns_preset").execute({"adapter": "WLAN"})
    assert r.success is True
    assert recorder.find("set dnsservers name=WLAN static 223.5.5.5 primary") is not None
    assert recorder.find("add dnsservers name=WLAN 223.6.6.6 index=2") is not None


def test_dns_preset_unknown_preset_rejected(recorder):
    r = repairs.get_action("dns_preset").execute({"adapter": "WLAN", "preset": "谷歌"})
    assert r.success is False
    assert "未知 DNS 预设" in r.message
    assert recorder.calls == []


def test_dns_preset_no_adapter_available(monkeypatch, recorder):
    _patch_snapshot(monkeypatch, _snap())
    r = repairs.get_action("dns_preset").execute({"preset": "阿里"})
    assert r.success is False
    assert "未指定网卡且无法自动选择" in r.message
    assert recorder.cmds == []


def test_ncsi_restore_restores_stock_values_with_reg_export_backup(recorder, reg_spy, events):
    r = repairs.get_action("ncsi_restore").execute({})
    assert r.success is True
    full = "HKLM\\" + config.NCSI_REG_PATH
    # 第一条命令就是 reg export 全量备份，落盘在 BACKUP_DIR
    assert recorder.cmds[0][:3] == ["reg", "export", full]
    assert recorder.cmds[0][3].startswith(config.BACKUP_DIR)
    assert recorder.cmds[0][3].endswith(".reg")
    assert recorder.cmds[0][4] == "/y"
    # 备份命令先于任何注册表写入
    assert events[0][1].startswith("reg export")
    assert next(i for i, e in enumerate(events) if e[0] == "regwrite") > 0
    # 写入值 = NCSI_STOCK 全集；EnableActiveProbing=1 为 DWORD，其余为 REG_SZ
    written = {name: (typ, data) for _p, name, typ, data in reg_spy.written}
    assert set(written) == set(config.NCSI_STOCK)
    assert written["EnableActiveProbing"] == (winreg.REG_DWORD, 1)
    for name, val in config.NCSI_STOCK.items():
        if name != "EnableActiveProbing":
            assert written[name] == (winreg.REG_SZ, str(val))
    # 打开的键路径与 config 常量一致，且为可写访问
    assert (full, winreg.KEY_SET_VALUE) in reg_spy.opened
    # 最后启动 NlaSvc 让探测立刻恢复
    assert recorder.cmds[-1] == ["sc", "start", "NlaSvc"]
    assert "备份" in r.message


def test_ncsi_restore_registry_failure_reported(recorder, reg_spy):
    reg_spy.fail_paths = ["NlaSvc"]          # 模拟无管理员权限
    r = repairs.get_action("ncsi_restore").execute({})
    assert r.success is False
    assert "写注册表失败" in r.message
    assert recorder.find("sc start") is None   # 写失败后不再启动服务
    # 备份仍然先行完成
    assert recorder.cmds and recorder.cmds[0][:2] == ["reg", "export"]


def test_ncsi_disable_sets_probing_0_with_backup(recorder, reg_spy):
    r = repairs.get_action("ncsi_disable").execute({})
    assert r.success is True
    assert [(w[1], w[2], w[3]) for w in reg_spy.written] == \
        [("EnableActiveProbing", winreg.REG_DWORD, 0)]
    assert recorder.cmds[0][:3] == ["reg", "export", "HKLM\\" + config.NCSI_REG_PATH]
    assert recorder.find("sc start") is None   # 关闭探测无需启动 NlaSvc


def test_ncsi_disable_registry_failure_reported(recorder, reg_spy):
    reg_spy.fail_paths = ["NlaSvc"]
    r = repairs.get_action("ncsi_disable").execute({})
    assert r.success is False
    assert "写注册表失败" in r.message


def _patch_nic_registry(monkeypatch, values_map, subkeys):
    from syskit import winregread
    base = config.NIC_CLASS_KEY
    monkeypatch.setattr(winregread, "subkeys",
                        lambda path: list(subkeys) if path == "HKLM\\" + base else [])
    monkeypatch.setattr(winregread, "values", lambda path: values_map.get(path, {}))


def test_power_off_writes_pnpcapabilities_24_and_disables_usb_suspend(
        recorder, reg_spy, monkeypatch):
    base = config.NIC_CLASS_KEY
    vals = {
        f"HKLM\\{base}\\0000": {"DriverDesc": "Intel(R) Wi-Fi 6 AX201"},
        f"HKLM\\{base}\\0001": {"DriverDesc": "Realtek PCIe GbE Family Controller"},
        f"HKLM\\{base}\\0002": {"DriverDesc": "WAN Miniport (IP)"},
        f"HKLM\\{base}\\0003": {"DriverDesc": "Microsoft Wi-Fi Direct Virtual Adapter #4"},
        f"HKLM\\{base}\\0004": {"DriverDesc": "Old NIC", "PnPCapabilities": "24"},
    }
    _patch_nic_registry(monkeypatch, vals,
                        ["0000", "0001", "0002", "0003", "0004", "0005", "Config"])
    reg_spy.fail_paths = ["\\0001"]          # Realtek 模拟写入失败（应计入跳过）
    r = repairs.get_action("power_off").execute({})
    assert r.success is True
    # 只对 0000 写 PnPCapabilities=24（REG_DWORD——标准存储类型，REG_SZ 不被
    # NDIS 电源策略识别）；0002 虚拟/0003 WAN Miniport/0004 已是 24 均跳过
    assert [(w[0].rsplit("\\", 1)[-1], w[1], w[2], w[3]) for w in reg_spy.written] == \
        [("0000", "PnPCapabilities", winreg.REG_DWORD, 24)]
    assert (f"HKLM\\{base}\\0000", winreg.KEY_SET_VALUE) in reg_spy.opened
    # 改前 note 备份记录原值，落在 BACKUP_DIR
    notes = list(Path(config.BACKUP_DIR).rglob("pnp_backup_0000.txt"))
    assert notes and "Intel(R) Wi-Fi 6 AX201" in notes[0].read_text(encoding="utf-8")
    # USB 选择性暂停关闭的两条 powercfg
    assert recorder.find("powercfg /setacvalueindex SCHEME_CURRENT "
                         "2a737441-1930-4402-8d77-b2bebba308a3 "
                         "48e6b7a6-50f5-4782-a5d4-53bb8f07e226 0") is not None
    assert recorder.find("powercfg /setactive SCHEME_CURRENT") is not None
    assert "Intel" in r.message and "Realtek" in r.message   # 成功 + 跳过都有交代


def test_power_off_nothing_to_change(recorder, reg_spy, monkeypatch):
    base = config.NIC_CLASS_KEY
    vals = {f"HKLM\\{base}\\0000": {"DriverDesc": "NIC", "PnPCapabilities": "24"}}
    _patch_nic_registry(monkeypatch, vals, ["0000"])
    r = repairs.get_action("power_off").execute({})
    assert r.success is False
    assert "没有需要修改的网卡" in r.message
    assert reg_spy.written == []
    assert recorder.find("powercfg /setactive SCHEME_CURRENT") is not None  # powercfg 仍执行


def test_del_persistent_route_auto_detect(monkeypatch, recorder):
    hijack = {"dest": "0.0.0.0/0", "nextHop": "172.16.2.1", "metric": 1}
    _patch_snapshot(monkeypatch,
                    _snap(persistent=[hijack], defaults=[dict(nextHop="172.16.2.1")]),
                    _snap())
    r = repairs.get_action("del_persistent_route").execute({})
    assert r.success is True
    assert recorder.cmds == [
        ["route", "print", "-4"],            # 备份：完整路由表存档
        ["route", "delete", "0.0.0.0", "mask", "0.0.0.0", "172.16.2.1"],
    ]
    ps = recorder.ps_scripts[0]
    for frag in ("Remove-NetRoute", "-DestinationPrefix 0.0.0.0/0",
                 "-NextHop 172.16.2.1", "-PolicyStore PersistentStore", "-Confirm:$false"):
        assert frag in ps
    # 备份文件包含还原命令
    notes = list(Path(config.BACKUP_DIR).rglob("route_backup.txt"))
    assert notes
    content = notes[0].read_text(encoding="utf-8")
    assert "route -p add 0.0.0.0 mask 0.0.0.0 172.16.2.1 metric 1" in content
    assert "已删除持久路由 0.0.0.0/0 → 172.16.2.1" in r.message


def test_del_persistent_route_explicit_params_use_prefix_mask(monkeypatch, recorder):
    _patch_snapshot(monkeypatch, _snap(), _snap())
    r = repairs.get_action("del_persistent_route").execute(
        {"dest": "10.0.0.0/24", "gw": "10.0.0.1"})
    assert r.success is True
    assert recorder.find("route delete 10.0.0.0 mask 255.255.255.0 10.0.0.1") is not None
    assert "Remove-NetRoute" in recorder.ps_scripts[0]


def test_del_persistent_route_no_default_found(monkeypatch, recorder):
    _patch_snapshot(monkeypatch, _snap())
    r = repairs.get_action("del_persistent_route").execute({})
    assert r.success is True
    assert "未发现持久默认路由" in r.message
    assert recorder.calls == []


def test_del_persistent_route_residual_reported(monkeypatch, recorder):
    hijack = {"dest": "0.0.0.0/0", "nextHop": "172.16.2.1", "metric": 1}
    _patch_snapshot(monkeypatch,
                    _snap(persistent=[hijack]),
                    _snap(persistent=[hijack], defaults=[dict(nextHop="172.16.2.1")]))
    r = repairs.get_action("del_persistent_route").execute({})
    assert r.success is False
    assert "请手动执行" in r.message
    assert "route delete 0.0.0.0 mask 0.0.0.0 172.16.2.1" in r.message


@pytest.mark.parametrize("prefix,expect", [
    ("10.0.0.0/24", "255.255.255.0"),
    ("10.0.0.0/8", "255.0.0.0"),
    ("10.0.0.0/32", "255.255.255.255"),
    ("0.0.0.0/0", "0.0.0.0"),
    ("garbage", "255.255.255.0"),            # 解析失败回退默认掩码
])
def test_prefix_to_mask(prefix, expect):
    assert repairs._prefix_to_mask(prefix) == expect


def test_proxy_off_wiring(recorder, monkeypatch):
    from syskit import proxy
    monkeypatch.setattr(proxy, "disable_proxy", lambda: True)
    a = repairs.get_action("proxy_off")
    r = a.execute({})
    assert r.success is True
    assert "系统代理已关闭" in r.message
    monkeypatch.setattr(proxy, "disable_proxy", lambda: False)
    r2 = a.execute({})
    assert r2.success is False
    assert "手动" in r2.message


def test_disable_proxy_writes_proxyenable_and_refreshes(monkeypatch):
    """proxy.disable_proxy 本体：写 HKCU ProxyEnable=0 并通知 WinINET 刷新。"""
    from syskit import proxy
    written = []
    opts = []

    class _K:
        def Close(self):
            pass

    class _Wininet:
        def InternetSetOptionW(self, *a):
            opts.append(a[1])

    monkeypatch.setattr(winreg, "OpenKey", lambda *a, **k: _K())
    monkeypatch.setattr(winreg, "SetValueEx",
                        lambda key, name, res, typ, data: written.append((name, typ, data)))
    monkeypatch.setattr(proxy.ctypes, "windll", type("W", (), {"wininet": _Wininet()})())
    assert proxy.disable_proxy() is True
    assert ("ProxyEnable", winreg.REG_DWORD, 0) in written
    assert opts == [39, 37]                  # SETTINGS_CHANGED + REFRESH


def test_disable_proxy_oserror_returns_false(monkeypatch):
    from syskit import proxy
    monkeypatch.setattr(winreg, "OpenKey",
                        lambda *a, **k: (_ for _ in ()).throw(OSError("denied")))
    assert proxy.disable_proxy() is False


def test_disable_proxy_clears_pac_when_present(monkeypatch):
    """回归：诊断对「PAC 自动代理脚本」给出的建议修复也是 proxy_off，
    因此 disable_proxy 必须一并清掉 AutoConfigURL，否则点了建议按钮毫无效果。"""
    from syskit import proxy
    written, deleted = [], []

    class _K:
        def Close(self):
            pass

    class _Wininet:
        def InternetSetOptionW(self, *a):
            pass

    monkeypatch.setattr(proxy.winregread, "values", lambda path: {
        "ProxyEnable": 1, "ProxyServer": "127.0.0.1:7890",
        "AutoConfigURL": "http://127.0.0.1:7890/pac"})
    monkeypatch.setattr(winreg, "OpenKey", lambda *a, **k: _K())
    monkeypatch.setattr(winreg, "SetValueEx",
                        lambda key, name, res, typ, data: written.append((name, data)))
    monkeypatch.setattr(winreg, "DeleteValue", lambda key, name: deleted.append(name))
    monkeypatch.setattr(proxy.ctypes, "windll", type("W", (), {"wininet": _Wininet()})())

    assert proxy.disable_proxy() is True
    assert ("ProxyEnable", 0) in written
    assert deleted == ["AutoConfigURL"]
    # 关闭前原值已备份到隔离的备份目录，便于人工还原
    assert any(p.name == "proxy_backup.json" for p in Path(config.BACKUP_DIR).rglob("*"))


def test_disable_proxy_without_pac_does_not_delete(monkeypatch):
    """未配置 PAC 时不应触碰 AutoConfigURL（避免多余写操作）。"""
    from syskit import proxy
    deleted = []

    class _K:
        def Close(self):
            pass

    class _Wininet:
        def InternetSetOptionW(self, *a):
            pass

    monkeypatch.setattr(proxy.winregread, "values",
                        lambda path: {"ProxyEnable": 1, "ProxyServer": "127.0.0.1:7890"})
    monkeypatch.setattr(winreg, "OpenKey", lambda *a, **k: _K())
    monkeypatch.setattr(winreg, "SetValueEx", lambda *a, **k: None)
    monkeypatch.setattr(winreg, "DeleteValue", lambda key, name: deleted.append(name))
    monkeypatch.setattr(proxy.ctypes, "windll", type("W", (), {"wininet": _Wininet()})())

    assert proxy.disable_proxy() is True
    assert deleted == []


# ---------------- MAC 随机化（改 XML 文件，全程在 tmp） ----------------

def _write_profile_xml(tmp_path, name="MyWiFi", randomization=None):
    """randomization=None 表示 XML 中没有 MacRandomization 块。"""
    if randomization is None:
        mac = ""
    else:
        mac = ('<MacRandomization xmlns="http://www.microsoft.com/networking/WLAN/profile/v3">'
               f"<EnableRandomization>{randomization}</EnableRandomization></MacRandomization>")
    content = ('<?xml version="1.0"?>\n'
               '<WLANProfile xmlns="http://www.microsoft.com/networking/WLAN/profile/v1">\n'
               f"  <name>{name}</name>\n  {mac}\n</WLANProfile>")
    d = tmp_path / "interfaces" / f"iface-guid-{name}"
    d.mkdir(parents=True, exist_ok=True)
    f = d / "0001.xml"
    f.write_text(content, encoding="utf-8")
    return f


def _glob_only_tmp(monkeypatch, *files):
    """把 WLAN profile 的 glob 指向 tmp 里的假 XML，其余 glob 调用不受影响。"""
    import glob
    real = glob.glob
    monkeypatch.setattr(
        glob, "glob",
        lambda pat: [str(f) for f in files] if "Wlansvc" in pat else real(pat))


def test_mac_random_off_flips_flag_backs_up_and_restarts_wlan(monkeypatch, recorder, tmp_path):
    f = _write_profile_xml(tmp_path, randomization="true")
    _glob_only_tmp(monkeypatch, f)
    recorder.respond_sub("netsh wlan show interfaces",
                         stdout="    SSID                  : HomeWifi")
    r = repairs.get_action("mac_random_off").execute({"profile": "MyWiFi"})
    assert r.success is True
    assert "已关闭 MyWiFi 的 MAC 随机化（1 个文件）" in r.message
    assert "<EnableRandomization>false</EnableRandomization>" in f.read_text(encoding="utf-8")
    # 末尾走 RestartWlan 重启服务并重连
    assert recorder.cmds[0] == ["netsh", "wlan", "show", "interfaces"]
    assert recorder.find("net start WlanSvc") is not None
    assert recorder.find("netsh wlan connect name=HomeWifi") is not None
    # 原 XML 备份进 BACKUP_DIR
    backups = list(Path(config.BACKUP_DIR).rglob("0001.xml"))
    assert backups and "<EnableRandomization>true" in backups[0].read_text(encoding="utf-8")


def test_mac_random_on_inserts_block_when_missing(monkeypatch, recorder, tmp_path):
    f = _write_profile_xml(tmp_path, name="OfficeWiFi", randomization=None)
    _glob_only_tmp(monkeypatch, f)
    r = repairs.get_action("mac_random_on").execute({"profile": "OfficeWiFi"})
    assert r.success is True
    text = f.read_text(encoding="utf-8")
    assert "MacRandomization" in text
    assert "<EnableRandomization>true</EnableRandomization>" in text


def test_mac_random_off_already_off_skips_restart(monkeypatch, recorder, tmp_path):
    f = _write_profile_xml(tmp_path, randomization="false")
    _glob_only_tmp(monkeypatch, f)
    r = repairs.get_action("mac_random_off").execute({"profile": "MyWiFi"})
    assert r.success is True
    assert "本来就是关闭" in r.message
    assert recorder.calls == []              # 无需重启 WLAN


def test_mac_random_on_already_on_skips_restart(monkeypatch, recorder, tmp_path):
    f = _write_profile_xml(tmp_path, randomization="true")
    _glob_only_tmp(monkeypatch, f)
    r = repairs.get_action("mac_random_on").execute({"profile": "MyWiFi"})
    assert r.success is True
    assert "本来就是开启" in r.message
    assert recorder.calls == []


def test_mac_random_profile_not_found(monkeypatch, recorder, tmp_path):
    f = _write_profile_xml(tmp_path, name="Other", randomization="true")
    _glob_only_tmp(monkeypatch, f)
    r = repairs.get_action("mac_random_off").execute({"profile": "NoSuchWiFi"})
    assert r.success is False
    assert "未找到名为 NoSuchWiFi" in r.message


def test_mac_random_off_without_profile_fails_cleanly(monkeypatch, recorder, tmp_path):
    """无参数且探测不到 SSID/配置文件 → 明确失败而非误改。"""
    f = _write_profile_xml(tmp_path, randomization="true")
    _glob_only_tmp(monkeypatch, f)
    r = repairs.get_action("mac_random_off").execute({})
    assert r.success is False
    assert "未指定 WiFi 配置文件" in r.message


def test_mac_random_state_reader(monkeypatch, tmp_path):
    f_true = _write_profile_xml(tmp_path, name="A", randomization="true")
    f_false = _write_profile_xml(tmp_path, name="B", randomization="false")
    f_none = _write_profile_xml(tmp_path, name="C", randomization=None)
    _glob_only_tmp(monkeypatch, f_true, f_false, f_none)
    assert repairs.mac_random_state("a") is True       # 匹配大小写不敏感
    assert repairs.mac_random_state("B") is False
    assert repairs.mac_random_state("C") is None       # 无该块 → 未知
    assert repairs.mac_random_state("Missing") is None


def test_mac_random_state_cache_skips_reread(monkeypatch, tmp_path):
    """哨兵（路径+mtime+size）未变时第二次查询零文件读取（UI 线程高频调用的前提）。"""
    f = _write_profile_xml(tmp_path, name="CacheWiFi", randomization="true")
    _glob_only_tmp(monkeypatch, f)
    real_open, calls = open, {"n": 0}

    def counting_open(file, *a, **k):
        if str(file) == str(f):
            calls["n"] += 1
        return real_open(file, *a, **k)

    monkeypatch.setattr("builtins.open", counting_open)
    assert repairs.mac_random_state("CacheWiFi") is True
    assert calls["n"] == 1
    assert repairs.mac_random_state("CacheWiFi") is True   # 缓存命中：不再 open
    assert calls["n"] == 1


def test_mac_random_state_cache_rescans_after_change(monkeypatch, tmp_path):
    """TTL 过期且外部改写 XML（mtime/size 变化）后，下一次查询重读并反映新状态。"""
    f = _write_profile_xml(tmp_path, name="CacheWiFi", randomization="true")
    _glob_only_tmp(monkeypatch, f)
    assert repairs.mac_random_state("CacheWiFi") is True
    new_content = f.read_text(encoding="utf-8").replace(
        "<EnableRandomization>true", "<EnableRandomization>false")
    f.write_text(new_content, encoding="utf-8")
    stat = f.stat()
    os.utime(f, ns=(stat.st_atime_ns, stat.st_mtime_ns + 5_000_000))  # 确保哨兵必变
    repairs._MAC_SCAN["checked"] = 0.0                  # 过 TTL，逼下一次走 stat 哨兵
    assert repairs.mac_random_state("CacheWiFi") is False


def test_set_mac_random_invalidates_cache(monkeypatch, recorder, tmp_path):
    """写入路径自身改盘后必须失效哨兵——随后状态查询看到新值而非旧缓存。"""
    f = _write_profile_xml(tmp_path, name="CacheWiFi", randomization="true")
    _glob_only_tmp(monkeypatch, f)
    assert repairs.mac_random_state("CacheWiFi") is True
    r = repairs.get_action("mac_random_off").execute({"profile": "CacheWiFi"})
    assert r.success is True
    assert repairs.mac_random_state("CacheWiFi") is False


def test_wlan_profiles_parses_saved_names(recorder):
    recorder.respond_sub("wlan show profiles",
                         stdout="    所有用户配置文件: HomeWifi\n"
                                "    所有用户配置文件: Office")
    assert repairs.wlan_profiles() == ["HomeWifi", "Office"]


def test_current_or_first_profile_prefers_ssid(recorder):
    recorder.respond_sub("wlan show interfaces",
                         stdout="    SSID                  : LiveSSID")
    assert repairs._current_or_first_profile() == "LiveSSID"


def test_current_or_first_profile_falls_back_to_saved(recorder):
    recorder.respond_sub("wlan show profiles",
                         stdout="    所有用户配置文件: SavedOne\n"
                                "    所有用户配置文件: SavedTwo")
    assert repairs._current_or_first_profile() == "SavedOne"


# ---------------- 强力类：命令构造 + need_reboot ----------------

def test_winsock_reset(recorder):
    recorder.respond_sub("winsock reset", stdout="成功重置 Winsock 目录。")
    r = repairs.get_action("winsock_reset").execute({})
    assert r.success is True
    assert recorder.cmds == [["netsh", "winsock", "reset"]]
    assert r.need_reboot is True
    assert "成功重置" in r.message


def test_ipstack_reset_resets_v4_and_v6(recorder):
    r = repairs.get_action("ipstack_reset").execute({})
    assert r.success is True
    assert recorder.cmds == [["netsh", "int", "ip", "reset"],
                             ["netsh", "int", "ipv6", "reset"]]
    assert r.need_reboot is True


def test_ipstack_reset_partial_failure(recorder):
    recorder.respond_sub("ipv6 reset", code=1)
    r = repairs.get_action("ipstack_reset").execute({})
    assert r.success is False
    assert r.need_reboot is True


# ---------------- 系统还原点（深度/重置组合的安全网） ----------------

def test_restore_point_success(recorder, monkeypatch):
    from syskit import admin
    monkeypatch.setattr(admin, "is_admin", lambda: True)
    recorder.respond_sub("Checkpoint-Computer", code=0)
    r = repairs.get_action("restore_point").execute({})
    assert r.success is True and "还原点" in r.message
    ps = [c for c in recorder.calls if c[0] == "ps"]
    assert ps and "Checkpoint-Computer" in ps[0][1]
    # -ErrorAction Stop 必须在：PS 非终止错误的 -Command 退出码恒 0（补55 A-F1
    # 实测），缺了它「系统保护未开启」会被当成已创建——安全网假成功
    assert "-ErrorAction Stop" in ps[0][1]


def test_restore_point_requires_admin(monkeypatch):
    from syskit import admin
    monkeypatch.setattr(admin, "is_admin", lambda: False)
    r = repairs.get_action("restore_point").execute({})
    assert r.success is False and "管理员" in r.message


def test_restore_point_failure_reports_honestly(recorder, monkeypatch):
    """系统还原未开启时 Checkpoint-Computer 失败：如实报失败并提示不影响继续修复。"""
    from syskit import admin
    monkeypatch.setattr(admin, "is_admin", lambda: True)
    recorder.respond_sub("Checkpoint-Computer", code=1,
                         stdout="", stderr="服务无法启动")
    r = repairs.get_action("restore_point").execute({})
    assert r.success is False
    assert "系统还原" in r.message and "不影响继续修复" in r.message


def test_restore_point_description_discloses_long_wait():
    """description 是 CLI 逐项确认（cli.cmd_repair）、单项 ConfirmDialog 与修复页
    tooltip 的唯一文案源——30~90s 空等必须在此如实披露（补55 F4 此前只给 UI
    组合确认弹窗加了该提示，CLI/单项路径仍会被静默卡住一分钟）。"""
    desc = repairs.get_action("restore_point").description
    assert "30~90" in desc
    assert "无法中止" in desc
