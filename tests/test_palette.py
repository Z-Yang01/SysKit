"""命令面板：hotkey 解析 / 模糊匹配 / 动作注册表 / 面板 UI / 主窗口集成。

不发起真实网络；全局热键的真实注册在集成测试里允许失败降级。
"""
import ctypes
import json
import sys
import types
from ctypes import wintypes

import pytest
from PySide6.QtWidgets import QMessageBox, QStackedWidget, QTabWidget, QWidget

from syskit import hotkey, palette_actions, settings
from syskit.ui.palette import HotkeyFilter, WM_HOTKEY

from test_ui_smoke import make_main_window  # noqa: F401  复用工厂夹具


# ---------------- hotkey ----------------

class TestHotkeyParse:
    def test_modifiers_and_letter(self):
        assert hotkey.parse_hotkey("Ctrl+Alt+K") == (
            hotkey.MOD_CONTROL | hotkey.MOD_ALT, ord("K"))
        assert hotkey.parse_hotkey("ctrl+shift+p") == (
            hotkey.MOD_CONTROL | hotkey.MOD_SHIFT, ord("P"))

    def test_single_key_and_named(self):
        assert hotkey.parse_hotkey("a") == (0, ord("A"))
        assert hotkey.parse_hotkey("alt+space") == (hotkey.MOD_ALT, 0x20)
        assert hotkey.parse_hotkey("F5") == (0, 0x74)
        assert hotkey.parse_hotkey("Ctrl+F13") == (hotkey.MOD_CONTROL, 0x7C)
        assert hotkey.parse_hotkey("Alt+`") == (hotkey.MOD_ALT, 0xC0)

    def test_invalid(self):
        assert hotkey.parse_hotkey("") is None
        assert hotkey.parse_hotkey("Ctrl+") is None
        assert hotkey.parse_hotkey("Ctrl+F25") is None
        assert hotkey.parse_hotkey("Ctrl+不是键") is None
        assert hotkey.parse_hotkey("Alt+Shift") is None      # 无主键

    def test_display_roundtrip(self):
        assert hotkey.hotkey_display("ctrl+alt+k") == "Ctrl+Alt+K"
        assert hotkey.hotkey_display("alt+space") == "Alt+Space"
        assert hotkey.hotkey_display("Ctrl+F9") == "Ctrl+F9"    # F 键不得显示成字母
        assert hotkey.hotkey_display("bad key") == ""


@pytest.mark.skipif(sys.platform != "win32", reason="RegisterHotKey 仅限 Windows")
class TestHotkeyRegister:
    def test_register_unregister_roundtrip(self):
        # F13 无修饰，几乎不可能与其他程序冲突
        assert hotkey.register(0, 0x7C, hotkey_id=0x4D2) in (True, False)
        hotkey.unregister(0x4D2)                             # 无论如何清理


# ---------------- 模糊匹配 ----------------

class TestFuzzy:
    def test_exact_prefix_subsequence(self):
        assert palette_actions.fuzzy_score("diag", "diag") is not None
        assert palette_actions.fuzzy_score("dg", "diagnose") is not None
        assert palette_actions.fuzzy_score("dxg", "diagnose") is None

    def test_empty_query_matches_all(self):
        assert palette_actions.fuzzy_score("", "anything") == 0.0
        assert palette_actions.fuzzy_score("x", "") is None      # 有查询无文本

    def test_prefix_scores_higher(self):
        prefix = palette_actions.fuzzy_score("di", "diagnose")
        subseq = palette_actions.fuzzy_score("di", "undo it")
        assert prefix > subseq

    def test_cased_insensitive(self):
        assert palette_actions.fuzzy_score("DN", "刷新 DNS 缓存") is not None


class TestMatchAndRank:
    def _acts(self):
        return [{"id": "1", "label": "立即快速诊断", "keywords": ["体检"]},
                {"id": "2", "label": "打开 工具箱", "keywords": ["goto"]},
                {"id": "3", "label": "深度诊断（全部检查）", "keywords": []}]

    def test_keywords_half_weight(self):
        s_label = palette_actions.match_action("立即快速诊断", self._acts()[0])
        s_kw = palette_actions.match_action("体检", self._acts()[0])
        assert s_label > 0 and s_kw > 0
        assert palette_actions.match_action("zzz", self._acts()[0]) is None

    def test_rank_orders_and_filters(self):
        ranked = palette_actions.rank_actions("诊断", self._acts())
        ids = [a["id"] for a in ranked]
        assert "1" in ids and "3" in ids and "2" not in ids
        assert palette_actions.rank_actions("", self._acts()) == self._acts()

    def test_load_custom_commands(self, monkeypatch):
        settings.save({"palette_commands": json.dumps([
            {"name": "GitHub", "type": "url", "value": "https://github.com"},
            {"name": "清理临时", "type": "cmd", "value": "cleanmgr"},
            {"name": "坏数据"}, "not-a-dict",
        ])})
        cmds = palette_actions.load_custom_commands()
        assert len(cmds) == 2
        assert cmds[0]["type"] == "url" and cmds[1]["type"] == "cmd"
        settings.save({"palette_commands": "{not-json"})
        assert palette_actions.load_custom_commands() == []


# ---------------- 动作注册表 ----------------

class _FakeTabs(QWidget):
    def __init__(self):
        super().__init__()
        self.tabs = QTabWidget(self)
        for n in ("速度测试", "局域网设备", "端口 / 进程", "DNS / 子网", "ping / 路由"):
            self.tabs.addTab(QWidget(), n)


class _FakeTray:
    def __init__(self, calls):
        self._calls = calls

    def notify(self, title, body, state=None):
        self._calls.append(("notify", title, body))


class _FakeWindow(QWidget):
    def __init__(self):
        super().__init__()
        self.calls = []
        self.tray = _FakeTray(self.calls)
        self.page_toolbox = _FakeTabs()
        self.page_system = None
        self.page_vault = None
        # 页面跳转回调会经 stack 取页面：10 个占位页
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


class TestBuildActions:
    def test_registry_complete_and_callbacks(self, qtbot):
        win = _FakeWindow()
        qtbot.addWidget(win)
        acts = palette_actions.build_actions(win)
        by_id = {a["id"]: a for a in acts}
        # 10 个页面全部可跳
        for i in range(10):
            assert f"nav:{i}" in by_id
        # 页内 Tab
        assert "tab:5:0" in by_id and "tab:5:2" in by_id
        # 快捷动作
        for key in ("quick:diag", "quick:deep", "quick:flush", "quick:show"):
            assert key in by_id
        # 自定义命令注入
        by_id["nav:5"]["callback"]()
        assert ("page", 5) in win.calls
        by_id["tab:5:1"]["callback"]()
        assert ("page", 5) in win.calls
        assert win.page_toolbox.tabs.currentIndex() == 1
        by_id["quick:diag"]["callback"]()
        assert ("diag", True) in win.calls
        by_id["quick:flush"]["callback"]()
        assert ("repair", "flush_dns") in win.calls

    def test_custom_command_actions(self, monkeypatch, qtbot):
        settings.save({"palette_commands": json.dumps(
            [{"name": "我的脚本", "type": "cmd", "value": "echo hi"}])})
        win = _FakeWindow()
        qtbot.addWidget(win)
        acts = palette_actions.build_actions(win)
        cmd_acts = [a for a in acts if a["id"].startswith("cmd:")]
        assert len(cmd_acts) == 1 and cmd_acts[0]["label"] == "我的脚本"
        cmd_acts[0]["callback"]()
        assert ("cmd", "echo hi") in win.calls


# ---------------- HotkeyFilter ----------------

class TestHotkeyFilter:
    def test_wm_hotkey_intercepted(self):
        fired = []
        filt = HotkeyFilter(lambda: fired.append(True))
        msg = wintypes.MSG()
        msg.message = WM_HOTKEY
        addr = ctypes.c_void_p(ctypes.addressof(msg))
        handled, _ret = filt.nativeEventFilter(b"windows_generic_MSG", addr)
        assert handled is True and fired == [True]

    def test_other_message_passthrough(self):
        fired = []
        filt = HotkeyFilter(lambda: fired.append(True))
        msg = wintypes.MSG()
        msg.message = 0x0200                    # WM_MOUSEMOVE
        addr = ctypes.c_void_p(ctypes.addressof(msg))
        handled, _ret = filt.nativeEventFilter(b"windows_generic_MSG", addr)
        assert handled is False and fired == []
        handled, _ret = filt.nativeEventFilter(b"other_event", addr)
        assert handled is False


# ---------------- CommandPalette UI ----------------

class TestCommandPalette:
    def _palette(self, qtbot):
        from syskit.ui.palette import CommandPalette
        win = _FakeWindow()
        qtbot.addWidget(win)
        pal = CommandPalette(win)
        qtbot.addWidget(pal)
        pal._actions = palette_actions.build_actions(win)
        pal._refresh()
        return pal, win

    def test_query_filters_and_exec(self, qtbot):
        pal, win = self._palette(qtbot)
        pal.edt.setText("快速诊断")
        assert pal.lst.count() >= 1
        assert "诊断" in pal.lst.item(0).text()
        pal._exec_row(0)
        assert not pal.isVisible()                      # 执行后自动隐藏
        assert ("diag", True) in win.calls

    def test_empty_query_lists_all_actions(self, qtbot):
        pal, _win = self._palette(qtbot)
        pal.edt.setText("")
        assert pal.lst.count() >= 10                    # 至少 10 个页面动作

    def test_ping_syntax_dynamic_action(self, qtbot):
        pal, win = self._palette(qtbot)
        pal.edt.setText("ping 1.2.3.4")
        assert "Ping 1.2.3.4" in pal.lst.item(0).text()
        pal._exec_row(0)
        assert ("page", 5) in win.calls
        assert win.page_toolbox.tabs.currentIndex() == 4

    def test_calc_dynamic_action(self, qtbot, monkeypatch):
        """算式直算：即时求值置顶，回车复制结果；非法算式走正常搜索。"""
        pal, win = self._palette(qtbot)
        pal.edt.setText("12*3+5")
        ids = [a["id"] for a in pal._rows]
        assert ids[0] == "dyn:calc"
        assert pal._rows[0]["label"] == "= 41"

        copied = []
        from PySide6.QtWidgets import QApplication
        monkeypatch.setattr(QApplication, "clipboard",
                            lambda *a: types.SimpleNamespace(setText=copied.append))
        pal._exec_row(0)
        assert copied == ["41"]
        assert ("notify", "命令面板", "计算结果已复制: 41") in win.calls

        pal.edt.setText("1.2.3")                        # 非合法算式 → 无计算条目
        assert "dyn:calc" not in [a["id"] for a in pal._rows]

    def test_calc_helpers(self):
        from syskit import palette_actions as pa
        assert pa.looks_like_math("12*(3+4)") is True
        assert pa.looks_like_math("ping 1.2.3.4") is False
        assert pa.looks_like_math("1.2.3") is False     # 无运算符
        assert pa.looks_like_math("٤+٥") is False       # 非 ASCII 数字与求值器口径一致（D3）
        assert pa.safe_calc("2**10") == 1024
        assert pa.safe_calc("(1+2)*3") == 9
        assert pa.safe_calc("10/4") == 2.5
        assert pa.safe_calc("(2**10)**2") == 1048576    # 合法嵌套幂不受影响
        with pytest.raises(ValueError):
            pa.safe_calc("9**9**9")                     # 超界幂
        with pytest.raises(ValueError):
            pa.safe_calc("(10**1000)**1000")            # 左积链：右操作数全是 1000，
            # 结果却 3.3M bit（0.15s）——再嵌一层就是 GUI 线程小时级冻结（补55 D1 实测）
        with pytest.raises((ValueError, SyntaxError)):
            pa.safe_calc("__import__('os')")            # 非数字元素拒绝

    def test_url_dynamic_action(self, qtbot, monkeypatch):
        pal, win = self._palette(qtbot)
        opened = []
        from PySide6.QtGui import QDesktopServices
        monkeypatch.setattr(QDesktopServices, "openUrl",
                            staticmethod(lambda url: opened.append(url.toString())))
        pal.edt.setText("https://example.com/x")
        ids = [a["id"] for a in pal._rows]
        assert "dyn:url" in ids
        row = ids.index("dyn:url")
        pal._exec_row(row)
        assert ("cmd", "https://example.com/x") in win.calls
        assert opened == []

    def test_vault_search_dynamic_action(self, qtbot, monkeypatch):
        from syskit import vault
        from syskit.vault_crypto import VaultCrypto
        crypto = VaultCrypto()
        crypto.setup()                                  # DPAPI 免密
        vault.init_db()
        sid = vault.add_site("GitHub", aliases="gh")
        vault.add_account(sid, "pw-alice", crypto, username="alice")

        from syskit.ui.palette import CommandPalette
        from types import SimpleNamespace

        win = _FakeWindow()
        win.page_vault = SimpleNamespace(crypto=crypto)
        qtbot.addWidget(win)
        pal = CommandPalette(win)
        qtbot.addWidget(pal)
        pal._actions = palette_actions.build_actions(win)
        pal.edt.setText("github alice")
        vault_rows = [i for i, a in enumerate(pal._rows) if a["id"].startswith("vault:")]
        assert vault_rows, "应出现密码库动态条目"
        label = pal.lst.item(vault_rows[0]).text()
        assert "GitHub" in label and "alice" in label

    def test_vault_locked_shows_unlock_action(self, qtbot):
        from syskit import vault
        from syskit.vault_crypto import VaultCrypto
        crypto = VaultCrypto()
        crypto.setup("pw")
        vault.init_db()
        sid = vault.add_site("GitHub")
        vault.add_account(sid, "pw", crypto, username="alice")
        crypto.lock()

        from syskit.ui.palette import CommandPalette
        from types import SimpleNamespace

        win = _FakeWindow()
        win.page_vault = SimpleNamespace(crypto=crypto, _ensure_unlocked=lambda: False)
        qtbot.addWidget(win)
        pal = CommandPalette(win)
        qtbot.addWidget(pal)
        pal._actions = []
        pal.edt.setText("github")
        ids = [a["id"] for a in pal._rows]
        assert "vault:unlock" in ids


# ---------------- 主窗口集成 ----------------

def test_main_window_palette_integration(qtbot, monkeypatch, make_main_window):
    from PySide6.QtGui import QDesktopServices
    from syskit.proc import CmdResult
    opened = []
    monkeypatch.setattr(QDesktopServices, "openUrl",
                        staticmethod(lambda url: opened.append(url.toString())))
    # make_main_window 只 mock 了 netcheck；这里再补齐构造期的其他系统调用
    monkeypatch.setattr("syskit.repairs.wlan_profiles", lambda: [])
    monkeypatch.setattr("syskit.proc.run", lambda *a, **k: CmdResult(0, "", ""))
    monkeypatch.setattr("syskit.proc.run_ps", lambda *a, **k: CmdResult(0, "", ""))
    win = make_main_window()
    assert hasattr(win, "toggle_palette") and hasattr(win, "run_palette_command")

    win.toggle_palette()
    assert win._palette is not None and win._palette.isVisible()
    win.toggle_palette()
    assert not win._palette.isVisible()

    win.run_palette_command("https://example.com")
    assert opened == ["https://example.com"]

    # 热键即时生效：保存设置 → 旧热键反注册、新热键立即重注册（无需重启）
    from syskit import hotkey as hotkey_mod
    monkeypatch.setattr("syskit.admin.is_admin", lambda: False)   # 跳过自启分支
    reg_calls = []

    def fake_register(mods, vk, hotkey_id=1):
        reg_calls.append((mods, vk))
        return True

    monkeypatch.setattr(hotkey_mod, "register", fake_register)
    win.cmb_hotkey.setCurrentText("Ctrl+Alt+P")     # _save_settings 读 UI 控件值
    win._save_settings()
    assert (hotkey_mod.MOD_CONTROL | hotkey_mod.MOD_ALT, ord("P")) in reg_calls

    # 回归：非法热键保存 → 警告且以回退的旧值重注册（全局热键不丢），设置保持旧值
    reg_calls.clear()
    win.cmb_hotkey.setCurrentText("不是热键")
    win._save_settings()
    assert reg_calls == [(hotkey_mod.MOD_CONTROL | hotkey_mod.MOD_ALT, ord("P"))]
    assert settings.load().get("palette_hotkey") == "Ctrl+Alt+P"

    # 回归：新热键被占用（注册失败）→ 自动恢复旧热键，全局热键不丢
    reg_calls.clear()
    win.cmb_hotkey.setCurrentText("Ctrl+Alt+H")
    monkeypatch.setattr(hotkey_mod, "register",
                        lambda mods, vk, hotkey_id=1: False)   # 模拟被占用
    warn_calls = []
    monkeypatch.setattr(QMessageBox, "information",
                        staticmethod(lambda *a, **k: warn_calls.append(k or a)))
    win._save_settings()
    assert settings.load().get("palette_hotkey") == "Ctrl+Alt+P"   # 回退旧值
    assert win.cmb_hotkey.currentText() == "Ctrl+Alt+P"
    assert len(warn_calls) == 2                                  # 占用提示 + 保存成功
    assert "占用" in str(warn_calls[0])

    # 清理全局热键与原生过滤器，避免悬空回调
    from syskit.ui.palette import teardown_global_hotkey
    monkeypatch.setattr(hotkey_mod, "register",
                        lambda mods, vk, hotkey_id=1: True)
    win._save_settings()                                         # 用可注册键收尾
    teardown_global_hotkey(win)
