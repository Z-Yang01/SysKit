"""命令面板：全局热键 / Ctrl+K 呼出，输入即搜，回车执行——所有功能的统一入口。

- 静态动作来自 palette_actions.build_actions（页面/Tab 跳转、快捷修复、自定义命令）。
- 动态条目：剪贴板最近条目、密码库全局搜索（github alice 直达账号）、
  ping <主机> / dns <域名> 语法、URL 直达。
- 全局热键走 RegisterHotKey(None,…)（与 Qt 主线程同队列）+ HotkeyFilter 接收
  WM_HOTKEY；注册失败静默降级（托盘菜单 / Ctrl+K 仍可用）。
"""
from ctypes import wintypes

from PySide6.QtCore import QAbstractNativeEventFilter, Qt
from PySide6.QtWidgets import (QApplication, QDialog, QLabel, QLineEdit, QListWidget,
                               QListWidgetItem, QVBoxLayout)

from .. import clipboard as clipboard_mod
from .. import hotkey, palette_actions, settings, vault, vault_crypto
from ..logger import LOGGER

WM_HOTKEY = 0x0312


def _message_addr(message) -> int:
    """Qt 传 sip.voidptr（支持 int()）；测试传 ctypes 指针（只能按字节读）。"""
    try:
        return int(message)
    except (TypeError, ValueError):
        try:
            raw = bytes(message)[:8]
            return int.from_bytes(raw, "little")
        except Exception:
            return 0


class HotkeyFilter(QAbstractNativeEventFilter):
    """在线程消息队列中拦截 WM_HOTKEY（与 RegisterHotKey(None,…) 配对）。

    expected_ids 声明本过滤器负责的注册 id 集合——Qt 原生过滤器后装先调用，
    多热键共存必须靠 wParam 判归属、不属己放行，否则后注册的会吞掉先注册的。
    """

    def __init__(self, on_trigger, expected_ids):
        super().__init__()
        self.on_trigger = on_trigger
        self.expected_ids = tuple(expected_ids)

    def nativeEventFilter(self, event_type, message):
        try:
            et = bytes(event_type)
        except Exception:
            et = event_type
        if et == b"windows_generic_MSG":
            try:
                addr = _message_addr(message)
                if addr:
                    msg = wintypes.MSG.from_address(addr)
                    if msg.message == WM_HOTKEY and msg.wParam in self.expected_ids:
                        self.on_trigger()
                        return True, 0
            except Exception:
                pass
        return False, 0


_installed_filter = None      # 模块级持活：过滤器装在 QApplication 上，不能被 GC


def _bind_hotkey(text, hotkey_id: int, on_trigger):
    """两个全局热键的共同流程：解析→注册→装过滤器。成功返回过滤器，失败 None。"""
    parsed = hotkey.parse_hotkey(text)
    if parsed is None:
        LOGGER.warning("全局热键无法解析: {}", text)
        return None
    if not hotkey.register(parsed[0], parsed[1], hotkey_id=hotkey_id):
        LOGGER.warning("全局热键 {} 注册失败（可能被占用），功能可经托盘菜单呼出", text)
        return None
    filt = HotkeyFilter(on_trigger, expected_ids=(hotkey_id,))
    QApplication.instance().installNativeEventFilter(filt)
    return filt


def _unbind_hotkey(filt, window, attr: str, hotkey_id: int):
    """teardown 共同流程：反注册 + 卸载过滤器 + 清 window 属性。"""
    hotkey.unregister(hotkey_id)
    if filt is not None:
        app = QApplication.instance()
        if app is not None:
            app.removeNativeEventFilter(filt)
        setattr(window, attr, None)


def setup_global_hotkey(window, hotkey_id: int = 1) -> bool:
    """注册命令面板全局热键（须在主线程调用）。成功返回 True。"""
    global _installed_filter
    text = settings.load().get("palette_hotkey", "Ctrl+Alt+K")
    filt = _bind_hotkey(text, hotkey_id, window.toggle_palette)
    if filt is None:
        return False
    _installed_filter = filt
    window._hotkey_filter = filt
    return True


def teardown_global_hotkey(window, hotkey_id: int = 1):
    """退出/窗口销毁路径：反注册并卸载过滤器。"""
    global _installed_filter
    filt = getattr(window, "_hotkey_filter", None)
    _unbind_hotkey(filt, window, "_hotkey_filter", hotkey_id)
    if _installed_filter is filt:
        _installed_filter = None


# ---------------- 快捷粘贴双热键（面板 id=3 / 快速直贴 id=2） ----------------

_installed_paste_filter = None
_installed_quick_filter = None


def setup_paste_hotkey(window, hotkey_id: int = 3) -> bool:
    """注册「剪贴板快捷粘贴面板」全局热键（设置键 clip_paste_hotkey）。

    与命令面板同款机制；注册/解析失败静默降级（面板仍可经托盘菜单呼出）。
    id=3（Ctrl+Win+V 习惯位）；快速直贴另占 id=2，见 setup_quick_paste_hotkey。
    """
    global _installed_paste_filter
    text = str(settings.load().get("clip_paste_hotkey", "") or "").strip()
    if not text:
        return False                    # 空串 = 用户显式关闭该热键
    filt = _bind_hotkey(text, hotkey_id, window.toggle_clip_paste)
    if filt is None:
        return False
    _installed_paste_filter = filt
    window._paste_hotkey_filter = filt
    return True


def teardown_paste_hotkey(window, hotkey_id: int = 3):
    global _installed_paste_filter
    filt = getattr(window, "_paste_hotkey_filter", None)
    _unbind_hotkey(filt, window, "_paste_hotkey_filter", hotkey_id)
    if _installed_paste_filter is filt:
        _installed_paste_filter = None


def setup_quick_paste_hotkey(window, hotkey_id: int = 2) -> bool:
    """注册「快速粘贴最新一条」全局热键（设置键 clip_quick_hotkey，id=2）。

    按下不弹面板：取历史最新直贴前台（window.quick_paste_latest）。
    注册/解析失败静默降级（同款模式，绝不钩子强拦）。
    """
    global _installed_quick_filter
    text = str(settings.load().get("clip_quick_hotkey", "") or "").strip()
    if not text:
        return False                    # 空串 = 用户显式关闭该热键
    filt = _bind_hotkey(text, hotkey_id, window.quick_paste_latest)
    if filt is None:
        return False
    _installed_quick_filter = filt
    window._quick_hotkey_filter = filt
    return True


def teardown_quick_paste_hotkey(window, hotkey_id: int = 2):
    global _installed_quick_filter
    filt = getattr(window, "_quick_hotkey_filter", None)
    _unbind_hotkey(filt, window, "_quick_hotkey_filter", hotkey_id)
    if _installed_quick_filter is filt:
        _installed_quick_filter = None


class CommandPalette(QDialog):
    """无边框弹出面板。生命周期：popup() → 输入过滤 → Enter 执行 → 自动隐藏。"""

    def __init__(self, window):
        super().__init__(window, Qt.Tool | Qt.FramelessWindowHint)
        self._window = window
        self._actions = []
        self._rows = []
        lay = QVBoxLayout(self)
        lay.setContentsMargins(8, 8, 8, 8)
        self.edt = QLineEdit()
        self.edt.setPlaceholderText(
            "搜索功能 / 页面 / 剪贴板 / 密码库…（ping 主机、dns 域名、粘贴网址直达）")
        self.edt.textChanged.connect(self._refresh)
        self.edt.returnPressed.connect(self._exec_current)
        lay.addWidget(self.edt)
        self.lst = QListWidget()
        # 执行走 itemActivated（双击/Enter）而非单击：单击执行会 hide 面板，
        # 双击的第二击穿透到下层主窗口同坐标控件造成误触
        self.lst.itemActivated.connect(lambda item: self._exec_row(self.lst.row(item)))
        self.lst.itemClicked.connect(lambda item: self.lst.setCurrentRow(self.lst.row(item)))
        lay.addWidget(self.lst, 1)
        self.lbl_hint = QLabel("↑↓ 选择 · Enter 执行 · Esc 关闭")
        self.lbl_hint.setObjectName("hint")
        lay.addWidget(self.lbl_hint)

    # ---------------- 生命周期 ----------------

    def popup(self):
        self._actions = palette_actions.build_actions(self._window)
        self.edt.clear()
        self._refresh()
        scr = QApplication.primaryScreen().availableGeometry()
        self.adjustSize()
        self.resize(min(600, scr.width() - 40),
                    min(430, max(260, self.sizeHint().height())))
        self.move(scr.center() - self.rect().center())
        self.show()
        self.raise_()
        self.activateWindow()
        self.edt.setFocus()

    def toggle(self):
        if self.isVisible():
            self.hide()
        else:
            self.popup()

    def reject(self):           # Esc 关闭
        self.hide()

    # ---------------- 过滤与执行 ----------------

    def _refresh(self):
        q = self.edt.text().strip()
        self._rows = self._dynamic_actions(q) + palette_actions.rank_actions(
            q, self._actions)
        self.lst.clear()
        for a in self._rows:
            label = a["label"]
            if a.get("hint"):
                label = f"{label}　—　{a['hint']}"
            self.lst.addItem(QListWidgetItem(label))
        if self._rows:
            self.lst.setCurrentRow(0)

    def _exec_current(self):
        row = max(0, self.lst.currentRow())
        self._exec_row(row)

    def _exec_row(self, row):
        if not (0 <= row < len(self._rows)):
            return
        action = self._rows[row]
        self.hide()
        try:
            action["callback"]()
        except Exception as e:
            LOGGER.error("命令面板动作 {} 执行失败: {}", action.get("id"), e)

    # ---------------- 动态条目 ----------------

    def _dynamic_actions(self, q: str) -> list:
        out = []
        low = q.lower()
        window = self._window

        # 算式直算：纯数字运算符输入即时求值，回车复制结果
        if palette_actions.looks_like_math(q):
            try:
                val = palette_actions.safe_calc(q)
                text = str(val)

                def _copy_calc(v=text):
                    from PySide6.QtWidgets import QApplication
                    clip = QApplication.clipboard()
                    if clip:
                        clip.setText(v)
                    window.tray.notify("命令面板", f"计算结果已复制: {v}")

                out.append({"id": "dyn:calc", "label": f"= {text}",
                            "hint": "计算 · 回车复制", "callback": _copy_calc})
            except (ValueError, SyntaxError, MemoryError, RecursionError):
                pass                    # 非法算式不显示条目（正常搜索路径照走）

        # ping / dns 语法 → 跳转到工具箱对应 Tab
        if low.startswith("ping ") and q[5:].strip():
            host = q[5:].strip()

            def _go_ping(h=host):
                window.switch_page(5)
                if getattr(window.page_toolbox, "tabs", None):
                    window.page_toolbox.tabs.setCurrentIndex(4)

            out.append({"id": "dyn:ping", "label": f"Ping {host}", "hint": "工具箱",
                        "callback": _go_ping})
        if low.startswith("dns ") and q[4:].strip():
            host = q[4:].strip()

            def _go_dns(h=host):
                window.switch_page(5)
                if getattr(window.page_toolbox, "tabs", None):
                    window.page_toolbox.tabs.setCurrentIndex(3)

            out.append({"id": "dyn:dns", "label": f"DNS 解析 {host}", "hint": "工具箱",
                        "callback": _go_dns})

        # URL 直达
        if low.startswith(("http://", "https://")):
            out.append({"id": "dyn:url", "label": f"打开 {clipboard_mod.ClipboardHistory.preview(q, 50)}",
                        "hint": "浏览器",
                        "callback": (lambda v=q: window.run_palette_command(v))})

        # 剪贴板最近条目（缓存：文件 mtime 未变不重读，避免每键一次 DPAPI 解密）
        if q:
            try:
                import os
                from .. import config as _cfg
                from .. import clipboard as _cb
                _cb.flush_pending()      # 先把防抖窗口内的未落盘复制刷盘，再读磁盘态
                path = os.path.join(_cfg.APPDATA_DIR, "clipboard.json")
                mtime = os.path.getmtime(path) if os.path.exists(path) else 0
                cache = getattr(self, "_clip_cache", None)
                if cache is None or cache[0] != mtime:
                    max_items = int(settings.load().get("clipboard_max", 50))
                    hist = clipboard_mod.ClipboardHistory(max_items=max_items)
                    self._clip_cache = (mtime, hist.items()[:5])
                items = self._clip_cache[1]
                for i, it in enumerate(items):
                    text = it.get("text", "")
                    kind = it.get("kind", "text")
                    # 图片条目用自带占位（含宽高），不再是纯「放回图片条目」
                    label = (f"放回: {text}" if kind == "image"
                             else f"复制: {clipboard_mod.ClipboardHistory.preview(text, 36)}")
                    payload = it.get("data", "") if kind == "image" else text
                    out.append({"id": f"clip:{i}", "label": label, "hint": "剪贴板",
                                "callback": (lambda t=payload, k=kind:
                                             self._copy_back(t, k))})
            except Exception:
                pass

        # 密码库全局搜索（github alice 直达账号）
        if q and len(q) >= 2 and getattr(window, "page_vault", None):
            vp = window.page_vault
            if vp.crypto.is_unlocked:
                try:
                    for hit in vault.global_search(q)[:5]:
                        who = hit.get("username") or hit.get("label") or ""
                        label = f"{hit['site_name']} · {who} — 复制密码"
                        out.append({"id": f"vault:{hit['id']}", "label": label,
                                    "hint": "密码库",
                                    "callback": (lambda aid=hit["id"]:
                                                 self._copy_vault_password(aid))})
                except Exception:
                    pass
            elif vp.crypto.is_provisioned:
                def _unlock():
                    window.switch_page(7)
                    vp._ensure_unlocked()

                out.append({"id": "vault:unlock", "label": "解锁密码库…",
                            "hint": "密码库", "callback": _unlock})
        return out

    # ---------------- 回调 ----------------

    def _copy_back(self, payload, kind="text"):
        clip = QApplication.instance().clipboard() if QApplication.instance() else None
        if not clip:
            return
        if kind == "image":
            from PySide6.QtGui import QImage
            path = clipboard_mod.image_path(payload)
            try:
                img = QImage(path)
                if not img.isNull():
                    clip.setImage(img)
                    self._window.tray.notify("剪贴板", "已把图片放回剪贴板")
                    return
            except Exception:
                pass
            return
        clip.setText(payload)
        self._window.tray.notify("剪贴板", "已复制到剪贴板")

    def _copy_vault_password(self, account_id: int):
        vp = self._window.page_vault
        if not vp._require_master_verified():   # 与 VaultPage 复制路径同一道门
            return
        try:
            pw = vault.get_secret(account_id, vp.crypto, "password")
        except vault_crypto.VaultLockedError:
            return
        if pw:
            vp._copy_sensitive(pw, "密码已复制（不进剪贴板历史）")
