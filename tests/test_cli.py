"""syskit/cli.py 单元测试：diagnose/repair 编排与 argparse 分流（全程 mock，零真实修复）。

Mock 边界（配合 conftest 哨兵）：
- admin.is_admin 控制权限分支；
- netcheck.run_all / verdict 换成假结果；
- repairs.get_action / act.execute 全部打桩，绝不执行真实命令；
- input() 用 monkeypatch 控制确认输入。
"""
import types

import pytest

from syskit import admin, cli, netcheck, repairs
from syskit.netcheck import CheckResult, FAIL, PASS
from syskit.repairs import RepairResult


def _result(cid, status, fix_ids=None, name=None):
    return CheckResult(cid, name or cid, status, f"detail-{cid}", fix_ids or [])


class FakeAct:
    def __init__(self, aid, title="动作", brief="简介", description="说明",
                 restore_hint="还原方法", result=None, need_reboot=False):
        self.id = aid
        self.title = title
        self.brief = brief
        self.description = description
        self.restore_hint = restore_hint
        self.need_reboot = need_reboot
        self.result = result if result is not None else RepairResult(True, "ok")
        self.executed = []

    def execute(self, param=None):
        self.executed.append(param)
        return self.result


@pytest.fixture
def fake_logger(monkeypatch):
    """收集 LOGGER 输出行，便于断言报告内容。"""
    lines = []

    class _Log:
        def info(self, msg, *a, **k):
            lines.append(str(msg))

        def warning(self, msg, *a, **k):
            lines.append(str(msg))

        def error(self, msg, *a, **k):
            lines.append(str(msg))

    monkeypatch.setattr(cli, "LOGGER", _Log())
    return lines


@pytest.fixture
def admin_on(monkeypatch):
    monkeypatch.setattr(admin, "is_admin", lambda: True)


@pytest.fixture
def admin_off(monkeypatch):
    monkeypatch.setattr(admin, "is_admin", lambda: False)


# ---------------- cmd_diagnose ----------------

class TestDiagnose:
    def test_reports_verdict_and_suggests_fixes(self, monkeypatch, fake_logger, admin_on):
        results = [
            _result("real_conn", PASS),
            _result("ncsi_probe", FAIL, fix_ids=["fix_nlasvc"], name="NlaSvc"),
            _result("dns_compare", PASS),
        ]
        monkeypatch.setattr(netcheck, "run_all",
                            lambda progress=None, quick=False: results)
        monkeypatch.setattr(netcheck, "verdict",
                            lambda rs: ("dns_broken", "能连上但 DNS 异常/缓慢；失败项: 1 个"))
        act = FakeAct("fix_nlasvc", title="修复 NlaSvc", brief="恢复服务")
        monkeypatch.setattr(repairs, "get_action",
                            lambda fid: act if fid == "fix_nlasvc" else None)

        rc = cli.cmd_diagnose(quick=False)
        assert rc == 0
        text = "\n".join(fake_logger)
        assert "诊断报告" in text
        assert "DNS 异常/缓慢" in text
        assert "[fix_nlasvc]" in text and "修复 NlaSvc" in text

    def test_warns_when_not_admin(self, monkeypatch, fake_logger, admin_off):
        monkeypatch.setattr(netcheck, "run_all", lambda progress=None, quick=False, should_stop=None: [])
        monkeypatch.setattr(netcheck, "verdict", lambda rs: ("normal", "网络正常"))
        rc = cli.cmd_diagnose()
        assert rc == 0
        assert any("未以管理员" in s for s in fake_logger)

    def test_quick_flag_passed_to_run_all(self, monkeypatch, admin_on):
        seen = {}

        def fake_run_all(progress=None, quick=False, should_stop=None):
            seen["quick"] = quick
            return []

        monkeypatch.setattr(netcheck, "run_all", fake_run_all)
        monkeypatch.setattr(netcheck, "verdict", lambda rs: ("normal", "网络正常"))
        cli.cmd_diagnose(quick=True)
        assert seen["quick"] is True


# ---------------- cmd_repair ----------------

class TestRepair:
    def test_non_admin_returns_2(self, monkeypatch, fake_logger, admin_off):
        rc = cli.cmd_repair(types.SimpleNamespace(combo="fast", yes=True))
        assert rc == 2
        assert any("管理员" in s for s in fake_logger)

    def test_unknown_combo_returns_2(self, monkeypatch, fake_logger, admin_on):
        monkeypatch.setattr(repairs, "get_action", lambda fid: None)
        rc = cli.cmd_repair(types.SimpleNamespace(combo="nope", yes=True))
        assert rc == 2
        assert any("未知修复项" in s for s in fake_logger)

    def test_fast_combo_executes_four_actions_in_order(self, monkeypatch, admin_on):
        order = []
        acts = {}

        def make(aid):
            a = FakeAct(aid, result=RepairResult(True, f"done-{aid}"))
            acts[aid] = a
            return a

        def get_action(fid):
            return make(fid) if fid in (
                "flush_dns", "renew_dhcp", "start_nlasvc", "restart_wlan"
            ) else None

        # execute 时记 id 顺序
        for aid in ("flush_dns", "renew_dhcp", "start_nlasvc", "restart_wlan"):
            pass

        def get_action2(fid):
            if fid not in acts:
                if fid not in ("flush_dns", "renew_dhcp", "start_nlasvc", "restart_wlan"):
                    return None
                a = FakeAct(fid)
                _orig_exec = a.execute

                def exec_and_record(param=None, _aid=fid, _e=_orig_exec):
                    order.append(_aid)
                    return _e(param)

                a.execute = exec_and_record
                acts[fid] = a
            return acts[fid]

        monkeypatch.setattr(repairs, "get_action", get_action2)
        rc = cli.cmd_repair(types.SimpleNamespace(combo="fast", yes=True))
        assert rc == 0
        assert order == ["flush_dns", "renew_dhcp", "start_nlasvc", "restart_wlan"]

    def test_single_action_with_yes(self, monkeypatch, admin_on):
        calls = []

        def get_action(fid):
            if fid != "flush_dns":
                return None
            a = FakeAct("flush_dns", result=RepairResult(True, "ok"))
            a.execute = lambda param=None: calls.append("flush_dns") or RepairResult(True, "ok")
            return a

        monkeypatch.setattr(repairs, "get_action", get_action)
        rc = cli.cmd_repair(types.SimpleNamespace(combo="flush_dns", yes=True))
        assert rc == 0
        assert calls == ["flush_dns"]

    def test_confirm_no_skips_action(self, monkeypatch, admin_on):
        executed = []

        def get_action(fid):
            if fid != "flush_dns":
                return None
            a = FakeAct("flush_dns")
            a.execute = lambda param=None: executed.append(1) or RepairResult(True, "ok")
            return a

        monkeypatch.setattr(repairs, "get_action", get_action)
        monkeypatch.setattr("builtins.input", lambda *a, **k: "n")
        rc = cli.cmd_repair(types.SimpleNamespace(combo="flush_dns", yes=False))
        assert rc == 0
        assert executed == []

    def test_confirm_yes_runs_action(self, monkeypatch, admin_on):
        executed = []

        def get_action(fid):
            a = FakeAct("flush_dns")
            a.execute = lambda param=None: executed.append(1) or RepairResult(True, "ok")
            return a

        monkeypatch.setattr(repairs, "get_action", get_action)
        monkeypatch.setattr("builtins.input", lambda *a, **k: "y")
        rc = cli.cmd_repair(types.SimpleNamespace(combo="flush_dns", yes=False))
        assert rc == 0
        assert executed == [1]

    def test_need_reboot_logged(self, monkeypatch, fake_logger, admin_on):
        def get_action(fid):
            a = FakeAct("winsock_reset", need_reboot=True,
                        result=RepairResult(True, "ok", need_reboot=True))
            return a

        monkeypatch.setattr(repairs, "get_action", get_action)
        rc = cli.cmd_repair(types.SimpleNamespace(combo="winsock_reset", yes=True))
        assert rc == 0
        assert any("重启" in s for s in fake_logger)


# ---------------- main argparse ----------------

class TestMainArgparse:
    def test_no_cmd_prints_help(self, monkeypatch, capsys):
        rc = cli.main([])
        assert rc == 0

    def test_diagnose_routes_to_cmd(self, monkeypatch, admin_on):
        seen = {}

        def fake_diagnose(quick=False):
            seen["quick"] = quick
            return 0

        monkeypatch.setattr(cli, "cmd_diagnose", fake_diagnose)
        monkeypatch.setattr(cli.logger, "setup", lambda **k: None)
        rc = cli.main(["diagnose", "--quick"])
        assert rc == 0
        assert seen["quick"] is True

    def test_repair_routes_to_cmd(self, monkeypatch, admin_on):
        seen = {}

        def fake_repair(args):
            seen["combo"] = args.combo
            seen["yes"] = args.yes
            return 0

        monkeypatch.setattr(cli, "cmd_repair", fake_repair)
        monkeypatch.setattr(cli.logger, "setup", lambda **k: None)
        rc = cli.main(["repair", "full", "-y"])
        assert rc == 0
        assert seen == {"combo": "full", "yes": True}

    def test_repair_default_combo_fast(self, monkeypatch, admin_on):
        seen = {}

        def fake_repair(args):
            seen["combo"] = args.combo
            return 0

        monkeypatch.setattr(cli, "cmd_repair", fake_repair)
        monkeypatch.setattr(cli.logger, "setup", lambda **k: None)
        rc = cli.main(["repair"])
        assert rc == 0
        assert seen["combo"] == "fast"
