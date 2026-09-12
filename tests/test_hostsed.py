"""syskit/hostsed.py + HostsTab 单元测试（tmp 文件注入，绝不触碰真实 hosts）。"""
import pytest

from syskit import hostsed

BOM = chr(0xFEFF).encode('utf-8')
NUL = chr(0).encode()
from test_ui_system_units import msgbox_recorder, recorder_kinds  # noqa: F401 复用弹窗记录夹具

BOM = chr(0xFEFF).encode('utf-8')   # UTF-8 BOM 字节
NUL = chr(0).encode()               # UTF-16 误读产生的 NUL 字节


SAMPLE = """# Copyright comment
127.0.0.1 localhost
# 10.0.0.5 blocked.example.com

10.0.0.6 db.local    # 数据库
::1 ip6-localhost
broken line without host
"""


@pytest.fixture
def hosts_file(tmp_path):
    p = tmp_path / "hosts"
    p.write_text(SAMPLE, encoding="utf-8")
    return str(p)


# ---------------- parse ----------------

class TestParse:
    def test_all_kinds(self):
        es = hostsed.parse(SAMPLE)
        kinds = [e["kind"] for e in es]
        assert kinds[0] == "comment"                      # 版权注释
        assert es[1]["kind"] == "entry"
        assert es[1]["ip"] == "127.0.0.1" and es[1]["host"] == "localhost"
        assert es[2]["kind"] == "disabled"                # 被注释的映射
        assert es[2]["host"] == "blocked.example.com"
        assert kinds[3] == "comment"                      # 空行
        db = es[4]
        assert db["kind"] == "entry" and db["comment"] == "数据库"
        assert kinds[6] == "comment"                      # 畸形行保留不炸

    def test_empty_and_none(self):
        assert hostsed.parse("") == []
        assert hostsed.parse(None) == []

    def test_aliases_and_inline_comment(self):
        es = hostsed.parse("1.2.3.4 foo.com bar.com baz.com    # 备注")
        assert es[0]["aliases"] == ["bar.com", "baz.com"]
        assert es[0]["comment"] == "备注"
        assert es[0]["enabled"] is True


# ---------------- serialize / roundtrip ----------------

class TestSerialize:
    def test_roundtrip_preserves_comments_and_entries(self, hosts_file):
        es = hostsed.read_entries(hosts_file)
        text = hostsed.serialize(es)
        es2 = hostsed.parse(text)
        assert [(e["kind"], e.get("ip"), e.get("host")) for e in es2] == \
               [(e["kind"], e.get("ip"), e.get("host")) for e in es]

    def test_disabled_rendered_as_comment(self):
        es = hostsed.parse("1.2.3.4 foo.com")
        hostsed.toggle_entry(es, 0)
        text = hostsed.serialize(es)
        assert text.strip() == "# 1.2.3.4 foo.com"

    def test_comment_with_remark(self):
        es = hostsed.parse("1.2.3.4 foo.com    # 老条目")
        hostsed.toggle_entry(es, 0)
        text = hostsed.serialize(es)
        assert "# 1.2.3.4 foo.com    # 老条目" in text


# ---------------- 增 / 删 / 切换 / 查找 ----------------

class TestMutate:
    def test_add_find_toggle_remove(self):
        es = hostsed.parse("# c\n")
        hostsed.add_entry(es, "127.0.0.1", "dev.local", comment="本地开发")
        assert hostsed.find_entry(es, "DEV.LOCAL") == 1     # 大小写不敏感
        assert hostsed.find_entry(es, "dev.local", ip="9.9.9.9") == -1
        hostsed.toggle_entry(es, 1)
        assert es[1]["enabled"] is False and es[1]["kind"] == "disabled"
        hostsed.toggle_entry(es, 1)
        assert es[1]["enabled"] is True
        hostsed.remove_entry(es, 1)
        assert hostsed.find_entry(es, "dev.local") == -1
        assert len(es) == 1                                 # 原注释保留

    def test_toggle_comment_no_effect(self):
        es = hostsed.parse("# pure")
        hostsed.toggle_entry(es, 0)
        assert es[0]["kind"] == "comment"

    def test_index_bounds(self):
        es = []
        hostsed.remove_entry(es, 5)                         # 不抛
        hostsed.toggle_entry(es, -1)


# ---------------- IO ----------------

class TestIO:
    def test_read_write_roundtrip_with_backup(self, hosts_file, tmp_path, monkeypatch):
        from syskit import config
        backup_dir = tmp_path / "backup"
        monkeypatch.setattr(config, "BACKUP_DIR", str(backup_dir), raising=False)
        es = hostsed.read_entries(hosts_file)
        hostsed.add_entry(es, "10.0.0.9", "new.local")
        dest = hostsed.write_entries(es, hosts_file, backup=True)
        assert dest and (tmp_path / "backup").exists()
        backups = list(backup_dir.iterdir())
        assert len(backups) == 1 and (backups[0] / "hosts").read_text(
            encoding="utf-8") == SAMPLE                    # 备份=原内容
        es2 = hostsed.read_entries(hosts_file)
        assert hostsed.find_entry(es2, "new.local") >= 0   # 新条目已写入

    def test_write_no_backup(self, hosts_file, tmp_path, monkeypatch):
        from syskit import config
        monkeypatch.setattr(config, "BACKUP_DIR", str(tmp_path / "nb"), raising=False)
        es = hostsed.read_entries(hosts_file)
        hostsed.write_entries(es, hosts_file, backup=False)
        assert not (tmp_path / "nb").exists()

    def test_read_missing_file_returns_empty(self, tmp_path):
        assert hostsed.read_entries(str(tmp_path / "nope")) == []

    def test_utf8_bom_first_entry_parsed(self, tmp_path):
        """回归：BOM 不得破坏首行 IP 判定（曾致首条目静默降级为注释）。"""
        p = tmp_path / "hosts_bom"
        p.write_bytes(BOM + b"127.0.0.1 first.local\n")
        es = hostsed.read_entries(str(p))
        assert es[0]["kind"] == "entry" and es[0]["host"] == "first.local"

    def test_binary_utf16_read_rejected_on_write(self, tmp_path, monkeypatch):
        """护栏：UTF-16/二进制误读（含 NUL 乱码）时拒绝写回，保护原文件。"""
        from syskit import config as cfg
        p = tmp_path / "hosts_u16"
        p.write_bytes("127.0.0.1 a.local\n".encode("utf-16"))
        monkeypatch.setattr(cfg, "BACKUP_DIR", str(tmp_path / "bk"), raising=False)
        es = hostsed.read_entries(str(p))
        with pytest.raises(OSError, match="编码异常"):
            hostsed.write_entries(es, str(p), backup=True)
        # 原文件未被破坏
        assert p.read_bytes() == "127.0.0.1 a.local\n".encode("utf-16")

    def test_backup_missing_source(self, tmp_path):
        assert hostsed.backup_hosts(str(tmp_path / "nope")) == ""


# ---------------- HostsTab UI ----------------

class TestHostsTab:
    def _tab(self, qtbot, hosts_file, monkeypatch, is_admin=True):
        from syskit.ui.toolbox_page import HostsTab
        monkeypatch.setattr("syskit.ui.toolbox_page.admin.is_admin", lambda: is_admin)
        tab = HostsTab(path=hosts_file)
        qtbot.addWidget(tab)
        return tab

    def test_load_toggle_delete_dirty_and_discard(self, qtbot, hosts_file, monkeypatch):
        tab = self._tab(qtbot, hosts_file, monkeypatch)
        assert tab.tbl.rowCount() == 7
        assert "管理员" in tab.lbl_info.text()
        assert not tab.btn_reload.isEnabled()               # 干净态

        tab.tbl.selectRow(2)                                # 禁用条目
        tab._toggle()
        assert tab._dirty and tab.btn_reload.isEnabled()
        assert "未保存" in tab.lbl_dirty.text()
        assert tab.tbl.item(2, 0).text().startswith("✔")    # 禁用→启用

        tab.tbl.selectRow(4)                                # db.local
        tab._remove()
        assert tab.tbl.rowCount() == 6

        tab.load()                                          # 放弃更改
        assert tab.tbl.rowCount() == 7 and not tab._dirty

    def test_add_validates_ip(self, qtbot, hosts_file, monkeypatch, msgbox_recorder):
        tab = self._tab(qtbot, hosts_file, monkeypatch)
        tab.edt_ip.setText("999.1.1.1")
        tab.edt_host.setText("bad.com")
        tab._add()
        assert tab.tbl.rowCount() == 7                      # 非法 IP 拒绝
        tab.edt_ip.setText("10.1.1.1")
        tab.edt_host.setText("ok.com")
        tab._add()
        assert tab.tbl.rowCount() == 8
        assert tab._dirty

    def test_save_requires_admin(self, qtbot, hosts_file, monkeypatch, msgbox_recorder):
        from syskit import hostsed as h

        tab = self._tab(qtbot, hosts_file, monkeypatch, is_admin=False)
        tab.tbl.selectRow(2)
        tab._toggle()
        writes = []
        monkeypatch.setattr(h, "write_entries", lambda *a, **k: writes.append(a) or "")
        tab._save()
        assert writes == []                                 # 非管理员不落盘
        assert recorder_kinds(msgbox_recorder).count("warning") == 1

        monkeypatch.setattr("syskit.ui.toolbox_page.admin.is_admin", lambda: True)
        tab.btn_save.setEnabled(True)
        tab._save()
        assert len(writes) == 1                             # 管理员写盘
        assert not tab._dirty

    def test_save_noop_when_clean(self, qtbot, hosts_file, monkeypatch):
        from syskit import hostsed as h

        tab = self._tab(qtbot, hosts_file, monkeypatch)
        writes = []
        monkeypatch.setattr(h, "write_entries", lambda *a, **k: writes.append(a) or "")
        tab._save()
        assert writes == []
