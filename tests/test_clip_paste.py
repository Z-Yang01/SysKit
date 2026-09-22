"""剪贴板快捷粘贴：paste 原语 / 回填抑制 / 第二全局热键 / ClipPastePanel UI。

不注入真实按键（focus_window/send_ctrl_v 在 UI 路径测试中一律被 mock）；
不依赖真实前台窗口（focus_window 用非法句柄走 False 分支）。
"""
import ctypes
import sys
import time
from ctypes import wintypes

import pytest
from PySide6.QtCore import QEvent, Qt
from PySide6.QtGui import QCursor
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

    def quit_app(self):                 # TrayHost 构造期即 connect（建议②托盘测试）
        pass


def _panel(qtbot):
    win = _FakeWindow()
    qtbot.addWidget(win)
    p = ClipPastePanel(win)
    qtbot.addWidget(p)
    return p, win


def _seed_history(*texts):
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
        assert isinstance(filt, HotkeyFilter) and filt.expected_ids == (2,)
        teardown_paste_hotkey(win)
        assert win._paste_hotkey_filter is None


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
        focused = []
        monkeypatch.setattr(paste_mod, "focus_window",
                            lambda hwnd: focused.append(hwnd) or True)
        monkeypatch.setattr(paste_mod, "send_ctrl_v", lambda: True)
        p.popup(0)
        p._do_paste(p._rows[0], 4321, p._paste_seq)
        assert focused == [4321]
        # 还焦成功后按键注入要等一拍事件循环（激活消息先派发）
        qtbot.waitUntil(lambda: bool(win.tray.notes), timeout=2000)
        assert win.tray.notes[-1][1].startswith("已粘贴")

        # 还焦失败：不注按键，立即降级提示
        win.tray.notes.clear()
        monkeypatch.setattr(paste_mod, "focus_window", lambda hwnd: False)
        p._do_paste(p._rows[0], 4321, p._paste_seq)
        assert "Ctrl+V" in win.tray.notes[-1][1]

        # 还焦成功但 SendInput 未生效 → 降级提示（等一拍后）
        win.tray.notes.clear()
        monkeypatch.setattr(paste_mod, "focus_window", lambda hwnd: True)
        monkeypatch.setattr(paste_mod, "send_ctrl_v", lambda: False)
        p._do_paste(p._rows[0], 4321, p._paste_seq)
        qtbot.waitUntil(lambda: bool(win.tray.notes), timeout=2000)
        assert "Ctrl+V" in win.tray.notes[-1][1]

    def test_fallback_wording_names_content_kind(self, qtbot, monkeypatch):
        """建议⑤：降级提示按条目类型点名——图片/文件普通文本框接不住
        Ctrl+V，说清「复制的是什么」用户才知道该换目标窗口还是重按。"""
        _seed_history("t")
        p, win = _panel(qtbot)
        p.popup(0)
        monkeypatch.setattr(p, "_write_back", lambda it: True)   # 免造真实图片文件
        monkeypatch.setattr(paste_mod, "focus_window", lambda hwnd: False)
        img = {"kind": "image", "text": "[图片]", "data": "x"}
        p._do_paste(img, 4321, p._paste_seq)
        assert win.tray.notes[-1][1].startswith("图片已复制")
        p._do_paste({"kind": "file", "text": "a.txt"}, 4321, p._paste_seq)
        assert win.tray.notes[-1][1].startswith("文件已复制")
        p._do_paste(p._rows[0], 4321, p._paste_seq)               # 文本：无前缀
        assert win.tray.notes[-1][1].startswith("已复制")
        # SendInput 未送达分支同样点名类型
        win.tray.notes.clear()
        monkeypatch.setattr(paste_mod, "focus_window", lambda hwnd: True)
        monkeypatch.setattr(paste_mod, "send_ctrl_v", lambda: False)
        p._do_paste(img, 4321, p._paste_seq)
        qtbot.waitUntil(lambda: bool(win.tray.notes), timeout=2000)
        assert "图片已复制" in win.tray.notes[-1][1]

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
    assert win._clip_paste is None               # 懒建
    win.toggle_clip_paste()
    assert win._clip_paste is not None and win._clip_paste.isVisible()
    win.toggle_clip_paste()
    assert not win._clip_paste.isVisible()

    # 设置页新控件存在且回填已存值
    assert win.cmb_paste_hotkey.currentText() == "Ctrl+Alt+V"

    # 保存即重注册（第二热键 ID），空串 = 禁用不再注册
    from syskit.ui.palette import teardown_paste_hotkey
    reg = []
    monkeypatch.setattr(hotkey_mod, "register",
                        lambda m, v, hotkey_id=1: reg.append(hotkey_id) or True)
    monkeypatch.setattr("syskit.admin.is_admin", lambda: False)
    win.cmb_paste_hotkey.setCurrentText("Ctrl+Alt+B")
    win._save_settings()
    assert 2 in reg
    win.cmb_paste_hotkey.setCurrentText("")
    reg.clear()
    win._save_settings()
    assert settings.load().get("clip_paste_hotkey") == ""
    assert 2 not in reg                          # 禁用：不注册
    teardown_paste_hotkey(win)


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
