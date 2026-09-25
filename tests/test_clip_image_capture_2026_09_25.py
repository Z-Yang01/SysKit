"""剪贴板图片/文件捕获回归：非文本载荷不得被「文本未变」去重吞掉（轮次25）。

真机症状「剪贴板不显示图片条目」的根因：截图（Win+Shift+S / PrintScreen）与
资源管理器文件复制不带 CF_UNICODETEXT，QClipboard.text() 恒为 ""，而
_on_clip_change 的 `text == self._last` 闸在 _last 同为 ""（启动初值 / 上一条
也是图片）时直接返回——图片/文件分支从未执行。73d02c1 引入图片捕获起即如此；
旧用例给图片场景配了非空 _text（真实图片复制不可能有），恰好掩盖了该缺陷。

本文件用假剪贴板复刻真实形态（text() 恒 ""）+ 真 QImage/PNG 落盘驱动
_on_clip_change，断言条目生成与行内缩略图数据就绪；零 Win32 注入。

后续补强（同轮次25·方向D）：
- 大图（>4MP）PNG 编码/落盘移交后台 FIFO 线程——dataChanged 在 GUI 线程
  触发，4K 截图同步编码百毫秒级冻界面；
- save_image_bytes 失败不得产生 data="" 的僵尸条目（毒化后续同尺寸截图）；
- clipboard_files 孤儿载荷（崩溃残留）按引用集+mtime 宽限清扫。
"""
import os
import threading
import time

import pytest
from PySide6.QtCore import QUrl
from PySide6.QtGui import QImage
from PySide6.QtWidgets import QApplication

from syskit import clipboard as clip_mod
from syskit import settings
from syskit.ui import system_page as system_page_mod
from syskit.ui.system_page import ClipboardTab


class _FakeMime:
    """只暴露捕获链路用到的探测位（对齐真实 QMimeData 的接口名）。"""

    def __init__(self, has_urls=False, urls=(), has_image=False,
                 has_html=False, html=""):
        self._has_urls, self._urls = has_urls, list(urls)
        self._has_image = has_image
        self._has_html, self._html = has_html, html

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


class _ImageOnlyClip:
    """image/file-only 复制的真实形态：text() 恒为 ""（无 CF_UNICODETEXT）。"""

    def __init__(self):
        self._text = ""
        self._mime = None
        self._image = None

    def text(self):
        return self._text

    def mimeData(self):
        return self._mime

    def image(self):
        return self._image


def _make_tab(qtbot, monkeypatch):
    monkeypatch.setattr(clip_mod, "should_record_now", lambda *a, **k: True)
    tab = ClipboardTab()
    qtbot.addWidget(tab)
    tab.hist.clear()
    tab._clip = _ImageOnlyClip()      # 覆盖真实剪贴板：dev 机内容不可控
    tab._last = ""                    # 显式钉死启动初值形态
    tab.chk_watch.setChecked(True)
    return tab


def _img(argb, w=4, h=4):
    img = QImage(w, h, QImage.Format_ARGB32)
    img.fill(argb)
    return img


class TestNonTextPayloadBypass:
    def test_image_only_copy_recorded(self, qtbot, monkeypatch):
        """截图真形态（text="" + hasImage）必须产生 image 条目——旧版在此被
        text==_last（""==""）吞掉，图片条目根本进不了历史。"""
        tab = _make_tab(qtbot, monkeypatch)
        tab._clip._image = _img(0xFF112233)
        tab._clip._mime = _FakeMime(has_image=True)
        tab._on_clip_change()
        items = tab.hist.items()
        assert len(items) == 1 and items[0]["kind"] == "image"
        assert items[0]["text"] == "[图片 4×4]"
        assert items[0]["data"].endswith(".png")
        assert os.path.exists(clip_mod.image_path(items[0]["data"]))

    def test_two_consecutive_images_both_recorded(self, qtbot, monkeypatch):
        """连续两张不同截图都入库——旧版第二张起 text("")==_last("") 再被吞。"""
        tab = _make_tab(qtbot, monkeypatch)
        tab._clip._mime = _FakeMime(has_image=True)
        tab._clip._image = _img(0xFF000001)
        tab._on_clip_change()
        tab._clip._image = _img(0xFF000002)
        tab._on_clip_change()
        imgs = [it for it in tab.hist.items() if it["kind"] == "image"]
        assert len(imgs) == 2
        assert len({it["data"] for it in imgs}) == 2     # 两个不同 PNG 载荷

    def test_identical_image_copy_not_duplicated(self, qtbot, monkeypatch):
        """同一张图重复复制：载荷 md5 + 条目身份去重，不产生重复条目。"""
        tab = _make_tab(qtbot, monkeypatch)
        tab._clip._mime = _FakeMime(has_image=True)
        tab._clip._image = _img(0xFF000003)
        tab._on_clip_change()
        tab._on_clip_change()
        assert len([it for it in tab.hist.items()
                    if it["kind"] == "image"]) == 1

    def test_file_copy_without_text_recorded(self, qtbot, monkeypatch):
        """资源管理器文件复制（仅 CF_HDROP，text 恒 ""）同受此闸误伤，一并修。"""
        tab = _make_tab(qtbot, monkeypatch)
        tab._clip._mime = _FakeMime(
            has_urls=True, urls=[QUrl.fromLocalFile("C:/a.txt")])
        tab._on_clip_change()
        items = tab.hist.items()
        assert len(items) == 1 and items[0]["kind"] == "file"
        assert items[0]["text"] == "C:/a.txt"

    def test_text_dedupe_still_blocks_same_text(self, qtbot, monkeypatch):
        """纯文本（无非文本载荷）的「内容未变不入库」闸保持原语义。"""
        tab = _make_tab(qtbot, monkeypatch)
        tab._clip._mime = _FakeMime()
        tab._clip._text = "dup"
        tab._on_clip_change()
        tab._on_clip_change()
        assert tab.hist.texts() == ["dup"]

    def test_capture_images_disabled_stays_silent(self, qtbot, monkeypatch):
        """clip_capture_images=False：图片载荷绕过去重闸也不落库（设置仍生效）。"""
        settings.save({"clip_capture_images": False})
        tab = _make_tab(qtbot, monkeypatch)
        tab._clip._image = _img(0xFF000004)
        tab._clip._mime = _FakeMime(has_image=True)
        tab._on_clip_change()
        assert tab.hist.items() == []

    def test_image_row_renders_inline_thumbnail(self, qtbot, monkeypatch):
        """显示侧取证收口：条目落库后 reload 的行内缩略图数据就绪（非空图标）。"""
        tab = _make_tab(qtbot, monkeypatch)
        tab._clip._image = _img(0xFF00AABB, 8, 8)
        tab._clip._mime = _FakeMime(has_image=True)
        tab._on_clip_change()
        assert tab.tbl.rowCount() == 1
        cell = tab.tbl.item(0, 2)
        assert cell is not None
        assert not cell.icon().isNull()          # 行内缩略图已挂上
        assert tab._thumb_for(tab.hist.items()[0]["data"]) is not None


class TestHasNonTextPayload:
    def test_none_and_plain_mime_false(self):
        assert ClipboardTab._has_nontext_payload(None) is False
        assert ClipboardTab._has_nontext_payload(_FakeMime()) is False
        assert ClipboardTab._has_nontext_payload(
            _FakeMime(has_html=True, html="<b>x</b>")) is False

    def test_urls_or_image_true(self):
        assert ClipboardTab._has_nontext_payload(
            _FakeMime(has_image=True)) is True
        assert ClipboardTab._has_nontext_payload(
            _FakeMime(has_urls=True, urls=[QUrl("https://x")])) is True

    def test_probe_exception_treated_as_text(self):
        class _Boom:
            def hasUrls(self):
                raise RuntimeError("mime provider gone")

        assert ClipboardTab._has_nontext_payload(_Boom()) is False


# ---------------- 方向D：大图异步编码（不阻塞 GUI 线程） ----------------

class TestLargeImageAsyncCapture:
    def test_large_image_goes_through_background_fifo(self, qtbot, monkeypatch):
        """超阈值大图走后台 FIFO：_on_clip_change 只拷位图不做 PNG 压缩，
        编码/入库由工作者线程按复制顺序完成，线程身份非 GUI 线程。"""
        tab = _make_tab(qtbot, monkeypatch)
        monkeypatch.setattr(system_page_mod, "_ASYNC_ENCODE_PIXELS", 0)
        seen = []

        def fake_encode(img):
            seen.append((threading.current_thread(), img.pixel(0, 0)))
            return tab.hist.add(f"[图片 {img.width()}×{img.height()}]",
                                kind="image", data=f"bg{len(seen)}.png")

        monkeypatch.setattr(tab, "_encode_and_add", fake_encode)
        tab._clip._image = _img(0xFF000001)
        tab._clip._mime = _FakeMime(has_image=True)
        tab._on_clip_change()
        tab._clip._image = _img(0xFF000002)
        tab._on_clip_change()

        assert tab._cap_thread is not None                     # 后台线程已启动
        qtbot.waitUntil(lambda: len(seen) == 2, timeout=5000)
        assert [px for _t, px in seen] == [0xFF000001, 0xFF000002]   # FIFO=复制顺序
        assert all(t is not threading.current_thread() for t, _px in seen)
        assert tab.hist.texts() == ["[图片 4×4]", "[图片 4×4]"]

    def test_large_image_end_to_end_png_on_disk(self, qtbot, monkeypatch):
        """真编码端到端：后台编码→落盘→入库→capture_added 回 GUI 刷新，
        条目载荷文件就绪、行内缩略图非空（显示链路在异步路径同样成立）。"""
        tab = _make_tab(qtbot, monkeypatch)
        monkeypatch.setattr(system_page_mod, "_ASYNC_ENCODE_PIXELS", 0)
        tab._clip._image = _img(0xFF00AACC, 16, 16)
        tab._clip._mime = _FakeMime(has_image=True)
        tab._on_clip_change()
        qtbot.waitUntil(lambda: bool(tab.hist.items()), timeout=5000)
        it = tab.hist.items()[0]
        assert it["kind"] == "image" and it["text"] == "[图片 16×16]"
        assert os.path.exists(clip_mod.image_path(it["data"]))
        qtbot.waitUntil(lambda: tab.tbl.rowCount() == 1, timeout=2000)
        assert not tab.tbl.item(0, 2).icon().isNull()

    def test_worker_failure_swallowed(self, qtbot, monkeypatch):
        """后台链路任一步异常：吞掉留痕不入库，线程存活可接后续任务。"""
        tab = _make_tab(qtbot, monkeypatch)
        monkeypatch.setattr(system_page_mod, "_ASYNC_ENCODE_PIXELS", 0)
        calls = {"n": 0}

        def boom(img):
            calls["n"] += 1
            raise RuntimeError("encode exploded")

        monkeypatch.setattr(tab, "_encode_and_add", boom)
        tab._clip._image = _img(0xFF000003)
        tab._clip._mime = _FakeMime(has_image=True)
        tab._on_clip_change()
        tab._cap_jobs.join()               # 确定性等工作者处理完，不赌时序
        QApplication.processEvents()       # 放掉可能 queued 的信号
        assert tab.hist.items() == []
        assert calls["n"] == 1
        assert tab._cap_thread.is_alive()

    def test_small_image_stays_synchronous(self, qtbot, monkeypatch):
        """阈值内小图保持同步路径（行为与既有用例形态不变），不启动后台线程。"""
        tab = _make_tab(qtbot, monkeypatch)
        tab._clip._image = _img(0xFF000004)
        tab._clip._mime = _FakeMime(has_image=True)
        tab._on_clip_change()
        assert tab._cap_thread is None
        assert len(tab.hist.items()) == 1


# ---------------- 方向D：落盘失败的僵尸条目守卫 ----------------

class TestZombieImageEntryGuard:
    def test_save_failure_no_zombie_entry(self, qtbot, monkeypatch):
        """save_image_bytes 失败（磁盘满等）不得入库：旧版产生 data="" 的
        僵尸条目——既不能复制回放，又把后续同尺寸截图挡在「与最新条相同」
        闸外（用户观感：截图又没进历史）。"""
        tab = _make_tab(qtbot, monkeypatch)
        real = clip_mod.save_image_bytes
        state = {"fail": True}

        def flaky_save(png):
            return "" if state["fail"] else real(png)

        monkeypatch.setattr(clip_mod, "save_image_bytes", flaky_save)
        tab._clip._image = _img(0xFF000005)
        tab._clip._mime = _FakeMime(has_image=True)
        tab._on_clip_change()
        assert tab.hist.items() == []          # 旧版此处产生 [图片 4×4]/data="" 僵尸

        state["fail"] = False                  # 恢复后同尺寸截图照常入库（不再被毒化）
        tab._clip._image = _img(0xFF000006)
        tab._on_clip_change()
        assert len(tab.hist.items()) == 1
        assert tab.hist.items()[0]["text"] == "[图片 4×4]"
        assert os.path.exists(clip_mod.image_path(tab.hist.items()[0]["data"]))


# ---------------- 方向D：clipboard_files 孤儿载荷清扫 ----------------

class TestOrphanPngSweep:
    def test_sweep_removes_unreferenced_aged_files_only(self, qtbot, monkeypatch):
        """孤儿清扫：删「过期 + 无引用」，保「新文件（在途捕获宽限）」与
        「仍被历史条目引用的载荷」（无论多老）。"""
        tab = _make_tab(qtbot, monkeypatch)
        d = clip_mod.files_dir()
        os.makedirs(d, exist_ok=True)
        t_old = time.time() - 7200

        orphan_old = os.path.join(d, "deadbeef.png")
        with open(orphan_old, "wb") as f:
            f.write(b"x")
        os.utime(orphan_old, (t_old, t_old))
        orphan_fresh = os.path.join(d, "cafebabe.png")
        with open(orphan_fresh, "wb") as f:
            f.write(b"y")

        tab.hist.add("[图片 1×1]", kind="image", data="kept.png")
        kept = clip_mod.image_path("kept.png")
        with open(kept, "wb") as f:
            f.write(b"z")
        os.utime(kept, (t_old, t_old))         # 被引用：即使很老也不删

        tab._purge_expired()
        assert not os.path.exists(orphan_old)
        assert os.path.exists(orphan_fresh)
        assert os.path.exists(kept)

    def test_sweep_edges_missing_dir_and_non_png(self, monkeypatch):
        """目录不存在返回 0；空引用名被忽略；非 .png 文件不动。"""
        assert clip_mod.sweep_orphan_files([]) == 0        # 目录尚未创建
        d = clip_mod.files_dir()
        os.makedirs(d, exist_ok=True)
        txt = os.path.join(d, "notes.txt")
        with open(txt, "w", encoding="utf-8") as f:
            f.write("keep")
        png = os.path.join(d, "a.png")
        with open(png, "wb") as f:
            f.write(b"p")
        os.utime(png, (time.time() - 7200,) * 2)
        assert clip_mod.sweep_orphan_files(["", None]) == 1
        assert not os.path.exists(png)
        assert os.path.exists(txt)

    def test_purge_expired_reports_sweep_count(self, qtbot, monkeypatch):
        """清扫发生时提示条带出残留清理数（removed=0 也有可感知反馈）。"""
        tab = _make_tab(qtbot, monkeypatch)
        d = clip_mod.files_dir()
        os.makedirs(d, exist_ok=True)
        orphan = os.path.join(d, "ghost.png")
        with open(orphan, "wb") as f:
            f.write(b"x")
        os.utime(orphan, (time.time() - 7200,) * 2)
        tab._purge_expired()
        assert "1 个残留图片文件" in tab.lbl.text()
