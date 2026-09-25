"""剪贴板快捷粘贴面板：全局热键（默认 Ctrl+Alt+V）呼出，选中条目直接粘进目标窗口。

与命令面板的分工：面板只把内容**复制**回剪贴板（用户再手动 Ctrl+V）；
本组件完成闭环——记录呼出前的前台窗口 → 写回剪贴板 → 焦点还给目标窗口 →
SendInput 模拟一次 Ctrl+V（syskit/paste.py）。任一步失败都降级为
「内容已在剪贴板，手动 Ctrl+V」提示，绝不静默丢内容。

焦点约束：面板 show 后本进程成为前台，SetForegroundWindow 因此被系统放行；
这也是必须在 popup **之前**记录目标 hwnd 的原因（呼出即抢焦点）。
回填内容经 clipboard.suppress_next 抑制，不会被自己的监听器记成新复制。
"""
from PySide6.QtCore import QEvent, Qt, QTimer
from PySide6.QtGui import QCursor, QImage
from PySide6.QtWidgets import (QApplication, QDialog, QLabel, QLineEdit,
                               QListWidget, QListWidgetItem, QVBoxLayout)

from .. import clipboard as clipboard_mod
from .. import hotkey as hotkey_mod
from .. import paste as paste_mod
from .. import settings
from ..logger import LOGGER
from .clip_thumbs import icon_for

_KIND_EMOJI = {"image": "🖼 ", "file": "📁 ", "html": "🌐 "}

# 降级提示按内容类型点名（建议⑤）：图片/文件条目普通文本框根本接不住
# Ctrl+V，含糊一句"已复制"用户无从判断是没送达还是目标窗口不支持
_KIND_WORD = {"image": "图片", "file": "文件", "html": "富文本"}


def _copied(it: dict) -> str:
    return _KIND_WORD.get(it.get("kind", "text"), "") + "已复制"


def hotkey_hint() -> str:
    """当前配置的快捷粘贴热键显示文本（如 'Ctrl+Alt+V'）；禁用/未设/非法返回 ''。

    托盘提示与系统助手页文案按设置动态取键（建议②）——写死的默认键在
    用户改键后会与真实热键脱节。
    """
    text = str(settings.load().get("clip_paste_hotkey", "") or "").strip()
    return hotkey_mod.hotkey_display(text)


class _SearchEdit(QLineEdit):
    """搜索框键盘增强：↑↓ 移动选择、1-9 直选当前可见行（Ditto 同语义）。"""

    def __init__(self, panel):
        super().__init__()
        self._panel = panel

    def keyPressEvent(self, e):
        k = e.key()
        if k == Qt.Key_Up:
            self._panel.move_selection(-1)
            return
        if k == Qt.Key_Down:
            self._panel.move_selection(1)
            return
        if Qt.Key_1 <= k <= Qt.Key_9:
            # NumLock 开启时小键盘数字在 Qt Windows 后端同为 Key_1..9，一并生效
            if self._panel.paste_index(k - Qt.Key_1):
                return
        super().keyPressEvent(e)


class ClipPastePanel(QDialog):
    """无边框弹出列表。生命周期：popup(hwnd) → 搜索/选择 → 回车粘贴 → 自动隐藏。"""

    def __init__(self, window):
        super().__init__(window, Qt.Tool | Qt.FramelessWindowHint)
        self._window = window
        self._rows = []
        self._all = []          # 本次呼出的条目快照：按键过滤走内存，不再读盘
        self._widgets = []      # 与快照同序的行控件：过滤只切显隐，不重建（建议⑤）
        self._target_hwnd = 0
        self._paste_seq = 0     # 连击守卫：新链/重呼出 bump 后旧链各步自行作废
        lay = QVBoxLayout(self)
        lay.setContentsMargins(8, 8, 8, 8)
        self.edt = _SearchEdit(self)
        self.edt.setPlaceholderText(
            "搜索剪贴板历史…　↑↓ 选择 · Enter/1-9 直接粘贴到原窗口 · Esc 取消")
        self.edt.textChanged.connect(self._refresh)
        self.edt.returnPressed.connect(self._exec_current)
        lay.addWidget(self.edt)
        self.lst = QListWidget()
        # 执行按行控件绑定的条目身份走（过滤后行号与 _rows 不再对齐，建议⑤）
        self.lst.itemActivated.connect(self._exec_item)
        self.lst.itemDoubleClicked.connect(self._exec_item)
        lay.addWidget(self.lst, 1)
        self.lbl_hint = QLabel("↑↓ 选择 · Enter 粘贴 · 1-9 直选 · Esc 取消")
        self.lbl_hint.setObjectName("hint")
        lay.addWidget(self.lbl_hint)
        self.resize(560, 380)

    # ---------------- 生命周期 ----------------

    def popup(self, target_hwnd: int = 0):
        """呼出面板。target_hwnd 必须是**显示本面板之前**记录的前台窗口句柄。"""
        self._target_hwnd = int(target_hwnd or 0)
        self._paste_seq += 1        # 作废所有在途粘贴链（呼出后旧链不得再动焦点）
        # 每次呼出读一次盘（含防抖刷盘）；此后按键只做内存过滤——
        # 旧实现每敲一键重建一遍条目（一次 DPAPI 解密 + O(N) 过滤）
        try:
            self._all = self._load_items()
        except Exception:
            LOGGER.warning("快捷粘贴面板读取历史失败", exc_info=True)
            self._all = []
        self._build_rows()          # 一次性填充行控件；此后按键只切显隐
        if self.edt.text():
            self.edt.clear()   # textChanged 信号会顺带 _refresh
        else:
            self._refresh()
        # 居中在鼠标所在屏（呼出时视线跟着鼠标走；旧实现钉死主屏，
        # 副屏呼出会面板"消失"在主屏——Ditto/PowerToys 同习惯）
        scr = (QApplication.screenAt(QCursor.pos())
               or QApplication.primaryScreen()).availableGeometry()
        self.move(scr.center() - self.rect().center())
        self.show()
        self.raise_()
        self.activateWindow()
        self.edt.setFocus()

    def toggle(self, target_hwnd: int = 0):
        if self.isVisible():
            self.hide()
        else:
            self.popup(target_hwnd)

    def event(self, e):
        # 焦点离开即收起（Ditto/Win+V 同语义）：点选其他窗口后面板不再悬浮。
        # 用 WindowDeactivate 而非 changeEvent(ActivationChange)——后者激活/失活
        # 都会来，还得反查 activeWindow 区分；粘贴链先 hide 再还焦，isVisible
        # 守卫避免把自身收尾当成「用户离开」
        if e.type() == QEvent.WindowDeactivate and self.isVisible():
            self.hide()
        return super().event(e)

    # ---------------- 过滤与选择 ----------------

    def _load_items(self) -> list:
        clipboard_mod.flush_pending()   # 先刷盘：防抖窗口内的最新复制可见
        max_items = int(settings.load().get("clipboard_max", 50))
        return clipboard_mod.ClipboardHistory(max_items=max_items).items()

    def _build_rows(self):
        """呼出时一次性填充行控件（≤200 行），行与源条目按下标绑定（UserRole）。

        旧实现每敲一键 clear+重建整个列表（O(击键×行数) 的分配与重排）；
        现在过滤只对既有控件做 setHidden——无闪烁，滚动位置保留。

        UserRole 存 _all 的**下标**而非条目 dict：PySide6 会把 dict 深拷成
        QVariantMap，data() 取回是内容相等的新对象——身份比较（`is`）永假，
        曾致双击/回车激活静默失灵（只剩数字直选可用）。int 过 QVariant 精确。

        图片条目行内直接给缩略图（与系统助手页单源 clip_thumbs）：文件仍在
        才用图标替位；载荷丢失回退 🖼 emoji 占位（与另两种类型同款前缀）。
        """
        self.lst.clear()
        self._widgets = []
        for i, it in enumerate(self._all[:200]):
            label = clipboard_mod.ClipboardHistory.preview(it.get("text", ""), 60)
            prefix = "📌 " if it.get("pinned") else ""
            w = QListWidgetItem(prefix + label)
            icon = (icon_for(it.get("data", ""))
                    if it.get("kind", "text") == "image" else None)
            if icon is not None:
                w.setIcon(icon)
            elif it.get("kind", "text") == "image":
                w.setText("🖼 " + w.text())     # 载荷丢失：回退 emoji 占位
            else:
                w.setText(_KIND_EMOJI.get(it.get("kind", "text"), "") + w.text())
            w.setData(Qt.UserRole, i)
            self.lst.addItem(w)
            self._widgets.append(w)

    def _refresh(self):
        """按键过滤：只切行显隐；当前选择仍可见时不打扰（不强制跳回首行）。"""
        q = self.edt.text().strip().lower()
        self._rows = []
        for i, w in enumerate(self._widgets):
            it = self._all[i]               # _widgets 与 _all[:200] 按构造同序
            hit = not q or q in str(it.get("text", "")).lower()
            w.setHidden(not hit)
            if hit:
                self._rows.append(it)       # 与 _all 共享同一批 dict（身份可比较）
        cur = self.lst.currentItem()
        if cur is not None and not cur.isHidden():
            return
        for w in self._widgets:
            if not w.isHidden():
                self.lst.setCurrentItem(w)
                self.lst.scrollToItem(w)
                break

    def move_selection(self, delta: int):
        n = self.lst.count()
        if n == 0:
            return
        step = 1 if delta > 0 else -1
        row = self.lst.currentRow()
        row = (0 if step > 0 else n - 1) if row < 0 else row + step
        while 0 <= row < n and self.lst.item(row).isHidden():
            row += step                 # 过滤后跳过隐藏行（↑↓ 只在可见行间移动）
        if not (0 <= row < n):
            return                      # 已在可见端点：保持不动
        self.lst.setCurrentRow(row)
        self.lst.scrollToItem(self.lst.item(row))

    def paste_index(self, i: int) -> bool:
        """数字键直选第 i 行（0 起）；越界返回 False 让按键回落到输入框。"""
        if 0 <= i < len(self._rows):
            self._exec_row(i)
            return True
        return False

    # ---------------- 执行 ----------------

    def _exec_current(self):
        w = self.lst.currentItem()
        if w is not None and not w.isHidden():
            self._exec_item(w)          # 过滤无命中时 currentItem 可能停在隐藏行

    def _exec_item(self, w):
        """按行控件绑定的条目身份执行：过滤态下行号 ≠ 可见序，不能直接当索引。

        行控件 UserRole 存 _all 下标，_rows 与 _all 共享同一批 dict 对象，
        身份（is）比较成立；换回 dict 存取则 PySide6 经 QVariantMap 深拷，
        is 永假、激活失灵（见 _build_rows 注释）。
        """
        idx = w.data(Qt.UserRole)
        if not isinstance(idx, int) or not (0 <= idx < len(self._all)):
            return
        it = self._all[idx]
        for i, row in enumerate(self._rows):
            if row is it:
                self._exec_row(i)
                return

    def _exec_row(self, row):
        if not (0 <= row < len(self._rows)):
            return
        it = self._rows[row]
        hwnd = self._target_hwnd
        self.hide()
        # 鼠标双击会先后发 itemActivated + itemDoubleClicked（快捷键同理连敲）：
        # 每次执行 bump seq，旧链在任一延时步骤里发现 token 不匹配即放弃，
        # 保证连击只粘最后一击选中的条目，且不会 double SendInput。
        self._paste_seq += 1
        seq = self._paste_seq
        # 面板隐藏后 Windows 会把焦点还给呼出前的窗口；稍等一拍再写剪贴板/还焦/按键
        QTimer.singleShot(120, lambda: self._do_paste(it, hwnd, seq))

    def _write_back(self, it: dict) -> bool:
        """条目内容写回系统剪贴板；文本类回填同时抑制监听器重复记录。"""
        clip = QApplication.instance().clipboard()
        kind = it.get("kind", "text")
        text = it.get("text", "")
        if kind == "image":
            path = clipboard_mod.image_path(it.get("data", ""))
            img = QImage(path) if path else QImage()
            if img.isNull():
                return False
            clip.setImage(img)
            clipboard_mod.exclude_os_history()
            return True
        clip.setText(text)
        clipboard_mod.suppress_next(text)
        # 自动回填不是"用户新复制"：自家历史用 suppress_next，系统侧
        # （Win+V/云剪贴板）用排除格式——密码库/WLAN 复制同口径
        clipboard_mod.exclude_os_history()
        return True

    def _do_paste(self, it: dict, hwnd: int, seq: int):
        if seq != self._paste_seq:
            return                      # 已被更新的粘贴/重新呼出取代
        if not self._write_back(it):
            self._notify("粘贴失败：内容不可用（图片文件可能已被清理）")
            return
        if not (hwnd and paste_mod.focus_window(hwnd)):
            self._notify(f"{_copied(it)}；切换窗口后按 Ctrl+V 粘贴")
            return
        # 焦点切换的 WM_ACTIVATE 要经本进程事件循环派发后才能注按键：
        # 同步 sleep 会卡住派发、首拍 Ctrl+V 可能被丢（真机冒烟实测）
        QTimer.singleShot(150, lambda: self._send_paste_keystroke(it, seq))

    def _send_paste_keystroke(self, it: dict, seq: int):
        if seq != self._paste_seq:
            return                      # 等待激活的 150ms 里用户已另起一次粘贴
        if paste_mod.send_ctrl_v():
            self._notify("已粘贴到原窗口")
        else:
            self._notify(f"{_copied(it)}；自动粘贴未送达，请按 Ctrl+V")

    def _notify(self, msg: str):
        tray = getattr(self._window, "tray", None)
        if tray is not None:
            try:
                tray.notify("快捷粘贴", msg)
                return
            except Exception:
                pass
        LOGGER.info("快捷粘贴: {}", msg)
