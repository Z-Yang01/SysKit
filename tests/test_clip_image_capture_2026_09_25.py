"""剪贴板图片/文件捕获回归：非文本载荷不得被「文本未变」去重吞掉（轮次25）。

真机症状「剪贴板不显示图片条目」的根因：截图（Win+Shift+S / PrintScreen）与
资源管理器文件复制不带 CF_UNICODETEXT，QClipboard.text() 恒为 ""，而
_on_clip_change 的 `text == self._last` 闸在 _last 同为 ""（启动初值 / 上一条
也是图片）时直接返回——图片/文件分支从未执行。73d02c1 引入图片捕获起即如此；
旧用例给图片场景配了非空 _text（真实图片复制不可能有），恰好掩盖了该缺陷。

本文件用假剪贴板复刻真实形态（text() 恒 ""）+ 真 QImage/PNG 落盘驱动
_on_clip_change，断言条目生成与行内缩略图数据就绪；零 Win32 注入。
"""
import os

import pytest
from PySide6.QtCore import QUrl
from PySide6.QtGui import QImage

from syskit import clipboard as clip_mod
from syskit import settings
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
