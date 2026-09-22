"""密码库页：左侧网站（容器）+ 右侧账号（条目）两级模型。

- 搜索两级：网站框匹配名称/域名/别名/标签；账号框只过滤当前网站的 username/label。
- 解锁语义：无主密码（DPAPI）全程免密；有主密码时，验证一次进入信任窗口
  （默认 30 分钟，settings.vault_trust_min），窗口内查看/复制不再重复索要；
  窗口结束时自动重锁。添加/编辑永不弹验证。
- 账号列表为三列表格（账号/密码/标签）：密码列未验证显示 ****，验证后「显示」
  按钮明文 20 秒；单击单元格即复制对应值。
- 密码/TOTP 复制走敏感剪贴板：不进剪贴板历史，TTL 到期自动清空（清前校验未被覆盖）。
"""
import time

from PySide6.QtCore import (QAbstractListModel, QModelIndex, QSortFilterProxyModel, Qt,
                            QTimer)
from PySide6.QtGui import QKeySequence, QShortcut
from PySide6.QtWidgets import (QAbstractItemView, QApplication, QDialog, QFormLayout, QGroupBox,
                               QHBoxLayout, QHeaderView, QLabel, QLineEdit, QListWidget,
                               QListWidgetItem, QMessageBox, QPushButton,
                               QTableWidgetItem, QVBoxLayout, QWidget)

from .. import clipboard as clipboard_mod
from .. import settings, vault
from ..vault_crypto import VaultCrypto, VaultLockedError
from ..logger import LOGGER
from .theme import make_table

FAV = "★ "


class DictListModel(QAbstractListModel):
    """承载 dict 行的轻量列表模型；row_item 供代理模型过滤。"""

    def __init__(self, display, parent=None):
        super().__init__(parent)
        self._rows = []
        self._display = display          # dict -> str

    def set_rows(self, rows):
        self.beginResetModel()
        self._rows = list(rows)
        self.endResetModel()

    def row_item(self, row):
        return self._rows[row] if 0 <= row < len(self._rows) else {}

    def rowCount(self, parent=QModelIndex()):
        return 0 if parent.isValid() else len(self._rows)

    def data(self, index, role=Qt.DisplayRole):
        if not index.isValid():
            return None
        if role == Qt.DisplayRole:
            return self._display(self._rows[index.row()])
        if role == Qt.UserRole:
            return self._rows[index.row()].get("id")
        return None


class DictFilterProxy(QSortFilterProxyModel):
    """大小写不敏感子串过滤，fields(item) 返回参与匹配的文本列表。"""

    def __init__(self, fields, parent=None):
        super().__init__(parent)
        self._kw = ""
        self._fields = fields

    def set_kw(self, kw):
        self._kw = (kw or "").strip().lower()
        self.invalidate()               # 触发 layoutChanged → 列表同步

    def filterAcceptsRow(self, source_row, source_parent):
        if not self._kw:
            return True
        item = self.sourceModel().row_item(source_row)
        return any(self._kw in str(f(item) or "").lower() for f in self._fields)


def _site_display(d):
    return f"{d.get('name', '')}（{d.get('n_accounts', 0)}）"


def _acct_display(d):
    name = d.get("username") or d.get("label") or "（未命名）"
    tail = f"　·　{d['label']}" if d.get("label") and d.get("username") else ""
    return (FAV if d.get("is_favorite") else "") + name + tail


class VaultPage(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        lay = QVBoxLayout(self)
        lay.setContentsMargins(8, 8, 8, 8)

        # ---- 顶部：解锁状态与安全操作 ----
        top = QHBoxLayout()
        self.lbl_state = QLabel()
        top.addWidget(self.lbl_state)
        self.btn_unlock = QPushButton("解锁…")
        self.btn_unlock.clicked.connect(self._unlock_clicked)
        self.btn_lock = QPushButton("锁定")
        self.btn_lock.clicked.connect(self._lock_clicked)
        self.btn_master = QPushButton("设主密码…")
        self.btn_master.clicked.connect(self._set_master)
        self.btn_del_master = QPushButton("移除主密码")
        self.btn_del_master.clicked.connect(self._remove_master)
        self.btn_add_site = QPushButton("＋ 网站")
        self.btn_add_site.setProperty("primary", True)
        self.btn_add_site.clicked.connect(self._add_site)
        top.addStretch(1)
        for b in (self.btn_unlock, self.btn_lock, self.btn_master,
                  self.btn_del_master, self.btn_add_site):
            top.addWidget(b)
        lay.addLayout(top)

        # ---- 左右分栏 ----
        cols = QHBoxLayout()
        lay.addLayout(cols, 1)

        left = QGroupBox("网站")
        llay = QVBoxLayout(left)
        self.edt_site = QLineEdit()
        self.edt_site.setPlaceholderText("搜索网站：名称 / 域名 / 别名 / 标签")
        self.edt_site.setClearButtonEnabled(True)
        self.edt_site.returnPressed.connect(self._select_first_site)   # 回车选中首个结果
        llay.addWidget(self.edt_site)
        self.lst_sites = QListWidget()
        llay.addWidget(self.lst_sites, 1)
        lrow = QHBoxLayout()
        self.btn_edit_site = QPushButton("编辑")
        self.btn_edit_site.clicked.connect(self._edit_site)
        self.btn_del_site = QPushButton("删除")
        self.btn_del_site.setProperty("danger", True)
        self.btn_del_site.clicked.connect(self._del_site)
        lrow.addStretch(1)
        lrow.addWidget(self.btn_edit_site)
        lrow.addWidget(self.btn_del_site)
        llay.addLayout(lrow)
        cols.addWidget(left)

        right = QGroupBox("账号")
        rlay = QVBoxLayout(right)
        self.edt_acct = QLineEdit()
        self.edt_acct.setPlaceholderText("搜索账号：用户名 / 标签（如 work、个人）")
        self.edt_acct.setClearButtonEnabled(True)
        rlay.addWidget(self.edt_acct)
        self.tbl_accts = make_table(["账号", "密码", "标签"], select_rows=True)
        self.tbl_accts.horizontalHeader().setSectionResizeMode(0, QHeaderView.Stretch)
        # 密码/标签列固定合理宽度：曾随内容收缩到极窄，掩码 **** 和按钮提示被挤压
        self.tbl_accts.setColumnWidth(1, 150)
        self.tbl_accts.setColumnWidth(2, 170)
        self.tbl_accts.setSelectionMode(QAbstractItemView.SingleSelection)
        # 单击单元格即复制：账号列→用户名，密码列→密码（需主密码门），标签列→标签
        self.tbl_accts.cellClicked.connect(self._on_acct_cell_clicked)
        rlay.addWidget(self.tbl_accts, 1)
        self.lbl_detail = QLabel("单击账号/密码/标签列即复制；密码列未解锁时显示 ****；"
                                 "「显示」按钮明文 20 秒；Ctrl+Enter 复制用户名，Alt+T 复制 TOTP")
        self.lbl_detail.setObjectName("hint")
        rlay.addWidget(self.lbl_detail)
        rrow = QHBoxLayout()
        self.btn_add_acct = QPushButton("＋ 账号")
        self.btn_add_acct.clicked.connect(self._add_account)
        self.btn_edit_acct = QPushButton("编辑")
        self.btn_edit_acct.clicked.connect(self._edit_account)
        self.btn_del_acct = QPushButton("删除")
        self.btn_del_acct.setProperty("danger", True)
        self.btn_del_acct.clicked.connect(self._del_account)
        self.btn_fav = QPushButton("收藏")
        self.btn_fav.clicked.connect(self._toggle_fav)
        self.btn_copy_user = QPushButton("复制用户名")
        self.btn_copy_user.clicked.connect(self._copy_username)
        self.btn_copy_pw = QPushButton("复制密码")
        self.btn_copy_pw.setProperty("primary", True)
        self.btn_copy_pw.clicked.connect(self._copy_password)
        self.btn_copy_totp = QPushButton("TOTP")
        self.btn_copy_totp.clicked.connect(self._copy_totp)
        self.btn_totp_qr = QPushButton("TOTP 码")
        self.btn_totp_qr.setToolTip("显示当前账号的 TOTP 实时验证码与二维码（手机验证器扫描导入）")
        self.btn_totp_qr.clicked.connect(self._show_totp_dialog)
        self.btn_totp_qr.setVisible(False)   # 仅选中含 TOTP 的账号时可见
        self.btn_show_pw = QPushButton("显示")
        self.btn_show_pw.clicked.connect(self._toggle_show_password)
        rrow.addWidget(self.btn_add_acct)
        rrow.addWidget(self.btn_edit_acct)
        rrow.addWidget(self.btn_del_acct)
        rrow.addWidget(self.btn_fav)
        rrow.addStretch(1)
        rrow.addWidget(self.btn_copy_user)
        rrow.addWidget(self.btn_copy_pw)
        rrow.addWidget(self.btn_copy_totp)
        rrow.addWidget(self.btn_totp_qr)
        rrow.addWidget(self.btn_show_pw)
        rlay.addLayout(rrow)
        cols.addWidget(right, 1)

        # 模型 + 过滤代理
        self.site_model = DictListModel(_site_display)
        self.site_proxy = DictFilterProxy(
            [lambda d: d.get("name"), lambda d: d.get("domain"),
             lambda d: d.get("aliases"), lambda d: d.get("tags"), lambda d: d.get("url")])
        self.site_proxy.setSourceModel(self.site_model)
        self.acct_model = DictListModel(_acct_display)
        self.acct_proxy = DictFilterProxy(
            [lambda d: d.get("username"), lambda d: d.get("label")])
        self.acct_proxy.setSourceModel(self.acct_model)

        self.edt_site.textChanged.connect(self.site_proxy.set_kw)
        self.edt_acct.textChanged.connect(self.acct_proxy.set_kw)
        self.site_proxy.layoutChanged.connect(lambda: self._sync_list(
            self.lst_sites, self.site_proxy, self._selected_site_id()))
        self.acct_proxy.layoutChanged.connect(lambda: self._sync_acct_table(
            self._selected_account_id()))
        self.lst_sites.currentRowChanged.connect(self._on_site_selected)
        self.tbl_accts.currentCellChanged.connect(
            lambda *_: self._on_acct_selected(self.tbl_accts.currentRow()))

        for _ks, _fn in ((QKeySequence("Ctrl+Return"), self._copy_username),
                         (QKeySequence("Alt+T"), self._copy_totp)):
            sc = QShortcut(_ks, self.tbl_accts, activated=_fn)
            sc.setContext(Qt.WidgetWithChildrenShortcut)   # 仅表格聚焦时生效，避免隐藏页误触

        # 自动重锁
        self._autolock = QTimer(self)
        self._autolock.setSingleShot(True)
        self._autolock.timeout.connect(self._auto_lock)
        self._showing_pw = False
        self._master_until = 0.0                 # 主密码信任窗口截止时间（会话内）
        self._show_timer = QTimer(self)          # 明文显示 20 秒后自动隐藏
        self._show_timer.setSingleShot(True)
        self._show_timer.timeout.connect(self._hide_password)

        self.crypto = VaultCrypto()
        self._ensure_provisioned()
        self.refresh_sites()
        self._refresh_state()

    # ---------------- 状态与数据 ----------------

    def _ensure_provisioned(self):
        if not self.crypto.is_provisioned:
            try:
                self.crypto.setup(None)          # 默认 DPAPI 免密模式
            except Exception as e:
                LOGGER.warning("密码库初始化失败: {}", e)
        elif not self.crypto.has_master_password() and not self.crypto.is_unlocked:
            self.crypto.unlock(None)             # DPAPI 自动解锁

    def _refresh_state(self):
        c = self.crypto
        if not c.is_provisioned:
            self.lbl_state.setText("⚠ 密码库不可用")
        elif c.is_unlocked:
            mode = "DPAPI 免密" if not c.has_master_password() else "主密码已解锁"
            self.lbl_state.setText(f"🟢 已解锁（{mode}）")
        else:
            self.lbl_state.setText("🔒 已锁定")
        self.btn_unlock.setVisible(not c.is_unlocked)
        self.btn_lock.setEnabled(c.is_unlocked)
        self.btn_master.setText("更改主密码…" if c.has_master_password() else "设主密码…")
        self.btn_del_master.setVisible(c.has_master_password())
        self._restart_autolock()
        self._hide_password()

    def _restart_autolock(self):
        """调度自动重锁。主密码模式下对齐信任窗口（到期即锁）；DPAPI 模式用设置值。"""
        if not self.crypto.is_unlocked:
            self._autolock.stop()
            return
        if self.crypto.has_master_password() and self._master_until > time.time():
            self._autolock.start(max(1000, int((self._master_until - time.time()) * 1000)))
            return
        minutes = int(settings.load().get("vault_autolock_min", 10))
        if minutes > 0:
            self._autolock.start(minutes * 60_000)
        else:
            self._autolock.stop()

    def _auto_lock(self):
        self.crypto.lock()
        self._master_until = 0.0
        self._refresh_state()
        self.lbl_detail.setText("已自动锁定（超过免验证窗口）")

    def refresh_sites(self, keep_id=None):
        try:
            vault.init_db()
            rows = vault.list_sites()
        except Exception as e:
            LOGGER.warning("密码库读取失败: {}", e)
            rows = []
        self.site_model.set_rows(rows)
        before = self.lst_sites.currentRow()
        self._sync_list(self.lst_sites, self.site_proxy, keep_id)
        # _sync_list 内部 blockSignals：行号未变（或列表清空）时收不到
        # currentRowChanged，账号列表须主动刷新一次（幂等）
        if self.lst_sites.currentRow() == before or not self.lst_sites.count():
            self.refresh_accounts()
        if self.lst_sites.currentRow() < 0 and self.lst_sites.count():
            self.lst_sites.setCurrentRow(0)

    def _sync_list(self, lst: QListWidget, proxy, keep_id=None):
        """把过滤代理的内容同步到 QListWidget。keep_id 尽量保持原选中。"""
        lst.blockSignals(True)
        lst.clear()
        for i in range(proxy.rowCount()):
            idx = proxy.index(i, 0)
            item = QListWidgetItem(idx.data(Qt.DisplayRole))
            item.setData(Qt.UserRole, idx.data(Qt.UserRole))
            lst.addItem(item)
            if keep_id is not None and idx.data(Qt.UserRole) == keep_id:
                lst.setCurrentItem(item)
        lst.blockSignals(False)
        if lst.currentRow() < 0 and lst.count():
            lst.setCurrentRow(0)

    def _selected_site_id(self):
        item = self.lst_sites.currentItem()
        return item.data(Qt.UserRole) if item else None

    def _select_first_site(self):
        """搜索框回车：选中过滤结果的第一项并聚焦账号表（键盘流：搜→回车→选账号）。"""
        if self.lst_sites.count():
            self.lst_sites.setCurrentRow(0)      # currentRowChanged 已联动账号刷新
            self.tbl_accts.setFocus()

    def _selected_account_id(self):
        r = self.tbl_accts.currentRow()
        if r < 0:
            return None
        it = self.tbl_accts.item(r, 0)
        return it.data(Qt.UserRole) if it else None

    def _sync_acct_table(self, keep_id=None):
        """把过滤代理的账号行同步到三列表格（账号/密码/标签）。

        密码列按 _showing_pw 显示明文或 ****；每个单元格带悬停提示与
        UserRole（账号 id），单击即复制对应值。
        明文模式整列一次批量解密（曾每行一次 SQLite 连接 + O(N²) 行查找）。
        """
        t = self.tbl_accts
        rows_by_id = {r.get("id"): r for r in self.acct_model._rows}
        pw_map = {}
        if self._showing_pw:
            try:
                pw_map = vault.get_secrets_bulk(
                    [self.acct_proxy.index(i, 0).data(Qt.UserRole)
                     for i in range(self.acct_proxy.rowCount())],
                    self.crypto, "password")
            except VaultLockedError:
                pw_map = {}              # 自动锁定等场景：整列回 ****，不中断渲染
        t.blockSignals(True)
        t.setRowCount(0)
        for i in range(self.acct_proxy.rowCount()):
            d = self.acct_proxy.index(i, 0).data(Qt.UserRole)
            row = rows_by_id.get(d)
            if row is None:
                continue
            r = t.rowCount()
            t.insertRow(r)
            who = row.get("username") or row.get("label") or "（未命名）"
            c0 = QTableWidgetItem((FAV if row.get("is_favorite") else "") + who)
            c0.setData(Qt.UserRole, row["id"])
            c0.setToolTip("单击复制用户名")
            t.setItem(r, 0, c0)
            c1 = QTableWidgetItem(pw_map.get(row["id"]) or "****")
            c1.setData(Qt.UserRole, row["id"])
            c1.setToolTip(self._pw_tooltip())
            t.setItem(r, 1, c1)
            c2 = QTableWidgetItem(row.get("label") or "")
            c2.setData(Qt.UserRole, row["id"])
            c2.setToolTip("单击复制标签")
            t.setItem(r, 2, c2)
            if keep_id is not None and row["id"] == keep_id:
                t.selectRow(r)
        t.blockSignals(False)
        if t.currentRow() < 0 and t.rowCount():
            t.selectRow(0)

    def _pw_tooltip(self) -> str:
        if self._showing_pw:
            return "单击复制密码"
        return ("**** — 单击复制密码" if not self.crypto.has_master_password()
                else "**** — 单击复制密码（需验证主密码）")

    def _on_acct_cell_clicked(self, r, c):
        """单击单元格即复制：0=用户名（免验证），1=密码（主密码门），2=标签。"""
        if self.tbl_accts.currentRow() != r:
            return
        aid = self._selected_account_id()
        if aid is None:
            return
        if c == 0:
            self._copy_username()
        elif c == 1:
            self._copy_password()
        elif c == 2:
            row = self._current_acct_row()
            clip = self._clip()
            if row and clip and row.get("label"):
                clip.setText(row["label"])
                self.lbl_detail.setText("标签已复制")

    def _current_acct_row(self):
        aid = self._selected_account_id()
        if aid is None:
            return None
        for r in self.acct_model._rows:
            if r.get("id") == aid:
                return r
        return None

    def _on_site_selected(self, row):
        self._hide_password()      # 换站收起明文（不残留跨站明文）
        self.refresh_accounts()

    def refresh_accounts(self):
        sid = self._selected_site_id()
        rows = vault.list_accounts(sid) if sid is not None else []
        self.acct_model.set_rows(rows)
        keep = self._selected_account_id()
        self._sync_acct_table(keep)
        if self.tbl_accts.currentRow() < 0 and self.tbl_accts.rowCount():
            self.tbl_accts.selectRow(0)
        self._on_acct_selected(self.tbl_accts.currentRow())

    def _on_acct_selected(self, row):
        r = self._current_acct_row()
        if r is None:
            self.lbl_detail.setText("单击账号/密码/标签列即复制；密码列未解锁时显示 ****；"
                                    "「显示」按钮明文 20 秒；Ctrl+Enter 复制用户名，Alt+T 复制 TOTP")
            return
        self.btn_totp_qr.setVisible(bool(r.get("has_totp")))
        extra = []
        if r.get("has_totp"):
            extra.append("含TOTP")
        if r.get("last_used_at"):
            extra.append(f"最近使用 {r['last_used_at']}")
        self.lbl_detail.setText(f"{_acct_display(r)}　{'　'.join(extra)}")

    # ---------------- 解锁 / 主密码 ----------------

    def _ask_password(self, title):
        from PySide6.QtWidgets import QInputDialog
        pwd, ok = QInputDialog.getText(self, title, "主密码：",
                                       QLineEdit.Password)
        return pwd if ok else None

    def _ensure_unlocked(self) -> bool:
        if self.crypto.is_unlocked:
            self._restart_autolock()
            return True
        # DPAPI 免密模式（未设主密码）：静默重解锁，不弹主密码框
        if self.crypto.is_provisioned and not self.crypto.has_master_password():
            if self.crypto.unlock(None):
                self._refresh_state()
                return True
            # key 损坏 / DPAPI 不可用（如重装系统换账户）：本库本无主密码，
            # 再弹主密码框只会得到无限「主密码错误」，误导用户删库
            QMessageBox.warning(
                self, "SysKit",
                "密码库解锁失败：key 文件可能损坏，或 Windows 账户凭据变化导致"
                " DPAPI 无法解封（详见日志）。\n如确认放弃历史数据，可删除 "
                "vault.key 后重启重建；否则请保留文件排查。")
            return False
        pwd = self._ask_password("解锁密码库")
        if pwd is None:
            return False
        if not self.crypto.unlock(pwd):
            QMessageBox.warning(self, "SysKit", "主密码错误")
            return False
        self._grant_trust()                     # 与复制路径同语义：验证主密码即授信
        self._refresh_state()
        return True

    def _unlock_clicked(self):
        self._ensure_unlocked()

    def _lock_clicked(self):
        self.crypto.lock()
        self._master_until = 0.0                # 手动锁定：清除免验证窗口
        self._refresh_state()

    def _set_master(self):
        if not self._ensure_unlocked():
            return
        if self.crypto.has_master_password():
            # 信任窗口内不验旧密码即可换主密码 = 离座 30 分钟窗口内接触键盘
            # 半分钟就能接管整个密码库（与移除主密码需验证的安全基线对齐）
            cur = self._ask_password("验证当前主密码")
            if cur is None:
                return
            if not self.crypto.verify_password(cur):
                QMessageBox.warning(self, "SysKit", "当前主密码错误")
                return
        pwd = self._ask_password("设置新主密码" if self.crypto.has_master_password()
                                 else "设置主密码")
        if pwd is None:
            return
        if not pwd:
            QMessageBox.warning(self, "SysKit", "主密码不能为空")
            return
        try:
            self.crypto.set_master_password(pwd)
        except Exception as e:
            QMessageBox.warning(self, "SysKit", f"设置失败: {e}")
            return
        self._refresh_state()
        QMessageBox.information(self, "SysKit", "主密码已设置；此后查看/复制密码每次都需要验证主密码")

    def _remove_master(self):
        if not self._ensure_unlocked():
            return
        pwd = self._ask_password("移除主密码（确认当前主密码）")
        if pwd is None:
            return
        if not self.crypto.verify_password(pwd):
            QMessageBox.warning(self, "SysKit", "主密码错误")
            return
        try:
            ok = self.crypto.remove_master_password(pwd)
        except Exception as e:
            QMessageBox.warning(self, "SysKit", f"移除失败: {e}")
            return
        self._refresh_state()
        if ok:
            QMessageBox.information(self, "SysKit", "已移除主密码，回到 DPAPI 免密模式")

    # ---------------- 网站操作 ----------------

    def _add_site(self):
        dlg = _SiteDialog(self)
        if dlg.exec() != QDialog.Accepted:
            return
        f = dlg.fields()
        if not f["name"]:
            QMessageBox.warning(self, "SysKit", "网站名称不能为空")
            return
        new_id = vault.add_site(**f)
        # 选中新建站点：兜底「选中第 0 行」会让紧接着的「＋ 账号」
        # 静默写进字典序第一个站（用户以为在给新站录账号）
        self.refresh_sites(keep_id=new_id)
        self.refresh_accounts()

    def _edit_site(self):
        sid = self._selected_site_id()
        cur = vault.get_site(sid) if sid is not None else None
        if cur is None:
            return
        dlg = _SiteDialog(self, cur)
        if dlg.exec() != QDialog.Accepted:
            return
        vault.update_site(sid, **dlg.fields())
        self.refresh_sites(keep_id=sid)

    def _del_site(self):
        sid = self._selected_site_id()
        if sid is None:
            return
        ret = QMessageBox.question(self, "删除网站",
                                   "删除该网站及其全部账号？此操作不可撤销。",
                                   QMessageBox.Yes | QMessageBox.No)
        if ret == QMessageBox.Yes:
            vault.delete_site(sid)
            self.refresh_sites()

    # ---------------- 账号操作 ----------------

    def _current_totp_or_blank(self, aid):
        try:
            return vault.get_secret(aid, self.crypto, "totp", touch=False)
        except VaultLockedError:
            return ""

    def _current_notes_or_blank(self, aid):
        try:
            return vault.get_secret(aid, self.crypto, "notes", touch=False)
        except VaultLockedError:
            return ""

    def _add_account(self):
        sid = self._selected_site_id()
        if sid is None:
            QMessageBox.information(self, "SysKit", "先选择（或创建）一个网站")
            return
        if not self._ensure_unlocked():          # 需要加密密钥，锁定时先解锁一次
            return
        self._autolock.stop()
        try:
            dlg = _AccountDialog(self)
            accepted = dlg.exec() == QDialog.Accepted
        finally:
            self._restart_autolock()
        if not accepted:
            return
        f = dlg.fields()
        password = f.pop("password")
        if not password:
            QMessageBox.warning(self, "SysKit", "密码不能为空")
            return
        try:
            vault.add_account(sid, password, crypto=self.crypto, **f)
        except Exception as e:
            QMessageBox.warning(self, "SysKit", f"添加失败: {e}")
            return
        self.refresh_sites()                     # 刷新账号数
        self.refresh_accounts()

    def _edit_account(self):
        aid = self._selected_account_id()
        cur = None
        for r in self.acct_model._rows:
            if r.get("id") == aid:
                cur = r
                break
        if cur is None:
            return
        if not self._ensure_unlocked():
            return
        self._autolock.stop()                # 模态编辑期间不自动锁（防保存时已锁定）
        try:
            dlg = _AccountDialog(self, edit=True, account={
                "username": cur.get("username") or "", "label": cur.get("label") or "",
                "totp_secret": self._current_totp_or_blank(aid), "notes": self._current_notes_or_blank(aid)})
            accepted = dlg.exec() == QDialog.Accepted
            f = dlg.fields() if accepted else None
        finally:
            self._restart_autolock()
        if not accepted:
            return
        f = dlg.fields()
        pw = f.pop("password")
        vault.update_account(aid, self.crypto, label=f["label"],
                             username=f["username"], password=pw or None,
                             totp_secret=f["totp_secret"], notes=f["notes"])
        self.refresh_accounts()

    def _del_account(self):
        aid = self._selected_account_id()
        if aid is None:
            return
        ret = QMessageBox.question(self, "删除账号", "确定删除该账号？",
                                   QMessageBox.Yes | QMessageBox.No)
        if ret == QMessageBox.Yes:
            vault.delete_account(aid)
            self.refresh_sites()
            self.refresh_accounts()

    def _toggle_fav(self):
        aid = self._selected_account_id()
        if aid is not None and vault.toggle_favorite(aid):
            self.refresh_accounts()

    # ---------------- 复制 / 查看（敏感剪贴板） ----------------

    def _clip(self):
        app = QApplication.instance()
        return app.clipboard() if app else None

    def _copy_sensitive(self, text, note):
        clip = self._clip()
        if not clip or not text:
            return
        ttl = int(settings.load().get("clip_sensitive_ttl", 25))
        clipboard_mod.mark_sensitive(text, ttl if ttl > 0 else 3600)
        clip.setText(text)
        clipboard_mod.exclude_os_history()   # 不进 Win+V 历史 / 不上云剪贴板
        self.lbl_detail.setText(note + (f"；{ttl}s 后自动清空" if ttl > 0 else ""))
        self._restart_autolock()
        if ttl > 0:
            QTimer.singleShot(ttl * 1000, lambda: self._auto_clear(text))

    def _auto_clear(self, text):
        clip = self._clip()
        if clip and clip.text() == text:            # 未被新内容覆盖才清
            clip.clear()

    def _require_master_verified(self) -> bool:
        """查看/复制密码的安全门：主密码验证一次进入信任窗口（默认 30 分钟），
        窗口内不再重复索要；DPAPI 免密模式（无主密码可验证）仅要求解锁。"""
        if not self.crypto.has_master_password():
            return self._ensure_unlocked()
        if time.time() < self._master_until:
            if not self.crypto.is_unlocked:      # 窗口内但已被锁定（不应发生）：重新解锁
                self._ensure_unlocked()
            self._restart_autolock()
            return self.crypto.is_unlocked
        if not self.crypto.is_unlocked:
            pwd = self._ask_password("解锁密码库")
            if pwd is None:
                return False
            if not self.crypto.unlock(pwd):
                QMessageBox.warning(self, "SysKit", "主密码错误")
                return False
            self._grant_trust()
            self._refresh_state()
            return True
        pwd = self._ask_password("验证主密码")
        if pwd is None:
            return False
        if not self.crypto.verify_password(pwd):
            QMessageBox.warning(self, "SysKit", "主密码错误")
            return False
        self._grant_trust()
        return True

    def _grant_trust(self):
        """验证通过：开启免验证窗口并让自动重锁对齐窗口截止时间。"""
        minutes = max(1, int(settings.load().get("vault_trust_min", 30)))
        self._master_until = time.time() + minutes * 60
        self._restart_autolock()

    def _copy_password(self):
        aid = self._selected_account_id()
        if aid is None or not self._require_master_verified():
            return
        try:
            pw = vault.get_secret(aid, self.crypto, "password")
        except VaultLockedError:
            return
        self._copy_sensitive(pw, "密码已复制（不进剪贴板历史）")

    def _copy_username(self):
        r = self._current_acct_row()
        if r is None:
            return
        clip = self._clip()
        if clip:
            clip.setText(r.get("username") or "")
            self.lbl_detail.setText("用户名已复制")
            self._restart_autolock()

    def _copy_totp(self):
        aid = self._selected_account_id()
        if aid is None or not self._ensure_unlocked():
            return
        try:
            secret = vault.get_secret(aid, self.crypto, "totp")
        except VaultLockedError:
            return
        code = vault.totp_code(secret)
        if code:
            self._copy_sensitive(code, f"TOTP {code} 已复制")
        elif (secret or "").strip():
            QMessageBox.information(self, "SysKit",
                                    "TOTP 密钥无效（应为 Base32 密钥），未复制")

    def _show_totp_dialog(self):
        """TOTP 二维码 + 实时验证码：otpauth URI 供手机验证器扫描导入。"""
        aid = self._selected_account_id()
        if aid is None or not self._ensure_unlocked():
            return
        try:
            secret = vault.get_secret(aid, self.crypto, "totp", touch=False)
        except VaultLockedError:
            return
        if not vault.totp_code(secret):
            msg = ("该账号未配置 TOTP 密钥" if not (secret or "").strip()
                   else "TOTP 密钥无效（应为 Base32 密钥，如 JBSWY3DPEHPK3PXP）")
            QMessageBox.information(self, "SysKit", msg)
            return
        r = self._current_acct_row() or {}
        site = next((s2.get("name") for s2 in self.site_model._rows
                     if s2.get("id") == self._selected_site_id()), "SysKit")
        who = r.get("username") or r.get("label") or ""
        # otpauth 规范要求 label 段与 issuer 参数百分号编码：
        # 站点名含空格/&/冒号时未编码的 URI 会被验证器解析错乱甚至拒绝导入
        from urllib.parse import quote
        uri = (f"otpauth://totp/{quote(site, safe='')}:{quote(who, safe='')}"
               f"?secret={secret}&issuer={quote(site, safe='')}&digits=6&period=30")
        dlg = QDialog(self)
        dlg.setWindowTitle(f"TOTP — {who or site}")
        v = QVBoxLayout(dlg)
        try:
            import io
            import qrcode
            from PySide6.QtGui import QPixmap
            img = qrcode.make(uri, box_size=6, border=2)
            buf = io.BytesIO()
            img.save(buf, format="PNG")
            pix = QPixmap()
            pix.loadFromData(buf.getvalue(), "PNG")
            lbl_qr = QLabel()
            lbl_qr.setPixmap(pix.scaled(200, 200, Qt.KeepAspectRatio, Qt.SmoothTransformation))
            lbl_qr.setAlignment(Qt.AlignCenter)
            v.addWidget(lbl_qr)
        except Exception as e:
            v.addWidget(QLabel(f"二维码生成失败: {e}"))
        lbl_code = QLabel()
        lbl_code.setAlignment(Qt.AlignCenter)
        lbl_code.setStyleSheet("font-size: 28px; font-weight: bold; letter-spacing: 6px;")
        v.addWidget(lbl_code)
        tip = QLabel("用手机验证器（Google Authenticator 等）扫描导入；"
                     "上方为当前验证码（每秒对时刷新，30 秒一换）。")
        tip.setObjectName("hint")
        tip.setWordWrap(True)
        v.addWidget(tip)

        def _refresh_code():
            lbl_code.setText(vault.totp_code(secret))

        btn = QPushButton("复制验证码")
        btn.setProperty("primary", True)

        def _copy_current():
            # 点击时实时取当前码：闭包捕获打开时旧值会复制到过期验证码
            c = vault.totp_code(secret)
            if c:
                self._copy_sensitive(c, f"TOTP {c} 已复制")

        btn.clicked.connect(_copy_current)
        v.addWidget(btn, 0, Qt.AlignCenter)
        timer = QTimer(dlg)
        timer.timeout.connect(_refresh_code)
        _refresh_code()
        timer.start(1000)
        # 展示的是账号长期种子（高于密码明文的敏感级）：模态期间暂停自动锁，
        # 否则信任窗口到期自动锁后二维码仍驻留屏幕
        self._autolock.stop()
        try:
            dlg.exec()
        finally:
            timer.stop()
            self._restart_autolock()

    def _toggle_show_password(self):
        """一键显示/隐藏：验证主密码（信任窗口内免验证）后，密码列整列明文
        （账号：密码 并排显示），明文 20 秒后自动隐藏回 ****。"""
        if self._showing_pw:
            self._hide_password()
            self.lbl_detail.setText("密码已隐藏（显示 ****）")
            return
        if not self._require_master_verified():
            return
        self._showing_pw = True
        self.btn_show_pw.setText("隐藏")
        self._sync_acct_table(self._selected_account_id())   # 重渲染密码列明文
        self.lbl_detail.setText("密码明文显示 20 秒后自动隐藏；单击密码格复制")
        self._show_timer.start(20_000)

    def _hide_password(self):
        was = self._showing_pw
        self._show_timer.stop()
        self._showing_pw = False
        self.btn_show_pw.setText("显示")
        if was:
            self._sync_acct_table(self._selected_account_id())   # 整列回 ****
            self.lbl_detail.setText("密码已隐藏（显示 ****）")


class _SiteDialog(QDialog):
    def __init__(self, parent=None, site: dict = None):
        super().__init__(parent)
        self.setWindowTitle("编辑网站" if site else "添加网站")
        form = QFormLayout(self)
        self.edt_name = QLineEdit((site or {}).get("name", ""))
        self.edt_domain = QLineEdit((site or {}).get("domain", ""))
        self.edt_url = QLineEdit((site or {}).get("url", ""))
        self.edt_aliases = QLineEdit((site or {}).get("aliases", ""))
        self.edt_aliases.setPlaceholderText("逗号分隔，如 gh, github")
        self.edt_tags = QLineEdit((site or {}).get("tags", ""))
        form.addRow("名称*", self.edt_name)
        form.addRow("域名", self.edt_domain)
        form.addRow("URL", self.edt_url)
        form.addRow("别名", self.edt_aliases)
        form.addRow("标签", self.edt_tags)
        row = QHBoxLayout()
        ok = QPushButton("保存")
        ok.setProperty("primary", True)
        ok.clicked.connect(self.accept)
        cancel = QPushButton("取消")
        cancel.clicked.connect(self.reject)
        row.addStretch(1)
        row.addWidget(ok)
        row.addWidget(cancel)
        form.addRow(row)

    def fields(self):
        return {"name": self.edt_name.text().strip(),
                "domain": self.edt_domain.text().strip(),
                "url": self.edt_url.text().strip(),
                "aliases": self.edt_aliases.text().strip(),
                "tags": self.edt_tags.text().strip()}


class _AccountDialog(QDialog):
    def __init__(self, parent=None, edit: bool = False, account: dict = None):
        super().__init__(parent)
        self.setWindowTitle("编辑账号" if edit else "添加账号")
        form = QFormLayout(self)
        acct = account or {}
        self.edt_label = QLineEdit()
        self.edt_label.setPlaceholderText("工作、个人、小号…")
        self.edt_label.setText(acct.get("label") or "")
        self.edt_username = QLineEdit()
        self.edt_username.setText(acct.get("username") or "")
        self.edt_password = QLineEdit()
        self.edt_password.setEchoMode(QLineEdit.Password)
        self.edt_password.setPlaceholderText("留空 = 不修改" if edit else "")
        self.edt_totp = QLineEdit()
        self.edt_totp.setPlaceholderText("Base32 密钥（可选）")
        self.edt_totp.setEchoMode(QLineEdit.Password)
        self.edt_totp.setText((acct.get("totp_secret") or "") if edit else "")
        self.edt_notes = QLineEdit()
        self.edt_notes.setText(acct.get("notes") or "")
        form.addRow("标签", self.edt_label)
        form.addRow("用户名", self.edt_username)
        form.addRow("密码", self.edt_password)
        form.addRow("TOTP", self.edt_totp)
        form.addRow("备注", self.edt_notes)
        row = QHBoxLayout()
        ok = QPushButton("保存")
        ok.setProperty("primary", True)
        ok.clicked.connect(self.accept)
        cancel = QPushButton("取消")
        cancel.clicked.connect(self.reject)
        row.addStretch(1)
        row.addWidget(ok)
        row.addWidget(cancel)
        form.addRow(row)

    def fields(self):
        return {"label": self.edt_label.text().strip(),
                "username": self.edt_username.text().strip(),
                "password": self.edt_password.text(),
                "totp_secret": self.edt_totp.text().strip(),
                "notes": self.edt_notes.text().strip()}
