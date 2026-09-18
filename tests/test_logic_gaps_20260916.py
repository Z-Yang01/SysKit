"""2026-09-16 覆盖率缺口补齐：五个纯逻辑模块的剩余分支（只新增本文件）。

覆盖目标（当日上午 coverage --show-missing 实测缺口）：
- syskit/vault_crypto.py  ：DPAPI 加密失败、短密文拒绝、损坏 key 文件（_meta 吞异常 +
  解锁记日志保持锁定）、非 Windows 守卫（DPAPI 依赖）、重复解锁、未初始化解锁、
  DPAPI 解包失败、verifier 明文不符、空主密码、DPAPI 模式校验密码、锁定态移除
  主密码、无主密码移除、非 Windows 移除、更改主密码重包裹；
- syskit/winregread.py    ：value() 三分支、subkeys() Close 异常吞噬、
  pnpcapabilities 的 disabled / enabled_off / None 三态；
- syskit/dnsquery.py      ：Answer 区非压缩指针名字跳过（根标签 + 多标签两条路）、
  resolve_system future 异常 → []；
- syskit/clipboard.py     ：save_image_bytes OSError、mark_sensitive(None)、
  foreground_process_name 四条岔路、save 吞异常、置顶同名异 kind 拒收、
  图片载荷删除 OSError 吞噬；
- syskit/secure_store.py  ：密封失败后清理 unlink 也失败 → 仍返回 False。

安全（叠加 conftest 哨兵）：全程无真实 DNS / 网络外连 / 注册表写 / 服务操作 ——
- winreg 读写全部注入假件（OpenKey/EnumValue/EnumKey 打在 winreg 模块上，自动还原）；
- DNS 用假 socket 收发自构报文（sendto 只进假件，绝不触网）；
- DPAPI 失败分支只把 crypt32 函数指针打桩为返回 0（不真实调用）；成功路径与
  既有测试一致走本地 DPAPI（无副作用，落盘在临时目录）；
- 非 Windows 分支通过替换 vault_crypto / clipboard 模块命名空间的 sys 引用驱动；
- 持久化全部落在 conftest 重定向的临时目录。
"""
import json
import os
import socket
import struct
from types import SimpleNamespace

import pytest

from syskit import clipboard as clip_mod
from syskit import config
from syskit import dnsquery as dns_mod
from syskit import secure_store as ss
from syskit import vault_crypto as vc_mod
from syskit import winregread
from syskit.vault_crypto import VaultCrypto, VaultLockedError


@pytest.fixture(autouse=True)
def fast_kdf(monkeypatch):
    """PBKDF2 迭代调小（与 test_vault_page_gaps 同范式，真实 AESGCM/DPAPI 本地计算）。"""
    monkeypatch.setattr(vc_mod, "KDF_ITER", 1000, raising=False)


# ============================================================
# vault_crypto.py
# ============================================================

class TestVaultCryptoGaps:
    def test_dpapi_protect_failure_raises_oserror(self, monkeypatch):
        """CryptProtectData 返回 0 → OSError（43 行）。打桩 crypt32 函数指针。"""
        import ctypes
        ctypes.windll.crypt32.CryptProtectData          # 先物化缓存属性再打桩

        def fail(*a, **k):
            return 0

        monkeypatch.setattr(ctypes.windll.crypt32, "CryptProtectData", fail)
        with pytest.raises(OSError, match="CryptProtectData"):
            vc_mod._dpapi_protect(b"payload")

    def test_open_seal_rejects_short_blob(self):
        """密文长度 ≤ nonce 长度 → ValueError（91 行）。"""
        with pytest.raises(ValueError, match="密文长度非法"):
            vc_mod._open_seal(b"k" * 32, b"tooshort")

    def test_meta_corrupt_json_returns_empty_dict(self, tmp_path):
        """key 文件损坏（非法 JSON）→ _meta() 吞异常返回 {}（124-125 行）。"""
        kf = tmp_path / "vault.key"
        kf.write_text("{not-json", encoding="utf-8")
        crypto = VaultCrypto(key_file=str(kf))
        assert crypto._meta() == {}
        assert crypto.has_master_password() is False

    def test_provision_requires_windows_for_dpapi(self, tmp_path, monkeypatch):
        """无主密码 + 非 Windows → RuntimeError（155 行），且不落任何文件。"""
        crypto = VaultCrypto(key_file=str(tmp_path / "vault.key"))
        monkeypatch.setattr(vc_mod, "sys", SimpleNamespace(platform="linux"))
        with pytest.raises(RuntimeError, match="DPAPI"):
            crypto._provision_new(None)
        assert not crypto.is_provisioned

    def test_unlock_when_already_unlocked_returns_true(self, tmp_path):
        """已解锁再 unlock 直接 True（166 行）。"""
        crypto = VaultCrypto(key_file=str(tmp_path / "vault.key"))
        crypto.setup()
        assert crypto.unlock() is True

    def test_unlock_without_key_file_returns_false(self, tmp_path):
        """未初始化 unlock → False（168 行）。"""
        crypto = VaultCrypto(key_file=str(tmp_path / "absent.key"))
        assert crypto.unlock() is False

    def test_unlock_corrupt_key_file_logs_and_stays_locked(self, tmp_path):
        """key 文件损坏：解锁记日志、保持锁定、返回 False 而非抛（172-178 行）。"""
        kf = tmp_path / "vault.key"
        kf.write_text("garbage{", encoding="utf-8")
        crypto = VaultCrypto(key_file=str(kf))
        assert crypto.unlock() is False                 # 掉电损坏不崩 GUI
        assert not crypto.is_unlocked

    def test_unlock_dpapi_failure_returns_false(self, tmp_path, monkeypatch):
        """DPAPI 模式解包失败 → False（183-184 行）。"""
        crypto = VaultCrypto(key_file=str(tmp_path / "vault.key"))
        crypto.setup()
        crypto.lock()

        def boom(blob):
            raise OSError("dpapi broken")

        monkeypatch.setattr(vc_mod, "_dpapi_unprotect", boom)
        assert crypto.unlock() is False

    def test_unlock_verifier_plaintext_mismatch_returns_false(self, tmp_path):
        """verifier 能解开但明文 != _VERIFIER → False（189 行，区别于 InvalidTag 岔路）。"""
        crypto = VaultCrypto(key_file=str(tmp_path / "vault.key"))
        crypto.setup("pw")
        meta = crypto._meta()
        dk = vc_mod._derive("pw", vc_mod._unb64(meta["kdf"]["salt"]))
        meta["verifier"] = vc_mod._b64(vc_mod._seal(dk, b"tampered", aad=b"vault-verify"))
        crypto._write_meta(meta)
        crypto.lock()
        assert crypto.unlock("pw") is False

    def test_set_master_password_empty_raises(self, tmp_path):
        """已解锁但空主密码 → ValueError（205 行）。"""
        crypto = VaultCrypto(key_file=str(tmp_path / "vault.key"))
        crypto.setup()
        with pytest.raises(ValueError, match="主密码不能为空"):
            crypto.set_master_password("")

    def test_verify_password_on_dpapi_mode_returns_false(self, tmp_path):
        """DPAPI 模式（无主密码）校验密码 → False（212 行）。"""
        crypto = VaultCrypto(key_file=str(tmp_path / "vault.key"))
        crypto.setup()
        assert crypto.verify_password("anything") is False

    def test_remove_master_password_locked_raises(self, tmp_path):
        """锁定态移除主密码 → VaultLockedError（223 行）。"""
        crypto = VaultCrypto(key_file=str(tmp_path / "vault.key"))
        crypto.setup("pw")
        crypto.lock()
        with pytest.raises(VaultLockedError):
            crypto.remove_master_password("pw")

    def test_remove_master_password_without_master_returns_false(self, tmp_path):
        """DPAPI 模式（无主密码）移除 → False（225 行）。"""
        crypto = VaultCrypto(key_file=str(tmp_path / "vault.key"))
        crypto.setup()
        assert crypto.remove_master_password("") is False

    def test_remove_master_password_requires_windows(self, tmp_path, monkeypatch):
        """移除主密码回 DPAPI 时非 Windows → RuntimeError（229 行）。"""
        crypto = VaultCrypto(key_file=str(tmp_path / "vault.key"))
        crypto.setup("pw")
        monkeypatch.setattr(vc_mod, "sys", SimpleNamespace(platform="linux"))
        with pytest.raises(RuntimeError, match="DPAPI"):
            crypto.remove_master_password("pw")

    def test_change_master_password_rewraps(self, tmp_path):
        """更改主密码：旧密码失效、新密码可解锁（235 行）。"""
        crypto = VaultCrypto(key_file=str(tmp_path / "vault.key"))
        crypto.setup("old")
        crypto.change_master_password("new")
        crypto.lock()
        assert crypto.unlock("old") is False
        assert crypto.unlock("new") is True

    def test_field_roundtrip_and_basics(self, tmp_path):
        """字段加解密往返 + 锁定态守卫 + 重复初始化守卫（兼收本文件独立运行的覆盖）。"""
        crypto = VaultCrypto(key_file=str(tmp_path / "vault.key"))
        with pytest.raises(VaultLockedError):
            crypto.encrypt(b"x")
        with pytest.raises(VaultLockedError):
            crypto.decrypt(b"y")
        crypto.setup("pw")
        with pytest.raises(RuntimeError, match="已初始化"):
            crypto.setup("again")
        assert crypto.has_master_password() is True
        assert crypto.verify_password("pw") is True
        blob = crypto.encrypt(b"secret-field")
        assert crypto.decrypt(blob) == b"secret-field"
        crypto.lock()
        assert not crypto.is_unlocked


# ============================================================
# winregread.py
# ============================================================

class _FakeRegKey:
    """假注册表句柄：Close 可配置为抛错（驱动 69-70 行的吞噬分支）。"""

    def __init__(self, values=None, keys=None, close_error=False):
        self._values = list(values or [])   # [(name, data, type), ...]
        self._keys = list(keys or [])
        self._close_error = close_error
        self.closed = False

    def Close(self):
        if self._close_error:
            raise OSError("handle already closed")
        self.closed = True


def _install_fake_winreg(monkeypatch, key):
    """把 winreg 读操作打在 winreg 模块属性上（monkeypatch 自动还原）。"""
    import winreg as winreg_mod

    monkeypatch.setattr(winreg_mod, "OpenKey", lambda *a, **k: key)

    def _enum_value(k, i):
        if i < len(k._values):
            return k._values[i]
        raise OSError(259, "No more data is available")

    def _enum_key(k, i):
        if i < len(k._keys):
            return k._keys[i]
        raise OSError(259, "No more data is available")

    monkeypatch.setattr(winreg_mod, "EnumValue", _enum_value)
    monkeypatch.setattr(winreg_mod, "EnumKey", _enum_key)


class TestWinregReadGaps:
    def test_values_reads_all_types_and_closes(self, monkeypatch):
        k = _FakeRegKey(values=[("Start", 3, 4), ("Desc", "abc", 1)])
        _install_fake_winreg(monkeypatch, k)
        assert winregread.values(r"HKLM\SOFTWARE\Test") == {"Start": 3, "Desc": "abc"}
        assert k.closed

    def test_values_open_failure_returns_empty(self, monkeypatch):
        import winreg as wr

        def boom(*a, **k):
            raise OSError("key missing")

        monkeypatch.setattr(wr, "OpenKey", boom)
        assert winregread.values(r"HKLM\SOFTWARE\Nope") == {}

    def test_values_unknown_hive_returns_empty(self):
        assert winregread.values(r"BADS\X") == {}

    def test_value_found_missing_and_empty(self, monkeypatch):
        """value()：命中 / 未命中给 default / 空键给 default（45-46 行）。"""
        k = _FakeRegKey(values=[("A", 1, 4)])
        _install_fake_winreg(monkeypatch, k)
        assert winregread.value(r"HKCU\S", "A") == 1
        assert winregread.value(r"HKCU\S", "B", "dft") == "dft"

        empty = _FakeRegKey()
        _install_fake_winreg(monkeypatch, empty)
        assert winregread.value(r"HKCU\S", "A", "d2") == "d2"

    def test_subkeys_lists_and_close_error_swallowed(self, monkeypatch):
        """EnumKey 列举 + Close 抛错被吞、结果照常返回（69-70 行）。"""
        k = _FakeRegKey(keys=["0000", "0001"], close_error=True)
        _install_fake_winreg(monkeypatch, k)
        assert winregread.subkeys(r"HKLM\SYSTEM\ControlSet\Class") == ["0000", "0001"]

    def test_subkeys_open_failure_returns_empty(self, monkeypatch):
        import winreg as wr

        def boom(*a, **k):
            raise OSError("key missing")

        monkeypatch.setattr(wr, "OpenKey", boom)
        assert winregread.subkeys(r"HKLM\Nope") == []
        assert winregread.subkeys(r"BADS\X") == []

    def test_pnpcapabilities_disabled_wins(self, monkeypatch):
        """任一键已禁止 → disabled；非 0 前缀子键跳过（86/88 行）。"""
        monkeypatch.setattr(winregread, "subkeys",
                            lambda p: ["0000", "0001", "Config"])
        vals_by_sub = {
            "0000": {"DriverDesc": "Intel(R) Wi-Fi", "PnPCapabilities": "24"},
            "0001": {"DriverDesc": "Intel(R) Wi-Fi"},           # 无 PnPCapabilities
        }

        def fake_values(path):
            sub = path.rsplit("\\", 1)[-1]
            return vals_by_sub.get(sub, {"DriverDesc": "别的网卡"})

        monkeypatch.setattr(winregread, "values", fake_values)
        assert winregread.pnpcapabilities("Intel(R) Wi-Fi") == "disabled"
        assert winregread.pnpcapabilities("不存在的驱动") is None

    def test_pnpcapabilities_all_enabled_off(self, monkeypatch):
        """匹配但未禁止 → enabled_off（90 行）；无匹配 → None。"""
        monkeypatch.setattr(winregread, "subkeys", lambda p: ["0000"])

        def fake_values(path):
            return {"DriverDesc": "Intel(R) Wi-Fi", "PnPCapabilities": "16"}

        monkeypatch.setattr(winregread, "values", fake_values)
        assert winregread.pnpcapabilities("Intel(R) Wi-Fi") == "enabled_off"
        assert winregread.pnpcapabilities("别的驱动") is None


# ============================================================
# dnsquery.py
# ============================================================

def _fake_dns_socket(answer_name: bytes, rtype: int, rdata: bytes):
    """假 UDP socket：记录 sendto 并按查询报文的 txid/question 自构应答。

    应答 Answer 区的 NAME 由用例给定（根标签 / 未压缩多标签），用于驱动
    query() 名字跳过的非 0xC0 分支；全程无真实网络。
    """

    class _FakeDnsSock:
        def __init__(self, *a, **k):
            self.closed = False
            self.resp = None

        def settimeout(self, t):
            pass

        def sendto(self, pkt, addr):
            assert addr[1] == 53
            txid = pkt[:2]
            question = pkt[12:]                 # 头 12B 后即 Question 区原样回显
            header = txid + struct.pack(">HHHHH", 0x8180, 1, 1, 0, 0)
            answer = (answer_name
                      + struct.pack(">HHIH", rtype, 1, 300, len(rdata)) + rdata)
            self.resp = header + question + answer

        def recvfrom(self, n):
            assert self.resp is not None, "sendto 未被调用"
            return self.resp, ("10.0.0.1", 53)

        def close(self):
            self.closed = True

    return _FakeDnsSock


class TestDnsQueryGaps:
    def test_query_answer_root_name(self, monkeypatch):
        """Answer NAME 为根标签（0x00）→ pos+=1 后 length==0 break（67-69 行）。"""
        monkeypatch.setattr(dns_mod.socket, "socket",
                            _fake_dns_socket(b"\x00", 1, socket.inet_aton("1.2.3.4")))
        assert dns_mod.query("www.example.com", "10.0.0.1") == ["1.2.3.4"]

    def test_query_answer_uncompressed_labels(self, monkeypatch):
        """Answer NAME 为未压缩多标签 → pos+=length 逐段跳过（67/70 行）。"""
        name = b"\x03www\x07example\x03com\x00"
        monkeypatch.setattr(dns_mod.socket, "socket",
                            _fake_dns_socket(name, 1, socket.inet_aton("9.9.9.9")))
        assert dns_mod.query("www.example.com", "10.0.0.1") == ["9.9.9.9"]

    def test_query_aaaa_uncompressed_labels(self, monkeypatch):
        """AAAA 记录（rtype 28, 16B）走未压缩名字应答。"""
        name = b"\x04mail\x07example\x03com\x00"
        v6 = socket.inet_pton(socket.AF_INET6, "fe80::1")
        monkeypatch.setattr(dns_mod.socket, "socket",
                            _fake_dns_socket(name, 28, v6))
        assert dns_mod.query("mail.example.com", "10.0.0.1", qtype=28) == ["fe80::1"]

    def test_resolve_system_future_error_returns_empty(self, monkeypatch):
        """getaddrinfo 抛错（daemon 线程内捕获）→ 返回 []，不向调用方穿透。"""
        import socket as socket_mod

        def boom(*a, **k):
            raise RuntimeError("getaddrinfo down")

        monkeypatch.setattr(dns_mod.socket, "getaddrinfo", boom)
        assert dns_mod.resolve_system("www.example.com", timeout=2.0) == []

    def test_resolve_system_hung_dns_times_out_without_exit_block(self, monkeypatch):
        """系统 DNS 挂死：join 超时返回 []；解析线程为 daemon，不拖住进程退出。

        回归：ThreadPoolExecutor.shutdown(wait=False) 的 atexit 钩子仍会 join
        未完成 worker——DNS 挂死时应用退出被卡住（改 daemon 线程后消除）。
        """
        import threading

        def hang(*a, **k):
            threading.Event().wait(5)          # 模拟挂死的 getaddrinfo

        monkeypatch.setattr(dns_mod.socket, "getaddrinfo", hang)
        assert dns_mod.resolve_system("www.example.com", timeout=0.2) == []

    def test_query_safe_swallows_and_passes(self, monkeypatch):
        """query_safe：异常 → []，正常 → 透传（兼收本文件独立运行的覆盖）。"""
        def boom(*a, **k):
            raise RuntimeError("net down")

        monkeypatch.setattr(dns_mod, "query", boom)
        assert dns_mod.query_safe("h", "10.0.0.1") == []
        monkeypatch.setattr(dns_mod, "query", lambda *a, **k: ["1.2.3.4"])
        assert dns_mod.query_safe("h", "10.0.0.1") == ["1.2.3.4"]

    def test_encode_qname_rejects_bad_labels(self):
        """空标签 / 超 63 字节标签 → ValueError（兼收本文件独立运行的覆盖）。"""
        with pytest.raises(ValueError):
            dns_mod._encode_qname("a." + "b" * 64 + ".com")
        with pytest.raises(ValueError):
            dns_mod._encode_qname("..")


# ============================================================
# clipboard.py
# ============================================================

class TestClipboardGaps:
    def test_save_image_bytes_oserror_returns_empty(self, tmp_path, monkeypatch):
        """clipboard_files 被同名文件占据 → makedirs 抛 OSError → 返回 ""（45-46 行）。"""
        monkeypatch.setattr(config, "APPDATA_DIR", str(tmp_path), raising=False)
        (tmp_path / "clipboard_files").write_text("I am a file", encoding="utf-8")
        assert clip_mod.save_image_bytes(b"\x89PNG-x") == ""

    def test_mark_sensitive_none_noop(self):
        """mark_sensitive(None) 直接返回，不产生摘要（53 行）。"""
        clip_mod._sensitive.clear()
        try:
            clip_mod.mark_sensitive(None)
            assert clip_mod._sensitive == []
        finally:
            clip_mod._sensitive.clear()

    def test_fg_name_non_windows(self, monkeypatch):
        """非 Windows 平台 → ""（70 行）；仅替换本模块命名空间的 sys 引用。"""
        monkeypatch.setattr(clip_mod, "sys", SimpleNamespace(platform="linux"))
        assert clip_mod.foreground_process_name() == ""

    def test_fg_name_no_foreground_window(self, monkeypatch):
        """无前台窗口（hwnd=0）→ ""（76 行）。"""
        import ctypes
        ctypes.windll.user32.GetForegroundWindow        # 先物化再打桩
        monkeypatch.setattr(ctypes.windll.user32, "GetForegroundWindow", lambda: 0)
        assert clip_mod.foreground_process_name() == ""

    def test_fg_name_zero_pid(self, monkeypatch):
        """取不到前台 PID → ""（80 行）。"""
        import ctypes
        ctypes.windll.user32.GetForegroundWindow
        monkeypatch.setattr(ctypes.windll.user32, "GetForegroundWindow", lambda: 4321)
        monkeypatch.setattr(ctypes.windll.user32, "GetWindowThreadProcessId",
                            lambda hwnd, pid_ptr: 0)    # 不写入 → pid.value 保持 0
        assert clip_mod.foreground_process_name() == ""

    def test_fg_name_exception_swallowed(self, monkeypatch):
        """user32 调用抛异常 → ""（82-83 行）。"""
        import ctypes
        ctypes.windll.user32.GetForegroundWindow

        def boom():
            raise RuntimeError("no desktop")

        monkeypatch.setattr(ctypes.windll.user32, "GetForegroundWindow", boom)
        assert clip_mod.foreground_process_name() == ""

    def test_fg_name_happy_path(self, monkeypatch):
        """正常链路：hwnd → pid → 进程名去 .exe 小写（兼收独立运行的覆盖）。"""
        import ctypes
        import psutil
        ctypes.windll.user32.GetForegroundWindow

        def _write_pid(hwnd, pid_ptr):
            pid_ptr._obj.value = 4242                   # byref 对象回写

        monkeypatch.setattr(ctypes.windll.user32, "GetForegroundWindow", lambda: 4321)
        monkeypatch.setattr(ctypes.windll.user32, "GetWindowThreadProcessId", _write_pid)

        class _Proc:
            def name(self):
                return "Notepad.EXE"

        monkeypatch.setattr(psutil, "Process", lambda pid: _Proc())
        assert clip_mod.foreground_process_name() == "notepad"

    def test_save_swallows_storage_errors(self, tmp_path, monkeypatch):
        """save() 底层抛错被吞不穿透（151-152 行）——secure_store 的 from-import
        绑定须打在 secure_store 命名空间。"""
        monkeypatch.setattr(config, "APPDATA_DIR", str(tmp_path), raising=False)
        h = clip_mod.ClipboardHistory()
        h.add("boom")

        def boom(path, data):
            raise RuntimeError("disk gone")

        monkeypatch.setattr(ss, "save_sealed_json", boom)
        h.save()                                        # 不抛即通过
        assert h.texts() == ["boom"]                    # 内存数据不受影响

    def test_pinned_dup_different_kind_rejected(self):
        """同名同 data 的置顶条目即便 kind 不同也拒收（172-174 行）：
        首条查重因 kind 不同放行，落到置顶查重分支。"""
        h = clip_mod.ClipboardHistory()
        assert h.add("secret")
        h.toggle_pin(0)
        assert h.add("secret", kind="html") is False
        assert h.texts() == ["secret"]

    def test_delete_image_files_oserror_swallowed(self, tmp_path, monkeypatch):
        """PNG 载荷删除失败（文件被占用）→ OSError 被吞（264-265 行）。"""
        monkeypatch.setattr(config, "APPDATA_DIR", str(tmp_path), raising=False)
        name = clip_mod.save_image_bytes(b"\x89PNG-doomed")
        path = clip_mod.image_path(name)
        assert os.path.exists(path)

        def boom(p):
            raise OSError("file locked")

        monkeypatch.setattr(os, "unlink", boom)
        clip_mod.ClipboardHistory._delete_image_files([{"kind": "image", "data": name}])
        assert os.path.exists(path)                     # 删除失败不崩、文件仍在


# ============================================================
# secure_store.py
# ============================================================

class TestSecureStoreGaps:
    def test_save_failure_cleanup_unlink_also_fails(self, tmp_path):
        """makedirs 失败（目录被同名文件占据）→ tmp 从未创建 → 清理 unlink
        再抛 FileNotFoundError → 内层 except 吞掉、仍返回 False（53-54 行）。"""
        blocker = tmp_path / "notadir"
        blocker.write_text("I am a file, not a directory", encoding="utf-8")
        p = str(blocker / "x.json")
        assert ss.save_sealed_json(p, {"a": 1}) is False
        assert not os.path.exists(p + ".tmp")

    def test_sealed_container_json_shape(self, tmp_path):
        """密封容器字段断言（兼收本文件独立运行的覆盖）。"""
        p = str(tmp_path / "shape.json")
        assert ss.save_sealed_json(p, {"k": "值"}) is True
        outer = json.load(open(p, encoding="utf-8"))
        assert outer == {"v": 1, "enc": "dpapi", "data": outer["data"]}
        assert isinstance(outer["data"], str)
