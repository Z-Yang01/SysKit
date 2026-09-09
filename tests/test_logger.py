"""syskit/logger.py 单元测试：双文件 sink 配置、重复 setup 安全、过期日志清扫（全程写临时目录）。

Mock 边界（配合 conftest 双保险）：
- LOG_DIR 由 conftest.isolated_appdata 重定向到 pytest 临时目录，真实 %LOCALAPPDATA% 不被触碰；
- "过期文件"用 os.utime 构造 mtime，不等待真实时间流逝；
- sink 数量读 LOGGER._core.handlers（本机 loguru 版本 remove() 返回 None，不能用返回值计数），
  每个用例结束后统一 LOGGER.remove() 防止 sink 泄漏到其他测试。

疑似业务 bug（只记录，不改源码）：
（暂无。sweep_old_logs 对删除失败的单个文件只跳过不计数，属可接受降级。）
"""
import os
import time
from io import StringIO

import pytest

from syskit import config
from syskit.logger import LOGGER, setup, sweep_old_logs


@pytest.fixture(autouse=True)
def _cleanup_sinks():
    """用例结束后移除全部 sink，避免 enqueue 线程与 stderr sink 泄漏到其他测试。"""
    yield
    LOGGER.remove()


def _sink_count():
    return len(LOGGER._core.handlers)


def _make_old(path, days=30):
    t = time.time() - days * 86400
    os.utime(path, (t, t))


# ---------------- setup ----------------

class TestSetup:
    def test_returns_logger_and_adds_two_sinks(self):
        assert setup() is LOGGER
        assert _sink_count() == 2                 # syskit.log + error.log

    def test_verbose_console_adds_third_sink(self):
        setup(verbose_console=True)
        assert _sink_count() == 3

    def test_setup_idempotent_removes_old_sinks(self):
        setup()
        setup(verbose_console=True)
        assert _sink_count() == 3                 # 旧 sink 被移除后重加

    def test_setup_creates_log_dir(self):
        setup()
        assert os.path.isdir(config.LOG_DIR)

    def test_logs_split_by_level(self):
        """INFO 进 syskit.log，ERROR 双写 syskit.log + error.log。"""
        setup()
        LOGGER.info("hello-info")
        LOGGER.warning("warn-msg")
        LOGGER.error("boom-error")
        LOGGER.remove()                           # enqueue sink 在 remove 时排空队列
        with open(os.path.join(config.LOG_DIR, "syskit.log"), "r", encoding="utf-8") as f:
            normal = f.read()
        with open(os.path.join(config.LOG_DIR, "error.log"), "r", encoding="utf-8") as f:
            err = f.read()
        assert "hello-info" in normal and "warn-msg" in normal and "boom-error" in normal
        assert "boom-error" in err
        assert "hello-info" not in err and "warn-msg" not in err


# ---------------- sweep_old_logs ----------------

class TestSweepOldLogs:
    def test_missing_dir_returns_zero(self, monkeypatch):
        monkeypatch.setattr(config, "LOG_DIR", os.path.join(str(config.LOG_DIR), "no_such"))
        assert sweep_old_logs() == 0

    def test_empty_dir_returns_zero(self):
        os.makedirs(config.LOG_DIR, exist_ok=True)
        assert sweep_old_logs() == 0

    def test_removes_only_old_prefixed_files(self):
        os.makedirs(config.LOG_DIR, exist_ok=True)
        paths = {
            "syskit.log": True,                   # 过期 → 删
            "syskit.log.2026-01-01_00-00-00_000": True,   # 轮转备份 → 删
            "error.log": True,                    # 过期 → 删
            "error.log.1": True,                   # 轮转备份 → 删
            "stdio.log": True,                    # 过期 → 删
            "syskit.log.fresh": False,            # 未过期 → 留
            "keep.txt": True,                     # 前缀不符 → 留
        }
        for name, old in paths.items():
            p = os.path.join(config.LOG_DIR, name)
            with open(p, "w", encoding="utf-8") as f:
                f.write("x")
            if old:
                _make_old(p)
        # 同名目录不是文件 → 跳过
        os.makedirs(os.path.join(config.LOG_DIR, "error.log.dir"), exist_ok=True)

        removed = sweep_old_logs(days=7)

        assert removed == 5
        assert os.path.isfile(os.path.join(config.LOG_DIR, "syskit.log.fresh"))
        assert os.path.isfile(os.path.join(config.LOG_DIR, "keep.txt"))
        assert not os.path.exists(os.path.join(config.LOG_DIR, "syskit.log"))

    def test_days_zero_removes_even_fresh(self):
        """days=0 时保留期归零：默认策略下会保留的文件也被清掉。"""
        os.makedirs(config.LOG_DIR, exist_ok=True)
        p = os.path.join(config.LOG_DIR, "syskit.log")
        with open(p, "w", encoding="utf-8") as f:
            f.write("x")
        os.utime(p, (time.time() - 5, time.time() - 5))   # 比 time.time() 粒度更细，保证确定性
        assert sweep_old_logs() == 0                  # 默认 7 天内 → 保留
        assert sweep_old_logs(days=0) == 1            # 保留期归零 → 删除
        assert not os.path.exists(p)

    def test_remove_error_skips_file(self, monkeypatch):
        """单个文件删除失败（OSError）→ 跳过继续，不中断清扫。"""
        os.makedirs(config.LOG_DIR, exist_ok=True)
        p = os.path.join(config.LOG_DIR, "error.log")
        with open(p, "w", encoding="utf-8") as f:
            f.write("x")
        _make_old(p)

        import syskit.logger as logger_mod

        def flaky_remove(path, *a, **kw):
            raise OSError("mock in-use")
        monkeypatch.setattr(logger_mod.os, "remove", flaky_remove)
        assert sweep_old_logs() == 0
        monkeypatch.undo()                         # 恢复 os.remove 后确认文件确实还在
        assert os.path.isfile(p)

    def test_removal_is_logged(self):
        os.makedirs(config.LOG_DIR, exist_ok=True)
        p = os.path.join(config.LOG_DIR, "syskit.log")
        with open(p, "w", encoding="utf-8") as f:
            f.write("x")
        _make_old(p)
        buf = StringIO()
        LOGGER.add(buf, level="INFO")
        assert sweep_old_logs() == 1
        assert "日志清理" in buf.getvalue() and "1 个" in buf.getvalue()
