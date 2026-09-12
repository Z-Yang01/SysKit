"""syskit/vault_crypto.py + syskit/vault.py + 密码库页单元测试。

真实 AESGCM / DPAPI 本地计算（无网络）；PBKDF2 迭代调小保证速度；
持久化路径由 conftest 重定向到临时目录，绝不触碰真实密码库。
"""
import base64
import sys
import time

import pytest

from syskit import vault
from syskit.vault_crypto import VaultCrypto, VaultLockedError

RFC_SECRET = base64.b32encode(b"12345678901234567890").decode("ascii")


@pytest.fixture(autouse=True)
def fast_kdf(monkeypatch):
    monkeypatch.setattr("syskit.vault_crypto.KDF_ITER", 1000, raising=False)


@pytest.fixture
def crypto(tmp_path):
    c = VaultCrypto(key_file=str(tmp_path / "vault.key"))
    yield c


@pytest.fixture
def seeded():
    """已初始化的库（默认路径，页面实例共享同一密钥）：
    GitHub(alice/work + bob)、AWS(bob/个人)。"""
    crypto = VaultCrypto()          # conftest 已重定向 APPDATA_DIR
    crypto.setup()                  # DPAPI 免密模式
    vault.init_db()
    gh = vault.add_site("GitHub", domain="github.com", url="https://github.com",
                        aliases="gh, github", tags="代码")
    vault.add_site("AWS", domain="aws.amazon.com", tags="云")
    vault.add_account(gh, "pw-alice", crypto, username="alice", label="work",
                      totp_secret=RFC_SECRET, notes="n1")
    vault.add_account(gh, "pw-gh2", crypto, username="bob")
    vault.add_account(vault.search_sites("AWS")[0]["id"], "pw-bob", crypto,
                      username="bob", label="个人")
    return crypto


# ---------------- VaultCrypto ----------------

class TestVaultCrypto:
    def test_setup_dpapi_auto_unlock_and_roundtrip(self, crypto):
        assert not crypto.is_provisioned
        crypto.setup()
        assert crypto.is_provisioned and crypto.is_unlocked
        assert not crypto.has_master_password()
        blob1 = crypto.encrypt(b"hello")
        blob2 = crypto.encrypt(b"hello")
        assert blob1 != blob2                       # 随机 nonce
        assert crypto.decrypt(blob1) == b"hello"

    def test_lock_gates_seal(self, crypto):
        crypto.setup()
        blob = crypto.encrypt(b"x")
        crypto.lock()
        assert not crypto.is_unlocked
        with pytest.raises(VaultLockedError):
            crypto.encrypt(b"y")
        with pytest.raises(VaultLockedError):
            crypto.decrypt(blob)

    @pytest.mark.skipif(sys.platform != "win32", reason="DPAPI 仅限 Windows")
    def test_dpapi_roundtrip(self):
        from syskit.vault_crypto import _dpapi_protect, _dpapi_unprotect
        blob = _dpapi_protect(b"dpapi-payload")
        assert blob != b"dpapi-payload"
        assert _dpapi_unprotect(blob) == b"dpapi-payload"

    def test_password_flow(self, crypto):
        crypto.setup("master-pw")
        assert crypto.has_master_password() and crypto.is_unlocked
        blob = crypto.encrypt(b"secret")
        crypto.lock()
        assert crypto.unlock("wrong") is False
        assert not crypto.is_unlocked
        with pytest.raises(VaultLockedError):
            crypto.decrypt(blob)
        assert crypto.unlock("master-pw") is True
        assert crypto.decrypt(blob) == b"secret"

    def test_set_master_password_requires_unlock(self, crypto):
        crypto.setup()                              # DPAPI 模式
        crypto.lock()
        with pytest.raises(VaultLockedError):
            crypto.set_master_password("np")
        assert crypto.unlock(None) is True          # DPAPI 免密解锁
        crypto.set_master_password("np")
        assert crypto.has_master_password()
        crypto.lock()
        assert crypto.unlock(None) is False         # 不再免密
        assert crypto.unlock("np") is True

    def test_verify_and_remove_master_password(self, crypto):
        crypto.setup("pw9")
        assert crypto.verify_password("bad") is False
        assert crypto.verify_password("pw9") is True
        assert crypto.remove_master_password("bad") is False
        assert crypto.has_master_password()
        assert crypto.remove_master_password("pw9") is True
        assert not crypto.has_master_password()
        crypto.lock()
        assert crypto.unlock(None) is True          # 回到 DPAPI 免密

    def test_setup_twice_rejected(self, crypto):
        crypto.setup()
        with pytest.raises(RuntimeError):
            crypto.setup()

    def test_keyfile_persistence(self, tmp_path):
        kf = str(tmp_path / "k.json")
        c1 = VaultCrypto(key_file=kf)
        c1.setup("pw")
        blob = c1.encrypt(b"persist")
        c2 = VaultCrypto(key_file=kf)
        assert c2.is_provisioned
        assert c2.unlock("pw")
        assert c2.decrypt(blob) == b"persist"


# ---------------- vault 存储 ----------------

class TestSites:
    def test_add_search_update_delete(self, seeded):
        rows = vault.search_sites("")
        assert len(rows) == 2
        assert rows[0]["name"] == "AWS"             # 名称序
        assert vault.search_sites("git")[0]["name"] == "GitHub"
        assert vault.search_sites("GH")[0]["name"] == "GitHub"      # 别名
        assert vault.search_sites("代码")[0]["name"] == "GitHub"     # 标签
        assert vault.search_sites("不存在的站") == []
        gh = vault.search_sites("GitHub")[0]
        vault.update_site(gh["id"], tags="代码,开源")
        assert vault.search_sites("开源")[0]["name"] == "GitHub"
        vault.delete_site(gh["id"])
        assert vault.search_sites("GitHub") == []

    def test_find_by_domain(self, seeded):
        assert vault.find_by_domain("github.com")["name"] == "GitHub"
        assert vault.find_by_domain("GITHUB.COM")["name"] == "GitHub"
        assert vault.find_by_domain("gh")["name"] == "GitHub"       # 别名兜底
        assert vault.find_by_domain("unknown.io") is None
        assert vault.find_by_domain("") is None

    def test_n_accounts_count(self, seeded):
        assert vault.search_sites("GitHub")[0]["n_accounts"] == 2
        assert vault.search_sites("AWS")[0]["n_accounts"] == 1


class TestAccounts:
    def test_add_list_secret_roundtrip(self, seeded):
        gh = vault.search_sites("GitHub")[0]
        accts = vault.list_accounts(gh["id"])
        assert len(accts) == 2
        alice = next(a for a in accts if a["username"] == "alice")
        assert alice["has_totp"] and alice["label"] == "work"
        # 元数据里绝不含密文字段
        assert "password_enc" not in alice
        assert vault.get_secret(alice["id"], seeded, "password") == "pw-alice"
        assert vault.get_secret(alice["id"], seeded, "totp") == RFC_SECRET
        assert vault.get_secret(alice["id"], seeded, "notes") == "n1"
        assert vault.get_secret(99999, seeded, "password") == ""

    def test_list_accounts_filter(self, seeded):
        gh = vault.search_sites("GitHub")[0]
        assert [a["username"] for a in vault.list_accounts(gh["id"], "bob")] == ["bob"]
        assert [a["username"] for a in vault.list_accounts(gh["id"], "WORK")] == ["alice"]

    def test_update_account_password(self, seeded):
        gh = vault.search_sites("GitHub")[0]
        alice = next(a for a in vault.list_accounts(gh["id"]) if a["username"] == "alice")
        vault.update_account(alice["id"], seeded, password="new-pw", label="工作")
        assert vault.get_secret(alice["id"], seeded, "password") == "new-pw"
        assert next(a for a in vault.list_accounts(gh["id"])
                    if a["id"] == alice["id"])["label"] == "工作"

    def test_favorite_and_mark_used(self, seeded):
        gh = vault.search_sites("GitHub")[0]
        bob = next(a for a in vault.list_accounts(gh["id"]) if a["username"] == "bob")
        assert vault.toggle_favorite(bob["id"])
        assert vault.list_accounts(gh["id"])[0]["username"] == "bob"    # 收藏置顶
        assert vault.toggle_favorite(bob["id"])
        vault.mark_used(bob["id"])
        row = next(a for a in vault.list_accounts(gh["id"]) if a["id"] == bob["id"])
        assert row["last_used_at"] is not None

    def test_delete_account_and_cascade(self, seeded):
        gh = vault.search_sites("GitHub")[0]
        alice = next(a for a in vault.list_accounts(gh["id"]) if a["username"] == "alice")
        vault.delete_account(alice["id"])
        assert [a["username"] for a in vault.list_accounts(gh["id"])] == ["bob"]
        vault.delete_site(gh["id"])                 # 级联删除账号
        assert vault.list_accounts(gh["id"]) == []
        assert vault.global_search("alice") == []

    def test_get_secret_locked(self, seeded):
        seeded.lock()
        gh = vault.search_sites("GitHub")[0]
        with pytest.raises(VaultLockedError):
            vault.get_secret(vault.list_accounts(gh["id"])[0]["id"], seeded, "password")

    def test_add_account_requires_password(self, seeded):
        gh = vault.search_sites("GitHub")[0]
        with pytest.raises(ValueError):
            vault.add_account(gh, "", seeded)


class TestGlobalSearch:
    def test_two_tokens_hit_account_level(self, seeded):
        hits = vault.global_search("github alice")
        assert len(hits) == 1
        assert hits[0]["username"] == "alice"
        assert hits[0]["site_name"] == "GitHub"
        assert "password_enc" not in hits[0]

    def test_partial_tokens_and_case(self, seeded):
        assert len(vault.global_search("git ali")) == 1
        assert len(vault.global_search("BOB")) == 2             # gh.bob + aws.bob
        assert len(vault.global_search("个人 bob")) == 1

    def test_label_matches(self, seeded):
        assert len(vault.global_search("work")) == 1

    def test_empty_query(self, seeded):
        assert vault.global_search("") == []
        assert vault.global_search("   ") == []


class TestTotp:
    def test_rfc6238_vectors(self):
        # RFC 6238 附录 B（SHA1，8 位）
        assert vault.totp_code(RFC_SECRET, at=59, digits=8) == "94287082"
        assert vault.totp_code(RFC_SECRET, at=1111111109, digits=8) == "07081804"
        assert vault.totp_code(RFC_SECRET, at=1234567890, digits=8) == "89005924"
        assert vault.totp_code(RFC_SECRET, at=2000000000, digits=8) == "69279037"

    def test_six_digits(self):
        assert vault.totp_code(RFC_SECRET, at=59) == "287082"

    def test_secret_normalization(self):
        assert vault.totp_code(RFC_SECRET.lower(), at=59, digits=8) == "94287082"
        assert vault.totp_code("jbsw y3dp ehpk 3pxp", at=59) != ""

    def test_empty_secret(self):
        assert vault.totp_code("") == ""
        assert vault.totp_code(None) == ""


# ---------------- VaultPage UI ----------------

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


class TestVaultPage:
    def _page(self, qtbot):
        from syskit.ui.vault_page import VaultPage
        page = VaultPage()
        qtbot.addWidget(page)
        return page

    def _select_site(self, page, name):
        for i in range(page.lst_sites.count()):
            if name in page.lst_sites.item(i).text():
                page.lst_sites.setCurrentRow(i)
                return
        raise AssertionError(f"site {name} 不在列表中")

    def _select_acct(self, page, name):
        t = page.tbl_accts
        for r in range(t.rowCount()):
            if name in (t.item(r, 0).text() or ""):
                t.selectRow(r)
                return
        raise AssertionError(f"account {name} 不在表格中")

    def test_two_level_lists_and_search(self, qtbot, seeded):
        page = self._page(qtbot)
        page.refresh_sites()
        assert page.lst_sites.count() == 2
        self._select_site(page, "GitHub")
        page.refresh_accounts()
        assert page.tbl_accts.rowCount() == 2
        assert "（2）" in next(page.lst_sites.item(i).text()
                              for i in range(2) if "GitHub" in page.lst_sites.item(i).text())
        # 网站级搜索过滤（代理 → 列表同步）
        page.edt_site.setText("aws")
        assert page.lst_sites.count() == 1
        assert "AWS" in page.lst_sites.item(0).text()
        page.edt_site.setText("")
        assert page.lst_sites.count() == 2
        # 账号级搜索（当前网站 GitHub）
        self._select_site(page, "GitHub")
        page.edt_acct.setText("work")
        assert page.tbl_accts.rowCount() == 1
        assert "alice" in page.tbl_accts.item(0, 0).text()
        page.edt_acct.setText("")
        assert page.tbl_accts.rowCount() == 2

    def test_del_last_site_clears_account_list(self, qtbot, seeded, monkeypatch):
        """回归：删除最后一个网站后，右侧账号列表必须清空——
        refresh_sites 依赖 setCurrentRow 间接触发账号刷新，列表为空时不触发。"""
        page = self._page(qtbot)
        page.refresh_sites()
        self._select_site(page, "GitHub")
        page.refresh_accounts()
        assert page.tbl_accts.rowCount() > 0
        # 删除两个网站（确认弹窗一律 Yes）
        from PySide6.QtWidgets import QMessageBox
        monkeypatch.setattr(QMessageBox, "question",
                            staticmethod(lambda *a, **k: QMessageBox.StandardButton.Yes))
        page.refresh_sites()
        while page.lst_sites.count():
            page.lst_sites.setCurrentRow(0)
            page._del_site()
        assert page.lst_sites.count() == 0
        assert page.tbl_accts.rowCount() == 0
        assert page.acct_model.rowCount() == 0

    def test_add_account_through_dialog(self, qtbot, seeded, monkeypatch):
        """回归：_add_account 必须把对话框里的密码传给 vault.add_account
        （曾因 pop 后漏传报 missing positional argument 'password'）。"""
        from syskit.ui import vault_page as vp_mod
        page = self._page(qtbot)
        page.refresh_sites()
        self._select_site(page, "GitHub")

        real_dialog = vp_mod._AccountDialog

        def fake_dialog(parent=None, edit=False):
            d = real_dialog(parent, edit=edit)
            d.edt_username.setText("carol")
            d.edt_password.setText("pw-carol")
            d.edt_label.setText("test")
            monkeypatch.setattr(d, "exec", lambda: 1)    # QDialog.Accepted
            return d

        monkeypatch.setattr(vp_mod, "_AccountDialog", fake_dialog)
        page._add_account()
        gh = vault.search_sites("GitHub")[0]
        assert gh["n_accounts"] == 3
        carol = next(a for a in vault.list_accounts(gh["id"]) if a["username"] == "carol")
        assert carol["label"] == "test"
        assert vault.get_secret(carol["id"], page.crypto, "password") == "pw-carol"

    def test_add_account_empty_password_rejected(self, qtbot, seeded, monkeypatch):
        from syskit import vault as vault_mod
        from syskit.ui import vault_page as vp_mod
        page = self._page(qtbot)
        page.refresh_sites()
        self._select_site(page, "GitHub")
        before = vault_mod.search_sites("GitHub")[0]["n_accounts"]

        real_dialog = vp_mod._AccountDialog

        def fake_dialog(parent=None, edit=False):
            d = real_dialog(parent, edit=edit)
            d.edt_username.setText("no-pw")
            monkeypatch.setattr(d, "exec", lambda: 1)
            return d

        monkeypatch.setattr(vp_mod, "_AccountDialog", fake_dialog)
        monkeypatch.setattr(page, "_ensure_unlocked", lambda: True)
        page._add_account()
        assert vault_mod.search_sites("GitHub")[0]["n_accounts"] == before

    def _master_page(self, qtbot):
        """已设主密码 + 一条账号的页面（账号 X/u，密码 pw-x）。"""
        from syskit.ui.vault_page import VaultPage
        c = VaultCrypto()
        c.setup("pw")
        vault.init_db()
        sid = vault.add_site("X")
        vault.add_account(sid, "pw-x", c, username="u")
        page = VaultPage()
        qtbot.addWidget(page)
        page.crypto = c
        page._refresh_state()
        page.refresh_sites()
        page.refresh_accounts()
        return page

    def test_edit_dialog_prefills_and_preserves_fields(self, qtbot, seeded, monkeypatch):
        """P0 回归：编辑对话框必须回填现有值；只改密码不得清空用户名/标签/TOTP/备注
        （曾因对话框不回填、保存用空串覆盖造成静默数据丢失）。"""
        from syskit.ui import vault_page as vp_mod
        page = self._page(qtbot)
        page.refresh_sites()
        self._select_site(page, "GitHub")
        page.refresh_accounts()
        self._select_acct(page, "alice")

        real_dialog = vp_mod._AccountDialog
        made = []

        def fake_dialog(parent=None, edit=False, account=None):
            d = real_dialog(parent, edit=edit, account=account)
            d.edt_password.setText("pw-new")
            made.append((dict(account or {}), d))
            monkeypatch.setattr(d, "exec", lambda: 1)
            return d

        monkeypatch.setattr(vp_mod, "_AccountDialog", fake_dialog)
        page._edit_account()
        acct_in, dlg = made[0]
        # 对话框回填现有值
        assert acct_in["username"] == "alice" and acct_in["label"] == "work"
        assert dlg.edt_username.text() == "alice"
        assert dlg.edt_label.text() == "work"
        # 只改密码保存 → 其余字段原值保留
        gh = vault.search_sites("GitHub")[0]
        alice = next(a for a in vault.list_accounts(gh["id"]) if a["username"] == "alice")
        assert vault.get_secret(alice["id"], page.crypto, "password") == "pw-new"
        assert vault.get_secret(alice["id"], page.crypto, "totp") == RFC_SECRET
        row = next(a for a in vault.list_accounts(gh["id"]) if a["id"] == alice["id"])
        assert row["username"] == "alice" and row["label"] == "work"
        assert vault.get_secret(alice["id"], page.crypto, "notes") == "n1"

    def test_copy_password_master_trust_window(self, qtbot, monkeypatch):
        """验证主密码一次 → 免重复输入窗口；窗口外再次索要；错误密码拒绝。"""
        import time as _t
        from syskit import clipboard as clip_mod
        page = self._master_page(qtbot)
        fake = _FakeQClipboard()
        monkeypatch.setattr(page, "_clip", lambda: fake)
        page.lst_sites.setCurrentRow(0)
        page.refresh_accounts()
        page.tbl_accts.selectRow(0)

        asked = []
        monkeypatch.setattr(page, "_ask_password",
                            lambda title: asked.append(title) or "pw")
        page._copy_password()
        assert asked == ["验证主密码"]
        assert fake.text_val == "pw-x"

        # 信任窗口内再复制：不再索要
        asked.clear()
        fake.text_val = ""
        page._copy_password()
        assert asked == [] and fake.text_val == "pw-x"

        # 窗口过期 → 再次索要
        asked.clear()
        page._master_until = _t.time() - 1
        page._copy_password()
        assert asked == ["验证主密码"] and fake.text_val == "pw-x"

        # 错误密码 → 拒绝复制
        asked.clear()
        fake.text_val = ""
        page._master_until = 0
        monkeypatch.setattr(page, "_ask_password", lambda title: "wrong")
        page._copy_password()
        assert fake.text_val == ""
        clip_mod._sensitive.clear()

    def test_account_table_masked_and_single_click_copy(self, qtbot, seeded, monkeypatch):
        """回归：密码列未显示时为 ****；单击账号格复制用户名、密码格复制密码。"""
        from syskit import clipboard as clip_mod
        page = self._page(qtbot)
        fake = _FakeQClipboard()
        monkeypatch.setattr(page, "_clip", lambda: fake)
        page.refresh_sites()
        self._select_site(page, "GitHub")
        page.refresh_accounts()
        t = page.tbl_accts
        assert t.item(0, 1).text() == "****"            # 未显示 → 掩码
        alice_row = next(r for r in range(t.rowCount())
                         if "alice" in t.item(r, 0).text())
        t.selectRow(alice_row)
        t.cellClicked.emit(alice_row, 0)                # 单击账号格 → 复制用户名
        assert fake.text_val == "alice"
        t.cellClicked.emit(alice_row, 1)                # 单击密码格 → 复制密码（DPAPI 免验证）
        assert fake.text_val == "pw-alice"
        clip_mod._sensitive.clear()

    def test_view_toggle_one_click_show_hide(self, qtbot, monkeypatch):
        """一键显示：验证主密码后密码列整列明文；再点隐藏回 ****；隐藏不索要主密码。"""
        page = self._master_page(qtbot)
        page.lst_sites.setCurrentRow(0)
        page.refresh_accounts()
        page.tbl_accts.selectRow(0)
        asked = []
        monkeypatch.setattr(page, "_ask_password",
                            lambda title: asked.append(title) or "pw")

        page._toggle_show_password()                # 显示
        assert asked == ["验证主密码"]
        assert page.tbl_accts.item(0, 1).text() == "pw-x"    # 明文在密码列
        assert page.btn_show_pw.text() == "隐藏"

        page._toggle_show_password()                # 隐藏（无第二次验证）
        assert asked == ["验证主密码"]
        assert page.btn_show_pw.text() == "显示"
        assert page.tbl_accts.item(0, 1).text() == "****"    # 回掩码

        # 信任窗口内再次显示：不索要主密码
        page._toggle_show_password()
        assert asked == ["验证主密码"]
        assert page.tbl_accts.item(0, 1).text() == "pw-x"

    def test_view_auto_hides_after_timeout(self, qtbot, monkeypatch):
        """明文显示 20 秒后由 _show_timer 自动隐藏回 ****。"""
        page = self._master_page(qtbot)
        monkeypatch.setattr(page, "_ask_password", lambda title: "pw")
        monkeypatch.setattr(page._show_timer, "start",
                            lambda ms: page._show_timer.setProperty("ms", ms))
        page.lst_sites.setCurrentRow(0)
        page.refresh_accounts()
        page.tbl_accts.selectRow(0)
        page._toggle_show_password()
        assert page.btn_show_pw.text() == "隐藏"
        page._show_timer.timeout.emit()             # 模拟 20 秒到点
        assert page.btn_show_pw.text() == "显示"
        assert page.tbl_accts.item(0, 1).text() == "****"

    def test_dpapi_relock_unlocks_silently(self, qtbot, monkeypatch):
        """回归：DPAPI 免密模式手动锁定后，_ensure_unlocked 应静默解锁，
        不得弹主密码输入框（本就无主密码）。"""
        from syskit.ui.vault_page import VaultPage
        c = VaultCrypto()
        c.setup()                                   # DPAPI 模式
        c.lock()
        page = VaultPage()
        qtbot.addWidget(page)
        page.crypto = c
        page._refresh_state()

        def _boom(title):
            monkey_calls.append(title)
            raise AssertionError("DPAPI 模式不应索要主密码")

        monkey_calls = []
        monkeypatch.setattr(page, "_ask_password", _boom)
        assert page._ensure_unlocked() is True
        assert monkey_calls == []
        assert page.crypto.is_unlocked

    def test_copy_password_is_sensitive(self, qtbot, seeded, monkeypatch):
        from syskit import clipboard as clip_mod
        page = self._page(qtbot)
        fake = _FakeQClipboard()
        monkeypatch.setattr(page, "_clip", lambda: fake)
        page.refresh_sites()
        self._select_site(page, "GitHub")
        page.refresh_accounts()
        self._select_acct(page, "alice")
        page._copy_password()
        assert fake.text_val == "pw-alice"
        assert "不进剪贴板历史" in page.lbl_detail.text()
        # 敏感内容已被标记：剪贴板监听不会入库
        h = clip_mod.ClipboardHistory()
        assert h.add("pw-alice") is False
        clip_mod._sensitive.clear()

    def test_copy_username_and_totp(self, qtbot, seeded, monkeypatch):
        page = self._page(qtbot)
        fake = _FakeQClipboard()
        monkeypatch.setattr(page, "_clip", lambda: fake)
        page.refresh_sites()
        self._select_site(page, "GitHub")
        page.refresh_accounts()
        self._select_acct(page, "alice")
        page._copy_username()
        assert fake.text_val == "alice"
        gh = vault.search_sites("GitHub")[0]
        alice = next(a for a in vault.list_accounts(gh["id"]) if a["username"] == "alice")
        secret = vault.get_secret(alice["id"], page.crypto, "totp")
        page._copy_totp()
        assert fake.text_val == vault.totp_code(secret)     # 同一 30s 时间桶

    def test_lock_blocks_copy_prompt_cancel(self, qtbot, monkeypatch):
        from syskit.ui.vault_page import VaultPage
        c = VaultCrypto()               # 默认路径（重定向后的 APPDATA）
        c.setup("pw")
        vault.init_db()
        sid = vault.add_site("X")
        vault.add_account(sid, "pw", c, username="u")
        page = VaultPage()
        qtbot.addWidget(page)
        page.crypto = c
        page.crypto.lock()
        page._refresh_state()
        page.refresh_sites()
        page.refresh_accounts()
        asked = []
        monkeypatch.setattr(page, "_ask_password",
                            lambda title: asked.append(title))
        page.lst_sites.setCurrentRow(0)
        page.refresh_accounts()
        page.tbl_accts.selectRow(0)
        page._copy_password()                       # 弹解锁 → 取消 → 不复制
        assert asked == ["解锁密码库"]
        assert page.crypto.is_unlocked is False
