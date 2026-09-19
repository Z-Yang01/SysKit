"""QThread 工作线程：诊断与修复在后台执行，信号回报主线程。"""
import traceback

from PySide6.QtCore import QThread, Signal

from .. import netcheck, repairs
from ..repairs import RepairResult
from ..logger import LOGGER


def reap(worker: QThread, owner=None) -> QThread:
    """统一 worker 生命周期设施：C++ 对象生命周期与 Python 引用解耦。

    - finished → deleteLater：线程真正结束后由事件循环删除 C++ 对象，
      彻底消除"emit 后收尾微秒窗口内被 GC 析构 → qFatal 0xC0000409"的家族性窗口
      （槽内 `self._worker = None` 释放 Python 引用从此安全）。
    - setParent(owner)：挂到宿主控件的对象树，退出时 quit_app 可沿
      findChildren(QThread) 全树枚举统一等待，不再依赖逐处清单。
    """
    worker.finished.connect(worker.deleteLater)
    if owner is not None:
        worker.setParent(owner)
    return worker


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


class PlainCmdWorker(QThread):
    """一次性本地命令（命令面板自定义命令）：proc.run 最长可阻塞 30s，
    必须在后台线程跑，结果经 done 回 UI 气泡。"""
    done = Signal(bool, str)       # (ok, 合并后的输出摘要)

    def __init__(self, cmd, timeout=30, parent=None):
        super().__init__(parent)
        self._cmd = cmd
        self._timeout = timeout

    def run(self):
        from .. import proc
        try:
            r = proc.run(self._cmd, timeout=self._timeout)
            ok = getattr(r, "code", 1) == 0
            out = (getattr(r, "stdout", "") or getattr(r, "stderr", "") or "").strip()
        except Exception as e:
            ok, out = False, str(e)
        self.done.emit(ok, out)

