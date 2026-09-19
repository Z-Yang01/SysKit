"""入口与 stdio 兜底缺口补测（2026-09-16 总控）。

- syskit/__main__.py（0%）：runpy 以 run_name="__main__" 进程内执行包入口代码，
  走 run() 分流（badcmd → SystemExit 2；不启动 GUI、无系统副作用、不触哨兵）。
- syskit/main.py:27（if __name__ 守卫）：runpy.run_module("syskit.main") 直跑守卫体。
- syskit/__init__.py _fix_windows_stdio（17-31 无句柄重定向路径）：
  纯 mock 驱动（os.open/dup2/fdopen 全替换），不触碰真实文件描述符。
"""
import os
import runpy
import sys

import pytest


class TestPackageEntry:
    def test_python_m_syskit_unknown_command_exits_2(self, monkeypatch, capsys):
        """python -m syskit dianose → __main__.py 执行 run() → 明确报错 exit 2。"""
        monkeypatch.setattr(sys, "argv", ["syskit", "dianose"])
        with pytest.raises(SystemExit) as ei:
            runpy.run_module("syskit", run_name="__main__")
        assert ei.value.code == 2
        assert "未知命令" in capsys.readouterr().out

    def test_python_m_syskit_main_guard_runs(self, monkeypatch, capsys):
        """python -m syskit.main 直跑：命中 main.py 的 if __name__ == "__main__" 守卫。"""
        monkeypatch.setattr(sys, "argv", ["syskit.main", "dianose"])
        with pytest.raises(SystemExit) as ei:
            runpy.run_module("syskit.main", run_name="__main__")
        assert ei.value.code == 2
        assert "未知命令" in capsys.readouterr().out


class TestFixWindowsStdio:
    def _import(self):
        import syskit
        return syskit._fix_windows_stdio

    def test_returns_early_when_streams_have_fileno(self, monkeypatch):
        fn = self._import()
        called = []

        monkeypatch.setattr(os, "dup2", lambda *a, **k: called.append("dup2"))
        # 带 fileno 的真实 io 对象（pytest 捕获层对象具备 fileno）
        assert hasattr(sys.stdout, "fileno")
        fn()
        assert called == []

    def test_redirects_when_streams_lack_fileno(self, monkeypatch):
        """GUI/pythonw 场景（stdout 无 fileno）：重定向到 LOG_DIR/stdio.log。"""
        fn = self._import()

        calls = {"open": [], "dup2": [], "fdopen": []}

        class _NoFileno:
            def write(self, *a):
                pass

        monkeypatch.setattr(sys, "stdout", _NoFileno())
        monkeypatch.setattr(sys, "stderr", _NoFileno())

        from syskit import config
        log_dir = config.LOG_DIR  # conftest autouse 已重定向到 tmp_path
        assert log_dir

        monkeypatch.setattr(os, "makedirs", lambda *a, **k: None)

        def fake_open(path, flags, mode=0o644):
            calls["open"].append(path)
            return 99

        monkeypatch.setattr(os, "open", fake_open)
        monkeypatch.setattr(os, "dup2", lambda fd, target: calls["dup2"].append((fd, target)))
        monkeypatch.setattr(os, "close", lambda fd: calls.setdefault("close", []).append(fd))

        import io

        def fake_fdopen(fd, *a, **k):
            calls["fdopen"].append(fd)
            return io.StringIO()

        monkeypatch.setattr(os, "fdopen", fake_fdopen)

        fn()

        assert calls["open"] == [os.path.join(log_dir, "stdio.log")]
        assert calls["dup2"] == [(99, 1), (99, 2)]
        assert 99 in calls.get("close", [])
        assert calls["fdopen"] == [1, 2]
        assert sys.stdout is not None

    def test_swallows_redirect_failure(self, monkeypatch):
        """重定向过程任何异常静默吞掉（不影响包导入）。

        打桩必须真的 raise：`lambda: OSError(...)` 只是「返回」异常对象，护栏一步都
        没测到（这类永绿用例正是本轮要清的）。os.open 一并打桩记录，用来证明异常
        确实发生在 makedirs、后续落盘一步都没走。
        """
        fn = self._import()

        class _NoFileno:
            def write(self, *a):
                pass

        monkeypatch.setattr(sys, "stdout", _NoFileno())
        monkeypatch.setattr(sys, "stderr", _NoFileno())
        opened = []

        def _boom(*a, **k):
            raise OSError(28, "disk full")

        monkeypatch.setattr(os, "makedirs", _boom)
        monkeypatch.setattr(os, "open", lambda *a, **k: opened.append(a))

        fn()                                     # 磁盘满也不许冒到导入路径
        assert opened == []                      # makedirs 真的抛了，后面一步都没走
