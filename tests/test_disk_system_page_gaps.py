"""覆盖率补强：syskit/diskusage.py、syskit/ui/disk_page.py、syskit/ui/system_page.py。

与既有 test_diskusage.py / test_ui_smoke.py / test_ui_system_units.py 互补，只补缺口：
- diskusage：_sh_delete 本体（对必然不存在的路径调用 SHFileOperationW，只验证返回
  码路径，不删除任何真实文件）、非 win32 平台分支、list_drives 过滤/OSError 分支、
  scan_tree 的迭代器 OSError / stat OSError / 子目录打不开 / heapreplace / 进度回调。
- disk_page：DiskScanWorker/DeleteWorker 同步 run（coverage 不追踪 QThread 线程，
  与既有做法一致直接调 run()）、stop()、_is_subpath、refresh_drives 异常、scan 守卫、
  _stop_scan、_scan_done 的取消/待提示合并分支、_fill_children 守卫、_collect 的
  表格选中行 + 父子去重、delete_checked 全链路（删除一律 mock）、_reveal 真路径。
- system_page：_ClipActionWorker 三分支、ClipboardTab 的 mime 文件/图片/HTML/
  异常降级四分支、暂停时读剪贴板异常、_run_action 全部动作分发（url/二维码/颜色/
  DNS/ping/IP 归属地 —— 网络/对话框全 mock，QDialog.exec 短路防模态阻塞）。

安全：删除/网络/注册表写全部 mock；SHFileOperationW 仅作用于不存在的路径。
"""
import os
import sys
import time
import types

import pytest

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from PySide6.QtCore import QUrl, Qt
from PySide6.QtGui import QImage

from syskit import diskusage as du


def _install_qt_msg_sink():
    try:
        from PySide6.QtCore import qInstallMessageHandler
    except ImportError:
        return
    import tempfile

    def _sink(msg_type, context, message):
        try:
            with open(os.path.join(tempfile.gettempdir(), "syskit_gaps_sink.log"),
                      "a", encoding="utf-8") as fh:
                fh.write("[qt] {}\n".format(message))
        except Exception:
            pass

    qInstallMessageHandler(_sink)


_install_qt_msg_sink()


# ================= diskusage.py =================

def test_sh_delete_builds_op_and_returns_success(tmp_path, monkeypatch):
    """_sh_delete 本体：mock 掉 _shell32.SHFileOperationW，验证结构体字段、
    返回码 0 且未中止 → True；返回码非 0 / 用户中止 → False。全程无真实 shell 调用。"""
    ops = []

    class _FakeShell:
        @staticmethod
        def SHFileOperationW(ptr):
            import ctypes
            op = ctypes.cast(ptr, ctypes.POINTER(du._SHFILEOPSTRUCTW)).contents
            ops.append((op.wFunc, op.pFrom, op.fFlags))
            return _FakeShell.ret

    _FakeShell.ret = 0
    monkeypatch.setattr(du, "_shell32", _FakeShell)
    assert du._sh_delete(str(tmp_path / "a.bin") + "\0") is True
    # ctypes 读回 c_wchar_p 时剥掉结尾的 \0，这里只比对内容
    assert ops[-1] == (du.FO_DELETE, str(tmp_path / "a.bin"), du._FOF_FLAGS)

    _FakeShell.ret = 5                      # shell 错误码 → False
    assert du._sh_delete("C:\\missing.bin\0") is False

    class _AbortedShell:
        @staticmethod
        def SHFileOperationW(ptr):
            import ctypes
            op = ctypes.cast(ptr, ctypes.POINTER(du._SHFILEOPSTRUCTW)).contents
            op.fAnyOperationsAborted = 1    # 返回 0 但用户中止 → 仍视为失败
            return 0

    monkeypatch.setattr(du, "_shell32", _AbortedShell)
    assert du._sh_delete("C:\\missing.bin\0") is False


def test_delete_to_recycle_bin_non_windows(monkeypatch):
    """非 win32 平台：整批返回 (0, 全部路径)，不触碰 shell。"""
    calls = []
    monkeypatch.setattr(du, "_sh_delete", lambda p: calls.append(p))
    monkeypatch.setattr(sys, "platform", "linux")
    ok, errors = du.delete_to_recycle_bin([r"C:\a.txt", r"C:\b.txt"])
    assert (ok, errors) == (0, [r"C:\a.txt", r"C:\b.txt"])
    assert calls == []


def test_list_drives_skips_empty_fstype_cdrom_and_oserror(monkeypatch):
    """list_drives：无 fstype / 光驱 / disk_usage OSError 三种跳过分支。"""
    import psutil

    parts = [
        types.SimpleNamespace(fstype="", mountpoint="A:\\", opts=""),
        types.SimpleNamespace(fstype="CDFS", mountpoint="D:\\", opts="cdrom"),
        types.SimpleNamespace(fstype="NTFS", mountpoint="E:\\", opts=""),
        types.SimpleNamespace(fstype="NTFS", mountpoint="F:\\", opts=""),
    ]
    monkeypatch.setattr(psutil, "disk_partitions", lambda all=False: parts)

    def fake_usage(mp):
        if mp == "E:\\":
            return types.SimpleNamespace(total=100, used=60, free=40)
        raise OSError("not ready")

    monkeypatch.setattr(psutil, "disk_usage", fake_usage)
    drives = du.list_drives()
    assert len(drives) == 1
    d = drives[0]
    assert (d.letter, d.mountpoint, d.total, d.used, d.free) == ("E:", "E:\\", 100, 60, 40)


class _OneIter:
    """定长目录迭代器假件（带 close，接口对齐 os.scandir 返回值）。"""

    def __init__(self, items):
        self._it = iter(items)

    def __iter__(self):
        return self

    def __next__(self):
        return next(self._it)

    def close(self):
        pass


class _BoomIter:
    def __iter__(self):
        return self

    def __next__(self):
        raise OSError("dir vanished")

    def close(self):
        pass


def test_scan_tree_subdir_iterator_oserror(tmp_path, monkeypatch):
    """子目录迭代器首次 next 即抛 OSError → 该子目录计 1 个错误并继续扫描其余。"""
    root = tmp_path / "tree"
    (root / "sub").mkdir(parents=True)
    (root / "a.txt").write_bytes(b"x" * 10)

    real_scandir = os.scandir

    def fake_scandir(path):
        if os.path.normcase(str(path)).endswith("sub"):
            return _BoomIter()
        return real_scandir(path)

    monkeypatch.setattr(os, "scandir", fake_scandir)
    res = du.scan_tree(str(root))
    by_name = {c.name: c for c in res.root.children}
    assert by_name["sub"].error_count == 1
    assert res.error_count == 1
    assert res.root.total == 10


def test_scan_tree_entry_stat_oserror(tmp_path, monkeypatch):
    """单个条目 stat 失败 → 跳过并计错误，不影响其余扫描。"""
    root = tmp_path / "tree"
    root.mkdir()
    real_scandir = os.scandir

    class _GhostEnt:
        name = "ghost.bin"
        path = str(root / "ghost.bin")

        def stat(self, follow_symlinks=False):
            raise OSError("stat failed")

    def fake_scandir(path):
        if os.path.normcase(str(path)) == os.path.normcase(str(root)):
            return _OneIter([_GhostEnt()])
        return real_scandir(path)

    monkeypatch.setattr(os, "scandir", fake_scandir)
    res = du.scan_tree(str(root))
    assert res.error_count == 1
    assert res.file_count == 0


def test_scan_tree_child_scandir_oserror(tmp_path, monkeypatch):
    """子目录条目存在但 scandir 被拒 → child.error_count=1，兄弟文件正常累计。"""
    root = tmp_path / "tree"
    root.mkdir()
    stat_src = tmp_path / "realstat"
    stat_src.mkdir()
    plain = tmp_path / "a.txt"
    plain.write_bytes(b"x" * 10)

    real_stat = os.stat

    class _Ent:
        def __init__(self, name, path, src):
            self.name, self.path = name, path
            self._st = real_stat(str(src))

        def stat(self, follow_symlinks=False):
            return self._st

    def fake_scandir(path):
        p = os.path.normcase(str(path))
        if p == os.path.normcase(str(root)):
            return _OneIter([_Ent("locked", str(root / "locked"), stat_src),
                             _Ent("a.txt", str(plain), plain)])
        raise OSError("access denied")

    monkeypatch.setattr(os, "scandir", fake_scandir)
    res = du.scan_tree(str(root))
    by_name = {c.name: c for c in res.root.children}
    assert by_name["locked"].error_count == 1
    assert res.error_count == 1
    assert res.root.total == 10


def test_scan_tree_heap_replace_and_progress_callback(tmp_path):
    """超过 200 个文件后更大的文件走 heapreplace；第 500 个文件触发进度回调。

    注意 scandir 按名称序返回：大文件命名 zz* 保证排在 500 个小文件之后，
    否则堆未满时大文件直接 heappush，heapreplace 分支不会触发。
    """
    root = tmp_path / "tree"
    root.mkdir()
    for i in range(500):
        (root / f"s{i:03d}.bin").write_bytes(b"x")
    for i in range(5):
        (root / f"zz{i}.bin").write_bytes(b"x" * 100)

    progress_calls = []
    res = du.scan_tree(str(root),
                       progress=lambda n, b, cur: progress_calls.append((n, b, cur)))
    sizes = [s for s, _p, _m in res.top_files]
    assert len(sizes) == 200
    assert sizes[:5] == [100] * 5                       # 大文件挤掉小文件（heapreplace）
    assert sizes[-1] == 1
    assert res.file_count == 505
    assert progress_calls and progress_calls[0][0] == 500
    assert progress_calls[0][2].endswith("tree")


# ================= disk_page.py =================

def _fake_scan_result(cancelled=False):
    from syskit.diskusage import DirNode, ScanResult
    root = DirNode(
        name="C:", path="C:\\", total=300, file_count=30,
        children=[DirNode(
            name="Users", path="C:\\Users", total=200, file_count=20,
            children=[DirNode(name="App", path="C:\\Users\\App",
                              total=100, file_count=10)])])
    return ScanResult(root=root, ext_stats={"zip": 200, "txt": 100},
                      top_files=[(200, "C:\\Users\\big.zip", time.time() - 60),
                                 (100, "C:\\note.txt", time.time())],
                      file_count=30, error_count=2, duration=1.0, cancelled=cancelled)


def _sync_worker_starts(monkeypatch):
    """两个 worker 的 start() 替换为主线程同步 run()（coverage 不追踪 QThread 线程）。"""
    from syskit.ui import disk_page as dp

    def _sync(self):
        self.run()

    monkeypatch.setattr(dp.DiskScanWorker, "start", _sync)
    monkeypatch.setattr(dp.DeleteWorker, "start", _sync)


def test_disk_scan_worker_run_progress_and_stop(monkeypatch):
    """DiskScanWorker.run 同步执行：进度信号格式 / done 结果 / stop 置位 / 异常降级。"""
    from syskit.ui import disk_page as dp

    seen_kwargs = {}

    def fake_scan(path, stop=None, progress=None):
        seen_kwargs["path"] = path
        progress(1, 10, "C:\\x")
        return _fake_scan_result()

    monkeypatch.setattr(du, "scan_tree", fake_scan)
    w = dp.DiskScanWorker("C:\\x")
    progress_msgs, results = [], []
    w.progress.connect(progress_msgs.append)
    w.done.connect(results.append)
    w.run()
    assert seen_kwargs["path"] == "C:\\x"
    assert progress_msgs == ["已扫描 1 个文件 / 10B　C:\\x"]
    assert results and results[0].root.total == 300

    w.stop()
    assert w._stop.is_set()

    # 扫描函数抛异常 → done(None)（UI 显示扫描失败）
    def boom(path, stop=None, progress=None):
        raise RuntimeError("scan exploded")

    monkeypatch.setattr(du, "scan_tree", boom)
    w2 = dp.DiskScanWorker("C:\\x")
    results2 = []
    w2.done.connect(results2.append)
    w2.run()
    assert results2 == [None]


def test_delete_worker_exception_reports_all_failed(monkeypatch, qtbot):
    """DeleteWorker：delete_to_recycle_bin 抛异常 → (0, 全部路径)。"""
    from syskit.ui import disk_page as dp

    def boom(paths):
        raise RuntimeError("shell gone")

    monkeypatch.setattr(du, "delete_to_recycle_bin", boom)
    w = dp.DeleteWorker(["C:\\a.txt", "C:\\b.txt"])
    out = []
    w.done.connect(lambda ok, errors: out.append((ok, errors)))
    w.run()
    assert out == [(0, ["C:\\a.txt", "C:\\b.txt"])]


def test_is_subpath_semantics():
    from syskit.ui import disk_page as dp
    assert dp._is_subpath(r"C:\a\b", r"C:\a")
    assert dp._is_subpath(r"C:\A\B", r"C:\a")            # normcase 不区分大小写
    assert not dp._is_subpath(r"C:\a", r"C:\a")          # 自身不算子路径
    assert not dp._is_subpath(r"C:\ab", r"C:\a")         # 仅前缀相同不算
    assert not dp._is_subpath(r"C:\a", r"C:\a\b")


def test_disk_tab_refresh_drives_list_error(qtbot, monkeypatch):
    """枚举磁盘抛异常 → 记日志，下拉/扫描按钮禁用。"""
    from syskit.ui import disk_page as dp

    def boom():
        raise RuntimeError("wmi gone")

    monkeypatch.setattr(du, "list_drives", boom)
    tab = dp.DiskTab()
    qtbot.addWidget(tab)
    assert tab.combo.count() == 0
    assert not tab.btn_scan.isEnabled()


def test_disk_tab_scan_guards_no_disk_and_stop(qtbot, monkeypatch, msgbox_noop):
    """scan 防重入守卫、无磁盘提示分支、_stop_scan 停止路径。"""
    from syskit.ui import disk_page as dp

    tab = dp.DiskTab()
    qtbot.addWidget(tab)

    sentinel = object()                                 # 已有 worker → 直接返回
    tab._worker = sentinel
    tab.scan("C:\\")
    assert tab._worker is sentinel                      # 未被替换（早退）
    tab._worker = None

    tab.combo.clear()                                   # 无可用磁盘 → 仅提示
    tab.scan()
    kinds = [k for k, _a in msgbox_noop]
    assert kinds[-1] == "information"
    assert "未找到可用磁盘" in msgbox_noop[-1][1][2]

    w = dp.DiskScanWorker("C:\\x")                      # _stop_scan：禁用按钮 + 置停止位
    tab._worker = w
    tab.btn_stop.setEnabled(True)
    tab._stop_scan()
    assert not tab.btn_stop.isEnabled()
    assert w._stop.is_set()


def test_disk_tab_scan_done_cancelled_and_pending_merge(qtbot):
    """_scan_done：已取消后缀、删除结果与重扫消息合并、失败分支。"""
    from syskit.ui import disk_page as dp

    tab = dp.DiskTab()
    qtbot.addWidget(tab)

    tab._pending_msg = "已删除 2 项"
    tab._scan_done(_fake_scan_result())
    text = tab.lbl_result.text()
    assert text.startswith("已删除 2 项；扫描完成")     # pending 合并并清空
    assert tab._pending_msg == ""

    tab._scan_done(_fake_scan_result(cancelled=True))
    assert "（已停止，结果不完整）" in tab.lbl_result.text()

    tab._scan_done(None)
    assert "扫描失败" in tab.lbl_progress.text()


def test_disk_tab_fill_children_guard_and_lazy(qtbot):
    """_fill_children：已填充/无结果早退；展开懒加载替换占位。"""
    from syskit.ui import disk_page as dp

    tab = dp.DiskTab()
    qtbot.addWidget(tab)

    empty_item = dp.QTreeWidgetItem()                   # 无结果 → 早退
    tab._fill_children(empty_item)
    assert empty_item.childCount() == 0

    tab._scan_done(_fake_scan_result())
    root_item = tab.tree.topLevelItem(0)
    assert root_item.child(0).text(0) == "Users"        # _render 的 setExpanded 已填充
    tab._fill_children(root_item)                       # ROLE_FILLED 置位 → 早退
    assert root_item.childCount() == 1

    users = root_item.child(0)
    assert users.childCount() == 1                      # 占位
    users.setExpanded(True)                             # 触发懒加载 App
    assert users.child(0).text(0) == "App"
    assert users.data(0, dp.ROLE_FILLED) is True


def test_disk_tab_collect_table_selection_and_ancestor_dedup(qtbot):
    """_collect：大文件表选中行入集；父子同时勾选只保留祖先。"""

    from syskit.ui import disk_page as dp

    tab = dp.DiskTab()
    qtbot.addWidget(tab)
    tab._scan_done(_fake_scan_result())

    root_item = tab.tree.topLevelItem(0)
    users = root_item.child(0)
    users.setExpanded(True)
    app = users.child(0)
    users.setCheckState(0, Qt.Checked)
    app.setCheckState(0, Qt.Checked)
    assert tab._collect() == [("C:\\Users", 200)]       # 子路径被祖先吸收
    users.setCheckState(0, Qt.Unchecked)
    app.setCheckState(0, Qt.Unchecked)

    tab.table.item(0, 0).setSelected(True)              # 表格选中行（big.zip）
    assert tab._collect() == [("C:\\Users\\big.zip", 200)]


def test_disk_tab_delete_checked_full_flow(qtbot, monkeypatch, msgbox_noop):
    """删除全链路：确认 → DeleteWorker（删除 mock）→ 结果合并重扫；取消与失败路径。"""
    from PySide6.QtWidgets import QMessageBox

    from syskit.ui import disk_page as dp

    _sync_worker_starts(monkeypatch)
    monkeypatch.setattr(du, "scan_tree",
                        lambda path, stop=None, progress=None: _fake_scan_result())
    tab = dp.DiskTab()
    qtbot.addWidget(tab)

    # 无结果时守卫直接返回（第一道闸）
    tab.delete_checked()
    assert tab._delete_worker is None

    tab._scanned_path = "C:\\fake"
    monkeypatch.setattr(du, "delete_to_recycle_bin",
                        lambda paths: (len(paths), []))
    tab._scan_done(_fake_scan_result())
    tab.table.item(0, 0).setSelected(True)

    tab.delete_checked()                                # Yes（conftest 默认）→ 删除 → 重扫
    assert tab._delete_worker is None
    assert "已删除 1 项" in tab.lbl_result.text()
    assert "扫描完成" in tab.lbl_result.text()          # 重扫结果已合并展示

    # 用户在确认框点 No → 不启动删除
    monkeypatch.setattr(QMessageBox, "question",
                        staticmethod(lambda *a, **k: QMessageBox.StandardButton.No))
    tab.delete_checked()
    assert tab._delete_worker is None

    # 删除全部失败 → 不重扫，结果标签橙色提示
    monkeypatch.setattr(QMessageBox, "question",
                        staticmethod(lambda *a, **k: QMessageBox.StandardButton.Yes))
    monkeypatch.setattr(du, "delete_to_recycle_bin",
                        lambda paths: (0, list(paths)))
    tab.delete_checked()
    assert tab._delete_worker is None
    assert "已删除 0 项" in tab.lbl_result.text()
    assert "1 个失败" in tab.lbl_result.text()


def test_disk_tab_delete_checked_with_delete_worker_guard(qtbot):
    """已有删除 worker 时 delete_checked 直接返回（第二道闸）。"""
    from syskit.ui import disk_page as dp

    tab = dp.DiskTab()
    qtbot.addWidget(tab)
    tab._scan_done(_fake_scan_result())
    sentinel = object()
    tab._delete_worker = sentinel
    tab.delete_checked()
    assert tab._delete_worker is sentinel               # 未被替换（早退）


def test_disk_tab_delete_checked_no_selection_hint(qtbot, monkeypatch, msgbox_noop):
    """有结果但无任何勾选/选中 → 仅提示，不启动删除。"""
    from syskit.ui import disk_page as dp

    tab = dp.DiskTab()
    qtbot.addWidget(tab)
    tab._scan_done(_fake_scan_result())
    tab.delete_checked()
    assert tab._delete_worker is None
    assert "请先在目录树勾选" in msgbox_noop[-1][1][2]


def test_disk_tab_delete_checked_over_10gb_warning(qtbot, monkeypatch, msgbox_noop):
    """选中总量超过 10 GB → 确认文案追加超限警示。"""
    from syskit.diskusage import DirNode, ScanResult

    from syskit.ui import disk_page as dp

    big = 11 * 1024 ** 3
    root = DirNode(name="C:", path="C:\\", total=big, file_count=1,
                   children=[DirNode(name="Users", path="C:\\Users",
                                     total=big, file_count=1)])
    huge = ScanResult(root=root, ext_stats={}, top_files=[],
                      file_count=1, error_count=0, duration=0.1)

    _sync_worker_starts(monkeypatch)
    monkeypatch.setattr(du, "scan_tree",
                        lambda path, stop=None, progress=None: huge)
    monkeypatch.setattr(du, "delete_to_recycle_bin", lambda paths: (len(paths), []))
    tab = dp.DiskTab()
    qtbot.addWidget(tab)
    tab._scanned_path = "C:\\fake"
    tab._scan_done(huge)
    root_item = tab.tree.topLevelItem(0)
    root_item.child(0).setCheckState(0, Qt.Checked)     # Users（11GB）
    tab.delete_checked()
    body = msgbox_noop[-1][1][2]                        # question 确认文案
    assert body.startswith("⚠ 选中内容超过 10 GB")
    assert tab._delete_worker is None
    assert "已删除 1 项" in tab.lbl_result.text()


def test_disk_tab_reveal_existing_path_opens_explorer(qtbot, monkeypatch):
    """双击定位：路径存在 → explorer /select（Popen mock，绝不真启动）。"""
    from syskit.ui import disk_page as dp

    tab = dp.DiskTab()
    qtbot.addWidget(tab)
    tab._scan_done(_fake_scan_result())
    calls = []
    monkeypatch.setattr(dp.subprocess, "Popen", lambda *a, **k: calls.append(a))
    monkeypatch.setattr(os.path, "exists", lambda p: True)
    tab._reveal(tab.table.item(0, 0))
    assert calls
    cmd = calls[0][0]
    assert cmd[0] == "explorer" and "/select," in cmd
    assert "big.zip" in os.path.normpath(cmd[2])


# ================= system_page.py =================

class _FakeMime:
    def __init__(self, has_urls=False, urls=(), has_image=False,
                 has_html=False, html=""):
        self._has_urls, self._urls = has_urls, list(urls)
        self._has_image, self._has_html, self._html = has_image, has_html, html

    def hasUrls(self):
        return self._has_urls

    def urls(self):
        return list(self._urls)

    def hasImage(self):
        return self._has_image

    def hasHtml(self):
        return self._has_html

    def html(self):
        return self._html


class _FakeMimeClip:
    """带 mimeData/image 的剪贴板假件（text/setText 对齐既有 _FakeClip）。"""

    def __init__(self, parent=None):
        self._text = ""
        self._mime = None
        self._image = None
        self.set_calls = []

    def text(self):
        return self._text

    def setText(self, t):
        self.set_calls.append(t)
        self._text = t

    def mimeData(self):
        return self._mime

    def image(self):
        return self._image


def _make_clip_tab(qtbot):
    from syskit.ui.system_page import ClipboardTab
    tab = ClipboardTab()
    qtbot.addWidget(tab)
    tab.hist.clear()
    return tab


class TestClipActionWorker:
    def test_success_none_and_exception(self, qtbot):
        from syskit.ui.system_page import _ClipActionWorker

        w = _ClipActionWorker(lambda arg: f"res:{arg}", "x")
        out = []
        w.done.connect(out.append)
        w.run()
        assert out == ["res:x"]

        w2 = _ClipActionWorker(lambda arg: None, "x")
        out2 = []
        w2.done.connect(out2.append)
        w2.run()
        assert out2 == ["完成"]

        def boom(arg):
            raise RuntimeError("net down")

        w3 = _ClipActionWorker(boom, "x")
        out3 = []
        w3.done.connect(out3.append)
        w3.run()
        assert out3 == ["失败: net down"]


class TestOnClipChangeMime:
    def test_file_image_html_and_exception_fallback(self, qtbot, monkeypatch):
        from syskit import clipboard as clip_mod

        monkeypatch.setattr(clip_mod, "should_record_now", lambda *a, **k: True)
        tab = _make_clip_tab(qtbot)
        fake = _FakeMimeClip()
        tab._clip = fake

        # 文件 URL 分支：只统计本地文件
        fake._mime = _FakeMime(has_urls=True, urls=[
            QUrl.fromLocalFile("C:/a.txt"), QUrl("https://example.com/x")])
        fake._text = "C:/a.txt"
        tab._on_clip_change()
        items = tab.hist.items()
        assert len(items) == 1 and items[0]["kind"] == "file"
        assert items[0]["text"] == "C:/a.txt"

        # 图片分支：真实 QImage → PNG 落盘（隔离目录）→ image 条目
        img = QImage(4, 4, QImage.Format_ARGB32)
        img.fill(0xFF112233)
        fake._image = img
        fake._mime = _FakeMime(has_image=True)
        fake._text = "[图片 4×4]"
        tab._on_clip_change()
        kinds = [it["kind"] for it in tab.hist.items()]
        assert "image" in kinds
        image_item = next(it for it in tab.hist.items() if it["kind"] == "image")
        assert image_item["text"] == "[图片 4×4]"
        assert image_item["data"].endswith(".png")

        # HTML 分支：html 与纯文本不同才入库
        fake._mime = _FakeMime(has_html=True, html="<b>bold body</b>")
        fake._text = "bold body"
        tab._on_clip_change()
        kinds = [it["kind"] for it in tab.hist.items()]
        assert "html" in kinds

        # mime 读取异常 → 降级按纯文本入库
        class _BoomMime:
            def hasUrls(self):
                raise RuntimeError("mime provider gone")

        fake._mime = _BoomMime()
        fake._text = "plain fallback"
        tab._on_clip_change()
        assert "plain fallback" in tab.hist.texts()

    def test_html_same_as_text_not_recorded(self, qtbot, monkeypatch):
        from syskit import clipboard as clip_mod

        monkeypatch.setattr(clip_mod, "should_record_now", lambda *a, **k: True)
        tab = _make_clip_tab(qtbot)
        fake = _FakeMimeClip()
        tab._clip = fake
        fake._mime = _FakeMime(has_html=True, html="  same  ")
        fake._text = "same"
        tab._on_clip_change()
        items = tab.hist.items()
        assert len(items) == 1 and items[0]["kind"] == "text"   # html 与文本相同 → 只记文本

    def test_mime_none_falls_back_to_text(self, qtbot, monkeypatch):
        from syskit import clipboard as clip_mod

        monkeypatch.setattr(clip_mod, "should_record_now", lambda *a, **k: True)
        tab = _make_clip_tab(qtbot)
        fake = _FakeMimeClip()
        tab._clip = fake

        class _NoMime:
            def mimeData(self):
                raise RuntimeError("clipboard format gone")

        fake.mimeData = _NoMime().mimeData
        fake._text = "just text"
        tab._on_clip_change()
        assert tab.hist.texts() == ["just text"]


def test_clipboard_toggle_pause_clip_read_error(qtbot):
    """暂停时读取剪贴板失败 → _last 置空串，不崩。"""
    tab = _make_clip_tab(qtbot)

    class _Boom:
        def text(self):
            raise RuntimeError("clipboard gone")

    tab._clip = _Boom()
    tab._toggle_pause()
    assert not tab.chk_watch.isChecked()
    assert tab.btn_pause.text() == "恢复监听"
    assert tab._last == ""


class TestRunActionDispatch:
    """_run_action 全部分发分支：网络/浏览器/对话框一律 mock，QDialog.exec 短路。"""

    @pytest.fixture(autouse=True)
    def _setup(self, qtbot, monkeypatch):
        from PySide6.QtGui import QDesktopServices
        from PySide6.QtWidgets import QDialog
        from syskit.ui import system_page as sp

        self.sp = sp
        self.tab = _make_clip_tab(qtbot)

        def _sync_start(self_worker):
            self_worker.run()

        monkeypatch.setattr(sp._ClipActionWorker, "start", _sync_start)
        self.opened = []
        monkeypatch.setattr(QDesktopServices, "openUrl",
                            lambda url: self.opened.append(url.toString()))
        monkeypatch.setattr(QDialog, "exec", lambda self: 0, raising=False)

    def test_url_open_and_file_actions(self):
        self.tab._run_action({"id": "url_open", "arg": "https://example.com"})
        self.tab._run_action({"id": "file_reveal", "arg": "C:\\dir\\file.txt"})
        self.tab._run_action({"id": "file_open", "arg": "C:\\dir\\file.txt"})
        assert self.opened[0] == "https://example.com"
        assert self.opened[1].endswith("C:/dir")            # file_reveal → 所在文件夹
        assert self.opened[2].endswith("C:/dir/file.txt")   # file_open → 文件本身

    def test_color_show_dialog_short_circuited(self):
        self.tab._run_action({"id": "color_show", "arg": "#ff8800"})   # exec 已短路

    def test_url_qr_success_and_qrcode_missing(self, monkeypatch):
        self.tab._run_action({"id": "url_qr", "arg": "https://example.com/qr"})
        import sys as _sys
        monkeypatch.setitem(_sys.modules, "qrcode", None)   # import qrcode → ImportError
        self.tab._run_action({"id": "url_qr", "arg": "https://example.com/qr"})

    def test_set_clipboard_text_without_clip(self):
        self.tab._clip = None
        self.tab._set_clipboard_text("t", "note")           # 无剪贴板句柄 → no-op

    def test_dns_lookup_success_and_empty(self, monkeypatch):
        from syskit import dnsquery
        monkeypatch.setattr(dnsquery, "query",
                            lambda host, *a, **k: ["1.2.3.4", "5.6.7.8"])
        self.tab._run_action({"id": "domain_dns", "arg": "example.com"})
        assert "example.com → 1.2.3.4, 5.6.7.8" in self.tab.lbl_detail.text()
        monkeypatch.setattr(dnsquery, "query", lambda host, *a, **k: [])
        self.tab._run_action({"id": "domain_dns", "arg": "quiet.com"})
        assert "无记录" in self.tab.lbl_detail.text()

    def test_ping_hit_and_miss(self, monkeypatch):
        from syskit import icmp
        monkeypatch.setattr(icmp, "ping", lambda host, timeout_ms=1500: 5.0)
        self.tab._run_action({"id": "domain_ping", "arg": "example.com"})
        assert "延迟 5.0 ms" in self.tab.lbl_detail.text()
        monkeypatch.setattr(icmp, "ping", lambda host, timeout_ms=1500: None)
        self.tab._run_action({"id": "ip_ping", "arg": "10.0.0.1"})
        assert "ping 不通" in self.tab.lbl_detail.text()

    def test_ip_geo_success_and_failure(self, monkeypatch):
        import json as _json
        import urllib.request

        class _Resp:
            def __init__(self, payload):
                self._p = payload

            def __enter__(self):
                return self

            def __exit__(self, *exc):
                return False

            def read(self):
                return _json.dumps(self._p).encode("utf-8")

        seen = []

        def fake_urlopen(url, timeout=5):
            seen.append(url)
            return _Resp({"status": "success", "country": "美国",
                          "regionName": "加州", "city": "洛杉矶", "isp": "TestISP"})

        monkeypatch.setattr(urllib.request, "urlopen", fake_urlopen)
        self.tab._run_action({"id": "ip_geo", "arg": "8.8.8.8"})
        assert seen and "ip-api.com" in seen[0]
        assert "洛杉矶" in self.tab.lbl_detail.text() and "TestISP" in self.tab.lbl_detail.text()

        monkeypatch.setattr(urllib.request, "urlopen",
                            lambda url, timeout=5: _Resp({"status": "fail"}))
        self.tab._run_action({"id": "ip_geo", "arg": "8.8.4.4"})
        assert "归属地查询失败" in self.tab.lbl_detail.text()

    def test_start_bg_reentry_guard(self):
        sentinel = object()
        self.tab._bg = sentinel                             # 已有后台任务 → 直接返回
        self.tab._run_action({"id": "domain_dns", "arg": "example.com"})
        assert self.tab._bg is sentinel                     # 未被替换（早退）
        self.tab._bg = None


class TestQrSave:
    def test_save_qr_writes_file_and_cancel_path(self, qtbot, monkeypatch, tmp_path):
        from PySide6.QtWidgets import QFileDialog

        tab = _make_clip_tab(qtbot)
        target = tmp_path / "out.png"
        monkeypatch.setattr(QFileDialog, "getSaveFileName",
                            staticmethod(lambda *a, **k: (str(target), "PNG (*.png)")))
        tab._save_qr(b"\x89PNG-fake-bytes")
        assert target.read_bytes() == b"\x89PNG-fake-bytes"

        monkeypatch.setattr(QFileDialog, "getSaveFileName",
                            staticmethod(lambda *a, **k: ("", "")))
        tab._save_qr(b"should-not-write")
        assert target.read_bytes() == b"\x89PNG-fake-bytes"  # 取消保存 → 不写


# ---------------- PerfTab / BatteryTab（ui_smoke 崩溃兜底，改为直测） ----------------


def test_perf_tab_update_tick_charts_and_proc_table(qtbot):
    """update_tick：cpu/mem 与磁盘读写推入图表；top_procs 刷新表格并保持排序状态。"""
    from syskit.ui.system_page import PerfTab

    tab = PerfTab()
    qtbot.addWidget(tab)

    tab.update_tick({"cpu": 12.0, "mem": 55.0})
    assert 12.0 in list(tab.chart_sys._series[0][2])
    assert 55.0 in list(tab.chart_sys._series[1][2])

    tab.update_tick({"disk_r": 1024, "disk_w": 2048})
    assert list(tab.chart_disk._series[0][2])[-1] == 1024
    assert list(tab.chart_disk._series[1][2])[-1] == 2048

    tab.update_tick({})                                 # 无任何键 → 图表不动
    tab.update_tick({"top_procs": None})                # 显式 None → 早退

    tab.update_tick({"top_procs": [
        {"pid": 7, "name": "busy.exe", "cpu": 9.5, "mem": 2048},
        {"pid": 3, "name": "idle.exe", "cpu": 0.6, "mem": 4096}]})
    assert tab.tbl.rowCount() == 2
    assert tab.tbl.item(0, 1).text() == "busy.exe"
    assert tab.tbl.item(0, 3).text() == "2.0KB"
    assert tab.tbl.item(1, 0).text() == "3"


def test_battery_tab_full_data_green_health(qtbot, monkeypatch):
    """电池数据齐全 → 容量/健康度/电量回填，健康度 >80% 绿色。"""
    from syskit import proc
    from syskit.ui.system_page import BatteryTab

    monkeypatch.setattr(proc, "run_ps",
                        lambda script, timeout=30, resident=False:
                        proc.CmdResult(0, "45000,40000,77", ""))
    monkeypatch.setattr("syskit.ui.system_page._BatteryWorker.start",
                        lambda self: self.run())            # worker 同步执行（项目惯例）
    tab = BatteryTab()                                  # 懒加载：构造期不查询
    qtbot.addWidget(tab)
    tab.refresh()                                       # 显式触发（首显由 showEvent 自动）
    assert tab.lbl_design.text() == "45000 mWh"
    assert tab.lbl_full.text() == "40000 mWh"
    assert tab.lbl_health.text() == "88.9%"
    assert tab.lbl_health.styleSheet().startswith("color:#2e9e44")
    assert tab.lbl_remain.text() == "77%"
    assert "电池报告" in tab.lbl_note.text()


def test_battery_tab_no_remain_shows_dash(qtbot, monkeypatch):
    """电量缺失（第三段为空）→ 健康度正常但当前电量显示 —。"""
    from syskit import proc
    from syskit.ui.system_page import BatteryTab

    monkeypatch.setattr(proc, "run_ps",
                        lambda script, timeout=30, resident=False:
                        proc.CmdResult(0, "50000,30000,", ""))
    monkeypatch.setattr("syskit.ui.system_page._BatteryWorker.start",
                        lambda self: self.run())            # worker 同步执行（项目惯例）
    tab = BatteryTab()
    qtbot.addWidget(tab)
    tab.refresh()                                       # 懒加载：显式触发
    assert tab.lbl_health.text() == "60.0%"             # 边界：不 >80、不 >60 → 红
    assert tab.lbl_health.styleSheet().startswith("color:#c62828")
    assert tab.lbl_remain.text() == "—"


def test_scan_blocked_while_delete_in_flight(qtbot, monkeypatch):
    """删除进行中 scan() 早退：否则自动重扫被在跑扫描顶掉、pending_msg 错挂。"""
    from syskit.ui import disk_page as dp
    tab = dp.DiskTab()
    qtbot.addWidget(tab)
    tab._delete_worker = object()          # 哨兵：模拟删除线程在飞
    tab.scan()
    assert tab._worker is None             # 早退，未起扫描
