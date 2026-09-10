"""syskit/proc.py 单元测试：subprocess 封装的解码 / 超时 / 缺失命令分支（不启动真实进程）。

Mock 边界（配合 conftest 哨兵双保险）：
- subprocess.run 由本用例按需打桩（conftest 哨兵保证任何未打桩的真实调用直接失败）；
- subprocess.TimeoutExpired / FileNotFoundError / PermissionError 手工构造或抛出；
- run_ps 只验证命令拼装与超时透传（proc.run 被打桩记录，不真实执行 PowerShell）。

已修复行为：run() 对 FileNotFoundError 与一般 OSError（PermissionError 等）
均转为失败 CmdResult，不再向上抛穿调用方。
"""
import subprocess
from types import SimpleNamespace

import pytest

from syskit import proc

# conftest.block_system_calls（测试期夹具）会把 proc.run/run_ps 换成哨兵；
# 本文件在「收集期导入」时先抓到真实函数对象，供测试临时还原——
# subprocess.run 仍由每个用例显式打桩，不会产生真实进程。
_REAL_RUN = proc.run
_REAL_RUN_PS = proc.run_ps


@pytest.fixture
def real_run(monkeypatch):
    """把哨兵临时还原为真实 proc.run / proc.run_ps（配合用例内打桩）。"""
    monkeypatch.setattr(proc, "run", _REAL_RUN)
    monkeypatch.setattr(proc, "run_ps", _REAL_RUN_PS)


# ---------------- CmdResult ----------------

class TestCmdResult:
    @pytest.mark.parametrize("code,timeout,ok", [
        (0, False, True),
        (0, True, False),      # 超时即使退出码 0 也不算成功
        (1, False, False),
        (-1, True, False),
        (2, False, False),
    ])
    def test_ok_property(self, code, timeout, ok):
        r = proc.CmdResult(code=code, stdout="", stderr="", timeout=timeout)
        assert r.ok is ok

    def test_defaults(self):
        r = proc.CmdResult(0, "o", "e")
        assert r.timeout is False and r.ok is True


# ---------------- run ----------------

class TestRun:
    def _patch_run(self, monkeypatch, behavior=None):
        """behavior(returncode, stdout, stderr) → 假 Popen 结果；或抛指定异常。"""
        calls = {}

        def fake_run(cmd, **kwargs):
            calls["cmd"] = cmd
            calls["kwargs"] = kwargs
            if isinstance(behavior, Exception):
                raise behavior
            code, out, err = behavior
            return SimpleNamespace(returncode=code, stdout=out, stderr=err)

        monkeypatch.setattr(proc.subprocess, "run", fake_run)
        return calls

    def test_success_utf8_list_cmd(self, monkeypatch, real_run):
        calls = self._patch_run(monkeypatch, (0, b"out-text", b"err-text"))
        r = proc.run(["netsh", "wlan", "show"], timeout=9)
        assert (r.code, r.stdout, r.stderr, r.timeout) == (0, "out-text", "err-text", False)
        assert r.ok is True
        assert calls["cmd"] == ["netsh", "wlan", "show"]
        kw = calls["kwargs"]
        assert kw["capture_output"] is True and kw["timeout"] == 9
        assert kw["shell"] is False                       # list 命令不开 shell
        assert kw["creationflags"] == proc.CREATE_NO_WINDOW   # 隐藏窗口

    def test_str_cmd_uses_shell(self, monkeypatch, real_run):
        calls = self._patch_run(monkeypatch, (0, b"", b""))
        proc.run("type a.txt")
        assert calls["kwargs"]["shell"] is True

    def test_gbk_output_falls_back_to_mbcs(self, monkeypatch, real_run):
        """非 UTF-8 字节回退 ANSI 解码（本机 ACP=936 → GBK）。"""
        self._patch_run(monkeypatch, (0, b"\xd6\xd0\xce\xc4", b""))
        r = proc.run(["cmd"])
        assert r.stdout == "中文"                          # CP936 下 GBK "中文"

    def test_timeout_returns_partial_output(self, monkeypatch, real_run):
        e = subprocess.TimeoutExpired("cmd", 20, output=b"partial", stderr=b"err!")
        self._patch_run(monkeypatch, e)
        r = proc.run(["ping", "1.2.3.4"])
        assert (r.code, r.stdout, r.stderr, r.timeout) == (-1, "partial", "err!", True)
        assert r.ok is False

    def test_timeout_without_partial_output(self, monkeypatch, real_run):
        """e.stdout / e.stderr 为 None → 解码为空串。"""
        self._patch_run(monkeypatch,
                        subprocess.TimeoutExpired("cmd", 5, output=None, stderr=None))
        r = proc.run(["cmd"])
        assert (r.stdout, r.stderr) == ("", "")

    def test_file_not_found_list_cmd(self, monkeypatch, real_run):
        self._patch_run(monkeypatch, FileNotFoundError(2, "no such"))
        r = proc.run(["nosuchtool.exe", "-x"])
        assert (r.code, r.stdout, r.timeout) == (-1, "", False)
        assert r.stderr == "命令不存在: nosuchtool.exe"
        assert r.ok is False

    def test_file_not_found_str_cmd(self, monkeypatch, real_run):
        self._patch_run(monkeypatch, FileNotFoundError(2, "no such"))
        r = proc.run("nosuchtool -x")
        assert r.stderr == "命令不存在: nosuchtool -x"

    def test_other_oserror_bounded(self, monkeypatch, real_run):
        """PermissionError 等 OSError 也应转为失败 CmdResult，不向上抛。"""
        self._patch_run(monkeypatch, PermissionError(13, "denied"))
        r = proc.run(["blocked.exe"])
        assert r.code == -1 and not r.ok and not r.timeout
        assert "命令执行失败" in r.stderr and "blocked.exe" in r.stderr

    def test_returns_cmdresult(self, monkeypatch, real_run):
        self._patch_run(monkeypatch, (0, b"", b""))
        assert isinstance(proc.run(["x"]), proc.CmdResult)


# ---------------- _decode ----------------

class TestDecode:
    def test_empty_bytes(self):
        assert proc._decode(b"") == ""

    def test_utf8(self):
        assert proc._decode("中文".encode("utf-8")) == "中文"

    def test_invalid_utf8_never_raises(self):
        """非法 UTF-8 序列回退 mbcs(errors=replace)，保证任何字节串都不抛异常。"""
        out = proc._decode(b"\x80\x81\xff")
        assert isinstance(out, str) and out

    def test_pure_ascii(self):
        assert proc._decode(b"plain") == "plain"


# ---------------- run_ps ----------------

class TestRunPs:
    def test_builds_powershell_command(self, monkeypatch, real_run):
        calls = {}

        def fake_run(cmd, timeout=20):
            calls["cmd"] = cmd
            calls["timeout"] = timeout
            return proc.CmdResult(0, "ok", "")
        monkeypatch.setattr(proc, "run", fake_run)
        r = proc.run_ps("Get-NetRoute | Select -First 1", timeout=7)
        assert r.stdout == "ok"
        assert calls["cmd"] == ["powershell", "-NoProfile", "-NonInteractive",
                                "-ExecutionPolicy", "Bypass", "-Command",
                                "Get-NetRoute | Select -First 1"]
        assert calls["timeout"] == 7

    def test_default_timeout_40(self, monkeypatch, real_run):
        seen = {}
        monkeypatch.setattr(proc, "run",
                            lambda cmd, timeout=20: seen.setdefault("t", timeout))
        proc.run_ps("Get-X")
        assert seen["t"] == 40
