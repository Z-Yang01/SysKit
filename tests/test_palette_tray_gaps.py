"""命令面板 / 托盘 / 全局热键覆盖率补强（palette / tray / hotkey）。

与 tests/test_palette.py、tests/test_ui_smoke.py 互补，只打此前未覆盖的分支：
- hotkey：win 修饰键解析、Shift/Win 规范化显示、空主键、register/unregister 的
  非 Windows 短路 / ctypes 异常兜底 / 假件成功路径 —— ctypes.windll 全程注入
  假件，绝不触碰真实 RegisterHotKey/UnregisterHotKey。
- palette：_message_addr 的 c_char_p 字节回退分支；HotkeyFilter 的 event_type
  不可字节化与 MSG 解析异常兜底；setup/teardown_global_hotkey 的解析失败 /
  注册失败 / 成功三分支（hotkey.register/unregister 全 mock）；面板 Esc 关闭、
  空表回车、越界行执行、回调异常吞掉；dns 动态语法；剪贴板最近条目与回填
  （含历史读取异常兜底）；密码库全局搜索异常兜底；锁定态「解锁密码库」动作
  执行；_copy_back 文本 / 图片 / 无剪贴板；_copy_vault_password 取消 / 锁定 /
  成功路径。
- tray：「最近剪贴板」子菜单重建（有历史含图片条目 / 历史读取异常）、菜单动作
  点击回填剪贴板、_copy_back 全分支（文本 / 图片 / 空文本 / 无剪贴板 / 空图）。

安全：剪贴板统一注入假件；showMessage 记录型 no-op；hotkey 注册与
ctypes.windll 全部 monkeypatch；不发任何真实系统调用。
"""
import base64
import ctypes
import os
import sys
from types import SimpleNamespace
from ctypes import wintypes

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

import pytest
from PySide6.QtCore import QBuffer
from PySide6.QtGui import QImage
from PySide6.QtWidgets import (QApplication, QSystemTrayIcon, QTabWidget,
                               QWidget)

import syskit.ui.palette as palette_mod
from syskit import clipboard as clip_mod
from syskit import hotkey, palette_actions, settings
from syskit.ui.palette import (CommandPalette, HotkeyFilter, WM_HOTKEY,
                               _message_addr, setup_global_hotkey,
                               teardown_global_hotkey)
from syskit.ui.tray import TrayHost
from syskit.vault_crypto import VaultCrypto, VaultLockedError

RFC_SECRET = base64.b32encode(b"12345678901234567890").decode("ascii")


# ---------------- 测试假件 ----------------

@pytest.fixture(autouse=True)
def _fast_kdf(monkeypatch):
    """主密码 PBKDF2 调小到 1000 次迭代（真实 AESGCM/DPAPI 本地计算）。"""
    monkeypatch.setattr("syskit.vault_crypto.KDF_ITER", 1000, raising=False)


@pytest.fixture(autouse=True)
def _no_tray_bubble(monkeypatch):
    """托盘气泡 showMessage 记录型 no-op（offscreen 下真实现可能原生崩溃）。"""
    shown = []
    monkeypatch.setattr(QSystemTrayIcon, "showMessage",
                        lambda self, title, body, *a, **k: shown.append((title, body)))
    yield shown


class _FakeClip:
    """剪贴板假件：文本 / 图片 / 清空全部记录，绝不触碰系统剪贴板。"""

    def __init__(self):
        self.text_val = ""
        self.set_calls = []
        self.images = []
        self.cleared = 0

    def text(self):
        return self.text_val

    def setText(self, t):
        self.set_calls.append(t)
        self.text_val = t

    def setImage(self, img):
        self.images.append(img)

    def clear(self):
        self.cleared += 1


class _FakeTabs(QWidget):
    def __init__(self):
        super().__init__()
        self.tabs = QTabWidget(self)
        for n in ("速度测试", "局域网设备", "端口 / 进程", "DNS / 子网", "ping / 路由"):
            self.tabs.addTab(QWidget(), n)


class _FakeWindow(QWidget):
    """CommandPalette 依赖的最小窗口假件（页面栈 + 记录型动作）。"""

    def __init__(self):
        super().__init__()
        self.calls = []
        self.page_toolbox = _FakeTabs()
        self.page_system = None
        self.page_vault = None
        self.tray = SimpleNamespace(
            notify=lambda *a: self.calls.append(("notify",) + a))
        from PySide6.QtWidgets import QStackedWidget
        self.stack = QStackedWidget(self)
        for i in range(10):
            self.stack.addWidget(self.page_toolbox if i == 5 else QWidget())

    def switch_page(self, idx):
        self.calls.append(("page", idx))
        self.stack.setCurrentIndex(idx)

    def run_diagnosis(self, quick=False):
        self.calls.append(("diag", quick))

    def run_repair(self, aid, param=None):
        self.calls.append(("repair", aid))

    def run_palette_command(self, value):
        self.calls.append(("cmd", value))

    def showNormal(self):
        self.calls.append(("show",))

    def toggle_palette(self):
        self.calls.append(("toggle",))


def _palette(qtbot, win=None):
    win = win or _FakeWindow()
    qtbot.addWidget(win)
    pal = CommandPalette(win)
    qtbot.addWidget(pal)
    pal._actions = palette_actions.build_actions(win)
    return pal, win


def _save_png():
    """往隔离 APPDATA 的 clipboard_files 存一张真实 4×4 PNG，返回文件名。"""
    img = QImage(4, 4, QImage.Format_ARGB32)
    img.fill(0xFF00FF00)
    buf = QBuffer()
    buf.open(QBuffer.WriteOnly)
    img.save(buf, "PNG")
    return clip_mod.save_image_bytes(bytes(buf.data()))


def _patch_clipboard(monkeypatch, fake):
    app = QApplication.instance()
    monkeypatch.setattr(app, "clipboard", lambda: fake)
    return fake


# ---------------- hotkey.py 补强 ----------------

class TestHotkeyGaps:
    def test_empty_key_win_modifier_and_display(self):
        assert hotkey._key_to_vk("   ") is None              # 空主键 → None
        assert hotkey.parse_hotkey("f17") == (0, 0x80)       # 小写 F 键分支
        assert hotkey.parse_hotkey("Win+K") == (
            hotkey.MOD_WIN, ord("K"))                        # win 修饰键分支
        assert hotkey.hotkey_display("ctrl+shift+k") == "Ctrl+Shift+K"
        assert hotkey.hotkey_display("win+k") == "Win+K"
        assert hotkey.hotkey_display("Ctrl+Alt+Win+Space") == "Ctrl+Alt+Win+Space"

    def test_register_unregister_non_windows_shortcircuit(self, monkeypatch):
        monkeypatch.setattr(sys, "platform", "linux")
        assert hotkey.register(hotkey.MOD_CONTROL, 0x4B) is False
        assert hotkey.unregister(1) is False

    def test_register_unregister_success_with_fake_windll(self, monkeypatch):
        calls = []

        def _reg(*a):
            calls.append(("reg",) + a)
            return 1

        def _unreg(*a):
            calls.append(("unreg",) + a)
            return 1

        fake_user32 = SimpleNamespace(RegisterHotKey=_reg, UnregisterHotKey=_unreg)
        monkeypatch.setattr(ctypes, "windll", SimpleNamespace(user32=fake_user32))
        assert hotkey.register(hotkey.MOD_CONTROL | hotkey.MOD_ALT, 0x4B,
                               hotkey_id=7) is True
        assert hotkey.unregister(7) is True
        assert calls[0] == ("reg", None, 7,
                            hotkey.MOD_CONTROL | hotkey.MOD_ALT | hotkey.MOD_NOREPEAT,
                            0x4B)   # 注册恒置 NOREPEAT（长按不重复触发）
        assert calls[1] == ("unreg", None, 7)

    def test_register_unregister_ctypes_exception_returns_false(self, monkeypatch):
        def _boom(*a, **k):
            raise OSError("blocked by test")

        fake_user32 = SimpleNamespace(RegisterHotKey=_boom, UnregisterHotKey=_boom)
        monkeypatch.setattr(ctypes, "windll", SimpleNamespace(user32=fake_user32))
        assert hotkey.register(0, 0x4B) is False              # 异常吞掉 → False
        assert hotkey.unregister(1) is False


# ---------------- HotkeyFilter / _message_addr 补强 ----------------

class TestHotkeyFilterGaps:
    def test_message_addr_char_p_bytes_fallback(self):
        # int() 失败 → bytes() 逐字读（Qt sip.voidptr 之外的调用约定）
        raw = b"abcdefgh"                                    # 无 NUL，bytes() 不截断
        assert _message_addr(memoryview(raw)) == int.from_bytes(raw, "little")
        assert _message_addr(object()) == 0                  # 双失败 → 0

    def test_event_type_not_bytesable_falls_through(self):
        class _NoBytes:
            def __bytes__(self):
                raise TypeError("cannot convert")

        fired = []
        filt = HotkeyFilter(lambda: fired.append(1), expected_ids=(1,))
        handled, ret = filt.nativeEventFilter(_NoBytes(), None)
        assert handled is False and ret == 0 and fired == []

    def test_msg_parse_exception_swallowed(self, monkeypatch):
        """wintypes.MSG.from_address 抛异常 → 记录在案并放行，不得外泄。"""
        fired = []

        class _BrokenMSG:
            @staticmethod
            def from_address(addr):
                raise ValueError("bad address")

        monkeypatch.setattr(palette_mod, "wintypes",
                            SimpleNamespace(MSG=_BrokenMSG))
        filt = HotkeyFilter(lambda: fired.append(1), expected_ids=(1,))
        msg = wintypes.MSG()
        msg.message = WM_HOTKEY
        addr = ctypes.c_void_p(ctypes.addressof(msg))
        handled, ret = filt.nativeEventFilter(b"windows_generic_MSG", addr)
        assert handled is False and ret == 0 and fired == []


# ---------------- setup / teardown_global_hotkey ----------------

class TestGlobalHotkeySetup:
    def test_unparseable_hotkey_returns_false(self, qtbot):
        settings.save({"palette_hotkey": "Alt+Shift"})       # 无主键 → 无法解析
        win = _FakeWindow()
        qtbot.addWidget(win)
        assert setup_global_hotkey(win) is False
        assert getattr(win, "_hotkey_filter", None) is None

    def test_register_failure_returns_false_without_filter(self, qtbot, monkeypatch):
        registered = []
        monkeypatch.setattr(hotkey, "register",
                            lambda m, v, hotkey_id=1: registered.append(hotkey_id) or False)
        settings.save({"palette_hotkey": "Ctrl+Alt+K"})
        win = _FakeWindow()
        qtbot.addWidget(win)
        assert setup_global_hotkey(win) is False
        assert registered == [1]                              # 用设置里的 id 调用过（已 mock）
        assert getattr(win, "_hotkey_filter", None) is None

    def test_success_installs_filter_and_teardown_removes(self, qtbot, monkeypatch):
        monkeypatch.setattr(hotkey, "register", lambda m, v, hotkey_id=1: True)
        monkeypatch.setattr(hotkey, "unregister", lambda hotkey_id=1: True)
        settings.save({"palette_hotkey": "Ctrl+Alt+K"})
        win = _FakeWindow()
        qtbot.addWidget(win)
        assert setup_global_hotkey(win) is True
        filt = win._hotkey_filter
        assert isinstance(filt, HotkeyFilter)
        assert palette_mod._installed_filter is filt
        teardown_global_hotkey(win)
        assert win._hotkey_filter is None
        assert palette_mod._installed_filter is None

    def test_teardown_without_filter_is_noop(self, qtbot, monkeypatch):
        monkeypatch.setattr(hotkey, "unregister", lambda hotkey_id=1: True)
        win = _FakeWindow()
        qtbot.addWidget(win)
        teardown_global_hotkey(win)                           # 无过滤器 → 不崩
        assert getattr(win, "_hotkey_filter", None) is None


# ---------------- CommandPalette 生命周期与执行 ----------------

class TestCommandPaletteLifecycle:
    def test_popup_toggle_and_reject(self, qtbot):
        pal, _win = _palette(qtbot)
        pal.popup()
        assert pal.isVisible()
        assert pal.lst.count() >= 10                          # 空查询列出全部页面动作
        assert pal.lst.currentRow() == 0
        pal.toggle()                                          # 可见 → 隐藏
        assert not pal.isVisible()
        pal.toggle()                                          # 隐藏 → 再次弹出
        assert pal.isVisible()
        pal.reject()                                          # Esc 关闭
        assert not pal.isVisible()

    def test_exec_current_runs_row_zero(self, qtbot):
        pal, win = _palette(qtbot)
        pal._refresh()
        pal.edt.setText("")                                   # currentRow 复位到 0
        pal._exec_current()                                   # 回车 → 执行第 0 行（nav:0）
        assert ("page", 0) in win.calls
        assert not pal.isVisible()

    def test_exec_current_and_row_out_of_range_noop(self, qtbot):
        pal, win = _palette(qtbot)
        pal._rows = []
        pal.lst.clear()
        pal._exec_current()                                   # 空表：row 取 max(0,-1)=0 → 越界 no-op
        pal._exec_row(99)                                     # 越界 no-op
        pal._exec_row(-1)
        assert win.calls == []
        assert not pal.isVisible()

    def test_callback_exception_swallowed_and_hidden(self, qtbot):
        win = _FakeWindow()
        qtbot.addWidget(win)
        pal = CommandPalette(win)
        qtbot.addWidget(pal)
        pal._actions = [{"id": "boom", "label": "炸", "callback": lambda: 1 / 0}]
        pal._refresh()
        pal._exec_row(0)                                      # 异常被 LOGGER 吞掉且面板已隐藏
        assert not pal.isVisible()
        assert not win.calls


# ---------------- 动态条目 ----------------

class TestDynamicActionsGaps:
    def test_dns_syntax_routes_to_dns_tab(self, qtbot):
        pal, win = _palette(qtbot)
        pal.edt.setText("dns example.com")
        ids = [a["id"] for a in pal._rows]
        assert "dyn:dns" in ids
        pal._exec_row(ids.index("dyn:dns"))
        assert ("page", 5) in win.calls
        assert win.page_toolbox.tabs.currentIndex() == 3      # DNS / 子网 Tab

    def test_clipboard_recent_entries_listed_and_copy_back(self, qtbot, monkeypatch):
        clip_mod._sensitive.clear()
        hist = clip_mod.ClipboardHistory()
        hist.add("first entry")
        hist.add("second entry")
        hist.flush_if_dirty()                 # 防抖语义：跨实例读取前刷盘
        win = _FakeWindow()
        fake = _patch_clipboard(monkeypatch, _FakeClip())
        pal, _ = _palette(qtbot, win)
        pal._actions = []
        pal.edt.setText("q")                                  # 非空查询即追加最近条目
        ids = [a["id"] for a in pal._rows]
        assert "clip:0" in ids and "clip:1" in ids
        labels = [a["label"] for a in pal._rows if a["id"].startswith("clip:")]
        assert any("second entry" in x for x in labels)       # 最新在前
        pal._exec_row(ids.index("clip:0"))                    # 点击 → 回填剪贴板 + 气泡
        assert fake.set_calls == ["second entry"]
        assert ("notify", "剪贴板", "已复制到剪贴板") in win.calls

    def test_clipboard_history_error_swallowed(self, qtbot, monkeypatch):
        def boom(*a, **k):
            raise RuntimeError("history io error")

        monkeypatch.setattr(clip_mod, "ClipboardHistory", boom)
        pal, _win = _palette(qtbot)
        pal._actions = []
        pal.edt.setText("x")
        assert not [a for a in pal._rows if a["id"].startswith("clip:")]

    def test_vault_global_search_error_swallowed(self, qtbot, monkeypatch):
        from syskit import vault as vault_mod
        crypto = VaultCrypto()
        crypto.setup()
        vault_mod.init_db()
        win = _FakeWindow()
        win.page_vault = SimpleNamespace(crypto=crypto)
        pal, _ = _palette(qtbot, win)

        def boom(q):
            raise RuntimeError("db gone")

        monkeypatch.setattr(vault_mod, "global_search", boom)
        pal.edt.setText("github")
        assert not [a for a in pal._rows if a["id"].startswith("vault:")]

    def test_vault_unlock_action_executes(self, qtbot):
        """锁定态动态项「解锁密码库…」执行 → 跳密码库页并触发解锁。"""
        from syskit import vault as vault_mod
        crypto = VaultCrypto()
        crypto.setup("pw")
        vault_mod.init_db()
        sid = vault_mod.add_site("GitHub")
        vault_mod.add_account(sid, "pw", crypto, username="alice")
        crypto.lock()
        ensure_calls = []
        win = _FakeWindow()
        win.page_vault = SimpleNamespace(crypto=crypto,
                                         _ensure_unlocked=lambda: ensure_calls.append(1))
        pal, _ = _palette(qtbot, win)
        pal._actions = []
        pal.edt.setText("github")
        ids = [a["id"] for a in pal._rows]
        assert "vault:unlock" in ids
        pal._exec_row(ids.index("vault:unlock"))
        assert ("page", 7) in win.calls
        assert ensure_calls == [1]


# ---------------- 回调：_copy_back / _copy_vault_password ----------------

class TestPaletteCopyCallbacks:
    def test_copy_back_text_image_and_no_clip(self, qtbot, monkeypatch):
        win = _FakeWindow()
        fake = _patch_clipboard(monkeypatch, _FakeClip())
        pal, _ = _palette(qtbot, win)

        pal._copy_back("hello")                               # 文本
        assert fake.set_calls == ["hello"]
        assert ("notify", "剪贴板", "已复制到剪贴板") in win.calls

        name = _save_png()
        pal._copy_back(name, kind="image")                    # 真实 PNG → 放回图片
        assert len(fake.images) == 1 and not fake.images[0].isNull()
        assert ("notify", "剪贴板", "已把图片放回剪贴板") in win.calls

        before = len(fake.set_calls)
        pal._copy_back("no-such-image.png", kind="image")     # 空图 → 静默返回
        assert len(fake.set_calls) == before

        monkeypatch.setattr(QApplication.instance(), "clipboard", lambda: None)
        pal._copy_back("x")                                   # 无剪贴板句柄 → no-op
        assert len(fake.set_calls) == before

    def test_copy_back_image_loader_crash_swallowed(self, qtbot, monkeypatch):
        """QImage 构造异常 → except 分支静默返回（不外泄、不动剪贴板）。"""
        win = _FakeWindow()
        fake = _patch_clipboard(monkeypatch, _FakeClip())
        pal, _ = _palette(qtbot, win)

        class _BoomImage:
            def __init__(self, *a, **k):
                raise RuntimeError("image plugin gone")

        with monkeypatch.context() as m:
            m.setattr("PySide6.QtGui.QImage", _BoomImage)
            pal._copy_back("whatever.png", kind="image")
        assert fake.set_calls == [] and not fake.images

    def test_copy_vault_password_cancel_and_locked(self, qtbot, monkeypatch):
        from syskit import vault as vault_mod
        crypto = VaultCrypto()
        crypto.setup("pw")
        vault_mod.init_db()
        sid = vault_mod.add_site("GitHub")
        aid = vault_mod.add_account(sid, "pw-secret", crypto, username="alice")
        crypto.lock()
        copied = []
        win = _FakeWindow()
        win.page_vault = SimpleNamespace(
            crypto=crypto,
            _require_master_verified=lambda: False,           # 用户取消验证
            _copy_sensitive=lambda t, note: copied.append(t))
        pal, _ = _palette(qtbot, win)
        pal._copy_vault_password(aid)
        assert copied == []

        win.page_vault = SimpleNamespace(
            crypto=crypto, _require_master_verified=lambda: True,
            _copy_sensitive=lambda t, note: copied.append(t))
        monkeypatch.setattr(vault_mod, "get_secret",
                            lambda *a, **k: (_ for _ in ()).throw(VaultLockedError("locked")))
        pal._copy_vault_password(aid)                         # 锁定异常 → 静默返回
        assert copied == []

    def test_copy_vault_password_success_delegates_to_page(self, qtbot):
        from syskit import vault as vault_mod
        crypto = VaultCrypto()
        crypto.setup("pw")
        vault_mod.init_db()
        sid = vault_mod.add_site("GitHub")
        aid = vault_mod.add_account(sid, "pw-secret", crypto, username="alice")
        copied = []
        win = _FakeWindow()
        win.page_vault = SimpleNamespace(
            crypto=crypto, _require_master_verified=lambda: True,
            _copy_sensitive=lambda t, note: copied.append((t, note)))
        pal, _ = _palette(qtbot, win)
        pal._copy_vault_password(aid)
        assert copied == [("pw-secret", "密码已复制（不进剪贴板历史）")]
        pal._copy_vault_password(99999)                       # 账号不存在 → 空密码不复制
        assert len(copied) == 1


# ---------------- tray.py 补强 ----------------

class _TrayWin(QWidget):
    def __init__(self):
        super().__init__()
        self.diag = []
        self.repairs = []
        self.quits = 0
        self.toggles = 0

    def run_diagnosis(self, quick=False):
        self.diag.append(quick)

    def run_repair(self, aid, param=None):
        self.repairs.append(aid)

    def quit_app(self):
        self.quits += 1

    def toggle_palette(self):
        self.toggles += 1


class TestTrayGaps:
    @pytest.fixture
    def host(self, qtbot):
        """(托盘, 窗口) 假件；teardown 尽早销毁托盘（防进程收尾原生崩溃）。"""
        win = _TrayWin()
        qtbot.addWidget(win)
        tray = TrayHost(win)
        yield tray, win
        tray.hide()
        tray.deleteLater()
        QApplication.processEvents()

    def _clip_menu(self, tray):
        menu = tray.contextMenu()
        texts = [a.text() for a in menu.actions()]
        assert any("命令面板" in t for t in texts)            # window 有 toggle_palette
        clip_action = next(a for a in menu.actions() if a.menu() is not None)
        return clip_action.menu()

    def test_clip_menu_rebuild_with_text_and_image_entries(self, qtbot, monkeypatch,
                                                           host):
        tray, _win = host
        clip_mod._sensitive.clear()
        hist = clip_mod.ClipboardHistory()
        hist.add("alpha clip entry")
        name = _save_png()
        hist.add("[图片 4×4]", kind="image", data=name)
        hist.flush_if_dirty()                 # 防抖语义：跨实例读取前刷盘
        fake = _patch_clipboard(monkeypatch, _FakeClip())
        clip_menu = self._clip_menu(tray)
        clip_menu.aboutToShow.emit()                          # 惰性重建
        acts = clip_menu.actions()
        assert len(acts) == 2
        assert acts[0].text() == "🖼 [图片 4×4]"             # 最新在前，图片占位含尺寸（补57）
        assert "alpha clip entry" in acts[1].text()
        acts[0].trigger()                                     # 点击图片条目 → 放回剪贴板
        assert len(fake.images) == 1
        acts[1].trigger()                                     # 点击文本条目 → 回填文本
        assert fake.set_calls == ["alpha clip entry"]

    def test_clip_menu_error_falls_back_to_empty_placeholder(self, qtbot, monkeypatch,
                                                             host):
        from syskit import settings as settings_mod
        tray, _win = host

        def boom():
            raise RuntimeError("settings io")

        monkeypatch.setattr(settings_mod, "load", boom)
        clip_menu = self._clip_menu(tray)
        tray._fill_clip_menu(clip_menu)                       # 历史读取异常 → （空）占位
        acts = clip_menu.actions()
        assert len(acts) == 1 and not acts[0].isEnabled()

    def test_copy_back_all_branches(self, qtbot, monkeypatch, host):
        tray, _win = host
        fake = _patch_clipboard(monkeypatch, _FakeClip())

        tray._copy_back("plain text")                         # 文本
        assert fake.set_calls == ["plain text"]

        tray._copy_back("")                                   # 空文本 → no-op
        assert fake.set_calls == ["plain text"]

        name = _save_png()
        tray._copy_back(name, kind="image")                   # 真实 PNG → setImage
        assert len(fake.images) == 1
        tray._copy_back("missing.png", kind="image")          # 空图 → 静默
        assert len(fake.images) == 1
        assert fake.set_calls == ["plain text"]               # 图片分支不得动文本

    def test_tray_copy_back_image_loader_crash_swallowed(self, qtbot, monkeypatch,
                                                         host):
        """QImage 构造异常 → except 分支静默返回（tray._copy_back 兜底）。"""
        tray, _win = host
        fake = _patch_clipboard(monkeypatch, _FakeClip())

        class _BoomImage:
            def __init__(self, *a, **k):
                raise RuntimeError("image plugin gone")

        with monkeypatch.context() as m:
            m.setattr("PySide6.QtGui.QImage", _BoomImage)
            tray._copy_back("whatever.png", kind="image")
        assert fake.set_calls == [] and not fake.images

    def test_clip_menu_aboutToShow_wiring_with_empty_history(self, qtbot, monkeypatch,
                                                             host):
        """空历史：aboutToShow 触发重建后仅灰显占位（触发接线在 aboutToShow）。"""
        tray, _win = host
        clip_mod._sensitive.clear()
        clip_mod.ClipboardHistory().clear()
        clip_menu = self._clip_menu(tray)
        clip_menu.aboutToShow.emit()
        acts = clip_menu.actions()
        assert len(acts) == 1 and not acts[0].isEnabled() and "空" in acts[0].text()
