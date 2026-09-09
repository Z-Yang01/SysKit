"""QThread 工作线程：诊断与修复在后台执行，信号回报主线程。"""
import traceback

from PySide6.QtCore import QThread, Signal

from .. import netcheck, repairs
from ..logger import LOGGER


class DiagnosisWorker(QThread):
    progress = Signal(object)      # CheckResult
    finished_ok = Signal(list)     # list[CheckResult]

    def __init__(self, quick=False, parent=None):
        super().__init__(parent)
        self.quick = quick

    def run(self):
        try:
            results = netcheck.run_all(progress=lambda r: self.progress.emit(r), quick=self.quick)
            self.finished_ok.emit(results)
        except Exception:
            LOGGER.error("诊断线程异常:\n{}", traceback.format_exc())
            self.finished_ok.emit([])


class RepairWorker(QThread):
    done = Signal(object)          # (action_id, RepairResult)

    def __init__(self, action_id, param=None, parent=None):
        super().__init__(parent)
        self.action_id = action_id
        self.param = param or {}

    def run(self):
        act = repairs.get_action(self.action_id)
        if not act:
            return
        result = act.execute(self.param)
        self.done.emit((self.action_id, result))


class ComboWorker(QThread):
    """按顺序执行一组修复动作，逐项回报。"""
    item_done = Signal(object)     # (action_id, RepairResult)
    all_done = Signal()

    def __init__(self, action_ids, parent=None):
        super().__init__(parent)
        self.action_ids = action_ids

    def run(self):
        for aid in self.action_ids:
            act = repairs.get_action(aid)
            if not act:
                continue
            result = act.execute({})
            self.item_done.emit((aid, result))
        self.all_done.emit()


class SnapshotWorker(QThread):
    got = Signal(object)           # Snapshot

    def run(self):
        from ..snapshot import take
        self.got.emit(take())


def action_summary(aid: str):
    act = repairs.get_action(aid)
    return act
