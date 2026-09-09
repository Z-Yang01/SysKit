"""subprocess 封装：隐藏窗口、GBK 解码、超时保护。"""
import subprocess
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


def _decode(b: bytes) -> str:
    if not b:
        return ""
    try:
        return b.decode("utf-8")
    except UnicodeDecodeError:
        return b.decode("mbcs", errors="replace")


def run_ps(script: str, timeout: int = 40) -> CmdResult:
    """PowerShell 5.1 执行脚本。ConvertTo-Json 会把非 ASCII 转义成 \\uXXXX，输出可安全按 ASCII 处理。"""
    cmd = ["powershell", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script]
    return run(cmd, timeout=timeout)
