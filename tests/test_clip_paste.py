"""剪贴板快捷粘贴：paste 原语 / 回填抑制 / 第二全局热键 / ClipPastePanel UI。

不注入真实按键（focus_window/send_ctrl_v 在 UI 路径测试中一律被 mock）；
不依赖真实前台窗口（focus_window 用非法句柄走 False 分支）。
"""
import ctypes
import sys
import time
from ctypes import wintypes
from types import SimpleNamespace

import pytest
from PySide6.QtCore import QBuffer, QEvent, Qt
from PySide6.QtGui import QCursor, QImage
from PySide6.QtWidgets import QApplication, QWidget

from syskit import clipboard as clipboard_mod
from syskit import hotkey as hotkey_mod
from syskit import paste as paste_mod
from syskit import settings
from syskit.ui import clip_paste as clip_paste_mod
from syskit.ui.clip_paste import ClipPastePanel
from syskit.ui.palette import (HotkeyFilter, WM_HOTKEY, setup_paste_hotkey,
                               teardown_paste_hotkey)

from test_ui_smoke import make_main_window  # noqa: F401  复用工厂夹具


# ---------------- 假对象 ----------------

class _FakeTray:
    def __init__(self):
        self.notes = []

    def notify(self, title, msg, level="ok"):
        self.notes.append((title, msg))


class _FakeWindow(QWidget):
    def __init__(self):
        super().__init__()
        self.tray = _FakeTray()

    def toggle_clip_paste(self):
        pass

    def quick_paste_latest(self):
        pass

    def quit_app(self):                 # TrayHost 构造期即 connect（建议②托盘测试）
        pass


def _panel(qtbot):
    win = _FakeWindow()
    qtbot.addWidget(win)
    p = ClipPastePanel(win)
    qtbot.addWidget(p)
    return p, win


def _seed_history(*texts):
    # 先清抑制窗口：前面用例的写回链可能对同文本做过 suppress_next（2s TTL），
    # 不清会让本用例的 add 被抑制闸拒绝、历史为空（跨用例污染实测）
    clipboard_mod._suppressed.clear()
    hist = clipboard_mod.ClipboardHistory()
    for t in texts:
        hist.add(t)
    hist.save()
    return hist


# ---------------- paste.py 原语 ----------------

@pytest.mark.skipif(sys.platform != "win32", reason="Win32 原语仅限 Windows")
class TestPastePrimitives:
    def test_foreground_window_returns_int(self):
        assert isinstance(paste_mod.foreground_window(), int)

    def test_focus_zero_or_invalid_hwnd_false(self):
        assert paste_mod.focus_window(0) is False
        assert paste_mod.focus_window(0x00DEAD) is False    # 非窗口句柄 → IsWindow False

    def test_hwnd_signatures_declared_at_import(self):
        """建议④：导入期声明 ctypes 签名，默认 c_int 会截断 x64 的 HWND。"""
        u = paste_mod._user32()
        assert u.GetForegroundWindow.restype is wintypes.HWND
        assert u.SetForegroundWindow.argtypes == [wintypes.HWND]
        assert u.SendInput.argtypes[1]._type_ is paste_mod.INPUT
        arr = (paste_mod.INPUT * 4)()
        u.SendInput.argtypes[1].from_param(arr)             # 数组→POINTER(INPUT) 可隐式转换
        # x64：type DWORD + 对齐 pad + MOUSEINPUT 宽的 union（旧版注释钉的 40 字节）
        assert ctypes.sizeof(paste_mod.INPUT) == ctypes.sizeof(paste_mod.MOUSEINPUT) + 8
        assert paste_mod._kernel32().GetCurrentThreadId.restype is wintypes.DWORD

    def test_focus_cross_thread_attach_path(self, monkeypatch):
        """还焦给「前台不属于本线程」的窗口：走 AttachThreadInput 兜底分支。

        第 2 轮真机冒烟回归：该分支曾用 u.GetCurrentThreadId()，而 user32 根本
        没有这个导出 → 每次跨进程还焦都 AttributeError 被吞、降级成手动粘贴。
        桩对象刻意复刻真实 DLL 导出表（user32 无 GetCurrentThreadId）。
        """
        calls = []
        state = {"fg": 0x1111}          # 面板自己占着前台

        class _U32:
            def IsWindow(self, h): return True
            def IsIconic(self, h): return False
            def GetForegroundWindow(self): return state["fg"]
            def GetWindowThreadProcessId(self, h, _): return 4242
            def AttachThreadInput(self, a, b, on):
                calls.append(("attach", on)); return True
            def BringWindowToTop(self, h): calls.append("top")
            def SetForegroundWindow(self, h):
                state["fg"] = h; calls.append("setfg"); return True

        monkeypatch.setattr(paste_mod, "_user32", lambda: _U32())
        assert paste_mod.focus_window(0x2222) is True
        assert ("attach", True) in calls and ("attach", False) in calls
        assert "top" in calls and "setfg" in calls

    # ---- 以下为轮次16盲区补强（paste.py 72%→100%）：_user32 一律打桩，绝不真实注入 ----

    def test_foreground_window_non_win32_zero(self, monkeypatch):
        """非 Windows 平台：foreground_window 返回 0（调用方按「无前台」降级）。"""
        monkeypatch.setattr(sys, "platform", "linux")
        assert paste_mod.foreground_window() == 0

    def test_foreground_window_error_zero(self, monkeypatch):
        """Win32 调用抛异常（如桌面会话分离）→ 0，不向上抛。"""
        class _U32:
            def GetForegroundWindow(self):
                raise OSError("no desktop")
        monkeypatch.setattr(paste_mod, "_user32", lambda: _U32())
        assert paste_mod.foreground_window() == 0

    def test_focus_window_restores_minimized_shortcut(self, monkeypatch):
        """最小化窗口先 ShowWindow(SW_RESTORE)；还原后已在前台 → 走短路 True
        （不再 AttachThreadInput）。"""
        calls = []

        class _U32:
            def IsWindow(self, h): return True
            def IsIconic(self, h): return True
            def ShowWindow(self, h, cmd):
                calls.append(("show", h, cmd)); return True
            def GetForegroundWindow(self): return 0x2222   # 还原后即前台

        monkeypatch.setattr(paste_mod, "_user32", lambda: _U32())
        assert paste_mod.focus_window(0x2222) is True
        assert calls == [("show", 0x2222, paste_mod._SW_RESTORE)]

    def test_focus_window_failure_swallowed(self, monkeypatch):
        """Win32 链路任意一步抛异常 → debug 留痕并返回 False，不抛穿调用方。"""
        notes = []

        class _U32:
            def IsWindow(self, h):
                raise OSError("gone")

        monkeypatch.setattr(paste_mod, "_user32", lambda: _U32())
        monkeypatch.setattr(paste_mod, "LOGGER",
                            SimpleNamespace(debug=lambda msg, *a, **kw: notes.append(msg)))
        assert paste_mod.focus_window(0x3333) is False
        assert len(notes) == 1 and "focus_window" in notes[0]

    def test_send_ctrl_v_non_win32_false(self, monkeypatch):
        monkeypatch.setattr(sys, "platform", "linux")
        assert paste_mod.send_ctrl_v() is False

    def test_send_ctrl_v_injects_ctrl_v_sequence(self, monkeypatch):
        """注入序列锁定为 Ctrl↓ V↓ V↑ Ctrl↑ 四事件；size 显式传 sizeof(INPUT)。"""
        seen = {}

        class _U32:
            def SendInput(self, n, arr, size):
                seen["n"], seen["arr"], seen["size"] = n, list(arr), size
                return n

        monkeypatch.setattr(paste_mod, "_user32", lambda: _U32())
        assert paste_mod.send_ctrl_v() is True
        assert seen["n"] == 4
        assert seen["size"] == ctypes.sizeof(paste_mod.INPUT)
        ks = [(e.iu.ki.wVk, e.iu.ki.dwFlags) for e in seen["arr"]]
        assert ks == [(0x11, 0), (0x56, 0), (0x56, paste_mod._KEY_UP),
                      (0x11, paste_mod._KEY_UP)]      # Ctrl↓ V↓ V↑ Ctrl↑

    def test_send_ctrl_v_partial_delivery_false(self, monkeypatch):
        """SendInput 少送事件（如被 UIPI 拦截）→ False，调用方降级手动粘贴。"""
        class _U32:
            def SendInput(self, n, arr, size):
                return n - 1

        monkeypatch.setattr(paste_mod, "_user32", lambda: _U32())
        assert paste_mod.send_ctrl_v() is False

    def test_send_ctrl_v_exception_false(self, monkeypatch):
        """SendInput 抛异常 → False（debug 日志被吞），不抛穿调用方。"""
        class _U32:
            def SendInput(self, n, arr, size):
                raise OSError("blocked")

        monkeypatch.setattr(paste_mod, "_user32", lambda: _U32())
        assert paste_mod.send_ctrl_v() is False


# ---------------- 回填抑制（clipboard.suppress_next） ----------------

class TestSuppressNext:
    def test_suppressed_text_not_recorded(self):
        hist = _seed_history("aa", "bb")          # 最新 "bb" 在前
        clipboard_mod.suppress_next("aa")
        assert hist.add("aa") is False             # 非最新条目：无抑制本应入库
        assert hist.texts() == ["bb", "aa"]

    def test_unsuppressed_text_recorded(self):
        hist = clipboard_mod.ClipboardHistory()
        assert hist.add("普通复制") is True

    def test_ttl_expiry_releases(self, monkeypatch):
        clipboard_mod.suppress_next("y", ttl=1)
        assert clipboard_mod._is_suppressed("y") is True
        future = time.time() + 10          # 冻结时间须用纯值，patch 全局 time.time 会自递归
        monkeypatch.setattr(clipboard_mod.time, "time", lambda: future)
        assert clipboard_mod._is_suppressed("y") is False

    def test_add_hashes_text_once(self, monkeypatch):
        """⑥：add 热路径敏感/抑制两道闸共享一次摘要（旧版各算一遍）。"""
        calls = []
        real = clipboard_mod._digest
        monkeypatch.setattr(clipboard_mod, "_digest",
                            lambda t: calls.append(1) or real(t))
        hist = clipboard_mod.ClipboardHistory()
        hist.add("hash-count-probe")
        assert len(calls) == 1


# ---------------- 第二个全局热键注册 ----------------

class TestPasteHotkeySetup:
    def test_empty_setting_means_disabled(self, qtbot, monkeypatch):
        calls = []
        monkeypatch.setattr(hotkey_mod, "register",
                            lambda m, v, hotkey_id=1: calls.append(hotkey_id) or True)
        settings.save({"clip_paste_hotkey": ""})
        win = _FakeWindow()
        qtbot.addWidget(win)
        assert setup_paste_hotkey(win) is False
        assert calls == []

    def test_unparseable_returns_false(self, qtbot):
        settings.save({"clip_paste_hotkey": "Alt+Shift"})
        win = _FakeWindow()
        qtbot.addWidget(win)
        assert setup_paste_hotkey(win) is False

    def test_register_failure_no_filter(self, qtbot, monkeypatch):
        monkeypatch.setattr(hotkey_mod, "register",
                            lambda m, v, hotkey_id=1: False)
        settings.save({"clip_paste_hotkey": "Ctrl+Alt+V"})
        win = _FakeWindow()
        qtbot.addWidget(win)
        assert setup_paste_hotkey(win) is False
        assert getattr(win, "_paste_hotkey_filter", None) is None

    def test_success_and_teardown(self, qtbot, monkeypatch):
        monkeypatch.setattr(hotkey_mod, "register",
                            lambda m, v, hotkey_id=1: True)
        monkeypatch.setattr(hotkey_mod, "unregister", lambda hotkey_id=1: True)
        settings.save({"clip_paste_hotkey": "Ctrl+Alt+V"})
        win = _FakeWindow()
        qtbot.addWidget(win)
        assert setup_paste_hotkey(win) is True
        filt = win._paste_hotkey_filter
        assert isinstance(filt, HotkeyFilter) and filt.expected_ids == (3,)
        teardown_paste_hotkey(win)
        assert win._paste_hotkey_filter is None

    def test_quick_setup_success_and_teardown(self, qtbot, monkeypatch):
        """快速粘贴热键（id=2，clip_quick_hotkey）：注册/卸载与面板键（id=3）平行。"""
        from syskit.ui.palette import setup_quick_paste_hotkey, teardown_quick_paste_hotkey
        monkeypatch.setattr(hotkey_mod, "register",
                            lambda m, v, hotkey_id=1: True)
        monkeypatch.setattr(hotkey_mod, "unregister", lambda hotkey_id=1: True)
        settings.save({"clip_quick_hotkey": "Ctrl+Alt+Z"})
        win = _FakeWindow()
        qtbot.addWidget(win)
        assert setup_quick_paste_hotkey(win) is True
        filt = win._quick_hotkey_filter
        assert isinstance(filt, HotkeyFilter) and filt.expected_ids == (2,)
        teardown_quick_paste_hotkey(win)
        assert win._quick_hotkey_filter is None

    def test_quick_empty_setting_means_disabled(self, qtbot, monkeypatch):
        from syskit.ui.palette import setup_quick_paste_hotkey
        calls = []
        monkeypatch.setattr(hotkey_mod, "register",
                            lambda m, v, hotkey_id=1: calls.append(hotkey_id) or True)
        settings.save({"clip_quick_hotkey": ""})
        win = _FakeWindow()
        qtbot.addWidget(win)
        assert setup_quick_paste_hotkey(win) is False
        assert calls == []


class TestFilterDispatch:
    _keepalive = []      # MSG 内存被 from_address 按地址读取：必须比指针活得更久

    @classmethod
    def _msg(cls, hid):
        msg = wintypes.MSG()
        msg.message = WM_HOTKEY
        msg.wParam = hid
        cls._keepalive.append(msg)
        return ctypes.c_void_p(ctypes.addressof(msg))

    def test_only_own_hotkey_id_consumed(self, qtbot):   # qtbot：需要 QApplication 存活
        fired = []
        filt = HotkeyFilter(lambda: fired.append(1), expected_ids=(2,))
        handled, _ = filt.nativeEventFilter(b"windows_generic_MSG", self._msg(1))
        assert handled is False and fired == []          # 别的热键 → 放行给另一过滤器
        handled, _ = filt.nativeEventFilter(b"windows_generic_MSG", self._msg(2))
        assert handled is True and fired == [1]


# ---------------- ClipPastePanel ----------------

class TestClipPastePanel:
    def test_popup_lists_all_and_filters(self, qtbot):
        _seed_history("alpha one", "beta two", "gamma")
        p, _win = _panel(qtbot)
        p.popup(0)
        assert p.isVisible()
        assert len(p._rows) == 3
        p.edt.setText("BETA")
        assert [it["text"] for it in p._rows] == ["beta two"]

    def test_typing_filters_snapshot_without_reload(self, qtbot, monkeypatch):
        """按键只过滤呼出时的内存快照：读盘每呼出一次，不随击键次数放大。"""
        _seed_history("alpha one", "beta two", "gamma")
        p, _win = _panel(qtbot)
        calls = []
        orig = p._load_items
        monkeypatch.setattr(p, "_load_items", lambda: calls.append(1) or orig())
        p.popup(0)
        p.edt.setText("bet")
        p.edt.setText("beta")
        assert len(calls) == 1
        assert [it["text"] for it in p._rows] == ["beta two"]
        p.popup(0)                              # 重新呼出 → 重新读一次盘
        assert len(calls) == 2

    def test_write_back_text_sets_clipboard_and_suppresses(self, qtbot):
        _seed_history("aa", "bb")                # 盘上最新 "bb"；回填非最新的 "aa"
        p, _win = _panel(qtbot)
        p.popup(0)
        it = next(i for i in p._rows if i["text"] == "aa")
        assert p._write_back(it) is True
        assert QApplication.clipboard().text() == "aa"
        hist = clipboard_mod.ClipboardHistory()
        assert hist.add("aa") is False           # 抑制窗口内监听器不重复记录

    def test_write_back_missing_image_false(self, qtbot):
        p, _win = _panel(qtbot)
        assert p._write_back({"kind": "image", "text": "[图片 1×1]",
                              "data": "missing.png"}) is False

    def test_write_back_excludes_os_history(self, qtbot, monkeypatch):
        """自动回填不进系统侧历史（Win+V/云剪贴板）——与密码库复制同口径；
        自家历史用 suppress_next，系统侧用排除格式，两边都不该被顶新。"""
        _seed_history("wx")
        p, _win = _panel(qtbot)
        calls = []
        monkeypatch.setattr(clipboard_mod, "exclude_os_history",
                            lambda: calls.append(1))
        p.popup(0)
        assert p._write_back(p._rows[0]) is True
        assert calls == [1]

    def test_popup_centers_on_cursor_screen(self, qtbot):
        """多屏习惯：面板居中在鼠标所在屏（视线跟鼠标走），不再钉死主屏。"""
        p, _win = _panel(qtbot)
        p.popup(0)
        scr = (QApplication.screenAt(QCursor.pos())
               or QApplication.primaryScreen()).availableGeometry()
        # 无框窗口 frame≈0，留 4px 容差防分数 DPI 取整
        assert (scr.center() - p.geometry().center()).manhattanLength() <= 4

    def test_do_paste_success_and_fallback(self, qtbot, monkeypatch):
        # 注意：绝不调用真实 send_ctrl_v（SendInput 会注入到用户当前焦点窗口）
        _seed_history("zz")
        p, win = _panel(qtbot)
        monkeypatch.setattr(clipboard_mod, "foreground_process_name",
                            lambda: "notepad")
        monkeypatch.setattr(paste_mod, "foreground_window", lambda: 4321)
        calls = []
        monkeypatch.setattr(paste_mod, "release_modifiers",
                            lambda: calls.append("rel") or True)
        monkeypatch.setattr(paste_mod, "send_ctrl_v",
                            lambda: calls.append("send") or True)
        p.popup(0)
        p._do_paste(p._rows[0], 4321, p._paste_seq)
        qtbot.waitUntil(lambda: bool(win.tray.notes), timeout=2000)
        assert win.tray.notes[-1][1] == "已粘贴到原窗口"
        assert calls == ["rel", "send"]      # 修饰键释放先于注键（硬约束④）

        # SendInput 未生效 → 降级提示（等一拍后）
        win.tray.notes.clear()
        monkeypatch.setattr(paste_mod, "send_ctrl_v", lambda: False)
        p._do_paste(p._rows[0], 4321, p._paste_seq)
        qtbot.waitUntil(lambda: bool(win.tray.notes), timeout=2000)
        assert "自动粘贴未送达" in win.tray.notes[-1][1]

        # 前台已切到第三方窗口 → 放弃注键（硬约束⑩），不抢焦点
        win.tray.notes.clear()
        calls.clear()
        monkeypatch.setattr(paste_mod, "send_ctrl_v", lambda: True)
        monkeypatch.setattr(paste_mod, "foreground_window", lambda: 0x9999)
        p._do_paste(p._rows[0], 4321, p._paste_seq)
        qtbot.waitUntil(lambda: bool(win.tray.notes), timeout=2000)
        assert "目标窗口已切换" in win.tray.notes[-1][1]
        assert calls == []                   # 第三方分支不注键（硬约束⑩）

    def test_do_paste_focus_drift_to_own_window_refocuses(self, qtbot, monkeypatch):
        """前台为本应用（面板刚隐藏过渡态）→ focus_window 还焦，复核通过才注键；
        还焦失败（复核不过）→ 取消并提示，绝不无条件抢焦点（硬约束⑩）。"""
        _seed_history("zz")
        p, win = _panel(qtbot)
        fg = {"hwnd": 0x1111}                # 本应用窗口占前台
        monkeypatch.setattr(clipboard_mod, "foreground_process_name",
                            lambda: "syskit")
        monkeypatch.setattr(paste_mod, "foreground_window", lambda: fg["hwnd"])
        monkeypatch.setattr(paste_mod, "focus_window",
                            lambda hwnd: fg.update(hwnd=hwnd) or True)
        calls = []
        monkeypatch.setattr(paste_mod, "release_modifiers", lambda: calls.append("rel") or True)
        monkeypatch.setattr(paste_mod, "send_ctrl_v", lambda: calls.append("send") or True)
        p.popup(0)
        p._do_paste(p._rows[0], 4321, p._paste_seq)
        qtbot.waitUntil(lambda: bool(win.tray.notes), timeout=2000)
        assert win.tray.notes[-1][1] == "已粘贴到原窗口"

        # 还焦失败：前台回到自身窗口 → 复核不过 → 取消
        win.tray.notes.clear()
        calls.clear()
        fg.update(hwnd=0x1111)               # 复位前台状态（第一段还焦已改写）
        monkeypatch.setattr(paste_mod, "focus_window", lambda hwnd: False)
        p._do_paste(p._rows[0], 4321, p._paste_seq)
        qtbot.waitUntil(lambda: bool(win.tray.notes), timeout=2000)
        assert "目标窗口已切换" in win.tray.notes[-1][1]
        assert calls == []                   # 复核不过不注键

    def test_fallback_wording_names_content_kind(self, qtbot, monkeypatch):
        """建议⑤：降级提示按条目类型点名——图片/文件普通文本框接不住
        Ctrl+V，说清「复制的是什么」用户才知道该换目标窗口还是重按。"""
        _seed_history("t")
        p, win = _panel(qtbot)
        p.popup(0)
        monkeypatch.setattr(p, "_write_back", lambda it: True)   # 免造真实图片文件
        monkeypatch.setattr(paste_mod, "foreground_window", lambda: 0)
        img = {"kind": "image", "text": "[图片]", "data": "x"}
        p._do_paste(img, 0, p._paste_seq)               # target=0：降级走一拍延时
        qtbot.waitUntil(lambda: len(win.tray.notes) >= 1, timeout=2000)
        assert win.tray.notes[-1][1].startswith("图片已复制")
        p._do_paste({"kind": "file", "text": "a.txt"}, 0, p._paste_seq)
        qtbot.waitUntil(lambda: len(win.tray.notes) >= 2, timeout=2000)
        assert win.tray.notes[-1][1].startswith("文件已复制")
        p._do_paste(p._rows[0], 0, p._paste_seq)               # 文本：无前缀
        qtbot.waitUntil(lambda: len(win.tray.notes) >= 3, timeout=2000)
        assert win.tray.notes[-1][1].startswith("已复制")
        # SendInput 未送达分支同样点名类型
        win.tray.notes.clear()
        monkeypatch.setattr(paste_mod, "foreground_window", lambda: 4321)
        monkeypatch.setattr(clipboard_mod, "foreground_process_name",
                            lambda: "notepad")
        monkeypatch.setattr(paste_mod, "release_modifiers", lambda: True)
        monkeypatch.setattr(paste_mod, "send_ctrl_v", lambda: False)
        p._do_paste(img, 4321, p._paste_seq)
        qtbot.waitUntil(lambda: bool(win.tray.notes), timeout=2000)
        assert "图片已复制" in win.tray.notes[-1][1]

    def test_do_paste_image_write_back_retries_once_async(self, qtbot, monkeypatch):
        """硬约束⑨：图片读取失败 → QTimer 异步重试一次（UI 线程零 sleep），
        再失败才降级提示。"""
        _seed_history("t")
        p, win = _panel(qtbot)
        p.popup(0)
        attempts = []
        monkeypatch.setattr(p, "_write_back",
                            lambda it: attempts.append(1) or False)
        monkeypatch.setattr(paste_mod, "foreground_window", lambda: 0)
        p._do_paste({"kind": "image", "text": "[图片]", "data": "x"},
                    0, p._paste_seq)
        qtbot.waitUntil(lambda: len(attempts) >= 2, timeout=2000)
        qtbot.waitUntil(lambda: bool(win.tray.notes), timeout=2000)
        assert len(attempts) == 2               # 恰好重试一次
        assert "粘贴失败" in win.tray.notes[-1][1]

    def test_exec_row_hides_then_schedules_paste(self, qtbot, monkeypatch):
        _seed_history("one", "two")
        p, _win = _panel(qtbot)
        calls = []
        monkeypatch.setattr(p, "_do_paste",
                            lambda it, hwnd, seq: calls.append((it, hwnd)))
        p.popup(777)
        p._exec_row(1)
        assert not p.isVisible()                  # 先隐藏把焦点让出去
        qtbot.waitUntil(lambda: bool(calls), timeout=1000)
        assert calls[0][1] == 777
        assert calls[0][0]["text"] == "one"       # 最新在前，第 1 行是 "one"

    def test_digit_and_arrow_keys(self, qtbot, monkeypatch):
        _seed_history("k1", "k2", "k3")
        p, _win = _panel(qtbot)
        calls = []
        monkeypatch.setattr(p, "_do_paste",
                            lambda it, hwnd, seq: calls.append(it["text"]))
        p.popup(0)
        assert p.lst.currentRow() == 0
        qtbot.keyClick(p.edt, Qt.Key_Down)
        assert p.lst.currentRow() == 1
        qtbot.keyClick(p.edt, Qt.Key_Up)
        assert p.lst.currentRow() == 0
        qtbot.keyClick(p.edt, Qt.Key_2)           # 数字直选第 2 行
        assert p.lst.count() >= 2
        qtbot.waitUntil(lambda: len(calls) == 1, timeout=1000)
        assert calls[0] == "k2"

    def test_paste_index_out_of_range_falls_through(self, qtbot):
        _seed_history("only")
        p, _win = _panel(qtbot)
        p.popup(0)
        assert p.paste_index(9) is False

    def test_reject_hides_without_pasting(self, qtbot):
        _seed_history("x")
        p, _win = _panel(qtbot)
        p.popup(0)
        p.reject()
        assert not p.isVisible()

    def test_double_exec_row_only_pastes_last(self, qtbot, monkeypatch):
        """连击（鼠标双击先后发 activated+doubleClicked 两信号）：旧链作废，
        只粘最后一击的条目一次——不会 double SendInput。"""
        _seed_history("one", "two")           # 最新在前：第 0 行 "two"、第 1 行 "one"
        p, _win = _panel(qtbot)
        wrote = []
        monkeypatch.setattr(p, "_write_back", lambda it: wrote.append(it["text"]) or True)
        p.popup(0)
        p._exec_row(0)
        p._exec_row(1)
        qtbot.wait(400)                       # 越过两链的 120ms 执行点
        assert wrote == ["one"]

    def test_repopup_cancels_inflight_chain(self, qtbot, monkeypatch):
        """粘贴链在途时重新呼出：旧链不得再写剪贴板/抢焦点，只认新选择。"""
        _seed_history("one", "two")
        p, _win = _panel(qtbot)
        wrote = []
        monkeypatch.setattr(p, "_write_back", lambda it: wrote.append(it["text"]) or True)
        p.popup(0)
        p._exec_row(1)                        # 链 A 选 "one"
        p.popup(0)                            # 立即重新呼出 → 链 A 作废
        qtbot.wait(400)
        assert wrote == []
        p._exec_row(0)                        # 链 B 选 "two"
        qtbot.waitUntil(lambda: bool(wrote), timeout=1000)
        assert wrote == ["two"]

    def test_filter_toggles_visibility_without_rebuild(self, qtbot):
        """⑤：按键过滤只切既有行显隐——控件数恒定，清空搜索全部恢复。"""
        _seed_history("alpha one", "beta two", "gamma")
        p, _win = _panel(qtbot)
        p.popup(0)
        assert p.lst.count() == 3
        p.edt.setText("beta")
        assert p.lst.count() == 3             # 不再 clear+重建
        assert [p.lst.item(i).isHidden() for i in range(3)] == [True, False, True]
        assert [it["text"] for it in p._rows] == ["beta two"]
        assert p.lst.currentRow() == 1        # 选中落在可见命中行上
        p.edt.setText("")
        assert all(not p.lst.item(i).isHidden() for i in range(3))
        assert len(p._rows) == 3

    def _png_entry(self, argb=0xFF00FF00):
        """造一枚真 PNG 载荷（md5 内容寻址落盘），返回文件名。"""
        img = QImage(4, 4, QImage.Format_ARGB32)
        img.fill(argb)
        buf = QBuffer()
        buf.open(QBuffer.WriteOnly)
        img.save(buf, "PNG")
        return clipboard_mod.save_image_bytes(bytes(buf.data()))

    def test_image_rows_show_thumbnail_and_fallback(self, qtbot):
        """图片行文件在 → 真缩略图替位（无 🖼）；载荷丢失 → 回退 emoji 占位。"""
        name = self._png_entry()
        hist = clipboard_mod.ClipboardHistory()
        hist.add("[图片 B]", kind="image", data="missing.png")
        hist.add("[图片 A]", kind="image", data=name)     # 最新在前：第 0 行 A
        hist.save()
        p, _win = _panel(qtbot)
        p.popup(0)
        w0 = p.lst.item(0)                                # [图片 A]（文件在）
        assert not w0.icon().isNull()
        assert "🖼" not in w0.text()
        w1 = p.lst.item(1)                                # [图片 B]（载荷丢失）
        assert w1.icon().isNull()
        assert w1.text().startswith("🖼 ")

    def test_icon_for_single_source_and_cache(self):
        """单源助手：同载荷返回同一 QIcon（缓存命中），丢失/空载荷稳定 None。"""
        from syskit.ui import clip_thumbs
        clip_thumbs._cache.clear()
        name = self._png_entry()
        a = clip_thumbs.icon_for(name)
        assert a is not None and a is clip_thumbs.icon_for(name)
        assert clip_thumbs.icon_for("missing.png") is None
        assert clip_thumbs.icon_for("") is None

    def test_icon_cache_bounded(self):
        """托盘长驻内存治理：缓存超上限按插入序淘汰最旧条目，上限不被突破。"""
        from syskit.ui import clip_thumbs
        clip_thumbs._cache.clear()
        cap = clip_thumbs._CACHE_CAP
        names = [self._png_entry(argb=0xFF000000 | (0x123456 + i)) for i in range(6)]
        for n in names[:5]:
            clip_thumbs.icon_for(n)
        assert len(clip_thumbs._cache) == 5
        clip_thumbs.icon_for("missing.png")            # 失败项也计入缓存
        assert len(clip_thumbs._cache) == 6
        # 模拟逼近上限：塞满后新条目触发淘汰，长度回落到 cap 以内
        clip_thumbs._cache.update({(f"pad{i}", 28): False for i in range(cap - 6)})
        clip_thumbs.icon_for(names[5])                  # 第 cap 个之后的新插入
        assert len(clip_thumbs._cache) <= cap
        # 最早插入的条目被淘汰（不保证具体哪几个，但首个必走）
        assert (names[0], 28) not in clip_thumbs._cache
        clip_thumbs._cache.clear()

    def test_arrow_keys_skip_hidden_rows(self, qtbot):
        """⑤：过滤后 ↑↓ 只在可见行间移动（中间隐藏行不消耗按键）。"""
        _seed_history("alpha", "bzzz", "apricot")     # 最新在前
        p, _win = _panel(qtbot)
        p.popup(0)
        p.edt.setText("a")                    # 命中第 0/2 行，第 1 行隐藏
        assert p.lst.currentRow() == 0
        qtbot.keyClick(p.edt, Qt.Key_Down)
        assert p.lst.currentRow() == 2
        qtbot.keyClick(p.edt, Qt.Key_Up)
        assert p.lst.currentRow() == 0

    def test_enter_on_no_match_pastes_nothing(self, qtbot, monkeypatch):
        """⑤：过滤无命中时 current 可能停在隐藏行——回车不得误粘旧选择。"""
        _seed_history("one")
        p, _win = _panel(qtbot)
        calls = []
        monkeypatch.setattr(p, "_do_paste", lambda it, hwnd, seq: calls.append(it))
        p.popup(0)
        p.edt.setText("不存在的关键字")
        assert p._rows == []
        p._exec_current()
        qtbot.wait(300)
        assert calls == []

    def test_item_activated_uses_bound_entry_identity(self, qtbot, monkeypatch):
        """itemActivated / itemDoubleClicked（含过滤态）必须按行控件绑定的
        条目身份执行（UserRole 身份回传），不得按可见序号取条目——过滤后
        行号与 _rows 不对齐，按序号会粘错行。"""
        _seed_history("alpha", "beta")
        p, _win = _panel(qtbot)
        calls = []
        monkeypatch.setattr(p, "_do_paste",
                            lambda it, hwnd, seq: calls.append(it["text"]))
        p.popup(0)
        p.edt.setText("bet")            # 过滤：仅 beta 命中（最新在前：beta 在第 0 行）
        w = p.lst.item(0)               # beta 的行控件
        assert w is not None and not w.isHidden()
        p.lst.itemActivated.emit(w)     # 激活路径（非 paste_index / 非 _exec_row）
        qtbot.wait(300)
        assert calls == ["beta"]

    def test_deactivate_hides_panel_without_pasting(self, qtbot, monkeypatch):
        """⑦：焦点离开自动收起（Ditto/Win+V 同语义），不写回不注键。"""
        _seed_history("one")
        p, _win = _panel(qtbot)
        sent = []
        monkeypatch.setattr(paste_mod, "send_ctrl_v", lambda: sent.append(1) or True)
        p.popup(0)
        assert p.isVisible()
        QApplication.sendEvent(p, QEvent(QEvent.WindowDeactivate))
        assert not p.isVisible()
        qtbot.wait(300)
        assert sent == []


# ---------------- 热键文案跟随设置（建议②） ----------------

class TestHotkeyHint:
    def test_hint_normalizes_configured_key(self):
        settings.save({"clip_paste_hotkey": "ctrl+alt+k"})
        assert clip_paste_mod.hotkey_hint() == "Ctrl+Alt+K"

    def test_hint_empty_when_disabled_or_invalid(self):
        settings.save({"clip_paste_hotkey": ""})
        assert clip_paste_mod.hotkey_hint() == ""
        settings.save({"clip_paste_hotkey": "Alt+Shift"})
        assert clip_paste_mod.hotkey_hint() == ""

    def test_tray_tip_follows_setting_and_refreshes_on_popup(self, qtbot):
        from syskit.ui.tray import TrayHost
        settings.save({"clip_paste_hotkey": "Ctrl+Shift+V"})
        win = _FakeWindow()                       # 带 toggle_clip_paste → 菜单有此条目
        qtbot.addWidget(win)
        tray = TrayHost(win)
        try:
            act = tray._act_paste
            assert act is not None
            assert "Ctrl+Shift+V" in act.toolTip()    # 不再是写死的默认键
            settings.save({"clip_paste_hotkey": ""})
            tray.contextMenu().aboutToShow.emit()     # 弹出菜单 → 刷新
            assert "未设热键" in act.toolTip()
        finally:
            tray.hide()                               # 仿 tray_gaps 夹具：尽早销毁防收尾崩溃
            tray.deleteLater()
            QApplication.processEvents()


# ---------------- 主窗口接线 ----------------

def test_main_window_clip_paste_integration(make_main_window, monkeypatch):
    from syskit import proc
    from syskit.proc import CmdResult
    monkeypatch.setattr("syskit.repairs.wlan_profiles", lambda: [])
    monkeypatch.setattr(proc, "run", lambda *a, **k: CmdResult(0, "", ""))
    monkeypatch.setattr(proc, "run_ps", lambda *a, **k: CmdResult(0, "", ""))
    win = make_main_window()
    assert hasattr(win, "toggle_clip_paste")
    assert hasattr(win, "quick_paste_latest")
    assert win._clip_paste is None               # 懒建
    win.toggle_clip_paste()
    assert win._clip_paste is not None and win._clip_paste.isVisible()
    win.toggle_clip_paste()
    assert not win._clip_paste.isVisible()

    # 设置页控件存在且回填默认值（面板 Ctrl+Win+V / 快速 Ctrl+Alt+V）
    assert win.cmb_paste_hotkey.currentText() == "Ctrl+Win+V"
    assert win.cmb_quick_hotkey.currentText() == "Ctrl+Alt+V"

    # 保存即重注册（面板 id=3 / 快速 id=2），空串 = 禁用不再注册
    from syskit.ui.palette import teardown_paste_hotkey, teardown_quick_paste_hotkey
    reg = []
    monkeypatch.setattr(hotkey_mod, "register",
                        lambda m, v, hotkey_id=1: reg.append(hotkey_id) or True)
    monkeypatch.setattr("syskit.admin.is_admin", lambda: False)
    win.cmb_paste_hotkey.setCurrentText("Ctrl+Alt+B")
    win._save_settings()
    assert 3 in reg                              # 面板键重注册
    assert 2 in reg                              # 快速键同场重注册（未改也重挂）
    win.cmb_paste_hotkey.setCurrentText("")
    reg.clear()
    win._save_settings()
    assert settings.load().get("clip_paste_hotkey") == ""
    assert 3 not in reg                          # 面板禁用：不注册
    assert 2 in reg                              # 快速键不受影响
    teardown_paste_hotkey(win)
    teardown_quick_paste_hotkey(win)


def test_rehook_fallback_to_disabled_wording(make_main_window, monkeypatch):
    """旧值为「禁用」时新键被占：回退提示不得渲染成「已恢复为「」」。"""
    from PySide6.QtWidgets import QMessageBox
    from syskit import proc
    from syskit.proc import CmdResult
    monkeypatch.setattr("syskit.repairs.wlan_profiles", lambda: [])
    monkeypatch.setattr(proc, "run", lambda *a, **k: CmdResult(0, "", ""))
    monkeypatch.setattr(proc, "run_ps", lambda *a, **k: CmdResult(0, "", ""))
    win = make_main_window()
    settings.save({"clip_paste_hotkey": ""})           # 旧值 = 禁用
    infos = []
    monkeypatch.setattr(QMessageBox, "information",
                        staticmethod(lambda *a, **k: infos.append(k or a)))
    win._rehook_hotkey("clip_paste_hotkey", lambda w: False, lambda w: None,
                       "Ctrl+Alt+Q", "", win.cmb_paste_hotkey, "x")
    assert len(infos) == 1
    assert "无热键（已禁用）" in str(infos[0])
    assert settings.load().get("clip_paste_hotkey") == ""    # 回退后仍为禁用

from syskit.ui.clip_paste import write_back_entry  # noqa: E402  （新块内直接引用）


# ---------------- Win32 原语扩展（NOREPEAT / release_modifiers / magic） ----------------

@pytest.mark.skipif(sys.platform != "win32", reason="Win32 原语仅限 Windows")
class TestPastePrimitiveExtensions:
    def test_register_sets_norepeat(self, monkeypatch):
        """硬约束（规格书 1）：注册恒置 MOD_NOREPEAT（长按不重复触发）。"""
        import types
        calls = []

        class _U32:
            def RegisterHotKey(self, hwnd, hid, mods, vk):
                calls.append((hid, mods, vk))
                return True

        monkeypatch.setattr(ctypes, "windll", types.SimpleNamespace(user32=_U32()))
        assert hotkey_mod.register(hotkey_mod.MOD_CONTROL | hotkey_mod.MOD_ALT,
                                   0x56, hotkey_id=2) is True
        hid, mods, vk = calls[0]
        assert (hid, vk) == (2, 0x56)
        assert mods == (hotkey_mod.MOD_CONTROL | hotkey_mod.MOD_ALT
                        | hotkey_mod.MOD_NOREPEAT)

    def test_release_modifiers_order_and_magic(self, monkeypatch):
        """硬约束④⑤：KEYUP 顺序固定 SHIFT→CONTROL→MENU→LWIN→RWIN，
        dwExtraInfo 统一 magic 0x53594B54。"""
        seen = {}

        class _U32:
            def SendInput(self, n, arr, size):
                seen["events"] = [(arr[i].iu.ki.wVk, arr[i].iu.ki.dwFlags,
                                   arr[i].iu.ki.dwExtraInfo) for i in range(n)]
                return n

        monkeypatch.setattr(paste_mod, "_user32", lambda: _U32())
        assert paste_mod.release_modifiers() is True
        ev = seen["events"]
        assert [e[0] for e in ev] == [paste_mod.VK_SHIFT, paste_mod.VK_CONTROL,
                                      paste_mod.VK_MENU, paste_mod.VK_LWIN,
                                      paste_mod.VK_RWIN]
        assert all(e[1] == paste_mod._KEY_UP for e in ev)
        assert all(e[2] == paste_mod.PASTE_MAGIC for e in ev)

    def test_send_ctrl_v_carries_magic(self, monkeypatch):
        """硬约束⑤：Ctrl+V 注入四事件全部携带 magic。"""
        seen = {}

        class _U32:
            def SendInput(self, n, arr, size):
                seen["vk"] = [arr[i].iu.ki.wVk for i in range(n)]
                seen["extra"] = [arr[i].iu.ki.dwExtraInfo for i in range(n)]
                return n

        monkeypatch.setattr(paste_mod, "_user32", lambda: _U32())
        assert paste_mod.send_ctrl_v() is True
        assert seen["vk"] == [paste_mod.VK_CONTROL, paste_mod.VK_V,
                              paste_mod.VK_V, paste_mod.VK_CONTROL]
        assert all(e == paste_mod.PASTE_MAGIC for e in seen["extra"])

    def test_send_count_mismatch_fails(self, monkeypatch):
        """硬约束⑥：SendInput 返回数 != 期望注入数 → 视为失败。"""

        class _U32:
            def SendInput(self, n, arr, size):
                return n - 1        # 模拟部分注入被拦

        monkeypatch.setattr(paste_mod, "_user32", lambda: _U32())
        assert paste_mod.release_modifiers() is False
        assert paste_mod.send_ctrl_v() is False


# ---------------- 写回多格式（file/html，硬约束⑦⑧） ----------------

class TestWriteBackMime:
    def test_file_entry_uses_uri_list(self, qtbot):
        """硬约束⑦：file 条目走 QMimeData.setUrls(fromLocalFile)，
        mime.formats() 必含 text/uri-list；禁止 setText(路径)。"""
        clipboard_mod._suppressed.clear()
        hist = clipboard_mod.ClipboardHistory()
        hist.add("C:\\测试目录\\a.txt\nC:\\测试目录\\b.txt", kind="file")
        hist.save()
        it = hist.items()[0]
        assert write_back_entry(it) is True
        mime = QApplication.clipboard().mimeData()
        assert "text/uri-list" in mime.formats()
        assert [u.toLocalFile() for u in mime.urls()] == \
            ["C:/测试目录/a.txt", "C:/测试目录/b.txt"]   # QUrl 归一化正斜杠
        # 回填不算一次新复制：路径列表文本在抑制窗口内不入库
        assert hist.add(it["text"], kind="file") is False

    def test_html_entry_sets_html_with_text_fallback(self, qtbot):
        """硬约束⑧：html 条目 setHtml(原始 HTML) + setText(纯文本兜底)。"""
        it = {"kind": "html", "text": "加粗文本", "data": "<b>加粗</b>"}
        assert write_back_entry(it) is True
        mime = QApplication.clipboard().mimeData()
        assert mime.html() == "<b>加粗</b>"
        assert mime.text() == "加粗文本"


# ---------------- 焦点守卫注键（硬约束⑩⑥单源） ----------------

class TestInjectGuard:
    def _patch_env(self, monkeypatch, fg, proc="notepad"):
        monkeypatch.setattr(paste_mod, "foreground_window", lambda: fg)
        monkeypatch.setattr(clipboard_mod, "foreground_process_name",
                            lambda: proc)
        calls = []
        monkeypatch.setattr(paste_mod, "release_modifiers",
                            lambda: calls.append("rel") or True)
        monkeypatch.setattr(paste_mod, "send_ctrl_v",
                            lambda: calls.append("send") or True)
        return calls

    def test_fg_is_target_pastes(self, qtbot, monkeypatch):
        notes = []
        calls = self._patch_env(monkeypatch, 0x4321)
        clip_paste_mod.inject_ctrl_v_guarded(0x4321, {"kind": "text", "text": "x"},
                                             notes.append)
        qtbot.waitUntil(lambda: bool(notes), timeout=2000)
        assert notes == ["已粘贴到原窗口"]
        assert calls == ["rel", "send"]      # 修饰键先于注键

    def test_third_party_fg_aborts(self, qtbot, monkeypatch):
        """硬约束⑩：前台为第三方应用 → 放弃注键并托盘提示，不抢焦点。"""
        notes = []
        calls = self._patch_env(monkeypatch, 0x9999)   # 0x9999 != target
        clip_paste_mod.inject_ctrl_v_guarded(0x4321, {"kind": "text", "text": "x"},
                                             notes.append)
        qtbot.waitUntil(lambda: bool(notes), timeout=2000)
        assert "目标窗口已切换" in notes[0] and "已取消自动粘贴" in notes[0]
        assert calls == []

    def test_own_fg_refocuses_then_verifies(self, qtbot, monkeypatch):
        """前台为本应用 → focus_window 后复核通过才注键。"""
        notes = []
        fg = {"hwnd": 0x1111}
        monkeypatch.setattr(clipboard_mod, "foreground_process_name",
                            lambda: "syskit")
        monkeypatch.setattr(paste_mod, "foreground_window", lambda: fg["hwnd"])
        monkeypatch.setattr(paste_mod, "focus_window",
                            lambda hwnd: fg.update(hwnd=hwnd) or True)
        calls = []
        monkeypatch.setattr(paste_mod, "release_modifiers",
                            lambda: calls.append("rel") or True)
        monkeypatch.setattr(paste_mod, "send_ctrl_v",
                            lambda: calls.append("send") or True)
        clip_paste_mod.inject_ctrl_v_guarded(0x4321, {"kind": "text", "text": "x"},
                                             notes.append)
        qtbot.waitUntil(lambda: bool(notes), timeout=2000)
        assert notes == ["已粘贴到原窗口"]
        assert calls == ["rel", "send"]

    def test_own_fg_refocus_fail_cancels(self, qtbot, monkeypatch):
        """还焦后复核不过（前台仍是自身）→ 取消，绝不注到错误窗口。"""
        notes = []
        self._patch_env(monkeypatch, 0x1111, proc="syskit")
        monkeypatch.setattr(paste_mod, "focus_window", lambda hwnd: False)
        clip_paste_mod.inject_ctrl_v_guarded(0x4321, {"kind": "text", "text": "x"},
                                             notes.append)
        qtbot.waitUntil(lambda: bool(notes), timeout=2000)
        assert "目标窗口已切换" in notes[0]

    def test_release_failure_degrades_without_send(self, qtbot, monkeypatch):
        """硬约束⑥：release_modifiers 失败 → 不注键，降级提示。"""
        notes = []
        monkeypatch.setattr(paste_mod, "foreground_window", lambda: 0x4321)
        monkeypatch.setattr(clipboard_mod, "foreground_process_name",
                            lambda: "notepad")
        calls = []
        monkeypatch.setattr(paste_mod, "release_modifiers", lambda: False)
        monkeypatch.setattr(paste_mod, "send_ctrl_v",
                            lambda: calls.append("send") or True)
        clip_paste_mod.inject_ctrl_v_guarded(0x4321, {"kind": "text", "text": "x"},
                                             notes.append)
        qtbot.waitUntil(lambda: bool(notes), timeout=2000)
        assert "自动粘贴未送达" in notes[0]
        assert calls == []

    def test_zero_target_falls_back_to_manual(self, qtbot, monkeypatch):
        notes = []
        self._patch_env(monkeypatch, 0)
        clip_paste_mod.inject_ctrl_v_guarded(
            0, {"kind": "image", "text": "[图片]", "data": "x"}, notes.append)
        qtbot.waitUntil(lambda: bool(notes), timeout=2000)
        assert notes[0].startswith("图片已复制")

    def test_seq_check_false_is_silent(self, qtbot, monkeypatch):
        notes = []
        calls = self._patch_env(monkeypatch, 0x4321)
        clip_paste_mod.inject_ctrl_v_guarded(0x4321, {"kind": "text", "text": "x"},
                                             notes.append, seq_check=lambda: False)
        qtbot.wait(300)
        assert notes == [] and calls == []


# ---------------- 双热键 id 分发互不串扰 ----------------

class TestDualIdDispatch:
    def test_quick_and_panel_ids_dispatch_independently(self, qtbot):
        quick_fired, panel_fired = [], []
        fq = HotkeyFilter(lambda: quick_fired.append(1), expected_ids=(2,))
        fp = HotkeyFilter(lambda: panel_fired.append(1), expected_ids=(3,))
        handled, _ = fq.nativeEventFilter(b"windows_generic_MSG",
                                          TestFilterDispatch._msg(2))
        assert handled is True and quick_fired == [1] and panel_fired == []
        handled, _ = fp.nativeEventFilter(b"windows_generic_MSG",
                                          TestFilterDispatch._msg(3))
        assert handled is True and panel_fired == [1]
        handled, _ = fp.nativeEventFilter(b"windows_generic_MSG",
                                          TestFilterDispatch._msg(2))
        assert handled is False                    # 快速键归另一过滤器


# ---------------- 快速粘贴链（MainWindow.quick_paste_latest） ----------------

def _stub_system(monkeypatch):
    """构造 MainWindow 前必须打桩的系统调用（conftest 哨兵红线）。"""
    from syskit import proc
    from syskit.proc import CmdResult
    monkeypatch.setattr("syskit.repairs.wlan_profiles", lambda: [])
    monkeypatch.setattr(proc, "run", lambda *a, **k: CmdResult(0, "", ""))
    monkeypatch.setattr(proc, "run_ps", lambda *a, **k: CmdResult(0, "", ""))


def _prep_settings(panel=None, quick=None, migrated=True):
    """构造窗口前落设置（面板/快速键、迁移标志）。"""
    patch = {"migrated_v2": migrated}
    if panel is not None:
        patch["clip_paste_hotkey"] = panel
    if quick is not None:
        patch["clip_quick_hotkey"] = quick
    settings.save(patch)


class TestQuickPasteLatest:
    def test_pastes_newest_entry_with_modifier_order(self, qtbot, make_main_window, monkeypatch):
        _stub_system(monkeypatch)
        win = make_main_window()
        notes = []
        monkeypatch.setattr(win.tray, "notify",
                            lambda title, msg, level="ok": notes.append(msg))
        hist = clipboard_mod.ClipboardHistory()
        hist.add("older")
        hist.add("newest")
        hist.save()
        monkeypatch.setattr(paste_mod, "foreground_window", lambda: 0x777)
        monkeypatch.setattr(clipboard_mod, "foreground_process_name",
                            lambda: "notepad")
        calls = []
        monkeypatch.setattr(paste_mod, "release_modifiers",
                            lambda: calls.append("rel") or True)
        monkeypatch.setattr(paste_mod, "send_ctrl_v",
                            lambda: calls.append("send") or True)
        win.quick_paste_latest()
        qtbot.waitUntil(lambda: bool(notes), timeout=2000)
        assert notes[0] == "已粘贴到原窗口"
        assert calls == ["rel", "send"]       # 修饰键先于注键
        assert QApplication.clipboard().text() == "newest"

    def test_empty_history_notifies_without_sending(self, qtbot, make_main_window, monkeypatch):
        _stub_system(monkeypatch)
        win = make_main_window()
        notes = []
        monkeypatch.setattr(win.tray, "notify",
                            lambda title, msg, level="ok": notes.append(msg))
        clipboard_mod.ClipboardHistory().clear()
        clipboard_mod.ClipboardHistory().save()
        monkeypatch.setattr(paste_mod, "foreground_window", lambda: 0x777)
        calls = []
        monkeypatch.setattr(paste_mod, "send_ctrl_v",
                            lambda: calls.append(1) or True)
        win.quick_paste_latest()
        assert "剪贴板历史为空" in notes[0]
        assert calls == []

    def test_write_back_failure_notifies_without_sending(self, qtbot, make_main_window, monkeypatch):
        _stub_system(monkeypatch)
        win = make_main_window()
        notes = []
        monkeypatch.setattr(win.tray, "notify",
                            lambda title, msg, level="ok": notes.append(msg))
        hist = clipboard_mod.ClipboardHistory()
        hist.add("something")
        hist.save()
        import syskit.ui.clip_paste as cp
        monkeypatch.setattr(cp, "write_back_entry", lambda it: False)
        monkeypatch.setattr(paste_mod, "foreground_window", lambda: 0x777)
        calls = []
        monkeypatch.setattr(paste_mod, "send_ctrl_v",
                            lambda: calls.append(1) or True)
        win.quick_paste_latest()
        assert "粘贴失败" in notes[0]
        assert calls == []


# ---------------- 迁移（硬约束⑪）与 Office 警示（硬约束①） ----------------

class TestMigrationAndWarn:
    def test_migration_moves_old_default_once(self, make_main_window, monkeypatch):
        _prep_settings(panel="Ctrl+Alt+V", migrated=False)
        _stub_system(monkeypatch)
        win = make_main_window()
        assert settings.load()["migrated_v2"] is True
        assert settings.load()["clip_paste_hotkey"] == "Ctrl+Win+V"
        assert settings.load()["clip_quick_hotkey"] == "Ctrl+Alt+V"
        assert win.cmb_paste_hotkey.currentText() == "Ctrl+Win+V"

    def test_migration_idempotent_user_change_respected(self, make_main_window, monkeypatch):
        """二次调用不再迁移——migrated_v2 幂等，用户改回旧值不被搬回。"""
        _prep_settings(panel="Ctrl+Alt+V", migrated=False)
        _stub_system(monkeypatch)
        win = make_main_window()               # 首启迁移完成
        settings.save({"clip_paste_hotkey": "Ctrl+Alt+V"})   # 用户改回旧键
        win._migrate_hotkey_settings_v2()      # 二次调用
        assert settings.load()["clip_paste_hotkey"] == "Ctrl+Alt+V"

    def test_migration_keeps_custom_panel_key(self, make_main_window, monkeypatch):
        _prep_settings(panel="Ctrl+Shift+V", migrated=False)
        _stub_system(monkeypatch)
        win = make_main_window()
        assert settings.load()["clip_paste_hotkey"] == "Ctrl+Shift+V"
        assert settings.load()["migrated_v2"] is True

    def test_office_warn_label_visibility(self, make_main_window, monkeypatch):
        """硬约束①：热键为 Ctrl+Alt+V 时显示覆盖警示，改键即隐。"""
        _stub_system(monkeypatch)
        win = make_main_window()
        warn_text = "此键会覆盖 Office 选择性粘贴等应用内快捷键，冲突时请改用其他组合。"
        assert warn_text in win.lbl_quick_warn.text()
        assert not win.lbl_quick_warn.isHidden()      # 快速键默认 Ctrl+Alt+V → 显示
        assert win.lbl_paste_warn.isHidden()          # 面板键默认 Ctrl+Win+V → 隐藏
        win.cmb_quick_hotkey.setCurrentText("Ctrl+Shift+V")
        assert win.lbl_quick_warn.isHidden()
        win.cmb_quick_hotkey.setCurrentText("ctrl+alt+v")   # 大小写不敏感
        assert not win.lbl_quick_warn.isHidden()


# ---------------- 面板热键回退链（硬约束③） ----------------

def _stub_register_by_win(monkeypatch, fail_win_key=True):
    """注册桩：Ctrl+Win 组合按需失败（模拟 1409 占用），其余成功。"""

    def fake_register(m, v, hotkey_id=1):
        if fail_win_key and (m & hotkey_mod.MOD_WIN) and v == 0x56:
            return False
        return True

    monkeypatch.setattr(hotkey_mod, "register", fake_register)
    monkeypatch.setattr(hotkey_mod, "unregister", lambda hotkey_id=1: True)


def _attach_boxes(monkeypatch, win):
    from PySide6.QtWidgets import QMessageBox
    boxes = {"info": [], "warn": []}
    monkeypatch.setattr(QMessageBox, "information",
                        staticmethod(lambda *a, **k: boxes["info"].append(str(a))))
    monkeypatch.setattr(QMessageBox, "warning",
                        staticmethod(lambda *a, **k: boxes["warn"].append(str(a))))
    return boxes


class TestPanelFallbackChain:
    def test_winv_occupied_falls_back_to_ctrl_alt_v(self, make_main_window, monkeypatch):
        """Ctrl+Win+V 被占 → 回退 Ctrl+Alt+V 预设（快速键未占用），提示回退。"""
        _prep_settings(panel="Ctrl+Win+V", quick="Ctrl+Shift+V")
        _stub_system(monkeypatch)
        win = make_main_window()
        _stub_register_by_win(monkeypatch, fail_win_key=True)
        boxes = _attach_boxes(monkeypatch, win)
        win._rehook_paste_hotkey("Ctrl+Win+V", "Ctrl+Win+V")
        assert settings.load()["clip_paste_hotkey"] == "Ctrl+Alt+V"
        assert win.cmb_paste_hotkey.currentText() == "Ctrl+Alt+V"
        assert len(boxes["info"]) == 1 and "Ctrl+Alt+V" in boxes["info"][0]

    def test_fallback_taken_by_quick_key_disables_and_prompts(self, make_main_window, monkeypatch):
        """Ctrl+Alt+V 已被快速键占用 → 面板热键禁用 + 提示自定义（硬约束③）。"""
        _prep_settings(panel="Ctrl+Win+V", quick="Ctrl+Alt+V")
        _stub_system(monkeypatch)
        win = make_main_window()
        _stub_register_by_win(monkeypatch, fail_win_key=True)
        boxes = _attach_boxes(monkeypatch, win)
        win._rehook_paste_hotkey("Ctrl+Win+V", "Ctrl+Win+V")
        assert settings.load()["clip_paste_hotkey"] == ""
        assert win.cmb_paste_hotkey.currentText() == ""
        assert len(boxes["warn"]) == 1 and "自定义" in boxes["warn"][0]

    def test_quick_disabled_fallback_free(self, make_main_window, monkeypatch):
        """快速键禁用时回退链不受占用限制。"""
        _prep_settings(panel="Ctrl+Win+V", quick="")
        _stub_system(monkeypatch)
        win = make_main_window()
        _stub_register_by_win(monkeypatch, fail_win_key=True)
        boxes = _attach_boxes(monkeypatch, win)
        win._rehook_paste_hotkey("Ctrl+Win+V", "Ctrl+Win+V")
        assert settings.load()["clip_paste_hotkey"] == "Ctrl+Alt+V"
        assert len(boxes["info"]) == 1
