"""密码库页：左侧网站（容器）+ 右侧账号（条目）两级模型。

- 搜索两级：网站框匹配名称/域名/别名/标签；账号框只过滤当前网站的 username/label。
- 解锁语义：无主密码（DPAPI）全程免密；有主密码时冷启动首次解锁，
  会话内添加/编辑不再验证；查看/复制密码在自动重锁后需再次解锁。
- 密码/TOTP 复制走敏感剪贴板：不进剪贴板历史，TTL 到期自动清空（清前校验未被覆盖）。
"""
from PySide6.QtCore import (QAbstractListModel, QModelIndex, QSortFilterProxyModel, Qt,
                            QTimer)
from PySide6.QtGui import QKeySequence, QShortcut
from PySide6.QtWidgets import (QAbstractItemView, QApplication, QDialog, QFormLayout, QGroupBox,
                               QHBoxLayout, QLabel, QLineEdit, QListWidget, QListWidgetItem,
                               QMessageBox, QPushButton, QVBoxLayout, QWidget)

from .. import clipboard as clipboard_mod
from .. import settings, vault
from ..vault_crypto import VaultCrypto, VaultLockedError
from ..logger import LOGGER

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
        self.lst_accts = QListWidget()
        self.lst_accts.setSelectionMode(QAbstractItemView.SingleSelection)
        self.lst_accts.itemDoubleClicked.connect(lambda *_: self._copy_password())
        rlay.addWidget(self.lst_accts, 1)
        self.lbl_detail = QLabel("双击账号复制密码；Ctrl+Enter 复制用户名，Alt+T 复制 TOTP")
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
        self.btn_show_pw = QPushButton("查看")
        self.btn_show_pw.clicked.connect(self._toggle_show_password)
        rrow.addWidget(self.btn_add_acct)
        rrow.addWidget(self.btn_edit_acct)
        rrow.addWidget(self.btn_del_acct)
        rrow.addWidget(self.btn_fav)
        rrow.addStretch(1)
        rrow.addWidget(self.btn_copy_user)
        rrow.addWidget(self.btn_copy_pw)
        rrow.addWidget(self.btn_copy_totp)
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
        self.acct_proxy.layoutChanged.connect(lambda: self._sync_list(
            self.lst_accts, self.acct_proxy, self._selected_account_id()))
        self.lst_sites.currentRowChanged.connect(self._on_site_selected)
        self.lst_accts.currentRowChanged.connect(self._on_acct_selected)

        QShortcut(QKeySequence("Ctrl+Return"), self.lst_accts,
                  activated=self._copy_username)
        QShortcut(QKeySequence("Alt+T"), self.lst_accts, activated=self._copy_totp)

        # 自动重锁
        self._autolock = QTimer(self)
        self._autolock.setSingleShot(True)
        self._autolock.timeout.connect(self._auto_lock)
        self._showing_pw = False

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
        minutes = int(settings.load().get("vault_autolock_min", 10))
        if minutes > 0 and self.crypto.is_unlocked:
            self._autolock.start(minutes * 60_000)
        else:
            self._autolock.stop()

    def _auto_lock(self):
        self.crypto.lock()
        self._refresh_state()
        self.lbl_detail.setText("已自动锁定")

    def refresh_sites(self, keep_id=None):
        try:
            vault.init_db()
            rows = vault.list_sites()
        except Exception as e:
            LOGGER.warning("密码库读取失败: {}", e)
            rows = []
        self.site_model.set_rows(rows)
        self._sync_list(self.lst_sites, self.site_proxy, keep_id)
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

    def _selected_account_id(self):
        item = self.lst_accts.currentItem()
        return item.data(Qt.UserRole) if item else None

    def _current_acct_row(self):
        aid = self._selected_account_id()
        if aid is None:
            return None
        for r in self.acct_model._rows:
            if r.get("id") == aid:
                return r
        return None

    def _on_site_selected(self, row):
        self.refresh_accounts()

    def refresh_accounts(self):
        sid = self._selected_site_id()
        rows = vault.list_accounts(sid) if sid is not None else []
        self.acct_model.set_rows(rows)
        keep = self._selected_account_id()
        self._sync_list(self.lst_accts, self.acct_proxy, keep)
        if self.lst_accts.currentRow() < 0 and self.lst_accts.count():
            self.lst_accts.setCurrentRow(0)
        self._on_acct_selected(self.lst_accts.currentRow())

    def _on_acct_selected(self, row):
        self._hide_password()
        r = self._current_acct_row()
        if r is None:
            self.lbl_detail.setText("双击账号复制密码；Ctrl+Enter 复制用户名，Alt+T 复制 TOTP")
            return
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
        pwd = self._ask_password("解锁密码库")
        if pwd is None:
            return False
        if not self.crypto.unlock(pwd):
            QMessageBox.warning(self, "SysKit", "主密码错误")
            return False
        self._refresh_state()
        return True

    def _unlock_clicked(self):
        self._ensure_unlocked()

    def _lock_clicked(self):
        self.crypto.lock()
        self._refresh_state()

    def _set_master(self):
        if not self._ensure_unlocked():
            return
        pwd = self._ask_password("设置主密码")
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
        QMessageBox.information(self, "SysKit", "主密码已设置；查看/复制密码在重锁后需要解锁")

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
        vault.add_site(**f)
        self.refresh_sites(keep_id=None)

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

    def _add_account(self):
        sid = self._selected_site_id()
        if sid is None:
            QMessageBox.information(self, "SysKit", "先选择（或创建）一个网站")
            return
        if not self._ensure_unlocked():          # 需要加密密钥，锁定时先解锁一次
            return
        dlg = _AccountDialog(self)
        if dlg.exec() != QDialog.Accepted:
            return
        f = dlg.fields()
        if not f.pop("password"):
            QMessageBox.warning(self, "SysKit", "密码不能为空")
            return
        try:
            vault.add_account(sid, crypto=self.crypto, **f)
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
        dlg = _AccountDialog(self, edit=True)
        if dlg.exec() != QDialog.Accepted:
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
        self.lbl_detail.setText(note + (f"；{ttl}s 后自动清空" if ttl > 0 else ""))
        self._restart_autolock()
        if ttl > 0:
            QTimer.singleShot(ttl * 1000, lambda: self._auto_clear(text))

    def _auto_clear(self, text):
        clip = self._clip()
        if clip and clip.text() == text:            # 未被新内容覆盖才清
            clip.clear()

    def _copy_password(self):
        aid = self._selected_account_id()
        if aid is None or not self._ensure_unlocked():
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

    def _toggle_show_password(self):
        if self._showing_pw:
            self._hide_password()
            return
        aid = self._selected_account_id()
        if aid is None or not self._ensure_unlocked():
            return
        try:
            pw = vault.get_secret(aid, self.crypto, "password")
        except VaultLockedError:
            return
        self._showing_pw = True
        self.btn_show_pw.setText("隐藏")
        self.lbl_detail.setText(f"密码：{pw}")

    def _hide_password(self):
        self._showing_pw = False
        self.btn_show_pw.setText("查看")


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
    def __init__(self, parent=None, edit: bool = False):
        super().__init__(parent)
        self.setWindowTitle("编辑账号" if edit else "添加账号")
        form = QFormLayout(self)
        self.edt_label = QLineEdit()
        self.edt_label.setPlaceholderText("工作、个人、小号…")
        self.edt_username = QLineEdit()
        self.edt_password = QLineEdit()
        self.edt_password.setEchoMode(QLineEdit.Password)
        self.edt_password.setPlaceholderText("留空 = 不修改" if edit else "")
        self.edt_totp = QLineEdit()
        self.edt_totp.setPlaceholderText("Base32 密钥（可选）")
        self.edt_notes = QLineEdit()
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
