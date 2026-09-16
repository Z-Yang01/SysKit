"""2026-09-16 覆盖率缺口补齐第二批：theme/workers/stores/telemetry 防御分支。

覆盖目标（当夜 coverage --term-missing 实测缺口）：
- syskit/ui/theme.py      ：sub_color()、add_shadow() 特效构造失败降级 None；
- syskit/ui/workers.py    ：NetCheckWorker.cancel() 中止标记、run() 异常兜底发空结果；
- syskit/stores.py        ：_save 落盘失败清理临时文件（unlink 二次失败仍 False）、
                            TrafficStore.add 零增量早退不标脏；
- syskit/telemetry.py     ：_top_procs 单进程信息读取异常不中断整轮采样
                            （153-156/167 为不可达防御代码：name 经 `or str(pid)` 恒真值、
                            非重建轮 seen 恒等于缓存键集——记录于 PROGRESS，不强凑）。

安全（叠加 conftest 哨兵）：全程无真实系统调用——netcheck.run_all 打桩、
os.replace/unlink 打桩（持久化目录已被 conftest 重定向到 tmp）、特效类打桩。
"""
import pytest
from types import SimpleNamespace


# ============================================================
# theme.py
# ============================================================

def test_theme_sub_color_returns_current_palette():
    from syskit.ui import theme

    assert theme.sub_color() == theme._current["sub"]


def test_theme_add_shadow_degrades_to_none_on_effect_error(qapp, monkeypatch):
    """特效构造失败 → 返回 None，不影响控件本身（卡片投影为纯增强）。"""
    from PySide6.QtWidgets import QWidget, QGraphicsDropShadowEffect
    from syskit.ui import theme

    def boom(*a, **k):
        raise RuntimeError("graphics effect unavailable")

    monkeypatch.setattr(QGraphicsDropShadowEffect, "__init__", boom)
    widget = QWidget()
    assert theme.add_shadow(widget) is None


# ============================================================
# workers.py
# ============================================================

def test_netcheck_worker_cancel_sets_flag(qapp):
    from syskit.ui.workers import DiagnosisWorker

    w = DiagnosisWorker(quick=True)
    assert w._cancelled is False
    w.cancel()                       # 正在执行的检查不可中断，其余跳过
    assert w._cancelled is True


def test_netcheck_worker_run_exception_emits_empty(qapp, monkeypatch):
    """诊断线程内异常：记日志 + 发空结果（调用方界面不能永久忙碌）。"""
    from syskit.ui import workers as workers_mod
    from syskit.ui.workers import DiagnosisWorker

    def boom(*a, **k):
        raise RuntimeError("probe exploded")

    monkeypatch.setattr(workers_mod.netcheck, "run_all", boom)
    w = DiagnosisWorker(quick=False)
    got = []
    w.finished_ok.connect(lambda r: got.append(r))
    w.run()                          # 同步直跑，不起线程
    assert got == [[]]


# ============================================================
# stores.py
# ============================================================

def _patch_save_fail(monkeypatch, unlinked):
    import syskit.stores as stores_mod

    def boom_replace(*a, **k):
        raise OSError("disk full")

    def fake_unlink(path):
        if unlinked is None:
            raise OSError("unlink denied")
        unlinked.append(path)

    monkeypatch.setattr(stores_mod.os, "replace", boom_replace)
    monkeypatch.setattr(stores_mod.os, "unlink", fake_unlink)


def test_store_save_replace_failure_cleans_tmp(monkeypatch):
    import syskit.stores as stores_mod

    unlinked = []
    _patch_save_fail(monkeypatch, unlinked)
    assert stores_mod._save("traffic.json", {"x": 1}) is False
    assert len(unlinked) == 1
    assert unlinked[0].endswith("traffic.json.tmp")


def test_store_save_cleanup_failure_still_false(monkeypatch):
    """tmp 清理本身也失败（unlink 二次 OSError）→ 仍返回 False 不外抛。"""
    import syskit.stores as stores_mod

    _patch_save_fail(monkeypatch, None)
    assert stores_mod._save("traffic.json", {"x": 1}) is False


def test_traffic_store_zero_delta_not_marked_dirty(tmp_path):
    from syskit.stores import TrafficStore

    st = TrafficStore()
    assert st._dirty is False
    st.add(0, 0)                     # 无增量：不标脏（避免空转写盘）
    assert st._dirty is False


# ============================================================
# telemetry.py
# ============================================================

def test_top_procs_single_proc_error_skipped():
    """单个进程信息读取异常 → 跳过该进程，不中断整轮采样。"""
    from syskit.telemetry import TelemetryWorker

    def boom(*a, **k):
        raise ValueError("process vanished mid-read")

    bad = SimpleNamespace(pid=4242, info={"name": "gone.exe"},
                          cpu_percent=boom, memory_info=boom)
    stub = SimpleNamespace(_tickno=2, _procs={4242: bad})

    out = TelemetryWorker._top_procs(stub, n=5)

    assert out == []
    assert 4242 in stub._procs       # 非重建轮缓存清理分支为不可达防御（见文件头）
