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
        for i in range(page.lst_accts.count()):
            if name in page.lst_accts.item(i).text():
                page.lst_accts.setCurrentRow(i)
                return
        raise AssertionError(f"account {name} 不在列表中")

    def test_two_level_lists_and_search(self, qtbot, seeded):
        page = self._page(qtbot)
        page.refresh_sites()
        assert page.lst_sites.count() == 2
        self._select_site(page, "GitHub")
        page.refresh_accounts()
        assert page.lst_accts.count() == 2
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
        assert page.lst_accts.count() == 1
        assert "alice" in page.lst_accts.item(0).text()
        page.edt_acct.setText("")
        assert page.lst_accts.count() == 2

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
        page.lst_accts.setCurrentRow(0)
        page._copy_password()                       # 弹解锁 → 取消 → 不复制
        assert asked == ["解锁密码库"]
        assert page.crypto.is_unlocked is False
