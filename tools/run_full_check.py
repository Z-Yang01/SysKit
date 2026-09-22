"""两段式全量验证工具：SEG1（除 ui_smoke）+ SEG2（ui_smoke）+ pyflakes 门，一条命令出结论。

为什么两段（README 既有约定）：test_ui_smoke.py 在 coverage/并发等高负载下
可复现 PySide6 离屏原生 teardown 崩溃；且该崩溃发生在全部用例之后、会吞掉 pytest 的
末尾汇总行——所以计数一律读 junitxml，不信 stdout 摘要。

用法：
    python tools/run_full_check.py              # 全量（本机约 1.5 分钟）
退出码：0 = 两段 0F/0E 且 pyflakes 0；1 = 任一不过（打印失败段末尾输出）。

注意：本工具是脚手架，不进单测——conftest 红线哨兵会拦测试里的 subprocess.*，
对它的验证就是每晚真实跑它（与 ui_preview.py 同一口径）。
"""
import subprocess
import sys
import time
import xml.etree.ElementTree as ET
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / ".workbuddy"
TAIL_ON_FAIL = 50


def _run(argv: list, timeout: int = 420):
    t0 = time.perf_counter()
    p = subprocess.run(argv, cwd=ROOT, capture_output=True,
                       text=True, encoding="utf-8", errors="replace", timeout=timeout)
    return p, time.perf_counter() - t0


def _tail(text: str, n: int = TAIL_ON_FAIL) -> str:
    lines = (text or "").strip().splitlines()
    return "\n".join(lines[-n:])


def _counts(xml_path: Path) -> dict:
    root = ET.parse(xml_path).getroot()
    s = root.find("testsuite") if root.tag == "testsuites" else root
    return {k: int(s.get(k) or 0) for k in ("tests", "failures", "errors", "skipped")}


def _segment(name: str, pytest_args: list) -> tuple:
    xml = OUT / f"fullcheck_{name}.xml"
    argv = [sys.executable, "-m", "pytest", *pytest_args,
            "--timeout=120", "-q", f"--junitxml={xml}"]
    p, dt = _run(argv)
    try:
        c = _counts(xml)
    except OSError:
        c = None   # xml 没生成 = 段中途崩（原生崩溃族），按失败处理
    print(f"[{name}] {dt:.1f}s  exit={p.returncode}  "
          f"counts={c if c else 'xml 缺失（段中途崩溃）'}")
    if c is None or p.returncode != 0 or c["failures"] or c["errors"]:
        print(f"---- {name} stdout 末尾 ----\n{_tail(p.stdout)}")
        if p.stderr.strip():
            print(f"---- {name} stderr 末尾 ----\n{_tail(p.stderr)}")
        return False
    return True


def main() -> int:
    OUT.mkdir(exist_ok=True)
    ok = True
    ok &= _segment("seg1", ["tests/", "--ignore=tests/test_ui_smoke.py"])
    ok &= _segment("seg2", ["tests/test_ui_smoke.py"])

    p, dt = _run([sys.executable, "-m", "pyflakes", "syskit", "tools", "conftest.py"])
    print(f"[pyflakes] {dt:.1f}s  exit={p.returncode}"
          + ("" if p.returncode == 0 else f"\n{_tail((p.stdout or '') + (p.stderr or ''), 20)}"))
    ok &= p.returncode == 0

    if ok:
        print("FULL-CHECK PASS：两段 0F/0E，pyflakes 0")
        return 0
    print("FULL-CHECK FAIL")
    return 1


if __name__ == "__main__":
    sys.exit(main())
