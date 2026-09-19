"""静态门：把 README 里手敲的 `python -m pyflakes syskit tools conftest.py` 钉成用例。

为什么值得钉：本轮多批 bug 修复里，死 import、未定义名、重复形参这类问题都是靠
人肉跑 pyflakes 发现的；一旦忘了跑，回归就静默进主干。用例直接调 pyflakes API
而非 subprocess——conftest 的红线 1 哨兵会拦 `subprocess.*`。

扫描范围只含产品与工具代码（`syskit/`、`tools/`、根 `conftest.py`），**不含
`tests/`**：用例里为触发 fixture 与模块级 mock 而做的「imported but unused」是
有意为之，纳入只会产生上百条非缺陷噪声，反而把真问题埋掉。
"""
import os

import pytest

pyflakes_api = pytest.importorskip("pyflakes.api", reason="pyflakes 为 dev 依赖，未安装则跳过")

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TARGETS = ("syskit", "tools", "conftest.py")


def _py_files():
    for target in TARGETS:
        path = os.path.join(ROOT, target)
        if os.path.isfile(path):
            yield path
            continue
        for dirpath, dirnames, names in os.walk(path):
            dirnames[:] = [d for d in dirnames if d != "__pycache__"]
            for name in sorted(names):
                if name.endswith(".py"):
                    yield os.path.join(dirpath, name)


class _Collector:
    """鸭子类型的 pyflakes reporter：只收消息，不写 stdout。"""

    def __init__(self):
        self.lines = []

    def flake(self, message):
        # pyflakes 的 Message.__str__ 已是 "文件:行:列: 说明"，且列号 0 基 → +1
        self.lines.append(str(message))

    def syntaxError(self, filename, msg, lineno, offset, text):
        self.lines.append(f"{filename}:{lineno}: 语法错误 {msg}")

    def unexpectedError(self, filename, msg):
        self.lines.append(f"{filename}: 无法解析（{msg}）")


def test_pyflakes_gate_is_clean():
    files = sorted(_py_files())
    assert len(files) > 50, f"扫描文件数异常偏少（{len(files)}），路径配置可能失效"
    rep = _Collector()
    for f in files:
        with open(f, "rb") as fh:
            rel = os.path.relpath(f, ROOT).replace("\\", "/")
            pyflakes_api.check(fh.read(), rel, rep)
    assert not rep.lines, "pyflakes 报告如下问题：\n" + "\n".join(rep.lines)
