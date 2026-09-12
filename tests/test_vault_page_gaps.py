"""密码库页面（syskit/ui/vault_page.py）覆盖率补强。

与 tests/test_vault.py 的页面用例互补，只打此前未覆盖的分支：
- 模型边缘：无效索引 / 未知角色的 data()；
- 初始化降级：VaultCrypto.setup 失败 → 「密码库不可用」；vault 读取异常 → 空列表恢复；
- 锁定状态机：自动锁定、手动解锁/锁定按钮、_ensure_unlocked 的取消/错误密码/
  成功三岔路（DPAPI 免密静默解锁已由 test_vault.py 覆盖）；
- 主密码：_set_master / _remove_master 全分支（取消、空密码、校验失败、
  底层异常、ok=False、成功），QInputDialog 全程注入假件（绝无真实模态）；
- 网站流：_SiteDialog 增/改/拒绝/空名/删除 Yes/No 与无选中守卫；
- 账号流：_AccountDialog 增/改守卫（无网站、未解锁、拒绝对话框、空密码、
  vault 异常）、删除 Yes/No、收藏切换、明细行（TOTP/最近使用）；
- 敏感剪贴板：_copy_sensitive 标记 + TTL（含 TTL=0 分支与真实 1s 定时自动
  清空）、_auto_clear 覆盖检测、_clip 无应用兜底、复制/查看/TOTP 的
  VaultLockedError 与无选中守卫 —— 剪贴板一律注入假件，绝不写真实剪贴板。

安全（叠加 conftest 哨兵）：PBKDF2 迭代调小（真实 AESGCM/DPAPI 本地计算）；
QInputDialog / QDialog.exec / 剪贴板全部 mock；持久化走 conftest 临时目录。
"""
import base64
import os

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

import pytest
from PySide6.QtCore import Qt, QModelIndex
from PySide6.QtWidgets import QApplication, QInputDialog

from syskit import clipboard as clip_mod
from syskit import settings as settings_mod
from syskit import vault
from syskit.ui import vault_page as vp_mod
from syskit.ui.vault_page import (_AccountDialog, _SiteDialog, DictListModel,
                                  VaultPage)
from syskit.vault_crypto import VaultCrypto, VaultLockedError

RFC_SECRET = base64.b32encode(b"12345678901234567890").decode("ascii")


# ---------------- 夹具与假件 ----------------

@pytest.fixture(autouse=True)
def fast_kdf(monkeypatch):
    """PBKDF2 迭代调小（真实 AESGCM/DPAPI 本地计算，无网络）。"""
    monkeypatch.setattr("syskit.vault_crypto.KDF_ITER", 1000, raising=False)


@pytest.fixture
def seeded():
    """DPAPI 免密库：GitHub(alice/work+totp、bob) + AWS(carol/个人)。"""
    crypto = VaultCrypto()
    crypto.setup()
    vault.init_db()
    gh = vault.add_site("GitHub", domain="github.com", url="https://github.com",
                        aliases="gh, github", tags="代码")
    awsid = vault.add_site("AWS", domain="aws.amazon.com", tags="云")
    vault.add_account(gh, "pw-alice", crypto, username="alice", label="work",
                      totp_secret=RFC_SECRET, notes="n1")
    vault.add_account(gh, "pw-gh2", crypto, username="bob")
    vault.add_account(awsid, "pw-aws", crypto, username="carol", label="个人")
    return crypto


@pytest.fixture
def seeded_master():
    """主密码库（构造后即锁定）：GitHub(alice+totp、bob) + AWS(carol)。"""
    crypto = VaultCrypto()
    crypto.setup("pw-master")
    vault.init_db()
    gh = vault.add_site("GitHub", domain="github.com", aliases="gh")
    awsid = vault.add_site("AWS")
    vault.add_account(gh, "pw-alice", crypto, username="alice", label="work",
                      totp_secret=RFC_SECRET)
    vault.add_account(gh, "pw-gh2", crypto, username="bob")
    vault.add_account(awsid, "pw-aws", crypto, username="carol")
    crypto.lock()
    return crypto


class _FakeQClipboard:
    def __init__(self):
        self.text_val = ""
        self.cleared = 0

    def text(self):
        return self.text_val

    def setText(self, t):
        self.text_val = t

    def clear(self):
        self.cleared += 1


class _BrokenCrypto:
    """setup 必失败的密码库假件（DPAPI 不可用等灾难场景）。"""

    is_provisioned = False
    is_unlocked = False

    def has_master_password(self):
        return False

    def setup(self, pw=None):
        raise RuntimeError("dpapi down")

    def unlock(self, pw=None):
        return False

    def lock(self):
        pass


def _page(qtbot):
    page = VaultPage()
    qtbot.addWidget(page)
    return page


def _patch_ask(monkeypatch, answers):
    """QInputDialog.getText 假件：answers 逐个消费，元素为 (pwd, ok)。

    返回记录到的标题列表供断言。QInputDialog.getText 未被 conftest 拦截，
    offscreen 下真实现会永久阻塞 —— 凡走解锁/主密码弹窗的用例必须先打此桩。
    """
    asked = []

    def _fake(parent, title, label, mode=None):
        asked.append(title)
        pwd, ok = answers.pop(0)
        return pwd, ok

    monkeypatch.setattr(QInputDialog, "getText", staticmethod(_fake))
    return asked


def _patch_account_dialog(monkeypatch, edit_vals=None, accept=True):
    """_AccountDialog 假工厂：预填字段并劫持 exec，返回已创建列表。"""
    made = []
    real = vp_mod._AccountDialog
    edit_vals = edit_vals or {}

    def factory(parent=None, edit=False):
        d = real(parent, edit=edit)
        for attr, val in edit_vals.items():
            getattr(d, attr).setText(val)
        monkeypatch.setattr(d, "exec", lambda: 1 if accept else 0)
        made.append(d)
        return d

    monkeypatch.setattr(vp_mod, "_AccountDialog", factory)
    return made


def _patch_site_dialog(monkeypatch, name_value="", accept=True):
    """_SiteDialog 假工厂：预填名称并劫持 exec，返回已创建列表。"""
    made = []
    real = vp_mod._SiteDialog

    def factory(parent=None, site=None):
        d = real(parent, site)
        d.edt_name.setText(name_value)
        monkeypatch.setattr(d, "exec", lambda: 1 if accept else 0)
        made.append(d)
        return d

    monkeypatch.setattr(vp_mod, "_SiteDialog", factory)
    return made


def _select(page, lst, needle):
    from PySide6.QtWidgets import QTableWidget
    if isinstance(lst, QTableWidget):
        for r in range(lst.rowCount()):
            if needle in (lst.item(r, 0).text() or ""):
                lst.selectRow(r)
                return r
        raise AssertionError(f"{needle} 不在表格中")
    for i in range(lst.count()):
        if needle in lst.item(i).text():
            lst.setCurrentRow(i)
            return i
    raise AssertionError(f"{needle} 不在列表中")


# ---------------- 模型与状态 ----------------

class TestModelAndState:
    def test_list_model_invalid_index_and_unknown_role(self):
        m = DictListModel(lambda d: d.get("name"))
        m.set_rows([{"id": 1, "name": "A"}])
        assert m.data(QModelIndex()) is None                   # 无效索引 → None
        assert m.data(m.index(0, 0), Qt.CheckStateRole) is None  # 未知角色 → None
        assert m.data(m.index(0, 0)) == "A"
        assert m.data(m.index(0, 0), Qt.UserRole) == 1
        assert m.row_item(9) == {}                             # 越界行 → 空 dict

    def test_provision_failure_marks_unavailable(self, qtbot, monkeypatch):
        monkeypatch.setattr(vp_mod, "VaultCrypto", lambda: _BrokenCrypto())
        page = VaultPage()
        qtbot.addWidget(page)
        assert "不可用" in page.lbl_state.text()
        assert not page.btn_unlock.isHidden()                  # 解锁按钮仍可见
        assert not page.btn_lock.isEnabled()                   # 锁定按钮禁用
        assert page.btn_master.text() == "设主密码…"
        assert page.btn_del_master.isHidden()

    def test_auto_lock_sets_locked_banner(self, qtbot, seeded):
        page = _page(qtbot)
        assert page.crypto.is_unlocked
        page._auto_lock()
        assert not page.crypto.is_unlocked
        assert "已锁定" in page.lbl_state.text()
        assert "已自动锁定" in page.lbl_detail.text()

    def test_refresh_sites_read_error_recovers(self, qtbot, seeded, monkeypatch):
        page = _page(qtbot)
        assert page.lst_sites.count() == 2

        def boom():
            raise RuntimeError("db locked")

        with monkeypatch.context() as m:
            m.setattr(vault, "list_sites", boom)
            page.refresh_sites()                               # 读取异常 → 空列表不崩
            assert page.lst_sites.count() == 0
        page.refresh_sites()
        assert page.lst_sites.count() == 2                     # 恢复后重新拉全

    def test_current_acct_row_edges(self, qtbot, seeded, monkeypatch):
        page = _page(qtbot)
        page.tbl_accts.setCurrentCell(-1, -1)                       # 构造时会自动选中行 0，先清掉
        assert page._current_acct_row() is None                # 未选中账号
        monkeypatch.setattr(page, "_selected_account_id", lambda: 99999)
        assert page._current_acct_row() is None                # 陈旧 id 不在模型中

    def test_account_detail_shows_totp_and_last_used(self, qtbot, seeded):
        page = _page(qtbot)
        _select(page, page.lst_sites, "GitHub")
        page.refresh_accounts()
        page.tbl_accts.setCurrentCell(-1, -1)
        page._on_acct_selected(-1)                             # 无选中 → 恢复默认提示
        assert "单击账号/密码/标签列即复制" in page.lbl_detail.text()

        gh = vault.search_sites("GitHub")[0]
        alice = next(a for a in vault.list_accounts(gh["id"])
                     if a["username"] == "alice")
        vault.mark_used(alice["id"])
        page.refresh_accounts()
        row = _select(page, page.tbl_accts, "alice")
        assert row == 0                                        # 有 last_used 置顶无关，alice 按名序
        text = page.lbl_detail.text()
        assert "含TOTP" in text and "最近使用" in text and "alice" in text


# ---------------- 解锁 / 锁定 / 主密码 ----------------

class TestUnlockAndMasterPassword:
    def _locked_master_page(self, qtbot):
        page = VaultPage()                                     # 读同一 key 文件：有主密码 → 锁定态
        qtbot.addWidget(page)
        assert page.crypto.has_master_password()
        assert not page.crypto.is_unlocked
        assert "已锁定" in page.lbl_state.text()
        return page

    def test_ensure_unlocked_cancel_wrong_and_success(self, qtbot, seeded_master,
                                                      monkeypatch, msgbox_noop):
        page = self._locked_master_page(qtbot)
        _patch_ask(monkeypatch, [("bad", True)])
        assert page._ensure_unlocked() is False                # 错误密码 → 警告
        assert [k for k, _ in msgbox_noop] == ["warning"]
        assert "主密码错误" in msgbox_noop[0][1][2]

        _patch_ask(monkeypatch, [("pw-master", True)])
        assert page._ensure_unlocked() is True
        assert page.crypto.is_unlocked
        assert "主密码已解锁" in page.lbl_state.text()

    def test_unlock_and_lock_buttons(self, qtbot, seeded_master, monkeypatch):
        page = self._locked_master_page(qtbot)
        _patch_ask(monkeypatch, [("pw-master", True)])
        page.btn_unlock.click()
        assert page.crypto.is_unlocked
        page.btn_lock.click()
        assert not page.crypto.is_unlocked
        assert "已锁定" in page.lbl_state.text()

    def test_set_master_all_branches(self, qtbot, seeded, monkeypatch, msgbox_noop):
        # DPAPI 免密页面（解锁已就绪，_set_master 不再索要解锁密码）
        page = _page(qtbot)
        _patch_ask(monkeypatch, [("np", False)])               # 设置弹窗取消 → return
        page._set_master()
        assert not page.crypto.has_master_password()

        _patch_ask(monkeypatch, [("", True)])                  # 空密码 → 警告
        page._set_master()
        assert not page.crypto.has_master_password()
        assert "主密码不能为空" in msgbox_noop[-1][1][2]

        orig = page.crypto.set_master_password
        state = {"fail": True}

        def flaky(pwd):
            if state["fail"]:
                raise RuntimeError("kdf failed")
            return orig(pwd)

        monkeypatch.setattr(page.crypto, "set_master_password", flaky)
        _patch_ask(monkeypatch, [("np", True)])                # 底层异常 → 警告
        page._set_master()
        assert "设置失败" in msgbox_noop[-1][1][2]

        state["fail"] = False
        _patch_ask(monkeypatch, [("np", True)])                # 成功 → 提示 + 状态刷新
        page._set_master()
        assert page.crypto.has_master_password()
        assert page.btn_master.text() == "更改主密码…"
        assert not page.btn_del_master.isHidden()
        assert "主密码已设置" in msgbox_noop[-1][1][2]

        # 解锁失败短路：key 文件此刻已有主密码 np → 新页面为锁定态
        locked_page = VaultPage()
        qtbot.addWidget(locked_page)
        assert locked_page.crypto.has_master_password()
        assert not locked_page.crypto.is_unlocked
        asked = _patch_ask(monkeypatch, [("", False)])         # 解锁弹窗 → 取消
        locked_page._set_master()
        assert asked == ["解锁密码库"]                          # 未走到设置步骤

    def test_remove_master_locked_cancel_blocks(self, qtbot, seeded_master,
                                                monkeypatch):
        """锁定态点「移除主密码」：解锁弹窗取消 → 不进入确认流程。"""
        page = VaultPage()
        qtbot.addWidget(page)
        assert not page.crypto.is_unlocked
        asked = _patch_ask(monkeypatch, [("", False)])         # 解锁弹窗 → 取消
        page._remove_master()
        assert asked == ["解锁密码库"]
        assert page.crypto.has_master_password()

    def test_remove_master_full_paths(self, qtbot, seeded_master, monkeypatch,
                                      msgbox_noop):
        page = VaultPage()
        qtbot.addWidget(page)
        assert page.crypto.unlock("pw-master")
        page._refresh_state()

        _patch_ask(monkeypatch, [("", False)])                 # 确认密码取消 → return
        page._remove_master()
        assert page.crypto.has_master_password()

        _patch_ask(monkeypatch, [("bad", True)])               # 确认密码错误 → 警告
        page._remove_master()
        assert "主密码错误" in msgbox_noop[-1][1][2]
        assert page.crypto.has_master_password()

        orig = page.crypto.remove_master_password
        state = {"mode": "raise"}

        def flaky(pwd):
            if state["mode"] == "raise":
                raise RuntimeError("dpapi fail")
            if state["mode"] == "false":
                return False
            return orig(pwd)

        monkeypatch.setattr(page.crypto, "remove_master_password", flaky)
        _patch_ask(monkeypatch, [("pw-master", True)])         # 底层异常 → 警告
        page._remove_master()
        assert "移除失败" in msgbox_noop[-1][1][2]

        state["mode"] = "false"
        _patch_ask(monkeypatch, [("pw-master", True)])         # ok=False → 无成功提示
        page._remove_master()
        assert page.crypto.has_master_password()
        assert [k for k, _ in msgbox_noop][-1] == "warning"

        state["mode"] = "real"
        _patch_ask(monkeypatch, [("pw-master", True)])         # 成功 → 回 DPAPI
        page._remove_master()
        assert not page.crypto.has_master_password()
        assert page.btn_del_master.isHidden()
        assert "已移除主密码" in msgbox_noop[-1][1][2]


# ---------------- 对话框单元 ----------------

class TestDialogs:
    def test_site_dialog_fields_strip_and_titles(self, qtbot):
        dlg = _SiteDialog(site={"name": "GitHub", "domain": "g.com",
                                "url": "https://g.com", "aliases": "gh",
                                "tags": "code"})
        qtbot.addWidget(dlg)
        assert dlg.windowTitle() == "编辑网站"
        dlg.edt_name.setText("  Renamed  ")
        f = dlg.fields()
        assert f["name"] == "Renamed"                          # fields 去空白
        assert f["domain"] == "g.com" and f["tags"] == "code"

        fresh = _SiteDialog()
        qtbot.addWidget(fresh)
        assert fresh.windowTitle() == "添加网站"
        assert fresh.fields() == {"name": "", "domain": "", "url": "",
                                  "aliases": "", "tags": ""}

    def test_account_dialog_edit_mode(self, qtbot):
        dlg = _AccountDialog(edit=True)
        qtbot.addWidget(dlg)
        assert dlg.windowTitle() == "编辑账号"
        assert dlg.edt_password.placeholderText() == "留空 = 不修改"
        assert set(dlg.fields()) == {"label", "username", "password",
                                     "totp_secret", "notes"}


# ---------------- 网站操作 ----------------

class TestSiteFlows:
    def test_add_site_reject_empty_and_ok(self, qtbot, seeded, monkeypatch,
                                          msgbox_noop):
        page = _page(qtbot)
        before = len(vault.list_sites())

        _patch_site_dialog(monkeypatch, accept=False)
        page._add_site()                                       # 拒绝对话框 → 不入库
        assert len(vault.list_sites()) == before

        _patch_site_dialog(monkeypatch, name_value="", accept=True)
        page._add_site()                                       # 空名称 → 警告
        assert "网站名称不能为空" in msgbox_noop[-1][1][2]
        assert len(vault.list_sites()) == before

        _patch_site_dialog(monkeypatch, name_value="NewSite", accept=True)
        page._add_site()
        assert vault.search_sites("NewSite")[0]["name"] == "NewSite"
        assert page.lst_sites.count() == before + 1            # 列表已刷新

    def test_edit_site_no_selection_reject_and_ok(self, qtbot, seeded, monkeypatch):
        page = _page(qtbot)
        page.lst_sites.setCurrentRow(-1)
        made = _patch_site_dialog(monkeypatch)
        page._edit_site()                                      # 无选中 → 不弹对话框
        assert made == []

        row = _select(page, page.lst_sites, "GitHub")
        sid = page._selected_site_id()
        _patch_site_dialog(monkeypatch, accept=False)
        page._edit_site()                                      # 拒绝 → 不更新
        assert vault.get_site(sid)["name"] == "GitHub"

        _patch_site_dialog(monkeypatch, name_value="GitHub Renamed", accept=True)
        page._edit_site()
        assert vault.get_site(sid)["name"] == "GitHub Renamed"
        assert page.lst_sites.currentRow() == row              # keep_id 保持选中

    def test_del_site_no_selection_yes_and_question_mock(self, qtbot, seeded,
                                                         monkeypatch, msgbox_noop):
        from PySide6.QtWidgets import QMessageBox
        page = _page(qtbot)
        page.lst_sites.setCurrentRow(-1)
        page._del_site()                                       # 无选中 → no-op
        assert len(vault.list_sites()) == 2

        _select(page, page.lst_sites, "AWS")
        page._del_site()                                       # 确认默认 Yes → 删除
        assert len(vault.list_sites()) == 1
        assert page.lst_sites.count() == 1
        assert [k for k, _ in msgbox_noop] == ["question"]

        monkeypatch.setattr(QMessageBox, "question",
                            staticmethod(
                                lambda *a, **k: QMessageBox.StandardButton.No))
        _select(page, page.lst_sites, "GitHub")
        page._del_site()                                       # No → 保留
        assert vault.search_sites("GitHub")


# ---------------- 账号操作 ----------------

class TestAccountFlows:
    def test_add_account_guards(self, qtbot, monkeypatch, msgbox_noop):
        # (a) 无网站可选中 → 仅提示（此页构造时已把 key 文件初始化为 DPAPI）
        page = _page(qtbot)
        made = _patch_account_dialog(monkeypatch)
        page._add_account()
        assert made == []
        assert "先选择" in msgbox_noop[-1][1][2]

        # (b) 锁定 + 主密码取消解锁 → 不弹账号对话框（复用同一 key 文件升主密码）
        vault.add_site("Solo")
        page.crypto.set_master_password("pw-m")                # DPAPI → 主密码模式
        page.crypto.lock()
        page2 = VaultPage()                                    # 有主密码 → 构造即锁定
        qtbot.addWidget(page2)
        assert page2.crypto.has_master_password() and not page2.crypto.is_unlocked
        made2 = _patch_account_dialog(monkeypatch)
        _patch_ask(monkeypatch, [("", False)])
        page2._add_account()
        assert made2 == []                                     # 解锁失败先于对话框

        # (c) 解锁后拒绝对话框 → 不入库
        _patch_ask(monkeypatch, [("pw-m", True)])
        _patch_account_dialog(monkeypatch, accept=False)
        page2._add_account()
        assert vault.list_sites()[0]["n_accounts"] == 0

    def test_add_account_vault_error_warns(self, qtbot, seeded, monkeypatch,
                                           msgbox_noop):
        page = _page(qtbot)
        _select(page, page.lst_sites, "GitHub")

        def boom(*a, **k):
            raise RuntimeError("db full")

        monkeypatch.setattr(vault, "add_account", boom)
        _patch_account_dialog(monkeypatch,
                              edit_vals={"edt_username": "carol",
                                         "edt_password": "pw-c"})
        page._add_account()
        assert "添加失败" in msgbox_noop[-1][1][2]
        assert len(vault.list_accounts(vault.search_sites("GitHub")[0]["id"])) == 2

    def test_edit_account_all_branches(self, qtbot, seeded, monkeypatch):
        page = _page(qtbot)
        _select(page, page.lst_sites, "GitHub")
        page.refresh_accounts()

        page.tbl_accts.setCurrentCell(-1, -1)
        made = _patch_account_dialog(monkeypatch)
        page._edit_account()                                   # 无选中 → 不弹框
        assert made == []

        with monkeypatch.context() as m:
            m.setattr(page, "_selected_account_id", lambda: 99999)
            page._edit_account()                               # 陈旧 id → 不弹框
            assert made == []

        row = _select(page, page.tbl_accts, "alice")
        gh = vault.search_sites("GitHub")[0]
        alice = next(a for a in vault.list_accounts(gh["id"])
                     if a["username"] == "alice")

        _patch_account_dialog(monkeypatch, accept=False)
        page._edit_account()                                   # 拒绝 → 不更新
        assert vault.get_secret(alice["id"], page.crypto, "password") == "pw-alice"

        _patch_account_dialog(monkeypatch,
                              edit_vals={"edt_username": "alice2",
                                         "edt_password": ""},   # 空密码 = 不改密
                              accept=True)
        page._edit_account()
        alice2 = next(a for a in vault.list_accounts(gh["id"])
                      if a["id"] == alice["id"])
        assert alice2["username"] == "alice2"
        assert vault.get_secret(alice["id"], page.crypto, "password") == "pw-alice"
        assert page.tbl_accts.currentRow() == row

    def test_edit_account_locked_cancel_blocks(self, qtbot, seeded_master,
                                               monkeypatch):
        page = VaultPage()
        qtbot.addWidget(page)
        _select(page, page.lst_sites, "GitHub")
        page.refresh_accounts()
        page.tbl_accts.selectRow(0)
        made = _patch_account_dialog(monkeypatch)
        _patch_ask(monkeypatch, [("", False)])                 # 解锁弹窗取消
        page._edit_account()
        assert made == []

    def test_del_account_no_selection_no_and_yes(self, qtbot, seeded, monkeypatch):
        from PySide6.QtWidgets import QMessageBox
        page = _page(qtbot)
        _select(page, page.lst_sites, "GitHub")
        page.refresh_accounts()

        page.tbl_accts.setCurrentCell(-1, -1)
        page._del_account()                                    # 无选中 → no-op
        assert len(vault.list_accounts(vault.search_sites("GitHub")[0]["id"])) == 2

        _select(page, page.tbl_accts, "bob")
        monkeypatch.setattr(QMessageBox, "question",
                            staticmethod(
                                lambda *a, **k: QMessageBox.StandardButton.No))
        page._del_account()                                    # No → 保留
        assert len(vault.list_accounts(vault.search_sites("GitHub")[0]["id"])) == 2

        monkeypatch.setattr(QMessageBox, "question",
                            staticmethod(
                                lambda *a, **k: QMessageBox.StandardButton.Yes))
        page._del_account()                                    # Yes → 删除并刷新
        gh = vault.search_sites("GitHub")[0]
        assert [a["username"] for a in vault.list_accounts(gh["id"])] == ["alice"]
        assert page.tbl_accts.rowCount() == 1

    def test_toggle_fav_paths(self, qtbot, seeded, monkeypatch):
        page = _page(qtbot)
        _select(page, page.lst_sites, "GitHub")
        page.refresh_accounts()

        page.tbl_accts.setCurrentCell(-1, -1)
        page._toggle_fav()                                     # 无选中 → no-op

        row = _select(page, page.tbl_accts, "bob")
        with monkeypatch.context() as m:
            m.setattr(vault, "toggle_favorite", lambda aid: False)
            refreshed = []
            m.setattr(page, "refresh_accounts", lambda: refreshed.append(1))
            page._toggle_fav()                                 # toggle False → 不刷新
            assert refreshed == []

        page._toggle_fav()                                     # 真实切换 → 置顶带星
        gh = vault.search_sites("GitHub")[0]
        assert vault.list_accounts(gh["id"])[0]["username"] == "bob"
        assert page.tbl_accts.item(0, 0).text().startswith("★ ")


# ---------------- 敏感剪贴板 / 复制 / 查看 ----------------

class TestSensitiveClipboard:
    def test_clip_none_without_app_and_empty_text_guard(self, qtbot, seeded,
                                                        monkeypatch):
        page = _page(qtbot)
        assert page._clip() is not None                        # 有应用 → 真实 QClipboard 句柄
        with monkeypatch.context() as m:
            m.setattr(vp_mod.QApplication, "instance",
                      staticmethod(lambda: None))
            assert page._clip() is None                        # 无应用 → None

        fake = _FakeQClipboard()
        monkeypatch.setattr(page, "_clip", lambda: fake)
        page._copy_sensitive("", "note")                       # 空文本 → no-op
        assert fake.text_val == ""

    def test_copy_sensitive_marks_history_and_ttl_zero(self, qtbot, seeded,
                                                       monkeypatch):
        settings_mod.save({"clip_sensitive_ttl": 0})
        page = _page(qtbot)
        fake = _FakeQClipboard()
        monkeypatch.setattr(page, "_clip", lambda: fake)

        page._copy_sensitive("pw-a", "已复制")
        assert fake.text_val == "pw-a"
        assert "后自动清空" not in page.lbl_detail.text()      # TTL=0 无清空提示
        assert clip_mod.ClipboardHistory().add("pw-a") is False  # 已标记敏感不入库

        page._auto_clear("pw-a")                               # 未被覆盖 → 清空
        assert fake.cleared == 1
        fake.text_val = "replaced"
        page._auto_clear("pw-a")                               # 已被覆盖 → 不清
        assert fake.cleared == 1
        clip_mod._sensitive.clear()

    def test_copy_sensitive_ttl_timer_auto_clears(self, qtbot, seeded,
                                                  monkeypatch):
        settings_mod.save({"clip_sensitive_ttl": 1})
        page = _page(qtbot)
        fake = _FakeQClipboard()
        monkeypatch.setattr(page, "_clip", lambda: fake)
        page._copy_sensitive("pw-t", "已复制")
        assert "1s 后自动清空" in page.lbl_detail.text()
        qtbot.wait_until(lambda: fake.cleared >= 1, timeout=6000)  # singleShot 真触发
        clip_mod._sensitive.clear()

    def test_copy_password_vault_locked_error_branch(self, qtbot, seeded,
                                                     monkeypatch):
        page = _page(qtbot)
        _select(page, page.lst_sites, "GitHub")
        page.refresh_accounts()
        _select(page, page.tbl_accts, "alice")
        fake = _FakeQClipboard()
        monkeypatch.setattr(page, "_clip", lambda: fake)

        def boom(*a, **k):
            raise VaultLockedError("locked")

        monkeypatch.setattr(vault, "get_secret", boom)
        page._copy_password()                                  # 锁定异常 → 静默返回
        assert fake.text_val == ""

    def test_copy_username_and_totp_guards(self, qtbot, seeded, monkeypatch):
        page = _page(qtbot)
        fake = _FakeQClipboard()
        monkeypatch.setattr(page, "_clip", lambda: fake)

        page.tbl_accts.setCurrentCell(-1, -1)
        page._copy_username()                                  # 无选中行 → no-op
        assert fake.text_val == ""
        page._copy_password()                                  # 无账号 id → no-op
        page._copy_totp()                                      # 无账号 id → no-op

        _select(page, page.lst_sites, "GitHub")
        page.refresh_accounts()
        _select(page, page.tbl_accts, "alice")

        def boom(*a, **k):
            raise VaultLockedError("locked")

        with monkeypatch.context() as m:
            m.setattr(vault, "get_secret", boom)
            page._copy_totp()                                  # TOTP 解密被锁 → 静默
            assert fake.text_val == ""

        page._copy_username()                                  # 正常复制用户名
        assert fake.text_val == "alice"

    def test_copy_totp_empty_secret_not_copied(self, qtbot, seeded, monkeypatch):
        page = _page(qtbot)
        _select(page, page.lst_sites, "GitHub")
        page.refresh_accounts()
        _select(page, page.tbl_accts, "bob")                   # bob 无 TOTP
        fake = _FakeQClipboard()
        monkeypatch.setattr(page, "_clip", lambda: fake)
        page._copy_totp()                                      # 空码 → 不复制
        assert fake.text_val == ""

    def test_toggle_show_password_all_branches(self, qtbot, seeded, monkeypatch):
        page = _page(qtbot)
        _select(page, page.lst_sites, "GitHub")
        page.refresh_accounts()

        page._showing_pw = True
        page._toggle_show_password()                           # 显示中 → 收起
        assert page.btn_show_pw.text() == "显示"
        assert page._showing_pw is False

        page.tbl_accts.setCurrentCell(-1, -1)
        page._toggle_show_password()                           # 无选中 → 整列显示（新语义）
        assert page._showing_pw is True
        page._hide_password()

        _select(page, page.tbl_accts, "alice")

        def boom(*a, **k):
            raise VaultLockedError("locked")

        with monkeypatch.context() as m:
            m.setattr(vault, "get_secret", boom)
            page._toggle_show_password()                       # 解密被锁 → 单格优雅降级 ****
            assert page._showing_pw is True
            assert page.tbl_accts.item(0, 1).text() == "****"
            page._hide_password()

        page._toggle_show_password()                           # 显示密码（DPAPI 免密：无需验证）
        assert page._showing_pw is True
        assert page.btn_show_pw.text() == "隐藏"
        assert page.tbl_accts.item(0, 1).text() == "pw-alice"  # 密码列整列明文
        page._toggle_show_password()                           # 再点收起 → 整列回 ****
        assert page.btn_show_pw.text() == "显示"
        assert page._showing_pw is False
        assert page.tbl_accts.item(0, 1).text() == "****"
