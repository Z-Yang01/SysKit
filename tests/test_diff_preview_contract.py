"""tools/diff_preview.py 的退出码契约测试。

背景：git 历史迭代日志 补充53 曾误记「diff_preview 整桌 ONLY-A 而退出码仍是 0」——
实测当前实现（ONLY-A/ONLY-B/SIZE-DIFF 均 failed=True → 返回 1）是对的，
本文件把契约钉死，防止未来改挂：全部一致=0；任一 DIFF/SIZE-DIFF/ONLY-A/ONLY-B=1。

实现方式：importlib 直载模块、monkeypatch sys.argv 后调 main()——不走 subprocess
（conftest 红线哨兵拦测试内 subprocess.*）。QImage 无 QApplication 也可读写，
仍挂 qapp 保稳（教训：纯逻辑测试文件用到 QtGui 前先想清楚）。
"""
import importlib.util
import sys
from pathlib import Path

import pytest
from PySide6.QtGui import QColor, QImage

ROOT = Path(__file__).resolve().parent.parent
_spec = importlib.util.spec_from_file_location(
    "diff_preview", ROOT / "tools" / "diff_preview.py")
dp = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(dp)


def _png(path: Path, w: int = 4, h: int = 4, color: str = "#ff0000"):
    img = QImage(w, h, QImage.Format_ARGB32)
    img.fill(QColor(color))
    assert img.save(str(path))


def _run(monkeypatch, dir_a: Path, dir_b: Path) -> int:
    monkeypatch.setattr(sys, "argv", ["diff_preview.py", str(dir_a), str(dir_b)])
    return dp.main()


@pytest.fixture
def imgs(tmp_path):
    a = tmp_path / "a"
    b = tmp_path / "b"
    a.mkdir()
    b.mkdir()
    return a, b


def test_identical_dirs_exit_zero(qapp, monkeypatch, imgs):
    a, b = imgs
    _png(a / "p.png")
    _png(b / "p.png")
    # 非 png 文件不参与比对（两侧内容不同也不影响退出码）
    (a / "note.txt").write_text("x", encoding="utf-8")
    (b / "note.txt").write_text("different", encoding="utf-8")
    assert _run(monkeypatch, a, b) == 0


def test_pixel_diff_exit_one(qapp, monkeypatch, imgs):
    a, b = imgs
    _png(a / "p.png", color="#ff0000")
    _png(b / "p.png", color="#00ff00")
    assert _run(monkeypatch, a, b) == 1


def test_size_diff_exit_one(qapp, monkeypatch, imgs):
    a, b = imgs
    _png(a / "p.png", w=4, h=4)
    _png(b / "p.png", w=5, h=5)
    assert _run(monkeypatch, a, b) == 1


def test_only_a_exit_one(qapp, monkeypatch, imgs):
    a, b = imgs
    _png(a / "p.png")     # 只在改前目录存在 = 改后截图缺失
    assert _run(monkeypatch, a, b) == 1


def test_only_b_exit_one(qapp, monkeypatch, imgs):
    a, b = imgs
    _png(b / "p.png")     # 只在改后目录存在 = 新增页面
    assert _run(monkeypatch, a, b) == 1


def test_mixed_reports_all_then_exit_one(qapp, monkeypatch, imgs, capsys):
    """一张 SAME 不掩盖另一张 DIFF：逐张都报，退出码取最坏。"""
    a, b = imgs
    _png(a / "same.png")
    _png(b / "same.png")
    _png(a / "diff.png", color="#ff0000")
    _png(b / "diff.png", color="#00ff00")
    assert _run(monkeypatch, a, b) == 1
    out = capsys.readouterr().out
    assert "SAME      same.png" in out
    assert "DIFF      diff.png" in out
