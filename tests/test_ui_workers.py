"""syskit/ui/workers.py 覆盖率补强：五个 Worker 的 run() 同步直调（既有口径——
coverage 不追踪 QThread 线程，参考 test_disk_system_page_gaps.py）。

后端（netcheck.run_all / repairs.get_action / snapshot.take / proc.run）全部
monkeypatch，信号直连列表收集；线程从不 start()，无 QThread 生命周期风险。
reap 的 setParent 挂树与返回值一并验证（finished→deleteLater 连接随调用建立）。
"""
import pytest

from syskit import netcheck, snapshot
from syskit.repairs import RepairResult
from syskit.ui import workers


# ---------------- reap ----------------

def test_reap_sets_parent_and_returns_worker(qtbot):
    from PySide6.QtWidgets import QWidget

    owner = QWidget()
    w = workers.DiagnosisWorker()
    try:
        assert workers.reap(w, owner) is w
        assert w.parent() is owner
    finally:
        owner.deleteLater()


# ---------------- DiagnosisWorker ----------------

def test_diagnosis_run_emits_progress_then_results(qtbot, monkeypatch):
    progress_seen, results_seen, kw = [], [], {}

    def fake_run_all(*, progress, quick, should_stop):
        kw["quick"] = quick
        assert should_stop() is False
        progress("r1")
        progress("r2")
        return ["R1", "R2"]

    monkeypatch.setattr(netcheck, "run_all", fake_run_all)
    w = workers.DiagnosisWorker(quick=True)
    w.progress.connect(progress_seen.append)
    w.finished_ok.connect(results_seen.append)
    w.run()
    assert progress_seen == ["r1", "r2"]
    assert results_seen == [["R1", "R2"]]
    assert kw["quick"] is True


def test_diagnosis_cancel_makes_should_stop_true(qtbot, monkeypatch):
    stops = []

    def fake_run_all(*, progress, quick, should_stop):
        stops.append(should_stop())
        return []

    monkeypatch.setattr(netcheck, "run_all", fake_run_all)
    w = workers.DiagnosisWorker()
    w.cancel()
    w.run()
    assert stops == [True]


def test_diagnosis_exception_emits_empty_list(qtbot, monkeypatch):
    def boom(*args, **kwargs):
        raise RuntimeError("netcheck down")

    monkeypatch.setattr(netcheck, "run_all", boom)
    results_seen = []
    w = workers.DiagnosisWorker()
    w.finished_ok.connect(results_seen.append)
    w.run()
    assert results_seen == [[]]


# ---------------- RepairWorker ----------------

def test_repair_unknown_action_still_emits_done(qtbot, monkeypatch):
    monkeypatch.setattr(workers.repairs, "get_action", lambda aid: None)
    got = []
    w = workers.RepairWorker("no-such-action")
    w.done.connect(got.append)
    w.run()
    aid, res = got[0]
    assert aid == "no-such-action"
    assert res.success is False and "未知修复动作" in res.message


def test_repair_known_action_executes_with_param(qtbot, monkeypatch):
    res = RepairResult(True, "ok")
    seen_param = {}

    class FakeAction:
        def execute(self, param):
            seen_param.update(param)
            return res

    monkeypatch.setattr(workers.repairs, "get_action", lambda aid: FakeAction())
    got = []
    w = workers.RepairWorker("fix1", {"iface": "eth0"})
    w.done.connect(got.append)
    w.run()
    assert got == [("fix1", res)]
    assert seen_param == {"iface": "eth0"}


# ---------------- ComboWorker ----------------

def _combo_fixture(monkeypatch, actions):
    monkeypatch.setattr(workers.repairs, "get_action", lambda aid: actions.get(aid))


def test_combo_runs_in_order_skips_unknown_and_reports(qtbot, monkeypatch):
    executed = []

    def mk(aid):
        class FakeAction:
            def execute(self, param):
                executed.append(aid)
                return RepairResult(True, aid)
        return FakeAction()

    _combo_fixture(monkeypatch, {"a": mk("a"), "c": mk("c")})
    items, all_done = [], []
    w = workers.ComboWorker(["a", "missing", "c"])
    w.item_done.connect(items.append)
    w.all_done.connect(lambda: all_done.append(1))
    w.run()
    assert executed == ["a", "c"]
    assert [aid for aid, _ in items] == ["a", "c"]
    assert all_done == [1]
    assert w.cancelled is False


def test_combo_cancel_skips_remaining_but_still_emits_all_done(qtbot, monkeypatch):
    executed = []

    def mk(aid):
        class FakeAction:
            def execute(self, param):
                executed.append(aid)
                return RepairResult(True, aid)
        return FakeAction()

    _combo_fixture(monkeypatch, {"a": mk("a"), "b": mk("b")})
    all_done = []
    w = workers.ComboWorker(["a", "b"])
    w.cancel()
    w.all_done.connect(lambda: all_done.append(1))
    w.run()
    assert executed == []
    assert w.cancelled is True
    assert all_done == [1]


def test_combo_execute_exception_still_emits_all_done(qtbot, monkeypatch):
    class BoomAction:
        def execute(self, param):
            raise RuntimeError("disk gone")

    _combo_fixture(monkeypatch, {"a": BoomAction()})
    all_done = []
    w = workers.ComboWorker(["a"])
    w.all_done.connect(lambda: all_done.append(1))
    w.run()
    assert all_done == [1]


# ---------------- SnapshotWorker ----------------

def test_snapshot_run_emits_snapshot(qtbot, monkeypatch):
    snap = snapshot.Snapshot()
    monkeypatch.setattr(snapshot, "take", lambda: snap)
    got = []
    w = workers.SnapshotWorker()
    w.got.connect(got.append)
    w.run()
    assert got == [snap]


def test_snapshot_failure_emits_default_snapshot(qtbot, monkeypatch):
    def boom():
        raise RuntimeError("no adapters")

    monkeypatch.setattr(snapshot, "take", boom)
    got = []
    w = workers.SnapshotWorker()
    w.got.connect(got.append)
    w.run()
    assert len(got) == 1 and isinstance(got[0], snapshot.Snapshot)


# ---------------- PlainCmdWorker ----------------

def test_plain_cmd_success_prefers_stdout(qtbot, monkeypatch):
    from syskit import proc

    seen = {}

    def fake_run(cmd, timeout=30):
        seen["timeout"] = timeout
        return proc.CmdResult(0, "out\n", "err")

    monkeypatch.setattr(proc, "run", fake_run)
    got = []
    w = workers.PlainCmdWorker(["cmd"], timeout=7)
    w.done.connect(lambda ok, out: got.append((ok, out)))
    w.run()
    assert got == [(True, "out")]
    assert seen["timeout"] == 7


def test_plain_cmd_nonzero_falls_back_to_stderr(qtbot, monkeypatch):
    from syskit import proc

    monkeypatch.setattr(proc, "run",
                        lambda cmd, timeout=30: proc.CmdResult(1, "", "bad things"))
    got = []
    w = workers.PlainCmdWorker(["cmd"])
    w.done.connect(lambda ok, out: got.append((ok, out)))
    w.run()
    assert got == [(False, "bad things")]


def test_plain_cmd_unexpected_exception_reports_failure(qtbot, monkeypatch):
    from syskit import proc

    def boom(cmd, timeout=30):
        raise RuntimeError("sentinel")

    monkeypatch.setattr(proc, "run", boom)
    got = []
    w = workers.PlainCmdWorker(["cmd"])
    w.done.connect(lambda ok, out: got.append((ok, out)))
    w.run()
    ok, out = got[0]
    assert ok is False and out == "sentinel"
