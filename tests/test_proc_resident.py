"""syskit/proc.py 常驻 PowerShell 会话测试（全部注入假 transport，零真实进程）。

接缝：proc._spawn_resident 模块级函数被 monkeypatch 成假 spawn——pump 线程
读的是 FakePipe，subprocess.Popen 永不被触（conftest 哨兵零接触）。
一次性回退路径打桩 proc.subprocess.run（test_proc 既有模式）。
"""
import threading
import time

import pytest

from syskit import proc

# conftest 哨兵会把 proc.run_ps 模块属性换成 AssertionError 桩——收集期
# （import 时）先抓真实函数，各用例直接调真实入口（test_proc 既有模式）。
REAL_RUN_PS = proc.run_ps


class FakePipe:
    """readline 先吐预置行，耗尽后：never=True 永久阻塞（测超时），否则 EOF。"""

    def __init__(self, lines=(), never=False):
        self._lines = list(lines)
        self._never = never

    def readline(self):
        if self._lines:
            return self._lines.pop(0)
        if self._never:
            threading.Event().wait()        # daemon 线程：测试结束即消亡
            return b""
        return b""


class FakeStdin:
    def __init__(self, fail_on_use=False):
        self.data = b""
        self.closed = False
        self._fail = fail_on_use

    def write(self, b):
        if self._fail:
            raise OSError("broken pipe")
        self.data += b

    def flush(self):
        pass

    def close(self):
        if self._fail:
            raise OSError("broken pipe")
        self.closed = True


class FakePopen:
    def __init__(self, out=(), err=(), never=False, stdin_fail=False):
        self.stdin = FakeStdin(fail_on_use=stdin_fail)
        self.stdout = FakePipe(out, never=never)
        self.stderr = FakePipe(err, never=never)
        self.killed = False

    def kill(self):
        self.killed = True


OUT_END = proc._RES_OUT_END + b"\n"
DONE0 = proc._RES_DONE_PREFIX + b"0>>\n"
DONE1 = proc._RES_DONE_PREFIX + b"1>>\n"


@pytest.fixture(autouse=True)
def _reset_resident_state():
    """模块级会话/永久回退标志逐例复位，防跨用例泄漏。"""
    proc._res_session = None
    proc._res_permanent_off = False
    yield
    proc._res_session = None
    proc._res_permanent_off = False


@pytest.fixture
def oneshot_stub(monkeypatch):
    """一次性执行器打桩：run_ps 回退到 run() 时返回 stdout="oneshot" 的假结果。"""
    calls = []

    def fake_run(cmd, timeout=20):
        calls.append(cmd)
        return proc.CmdResult(0, "oneshot", "")

    monkeypatch.setattr(proc, "run", fake_run)
    return calls


# ---------------- 正常路径 ----------------

def test_happy_path_returns_stdout_code0(monkeypatch):
    monkeypatch.setattr(proc, "_spawn_resident",
                        lambda: FakePopen(out=[b"hello\n", OUT_END], err=[DONE0]))
    r = REAL_RUN_PS("Get-Date", timeout=5, resident=True)
    assert r.code == 0 and not r.timeout
    assert r.stdout == "hello\n"


def test_ps_error_maps_to_code1(monkeypatch):
    """catch 分支：异常消息进 stderr、DONE:1 → code=1（调用方按 r.ok 判定）。"""
    monkeypatch.setattr(proc, "_spawn_resident",
                        lambda: FakePopen(out=[OUT_END],
                                          err=[b"boom\n", DONE1]))
    r = REAL_RUN_PS("1/0", timeout=5, resident=True)
    assert r.code == 1 and "boom" in r.stderr and not r.ok


def test_multiline_stdout_preserved(monkeypatch):
    """多行脚本输出（appnet 式哨兵+多行）逐行保真。"""
    monkeypatch.setattr(proc, "_spawn_resident",
                        lambda: FakePopen(out=[b"SysKit-AppNet-PS-Available\n",
                                               b"rule1=True=Block\n", OUT_END],
                                          err=[DONE0]))
    r = REAL_RUN_PS("x", timeout=5, resident=True)
    assert r.stdout == "SysKit-AppNet-PS-Available\nrule1=True=Block\n"


# ---------------- 超时与会话死亡 ----------------

def test_timeout_kills_session_returns_partial(monkeypatch):
    """超时 = kill + 废弃会话 + timeout=True + 带回部分输出（与一次性语义对齐）。"""
    spawns = []
    mk = FakePopen(out=[b"partial\n"], err=[], never=True)
    spawns.append(mk)
    monkeypatch.setattr(proc, "_spawn_resident", lambda: spawns.append(FakePopen(never=True)) or spawns[0])
    r = REAL_RUN_PS("挂死脚本", timeout=0.15, resident=True)
    assert r.timeout is True and "partial" in r.stdout
    assert spawns[0].killed is True
    assert proc._res_session is None                # 会话已废弃


def test_dead_session_retries_once_with_fresh(monkeypatch):
    """会话中途死亡（EOF 无标记）→ 废弃换新会话重试一次 → 新会话健康则成功。"""
    dead = FakePopen(out=[b"partial\n"], err=[b""])   # 双 EOF，无任何标记
    healthy = FakePopen(out=[b"fine\n", OUT_END], err=[DONE0])
    sessions = iter([dead, healthy])
    monkeypatch.setattr(proc, "_spawn_resident", lambda: next(sessions))
    r = REAL_RUN_PS("x", timeout=5, resident=True)
    assert r.code == 0 and "fine" in r.stdout
    assert dead.killed is True


def test_double_dead_falls_back_to_oneshot(monkeypatch, oneshot_stub):
    s1 = FakePopen(out=[b""], err=[b""])
    s2 = FakePopen(out=[b""], err=[b""])
    sessions = iter([s1, s2])
    monkeypatch.setattr(proc, "_spawn_resident", lambda: next(sessions))
    r = REAL_RUN_PS("x", timeout=5, resident=True)
    assert r.stdout == "oneshot"                    # 调用方无感：拿到的仍是一次性结果
    assert s1.killed and s2.killed


# ---------------- 回退开关 ----------------

def test_spawn_failure_disables_resident_permanently(monkeypatch, oneshot_stub):
    state = {"raise": True}

    def flaky_spawn():
        if state["raise"]:
            raise RuntimeError("powershell 不存在")
        return FakePopen(out=[OUT_END], err=[DONE0])
    monkeypatch.setattr(proc, "_spawn_resident", flaky_spawn)

    r1 = REAL_RUN_PS("x", timeout=5, resident=True)
    assert r1.stdout == "oneshot"                   # spawn 失败 → 本次回退一次性
    assert proc._res_permanent_off is True
    state["raise"] = False                          # 修好了也不再试：永久回退
    r2 = REAL_RUN_PS("x", timeout=5, resident=True)
    assert r2.stdout == "oneshot"


def test_env_kill_switch(monkeypatch, oneshot_stub):
    monkeypatch.setenv("SYSKIT_NO_RESIDENT_PS", "1")
    spawns = []
    monkeypatch.setattr(proc, "_spawn_resident",
                        lambda: spawns.append(1) or FakePopen(out=[OUT_END], err=[DONE0]))
    r = REAL_RUN_PS("x", timeout=5, resident=True)
    assert r.stdout == "oneshot" and spawns == []   # 完全不 spawn


def test_exit_statement_denied_resident(monkeypatch, oneshot_stub):
    """脚本含行首 exit → 拒绝常驻（exit 会杀死整个会话进程）。"""
    spawns = []
    monkeypatch.setattr(proc, "_spawn_resident",
                        lambda: spawns.append(1) or FakePopen(out=[OUT_END], err=[DONE0]))
    REAL_RUN_PS("appnet 式脚本\nexit 0", timeout=5, resident=True)
    assert spawns == []


def test_resident_false_never_spawns(monkeypatch, oneshot_stub):
    monkeypatch.setattr(proc, "_spawn_resident",
                        lambda: (_ for _ in ()).throw(AssertionError("不应 spawn")))
    r = REAL_RUN_PS("x", timeout=5)
    assert r.stdout == "oneshot"


# ---------------- 串行化 ----------------

def test_concurrent_calls_serialized_by_lock(monkeypatch):
    timeline = []

    class FakeSess:
        dead = False

        def exec_script(self, b64, timeout):
            timeline.append("in")
            time.sleep(0.05)
            timeline.append("out")
            return "OK", b"", b"", "0"

        def close(self, kill=False):
            pass

    monkeypatch.setattr(proc, "_spawn_resident", lambda: object())
    monkeypatch.setattr(proc, "_ResidentSession", lambda p: FakeSess())
    ts = [threading.Thread(target=lambda: REAL_RUN_PS("x", timeout=5, resident=True))
          for _ in range(2)]
    for t in ts:
        t.start()
    for t in ts:
        t.join()
    assert timeline == ["in", "out", "in", "out"]   # 锁内串行：无交错


# ---------------- 会话原语分支补齐（2026-09-22，覆盖率缺口） ----------------

def test_exec_dead_when_stdin_write_fails():
    """stdin 写失败（管道断）= 会话已死 → DEAD（不炸穿调用方）。"""
    s = proc._ResidentSession(FakePopen(stdin_fail=True))
    assert s.exec_script("aGk=", 5) == ("DEAD", b"", b"", None)


def test_exec_timeout_deadline_branch_returns_partial_before_queue_wait():
    """deadline 已到（remain<=0）→ 立即 TIMEOUT，不进队列等待。"""
    s = proc._ResidentSession(FakePopen(out=[b"p\n"], err=[]))
    kind, out, err, _ = s.exec_script("aGk=", 0)
    assert (kind, out, err) == ("TIMEOUT", b"", b"")


def test_close_graceful_closes_stdin_so_child_exits_on_eof():
    fake = FakePopen()
    s = proc._ResidentSession(fake)
    s.close()
    assert s.dead and fake.stdin.closed and not fake.killed


def test_close_graceful_stdin_failure_falls_back_to_kill():
    fake = FakePopen(stdin_fail=True)
    s = proc._ResidentSession(fake)
    s.close()
    assert fake.killed


def test_close_resident_closes_and_clears_live_session():
    fake = FakePopen(out=[OUT_END], err=[DONE0])
    s = proc._ResidentSession(fake)
    proc._res_session = s
    proc.close_resident()
    assert proc._res_session is None
    assert fake.stdin.closed and s.dead
