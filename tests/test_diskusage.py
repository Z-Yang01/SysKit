"""syskit/diskusage.py 单元测试：目录树扫描 / 回收站删除（全程无真实删除）。"""
import os
import shutil
import threading

import pytest

from syskit import diskusage as du


# ---------------- 扫描 ----------------

def _build_tree(root):
    """构造已知大小的目录树：
    root/a.txt=1000B, root/b.log=3000B, root/sub/c.bin=2000B, root/sub/deep/d.txt=500B
    """
    os.makedirs(os.path.join(root, "sub", "deep"))
    with open(os.path.join(root, "a.txt"), "wb") as f:
        f.write(b"x" * 1000)
    with open(os.path.join(root, "b.log"), "wb") as f:
        f.write(b"y" * 3000)
    with open(os.path.join(root, "sub", "c.bin"), "wb") as f:
        f.write(b"z" * 2000)
    with open(os.path.join(root, "sub", "deep", "d.txt"), "wb") as f:
        f.write(b"w" * 500)


def _walk_size(path):
    total = 0
    for r, _dirs, files in os.walk(path):
        for fn in files:
            try:
                total += os.path.getsize(os.path.join(r, fn))
            except OSError:
                pass
    return total


def test_scan_tree_sizes_and_ext_stats(tmp_path):
    root = tmp_path / "tree"
    _build_tree(str(root))

    res = du.scan_tree(str(root))
    assert res.root.total == 6500
    assert res.file_count == 4
    assert res.error_count == 0
    assert not res.cancelled
    assert res.duration >= 0

    # 扩展名统计（无点小写）
    assert res.ext_stats == {"txt": 1500, "log": 3000, "bin": 2000}

    # 递归目录大小：sub = 2500，deep = 500
    by_name = {c.name: c for c in res.root.children}
    assert by_name["sub"].total == 2500
    assert by_name["sub"].file_count == 2
    deep = by_name["sub"].children[0]
    assert deep.name == "deep" and deep.total == 500

    # 与 os.walk 对账
    assert res.root.total == _walk_size(str(root))


def test_scan_tree_top_files_sorted(tmp_path):
    root = tmp_path / "tree"
    _build_tree(str(root))
    res = du.scan_tree(str(root))
    sizes = [s for s, _p, _m in res.top_files]
    assert sizes == sorted(sizes, reverse=True)
    assert sizes[0] == 3000
    assert any(p.endswith("b.log") for _s, p, _m in res.top_files)
    # mtime 是合理的时间戳
    assert all(m > 0 for _s, _p, m in res.top_files)


def test_scan_tree_junction_skipped(tmp_path):
    """junction/符号链接不入递归：环状 junction 不导致死循环或重复计数。"""
    import _winapi
    root = tmp_path / "tree"
    _build_tree(str(root))
    _winapi.CreateJunction(str(root / "sub"), str(root / "loop"))

    res = du.scan_tree(str(root))
    assert res.root.total == 6500                    # junction 内文件不重复计
    assert "loop" not in {c.name for c in res.root.children}


def test_scan_tree_cancel(tmp_path):
    root = tmp_path / "tree"
    _build_tree(str(root))
    stop = threading.Event()
    stop.set()
    res = du.scan_tree(str(root), stop=stop)
    assert res.cancelled


def test_scan_tree_progress_callback(tmp_path):
    root = tmp_path / "tree"
    _build_tree(str(root))
    calls = []

    def progress(files, bytes_seen, cur):
        calls.append((files, bytes_seen, cur))

    du.scan_tree(str(root), progress=progress)
    # 4 个文件 < 500，最多回调一次（也可能 0 次），但参数必须自洽
    for files, bytes_seen, cur in calls:
        assert files % 500 == 0
        assert bytes_seen <= 6500
        assert os.path.isdir(cur)


def test_scan_tree_missing_root(tmp_path):
    res = du.scan_tree(str(tmp_path / "no_such_dir"))
    assert res.root.total == 0
    assert res.file_count == 0
    assert res.error_count == 1


def test_scan_tree_deep_nesting_no_recursion_error(tmp_path):
    """300 层嵌套不爆 Python 递归栈（栈式遍历）。"""
    p = tmp_path
    for _ in range(300):
        p = p / "n"
    p.mkdir(parents=True)
    (p / "leaf.txt").write_bytes(b"deep")

    res = du.scan_tree(str(tmp_path))
    assert res.file_count == 1
    assert res.root.total == 4


def test_human_bytes():
    """human_bytes 的唯一实现在 ui/charts.py（diskusage 的重复份已删）。"""
    from syskit.ui.charts import human_bytes
    assert human_bytes(0) == "0B"
    assert human_bytes(999) == "999B"
    assert human_bytes(1024) == "1.0KB"
    assert human_bytes(1024 * 1024) == "1.0MB"
    assert human_bytes(1024 ** 3) == "1.0GB"
    assert human_bytes(1536) == "1.5KB"


def test_list_drives_returns_driveinfo():
    drives = du.list_drives()
    assert isinstance(drives, list)
    for d in drives:
        assert d.letter and d.mountpoint
        assert d.used + d.free <= d.total


# ---------------- 回收站删除（mock _sh_delete，绝不真实删除） ----------------

def test_delete_batch_builds_double_null_list(monkeypatch):
    calls = []

    def fake_sh_delete(pfrom):
        calls.append(pfrom)
        return True

    monkeypatch.setattr(du, "_sh_delete", fake_sh_delete)
    ok, errors = du.delete_to_recycle_bin([r"C:\a.txt", r"C:\dir"])
    assert (ok, errors) == (2, [])
    assert len(calls) == 1
    # 双 null 结尾的路径列表
    assert calls[0] == "C:\\a.txt\0C:\\dir\0"


def test_delete_fallback_per_path_on_batch_failure(monkeypatch):
    """整批失败时逐个回退定位，只标记真实失败的那一个。"""
    calls = []

    def fake_sh_delete(pfrom):
        calls.append(pfrom)
        return "\0" in pfrom          # 整批调用失败，单路径调用成功

    monkeypatch.setattr(du, "_sh_delete", fake_sh_delete)
    ok, errors = du.delete_to_recycle_bin([r"C:\good.txt", r"C:\bad.txt"])
    assert ok == 2 and errors == []


def test_delete_reports_failed_path(monkeypatch, tmp_path):
    good = tmp_path / "good.txt"
    bad = tmp_path / "bad.txt"
    good.write_bytes(b"a")
    bad.write_bytes(b"b")

    def fake_sh_delete(pfrom):
        # 整批调用（两个以上 null）一律失败；单路径调用中只有 good 成功
        return pfrom.count("\0") == 1 and "good" in pfrom

    monkeypatch.setattr(du, "_sh_delete", fake_sh_delete)
    ok, errors = du.delete_to_recycle_bin([str(good), str(bad)])
    assert ok == 1
    assert [os.path.basename(e) for e in errors] == ["bad.txt"]


def test_delete_empty_input(monkeypatch):
    monkeypatch.setattr(du, "_sh_delete", lambda p: pytest.fail("不应调用"))
    assert du.delete_to_recycle_bin([]) == (0, [])


def test_delete_aborted_counts_as_failure(monkeypatch, tmp_path):
    """SHFileOperation 返回 0 但 fAnyOperationsAborted 置位 → 视为失败。"""
    f = tmp_path / "x.txt"
    f.write_bytes(b"x")
    monkeypatch.setattr(du, "_sh_delete", lambda p: False)
    ok, errors = du.delete_to_recycle_bin([str(f)])
    assert ok == 0 and errors == [str(f)]


def test_delete_retry_skips_vanished_paths(monkeypatch, tmp_path):
    """整批部分失败重试时，首轮已删掉（路径已消失）的文件不再重试、不计失败。"""
    gone = tmp_path / "gone.txt"        # 首轮已删：重试前已不存在
    stuck = tmp_path / "stuck.txt"      # 首轮未删且删不掉
    gone.write_bytes(b"g")
    stuck.write_bytes(b"s")

    def fake_sh_delete(pfrom):
        # 整批调用失败（模拟中途遇锁定文件中止）；单路径只有 stuck 失败
        if pfrom.count("\0") > 1:
            return False
        return "stuck" not in pfrom

    monkeypatch.setattr(du, "_sh_delete", fake_sh_delete)
    ok, errors = du.delete_to_recycle_bin([str(gone), str(stuck)])
    assert ok == 1 and errors == [str(stuck)]   # gone 计成功而非"失败（无权限）"
