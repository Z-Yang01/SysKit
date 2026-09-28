"""无人值守轮次30 · 方向A（逻辑层 bug 猎手）回归锁。

两个实锤修复的锁（变异验证见各自用例 docstring，红→复绿均已在 6f214a4 上演过）：
- admin.elevate_and_restart：提权重启参数从 `" ".join` 改为 list2cmdline 逐参加引号
  ——argv 里含空格的参数（如 --open "C:\\Program Files\\a.txt"）旧实现会被新进程
  拆成两个参数；同时修正 `cli_args or argv` 把显式 [] 偷换成「继承当前 argv」的
  契约混同；删掉 frozen 分支重复计算 params 的死行。
- modes.set_mute 取消静音：恢复记录的「可用性」同时约束音量值与提示语——
  0（记录的本来就是静音态）/ bool（JSON 手改 true，isinstance(True,int) 恒真）/
  非 int / 越界一律按「未找到，设为最大」如实提示；旧实现 `0 <= prev` 含 0 且
  提示只看 `prev is not None`，脏记录会恢复成 vol=0（仍无声）或最大音量却谎报
  「已恢复原音量」。

另锁静音分支此前零覆盖的「先存后静音」契约（settings.save 的确切落参、
读数为 0 不得覆盖更早记录）。

核查干净（本轮取证，无改动）：appnet.py（增删查/回查链有 tests/appnet_fake.py
有状态假防火墙端到端锁，规则名 ASCII 哈希单源）、wifi.py（80MHz 块边界/DFS
52-144/165 不属块/信号钳位已有轮次28 分支锁）、autostart.py（/TR 引号包裹、
重复 /F 覆盖、disable 双删语义均有既有锁）。

红线：所有系统调用显式 mock，不触发真实 UAC / 音量改动 / 注册表写入。
"""
import ctypes
import sys

import pytest

from syskit import admin, modes
from syskit.proc import CmdResult


# ---------------- 局部 helper ----------------

def _patch_shexec(monkeypatch):
    """拦截 ShellExecuteW（返回 33 > 32 = 成功），记录完整 6 参
    (hwnd, verb, exe, params, directory, show)——params 固定在下标 3。"""
    calls = []

    def _fake(*args):
        calls.append(args)
        return 33

    monkeypatch.setattr(ctypes.windll.shell32, "ShellExecuteW", _fake, raising=False)
    return calls


def _params_of(calls):
    return calls[0][3]


def _fake_ps_get_first(get_volume_output):
    """run_ps 假体：waveOutGetVolume 脚本返回给定读数，其余（SetVolume）成功。"""
    scripts = []

    def _fake(script, timeout=40):
        scripts.append(script)
        if "waveOutGetVolume" in script:
            return CmdResult(0, get_volume_output, "")
        return CmdResult(0, "", "")

    return scripts, _fake


# ---------------- admin.elevate_and_restart：参数引号与 [] 契约 ----------------

class TestElevateParamsQuoting:
    def test_frozen_quotes_args_with_spaces(self, monkeypatch):
        """变异验证：回退成 " ".join → params 变 '--open C:\\Program Files\\a b.txt'，
        新进程把路径拆成两个参数，本用例红。"""
        monkeypatch.setattr(sys, "frozen", True, raising=False)
        calls = _patch_shexec(monkeypatch)
        assert admin.elevate_and_restart(["--open", r"C:\Program Files\a b.txt"]) is True
        _, verb, _exe, params, _dir, show = calls[0]
        assert verb == "runas"
        assert params == r'--open "C:\Program Files\a b.txt"'
        assert show == admin.SW_SHOWNORMAL

    def test_source_mode_quotes_args_with_spaces(self, monkeypatch):
        """源码模式同样要过引号：-m syskit 后的路径参数必须整体保真。"""
        monkeypatch.delattr(sys, "frozen", raising=False)
        monkeypatch.setattr(sys, "argv",
                            ["syskit", "--open", r"C:\Program Files\a b.txt"],
                            raising=False)
        calls = _patch_shexec(monkeypatch)
        admin.elevate_and_restart()
        assert _params_of(calls) == r'-m syskit --open "C:\Program Files\a b.txt"'

    def test_explicit_empty_args_do_not_inherit_argv(self, monkeypatch):
        """变异验证：回退成 `cli_args or argv` → [] 被偷换成继承 ['--stray']，
        params 变 '--stray'，本用例红。显式 [] 就是不带参数重启。"""
        monkeypatch.setattr(sys, "frozen", True, raising=False)
        monkeypatch.setattr(sys, "argv", ["syskit", "--stray"], raising=False)
        calls = _patch_shexec(monkeypatch)
        assert admin.elevate_and_restart([]) is True
        assert _params_of(calls) == ""

    def test_none_still_inherits_argv_frozen(self, monkeypatch):
        """锁旧契约不回退：None 才继承当前进程 argv。"""
        monkeypatch.setattr(sys, "frozen", True, raising=False)
        monkeypatch.setattr(sys, "argv", ["syskit", "--tray", "--minimized"],
                            raising=False)
        calls = _patch_shexec(monkeypatch)
        assert admin.elevate_and_restart() is True
        assert _params_of(calls) == "--tray --minimized"


# ---------------- modes.set_mute：先存后静音（此前零覆盖的保存链） ----------------

class TestMuteSavesPreviousVolume:
    def test_save_called_with_exact_reading_before_muting(self, monkeypatch):
        """变异验证：删掉 `settings.save({"modes_prev_volume": prev})` → 本用例红。
        锁「静音前先读并保存原音量」这条链的确切落参。"""
        scripts, fake = _fake_ps_get_first("1073741823\n")   # ≈25%
        monkeypatch.setattr(modes.proc, "run_ps", fake)
        saved = []
        monkeypatch.setattr("syskit.settings.save",
                            lambda patch: saved.append(dict(patch)))
        r = modes.set_mute(True)
        assert r["ok"] and "已静音" in r["detail"]
        assert saved == [{"modes_prev_volume": 1073741823}]
        assert any("[uint32]0" in s for s in scripts)        # 静音动作仍在

    def test_zero_reading_does_not_overwrite_earlier_record(self, monkeypatch):
        """读数为 0 = 已处于静音态（多半是本工具此前所为）→ 不得覆盖更早的
        原音量记录。变异验证：守卫改成恒真 → 本用例红。"""
        scripts, fake = _fake_ps_get_first("0\n")
        monkeypatch.setattr(modes.proc, "run_ps", fake)
        saved = []
        monkeypatch.setattr("syskit.settings.save",
                            lambda patch: saved.append(dict(patch)))
        modes.set_mute(True)
        assert saved == []


# ---------------- modes.set_mute：恢复侧退化记录的诚实性 ----------------

class TestUnmuteDegenerateRecords:
    def _run(self, monkeypatch, stored):
        scripts = []

        def fake_ps(script, timeout=40):
            scripts.append(script)
            return CmdResult(0, "", "")

        monkeypatch.setattr(modes.proc, "run_ps", fake_ps)
        monkeypatch.setattr("syskit.settings.load", lambda: {"modes_prev_volume": stored})
        r = modes.set_mute(False)
        return r, scripts

    def test_zero_record_falls_back_to_max_and_admits_it(self, monkeypatch):
        """变异验证：回退旧实现（`0 <= prev` + 提示看 prev is not None）→
        vol 变 [uint32]0 且谎报「已恢复原音量」，本用例两断言皆红。"""
        r, scripts = self._run(monkeypatch, 0)
        assert r["ok"]
        assert any("[uint32]4294967295" in s for s in scripts)
        assert "已恢复原音量" not in r["detail"]
        assert "未找到" in r["detail"]

    @pytest.mark.parametrize("dirty", [True, "805306368", 2**32])
    def test_dirty_records_fall_back_to_max_and_admit_it(self, monkeypatch, dirty):
        """bool（isinstance(True,int) 恒真）/ 字符串 / 越界值同按「无可用记录」。
        变异验证：旧实现对 True 会拼出 '[uint32]True'（运行期报错）、对字符串/越界
        会谎报「已恢复原音量」，各分支分别红。"""
        r, scripts = self._run(monkeypatch, dirty)
        assert r["ok"]
        assert any("[uint32]4294967295" in s for s in scripts)
        assert "已恢复原音量" not in r["detail"]

    def test_full_scale_record_is_usable_and_restored(self, monkeypatch):
        """上沿 0xFFFFFFFF 必须按「可用记录」原值恢复并如实报「已恢复原音量」——
        钉死 usable 的右闭边界，防退化成开区间后把合法记录当脏数据。"""
        r, scripts = self._run(monkeypatch, 0xFFFFFFFF)
        assert r["ok"]
        assert any("[uint32]4294967295" in s for s in scripts)
        assert "已恢复原音量" in r["detail"]
