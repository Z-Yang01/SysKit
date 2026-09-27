"""真机冒烟脚手架回归锁（轮次29 新鲜眼审计，d4f129a 实锤修复配套）。

修复的实锤：tools/test_smoke_real.py 全部靶位靠 os.startfile 真实拉起
记事本/终端/资源管理器，而根 conftest 的 block_system_calls 哨兵曾对所有
用例无差别武装——文档化的唯一运行方式 `pytest tools/test_smoke_real.py
--run-real` 在第一个靶上必挂（探针实证：AssertionError「红线禁止」文案）。
修复三处：
① conftest._sentinels_armed_for：@pytest.mark.real 且带 --run-real 才豁免；
② conftest.pytest_addoption：注册 --run-real 选项——未注册时 pytest 在参数
   解析阶段 usage error（exit 4），文档化运行方式连收集都进不去（探针实证）；
③ 冒烟模块的 QT_QPA_PLATFORM pop 仅在 --run-real 时执行——集合期导入
   （pytest 收集 tools/ 即导入模块）不再污染整个会话的平台设置。
本文件钉死：豁免判定矩阵、无 --run-real 时 real 标记绝不减防、平台变量
按需 pop、12 条真机靶清点（8 靶 + 3 边界 + 1 触发语义用例删一个即红）。
"""
import importlib.util
import inspect
import os
import sys
import types

import pytest

_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
_SMOKE = os.path.join(_ROOT, "tools", "test_smoke_real.py")


def _load_smoke_module(name):
    """按路径加载 tools/test_smoke_real.py（不注册进 pytest 收集）。"""
    spec = importlib.util.spec_from_file_location(name, _SMOKE)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def _fake_request(markers, run_real=None):
    """模拟 FixtureRequest：node.get_closest_marker / config.getoption 与真身
    同语义（缺则 None / ValueError）。run_real=None 时 config 桩抛 ValueError，
    走 conftest 的 sys.argv 兜底分支。"""
    known = set(markers)

    def _marker(name):
        return object() if name in known else None

    def _getoption(name):
        if run_real is None:
            raise ValueError("unknown option (stub)")
        if name == "--run-real":
            return run_real
        raise ValueError(name)

    return types.SimpleNamespace(
        node=types.SimpleNamespace(get_closest_marker=_marker),
        config=types.SimpleNamespace(getoption=_getoption))


class TestSentinelGate:
    """conftest 豁免判定矩阵（修复①的单源逻辑）。"""

    def test_matrix(self, monkeypatch):
        import conftest
        # config.getoption 主路径：real 标记 + --run-real → 唯一豁免组合
        assert conftest._sentinels_armed_for(
            _fake_request({"real"}, run_real=True)) is False
        # 无 real 标记：即便带 --run-real 也武装（tests/ 用例绝不豁免）
        assert conftest._sentinels_armed_for(
            _fake_request(set(), run_real=True)) is True
        # real 标记但 --run-real 未给：武装（双保险，默认收集/门禁路径）
        assert conftest._sentinels_armed_for(
            _fake_request({"real"}, run_real=False)) is True
        # getoption 不可用时回退 sys.argv 扫描（同矩阵）
        monkeypatch.setattr(sys, "argv",
                            ["pytest", "tools/test_smoke_real.py", "--run-real"])
        assert conftest._sentinels_armed_for(_fake_request({"real"})) is False
        monkeypatch.setattr(sys, "argv", ["pytest", "tools/test_smoke_real.py"])
        assert conftest._sentinels_armed_for(_fake_request({"real"})) is True

    def test_run_real_option_registered(self):
        """--run-real 必须是已注册的 pytest 选项：未注册时 pytest 在参数解析
        阶段 usage error 退出（exit 4），冒烟脚本连收集都进不去（审计实锤）。"""
        import conftest
        opts = []
        parser = types.SimpleNamespace(addoption=lambda *a, **k: opts.append((a, k)))
        conftest.pytest_addoption(parser)
        assert any("--run-real" == a[0] for a, _ in opts), "--run-real 选项丢失"

    def test_real_marked_without_run_real_keeps_sentinel(self):
        """红线不变量：无 --run-real 时 real 标记不减防——本用例即 real 标记，
        哨兵必须在位（os.startfile 必须被击落而非真实执行）。"""
        try:
            os.startfile("__no_such_syskit_probe__.xyz")
        except AssertionError:
            return                          # 哨兵在位（期望）
        except OSError:
            pytest.fail("哨兵对 real 标记用例未武装（--run-real 豁免泄漏到默认路径）")
        pytest.fail("os.startfile 未抛任何异常——哨兵未武装且文件竟存在？")


class TestSmokeModuleImportSafety:
    """修复②：模块导入的平台变量行为按 RUN_REAL 分叉。"""

    def test_import_without_run_real_keeps_offscreen(self, monkeypatch):
        monkeypatch.setattr(sys, "argv", ["pytest", "tools/test_smoke_real.py"])
        mod = self._reload("syskit_smoke_audit_skip")
        assert mod.RUN_REAL is False
        assert os.environ.get("QT_QPA_PLATFORM") == "offscreen"   # 不被 pop

    def test_import_with_run_real_pops_platform(self, monkeypatch):
        monkeypatch.setattr(sys, "argv", ["pytest", "--run-real"])
        self._reload("syskit_smoke_audit_real")
        assert os.environ.get("QT_QPA_PLATFORM") is None          # 真机跑需真实平台
        # monkeypatch 拆卸时恢复 offscreen，不影响后续用例

    @staticmethod
    def _reload(name):
        return _load_smoke_module(name)


def test_smoke_target_inventory_lock():
    """12 条真机靶清点：8 靶 + 3 边界 + 1 触发语义用例（离散三按粘 3 次）。
    删靶/新增靶都应回来同步本锁，防止靶位悄悄缩水。"""
    mod = _load_smoke_module("syskit_smoke_audit_inventory")
    n = 0
    for _, cls in inspect.getmembers(mod, inspect.isclass):
        if cls.__module__ != mod.__name__ or not cls.__name__.startswith("Test"):
            continue
        n += sum(1 for m in vars(cls).values()
                 if callable(m) and getattr(m, "__name__", "").startswith("test"))
    n += len([k for k in vars(mod) if k.startswith("test")])      # 模块级靶位
    assert n == 12, f"真机靶位数量漂移：{n}（预期 12）"
