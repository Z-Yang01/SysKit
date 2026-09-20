"""subprocess 封装：隐藏窗口、GBK 解码、超时保护、常驻 PowerShell 会话。"""
import base64
import os
import queue
import re
import subprocess
import threading
import time
from dataclasses import dataclass

CREATE_NO_WINDOW = 0x08000000


@dataclass
class CmdResult:
    code: int
    stdout: str
    stderr: str
    timeout: bool = False

    @property
    def ok(self) -> bool:
        return self.code == 0 and not self.timeout


def run(cmd, timeout: int = 20) -> CmdResult:
    """执行命令并捕获输出。cmd 为 list（推荐）或 str。
    解码自适应：部分系统 netsh 输出 UTF-8，部分 GBK——先试 UTF-8 再回退 ANSI。"""
    try:
        p = subprocess.run(
            cmd,
            capture_output=True,
            timeout=timeout,
            creationflags=CREATE_NO_WINDOW if hasattr(subprocess, "CREATE_NO_WINDOW") else 0,
            shell=isinstance(cmd, str),
        )
        out = _decode(p.stdout)
        err = _decode(p.stderr)
        return CmdResult(p.returncode, out, err)
    except subprocess.TimeoutExpired as e:
        out = _decode(e.stdout or b"")
        err = _decode(e.stderr or b"")
        return CmdResult(-1, out, err, timeout=True)
    except FileNotFoundError:
        return CmdResult(-1, "", f"命令不存在: {cmd if isinstance(cmd, str) else cmd[0]}")
    except OSError as e:
        # 权限拒绝/映像损坏等启动失败也不应炸穿调用方
        name = cmd if isinstance(cmd, str) else cmd[0]
        return CmdResult(-1, "", f"命令执行失败({name}): {e}")


def _decode(b: bytes) -> str:
    if not b:
        return ""
    try:
        return b.decode("utf-8")
    except UnicodeDecodeError:
        return b.decode("mbcs", errors="replace")


def run_ps(script: str, timeout: int = 40, resident: bool = False) -> CmdResult:
    """PowerShell 5.1 执行脚本。ConvertTo-Json 会把非 ASCII 转义成 \\uXXXX，输出可安全按 ASCII 处理。

    resident=True 时走常驻会话（免 powershell.exe 冷启动与 cmdlet 暖机，
    只读脚本的采集从秒级降到百毫秒级）。opt-in 显式声明：写类/长类脚本
    保持一次性进程隔离。任何常驻层异常都回退到一次性路径，本函数
    永不抛异常的契约不变。
    """
    if (resident
            and os.environ.get("SYSKIT_NO_RESIDENT_PS") != "1"
            and not _res_permanent_off
            and not _RESIDENT_DENY_RE.search(script)):
        r = _resident_run(script, timeout)
        if r is not None:
            return r
    cmd = ["powershell", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script]
    return run(cmd, timeout=timeout)


# ---------------- 常驻 PowerShell 会话（补56） ----------------
#
# 形态：懒 spawn 一个 powershell.exe，bootstrap 循环从 stdin 读一行 base64
#（规避换行/注入），解码后 try{Invoke-Expression}catch{}——异常杀不死会话。
# 脚本 stdout 原样直通（解析器零风险）；每条脚本执行完向 stdout 写 OUT-END
# 行、向 stderr 写 DONE:{0|1} 行作为响应边界。父进程退出 → stdin EOF →
# 子进程自然退出。
#
# 语义与一次性路径逐条对齐：CmdResult 四字段、超时带回部分输出、_decode、
# 永不抛。会话死亡重试一次新会话，再失败回退一次性；spawn 失败永久回退；
# SYSKIT_NO_RESIDENT_PS=1 整体停用。

_RESIDENT_BOOTSTRAP = (
    "$ErrorActionPreference='Continue';"
    "[Console]::OutputEncoding=[System.Text.Encoding]::UTF8;"
    "while($true){"
    "$l=[Console]::In.ReadLine();"
    "if($null -eq $l){break};"
    "$f=0;"
    "try{Invoke-Expression ([System.Text.Encoding]::UTF8.GetString"
    "([Convert]::FromBase64String($l)));"
    "[Console]::Out.WriteLine('<<SYSKIT:OUT-END>>');"
    "[Console]::Error.WriteLine('<<SYSKIT:DONE:0>>')}"
    "catch{[Console]::Out.WriteLine('<<SYSKIT:OUT-END>>');"
    "[Console]::Error.WriteLine($_.Exception.Message);"
    "[Console]::Error.WriteLine('<<SYSKIT:DONE:1>>')}}"
)
_RES_OUT_END = b"<<SYSKIT:OUT-END>>"
_RES_DONE_PREFIX = b"<<SYSKIT:DONE:"
# 脚本里的 exit 会终止整个常驻进程（appnet 的 status 脚本就有 exit 0）——
# 命中即拒绝常驻，回退一次性
_RESIDENT_DENY_RE = re.compile(r"(?m)^\s*exit\b")

_RES_LOCK = threading.Lock()
_res_session = None
_res_permanent_off = False


def _spawn_resident():
    """创建常驻 PS 进程的 transport seam（测试 monkeypatch 此函数注入假进程）。"""
    return subprocess.Popen(
        ["powershell", "-NoProfile", "-NonInteractive", "-ExecutionPolicy",
         "Bypass", "-Command", _RESIDENT_BOOTSTRAP],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        creationflags=CREATE_NO_WINDOW if hasattr(subprocess, "CREATE_NO_WINDOW") else 0)


class _ResidentSession:
    """单条 PS 会话：双泵线程把 stdout/stderr 行喂进队列，exec 阻塞到双边界。"""

    def __init__(self, popen):
        self.p = popen
        self.dead = False
        self.q = queue.Queue()
        for pipe, tag in ((popen.stdout, "o"), (popen.stderr, "e")):
            threading.Thread(target=self._pump, args=(pipe, tag), daemon=True).start()

    def _pump(self, pipe, tag):
        try:
            for line in iter(pipe.readline, b""):
                if line:
                    self.q.put((tag, line))
        except (OSError, ValueError):
            pass
        finally:
            self.q.put((tag, None))          # EOF 哨兵

    def exec_script(self, b64line: str, timeout: int):
        """返回 (kind, stdout, stderr, fail)。kind: OK / TIMEOUT / DEAD。"""
        try:
            self.p.stdin.write(b64line.encode("ascii") + b"\n")
            self.p.stdin.flush()
        except (OSError, ValueError):
            return "DEAD", b"", b"", None
        deadline = time.monotonic() + timeout
        out, err = bytearray(), bytearray()
        got_out = got_done = False
        fail = "0"
        while not (got_out and got_done):
            remain = deadline - time.monotonic()
            if remain <= 0:
                return "TIMEOUT", bytes(out), bytes(err), None
            try:
                tag, item = self.q.get(timeout=remain)
            except queue.Empty:
                return "TIMEOUT", bytes(out), bytes(err), None
            if item is None:
                # 管道 EOF：OUT-END/DONE 已见后的 EOF 是健康收尾（脚本输出完
                # 即关流），直接忽略；边界标记未见到就 EOF = 进程已死 → DEAD
                if (tag == "o") == got_out:
                    continue
                return "DEAD", bytes(out), bytes(err), None
            if tag == "o":
                if item.strip() == _RES_OUT_END:
                    got_out = True
                else:
                    out.extend(item)
            elif item.startswith(_RES_DONE_PREFIX):
                got_done = True
                digits = item.strip()[len(_RES_DONE_PREFIX):].rstrip(b">>")
                fail = digits.decode("ascii", "replace") or "0"
            else:
                err.extend(item)
        return "OK", bytes(out), bytes(err), fail

    def close(self, kill: bool = False):
        self.dead = True
        try:
            if kill:
                self.p.kill()
            else:
                try:
                    self.p.stdin.close()     # EOF → 子进程自然退出
                except (OSError, ValueError):
                    self.p.kill()
        except (OSError, ValueError):
            pass


def _get_resident_session():
    """取活会话；需要时新开。spawn 失败置永久回退并返回 None。"""
    global _res_session, _res_permanent_off
    if _res_session is None or _res_session.dead:
        try:
            _res_session = _ResidentSession(_spawn_resident())
        except Exception:
            _res_permanent_off = True
            return None
    return _res_session


def _resident_run(script: str, timeout: int):
    """常驻执行。返回 CmdResult；返回 None = 回退一次性路径。"""
    global _res_session
    b64 = base64.b64encode(script.encode("utf-8")).decode("ascii")
    with _RES_LOCK:
        for _attempt in (0, 1):
            sess = _get_resident_session()
            if sess is None:
                return None
            kind, out, err, fail = sess.exec_script(b64, timeout)
            if kind == "OK":
                return CmdResult(1 if fail == "1" else 0, _decode(out), _decode(err))
            if kind == "TIMEOUT":
                # 与一次性语义对齐：kill + 废弃会话 + 带回部分输出
                sess.close(kill=True)
                _res_session = None
                return CmdResult(-1, _decode(out), _decode(err), timeout=True)
            # DEAD：会话死亡（EOF/管道断）。废弃后换新会话再试一次，
            # 仍死则回退一次性——不给调用方任何常驻层痕迹
            sess.close(kill=True)
            _res_session = None
    return None


def close_resident():
    """显式关闭常驻会话（quit_app 收尾用；父进程退出时 stdin EOF 也会自然关闭）。"""
    global _res_session
    with _RES_LOCK:
        sess, _res_session = _res_session, None
        if sess is not None:
            sess.close()
