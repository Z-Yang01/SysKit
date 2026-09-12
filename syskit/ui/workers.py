"""QThread 工作线程：诊断与修复在后台执行，信号回报主线程。"""
import traceback

from PySide6.QtCore import QThread, Signal

from .. import netcheck, repairs
from ..repairs import RepairResult
from ..logger import LOGGER


class DiagnosisWorker(QThread):
    progress = Signal(object)      # CheckResult
    finished_ok = Signal(list)     # list[CheckResult]

    def __init__(self, quick=False, parent=None):
        super().__init__(parent)
        self.quick = quick
        self._cancelled = False

    def cancel(self):
        """请求中止：正在执行的检查不可中断，其余检查跳过。"""
        self._cancelled = True

    def run(self):
        try:
            results = netcheck.run_all(
                progress=lambda r: self.progress.emit(r), quick=self.quick,
                should_stop=lambda: self._cancelled)
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
            # 必须发 done，否则调用方 _repair_worker 永不复位、界面永久忙碌
            self.done.emit((self.action_id, RepairResult(False, f"未知修复动作: {self.action_id}")))
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
        self._cancelled = False
        self.cancelled = False     # run 结束后供主线程读取：是否被中止

    def cancel(self):
        """请求中止：正在执行的动作不可中断（系统操作无法撤销），其余跳过。"""
        self._cancelled = True

    def run(self):
        try:
            for aid in self.action_ids:
                if self._cancelled:
                    self.cancelled = True
                    break
                act = repairs.get_action(aid)
                if not act:
                    continue
                result = act.execute({})
                self.item_done.emit((aid, result))
        except Exception:
            # execute() 本身已兜底，这里再保一层：无论如何都要发 all_done，
            # 否则调用方的 _combo_worker 永不复位、界面永久忙碌。
            LOGGER.error("组合修复线程异常:\n{}", traceback.format_exc())
        finally:
            if self._cancelled:
                self.cancelled = True
            self.all_done.emit()


class SnapshotWorker(QThread):
    got = Signal(object)           # Snapshot

    def run(self):
        from ..snapshot import take
        try:
            snap = take()
        except Exception:
            # 快照失败也必须回报，否则 _snap_worker 永不复位、「刷新网卡」永久失效
            LOGGER.error("网卡快照线程异常:\n{}", traceback.format_exc())
            from ..snapshot import Snapshot
            snap = Snapshot()
        self.got.emit(snap)


def action_summary(aid: str):
    act = repairs.get_action(aid)
    return act
