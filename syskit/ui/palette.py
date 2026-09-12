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
    """在线程消息队列中拦截 WM_HOTKEY（与 RegisterHotKey(None,…) 配对）。"""

    def __init__(self, on_trigger):
        super().__init__()
        self.on_trigger = on_trigger

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
                    if msg.message == WM_HOTKEY:
                        self.on_trigger()
                        return True, 0
            except Exception:
                pass
        return False, 0


_installed_filter = None      # 模块级持活：过滤器装在 QApplication 上，不能被 GC


def setup_global_hotkey(window, hotkey_id: int = 1) -> bool:
    """读取设置并注册全局热键（须在主线程调用）。成功返回 True。"""
    global _installed_filter
    text = settings.load().get("palette_hotkey", "Ctrl+Alt+K")
    parsed = hotkey.parse_hotkey(text)
    if parsed is None:
        LOGGER.warning("命令面板热键无法解析: {}", text)
        return False
    if not hotkey.register(parsed[0], parsed[1], hotkey_id=hotkey_id):
        LOGGER.warning("全局热键 {} 注册失败（可能被占用），面板可经托盘菜单呼出", text)
        return False
    _installed_filter = HotkeyFilter(window.toggle_palette)
    QApplication.instance().installNativeEventFilter(_installed_filter)
    window._hotkey_filter = _installed_filter
    return True


def teardown_global_hotkey(window, hotkey_id: int = 1):
    """退出/窗口销毁路径：反注册并卸载过滤器。"""
    global _installed_filter
    hotkey.unregister(hotkey_id)
    filt = getattr(window, "_hotkey_filter", None)
    if filt is not None:
        app = QApplication.instance()
        if app is not None:
            app.removeNativeEventFilter(filt)
        if _installed_filter is filt:
            _installed_filter = None
        window._hotkey_filter = None


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
        self.lst.itemActivated.connect(lambda item: self._exec_row(self.lst.row(item)))
        self.lst.itemClicked.connect(lambda item: self._exec_row(self.lst.row(item)))
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
                    label = ("放回图片条目" if kind == "image"
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
