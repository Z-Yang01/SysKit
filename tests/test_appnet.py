"""syskit/appnet.py 单测：只验证「构造了什么命令、防火墙变成什么样、台账怎么改」，绝不碰真实防火墙。

Mock 边界（配合 conftest 红线 1 哨兵：proc.run / proc.run_ps 未替换即 AssertionError）：
- 局部 Recorder 替换 proc.run / proc.run_ps：**未显式打桩的命令一律交给 tests/appnet_fake.py
  的有状态假防火墙**执行（不是「默认返回成功」——那样"写完回查""探测超时不敢 add"
  这些语义根本测不出来），显式 respond_sub 才用于注入失败与超时；
- admin.is_admin 打桩（默认 True，另有专门的「非管理员」用例）；
- os.path.isfile 打桩，避免依赖机器上真实存在哪个 exe。
持久化由 conftest 把 %APPDATA% 重定向到临时目录，用例之间互不污染。
"""
import pytest

from appnet_fake import Recorder
from syskit import appnet


# ---------------- 局部夹具 ----------------

@pytest.fixture
def rec(monkeypatch):
    """命令层全部落在有状态假防火墙上；isfile 保持真实（有专门的"文件不存在"用例）。"""
    r = Recorder()
    r.patch(monkeypatch)
    return r


@pytest.fixture
def exe(tmp_path, monkeypatch):
    """一个「存在」的假 exe 路径（不真的落盘，只打桩 isfile）。"""
    path = r"C:\Program Files\Reader\阅读.exe"
    monkeypatch.setattr(appnet.os.path, "isfile",
                        lambda p: appnet.norm_path(p) == appnet.norm_path(path))
    return path


# ---------------- 纯函数：路径与规则名 ----------------

def test_norm_path_collapses_equivalent_spellings():
    a = appnet.norm_path('  "C:/Program Files/Reader/Rd.exe"  ')
    b = appnet.norm_path(r"c:\program files\reader\rd.exe")
    assert a == b
    assert appnet.norm_path("") == ""
    assert appnet.norm_path(None) == ""
    assert appnet.norm_path("nul\x00bad").strip() != ""      # 非法路径不抛


def test_rule_name_is_ascii_and_stable():
    name = appnet.rule_name(r"C:\Program Files\Reader\阅读.exe")
    assert name.startswith(appnet.RULE_PREFIX + "-")
    assert name.isascii(), "规则名掺中文/空格会把引号问题带进 netsh"
    assert name == appnet.rule_name(r"c:\program files\reader\阅读.exe")
    assert name != appnet.rule_name(r"C:\Program Files\Reader\阅读2.exe")


def test_ps_status_script_uses_quoted_name_literals():
    name = appnet.rule_name(r"C:\a.exe")
    script = appnet._ps_status_script([r"C:\a.exe"])
    assert f"'{name}'" in script                             # PS 侧是带引号的字面量
    assert "Get-NetFirewallRule" in script and "$_.Enabled" in script
    assert appnet.RULE_PREFIX in script


def test_ps_status_script_emits_probe_and_forces_exit_zero():
    """真机结论：-Name 含不存在的规则名时 powershell 退出码 1 但输出正常。

    脚本必须自己输出可用性哨兵并以 exit 0 收尾，否则 query_states 无法区分
    「模块不可用」和「规则被外部删了」。
    """
    script = appnet._ps_status_script([r"C:\a.exe", r"C:\b.exe"])
    assert appnet.PS_PROBE in script
    assert "Get-Command" in script                           # 哨兵前探测 cmdlet 存在
    assert script.rstrip().endswith("exit 0")


# ---------------- 命令构造 ----------------

def test_cmd_add_shape_and_quoting():
    p = r"C:\Program Files\Reader\Rd.exe"
    cmd = appnet.cmd_add(p, enable=True)
    assert cmd[:5] == ["netsh", "advfirewall", "firewall", "add", "rule"]
    assert isinstance(cmd, list) and all(isinstance(x, str) for x in cmd)
    assert "dir=out" in cmd and "action=block" in cmd and "enable=yes" in cmd
    # 带空格的路径必须是**单个 argv 元素**（由 Python 负责加引号），不能预先拆词
    assert "program=" + p in cmd
    assert not any(" " in x and "=" not in x for x in cmd)
    assert "name=" + appnet.rule_name(p) in cmd


def test_cmd_add_enable_flag_follows_target_state():
    assert "enable=no" in appnet.cmd_add(r"C:\a.exe", enable=False)
    assert "enable=yes" in appnet.cmd_add(r"C:\a.exe", enable=True)


def test_cmd_add_uses_only_netsh_documented_params():
    """补55 真机 P0：netsh `add rule` **不支持** group/displayName（那是 PowerShell
    New-NetFirewallRule 的参数），带上即报「不是这个指令的有效参数」——初版写路径
    在任何机器上都从未成功过，被桩测试掩盖。此用例钉死参数表。"""
    cmd = appnet.cmd_add(r"C:\Program Files\Reader\Rd.exe")
    assert not any(x.startswith(("group=", "displayName=")) for x in cmd)
    assert any(x.startswith("description=") for x in cmd)      # 人类可读信息全在 description


def test_netsh_state_bilingual_parsing(monkeypatch):
    """netsh show 的状态字段中英双语解析（netsh/PS 存储分歧机器上的兜底读侧）。"""
    from types import SimpleNamespace

    name = appnet.rule_name(r"C:\a.exe")

    def fake_show(stdout):
        monkeypatch.setattr(appnet.proc, "run",
                            lambda cmd, timeout=20: SimpleNamespace(
                                code=0, ok=True, stdout=stdout, stderr="", timeout=False))

    fake_show(f"规则名称:   {name}\n已启用:    是\n操作:   阻止")
    assert appnet.netsh_state(r"C:\a.exe") == "on"
    fake_show(f"Rule Name:   {name}\nEnabled:   No\nAction:   Block")
    assert appnet.netsh_state(r"C:\a.exe") == "off"
    fake_show(f"规则名称:   {name}\n已启用:    是\n操作:   允许")
    assert appnet.netsh_state(r"C:\a.exe") == "drift"          # 动作被外部改允许：无论启停都不拦
    fake_show("没有与指定标准相匹配的规则。")
    assert appnet.netsh_state(r"C:\a.exe") == "missing"
    fake_show(f"规则名称:   {name}\n（字段缺失的意外输出）")
    assert appnet.netsh_state(r"C:\a.exe") is None             # 解析不出 = 不知道，不猜


def test_delete_rule_tolerates_already_gone(rec):
    """补55 真机：netsh no-match 退出码已从 0 漂移为 1——规则被外部删掉后点
    「移除」必须按终态达成处理，而不是报失败把台账行变成永远删不掉的死条目。"""
    ok, msg = appnet.delete_rule(r"C:\a.exe")                  # 假防火墙里本来就没有
    assert ok is True and "已不存在" in msg


def test_cmd_set_delete_show_target_the_same_rule():
    p = r"C:\a b\app.exe"
    name = appnet.rule_name(p)
    assert appnet.cmd_set_enable(p, False) == [
        "netsh", "advfirewall", "firewall", "set", "rule", f"name={name}",
        "new", "enable=no"]
    assert appnet.cmd_delete(p) == [
        "netsh", "advfirewall", "firewall", "delete", "rule", f"name={name}"]
    assert appnet.cmd_show(p) == [
        "netsh", "advfirewall", "firewall", "show", "rule", f"name={name}"]


# ---------------- 读侧解析 ----------------

def test_parse_status_output_maps_enabled_and_action_and_ignores_noise():
    n1 = appnet.rule_name(r"C:\a.exe")
    n2 = appnet.rule_name(r"C:\b.exe")
    text = (f"{appnet.PS_PROBE}\n{n1}=True=Block\n找不到名为 'x' 的规则\n"
            f"{n2}=False=Block\n{appnet.RULE_PREFIX}-zz=maybe\n{n1}=True=Allow")
    st = appnet.parse_status_output(text)
    assert st[n1] == "drift"                                   # 同名行取后者（PS 只回一条）
    assert st[n2] == "off"
    assert f"{appnet.RULE_PREFIX}-zz" not in st                # 段数不对 = 不猜
    assert appnet.PS_PROBE not in st                            # 哨兵不是规则，不进结果
    assert appnet.parse_status_output("") == {}
    assert appnet.parse_status_output(f"{n1}=True") == {}       # 缺 Action 段不猜


def test_parse_status_output_action_allow_is_drift_not_on():
    """用户在 wf.msc 把规则动作改成「允许」：Enabled 仍 True，但已经不拦了。

    读侧若只看 Enabled，安全控制会被报成「已禁止联网」——这是实质性错报。
    """
    n = appnet.rule_name(r"C:\a.exe")
    assert appnet.parse_status_output(f"{n}=True=Allow")[n] == "drift"
    assert appnet.parse_status_output(f"{n}=False=Allow")[n] == "drift"
    assert appnet.parse_status_output(f"{n}=True=Block")[n] == "on"


def test_rule_exists_matches_by_ascii_name_not_by_localized_text(rec):
    p = r"C:\a.exe"
    assert appnet.rule_exists(p) is False                      # 假防火墙里还没这条规则
    rec.fw.seed(appnet.rule_name(p), program=p)
    assert appnet.rule_exists(p) is True
    rec.fw.show_timeout = True
    assert appnet.rule_exists(p) is None                       # 超时 = 不知道，不是「不在」


def test_query_states_keys_by_normalized_path(rec):
    """键必须是 norm_path：查询侧与显示侧各拿一份台账路径，大小写不同也不能查不到。"""
    p = r"C:\Program Files\A\a.exe"
    rec.fw.seed(appnet.rule_name(p), program=p)
    got = appnet.query_states([p])
    assert got == {appnet.norm_path(p): "on"}
    assert appnet.query_states([p.upper()]) == {appnet.norm_path(p): "on"}


def test_query_states_returns_none_when_powershell_fails(rec):
    p = r"C:\a.exe"
    rec.fw.ps_available = False
    assert appnet.query_states([p]) is None                    # 无哨兵 = 查不动 ≠ 规则被删
    rec.reset()
    rec.fw.ps_available = True
    rec.respond_sub("Get-NetFirewallRule", stdout="", timeout=True)
    assert appnet.query_states([p]) is None                    # 超时同样不能折成「都不在」
    rec.reset()
    rec.fw.seed(appnet.rule_name(p), program=p)
    assert appnet.query_states([p]) == {appnet.norm_path(p): "on"}
    assert appnet.query_states([]) == {}
    assert appnet.query_states([""]) == {}


def test_query_states_trusts_probe_over_exit_code(rec):
    """真机：查 2 条里有 1 条被外部删除时 powershell 返回退出码 1、输出正常。

    此时必须报「missing」而不是「未校验」，否则用户以为没查过、会去重复建规则。
    """
    a, b = r"C:\a.exe", r"C:\b.exe"
    rec.fw.seed(appnet.rule_name(a), program=a)                # 只有 a 还在
    got = appnet.query_states([a, b])
    assert got == {appnet.norm_path(a): "on", appnet.norm_path(b): "missing"}


def test_actual_state_none_means_unverified_not_missing(rec):
    """双侧读不出来才是「未校验」；单侧 missing 都能给出确定事实。"""
    p = r"C:\a.exe"
    assert appnet.actual_state(p) == "missing"                 # PS 可用、确实没规则
    rec.fw.ps_available = False
    rec.fw.show_timeout = True
    assert appnet.actual_state(p) is None                      # PS + netsh 双挂 = 无法判定


def test_actual_state_ps_unavailable_falls_back_to_netsh(rec):
    """补55 真机：netsh 建的规则在 CIM 视图里长期不可见（存储分歧）——PS 报
    missing 或整体不可用时必须用 netsh 兜底，而不是误判「被外部删除」或「未校验」。"""
    p = r"C:\a.exe"
    rec.fw.ps_available = False
    rec.fw.seed(appnet.rule_name(p), program=p)                # 只有 netsh 侧看得到
    assert appnet.actual_state(p) == "on"
    rec.fw.rules.clear()
    assert appnet.actual_state(p) == "missing"                 # 两边都没有 = 真的不在


def test_parse_status_output_ignores_foreign_rule_names():
    """段数对得上但不是我们的规则名 → 忽略，不参与任何判定。"""
    assert appnet.parse_status_output("FPS-ICMP6-ERQ-In-V2=True=Block") == {}


def test_real_path_never_raises_on_illegal_characters():
    """含 NUL 的路径交给上层按「文件不存在」处理，绝不在路径函数里炸。"""
    bad = "ab\x00cd.exe"
    assert isinstance(appnet.real_path(bad), str)
    assert appnet.rule_name(bad).startswith(appnet.RULE_PREFIX)


def test_real_path_falls_back_to_raw_text_when_abspath_fails(monkeypatch):
    """绝对化失败时退回原串：让 validate_program 用 isfile 报「不存在」，而不是 traceback。"""
    def boom(p):
        raise ValueError("embedded null byte")
    monkeypatch.setattr(appnet.os.path, "abspath", boom)
    assert appnet.real_path('  "C:\\a\\b.exe"  ') == r"C:\a\b.exe"


# ---------------- 校验与门禁 ----------------

def test_validate_program_rejects_missing_and_non_exe(rec, tmp_path):
    assert appnet.validate_program("") != ""
    assert appnet.validate_program(r"C:\a\reader.txt") != ""
    missing = str(tmp_path / "nope.exe")
    assert "不存在" in appnet.validate_program(missing)       # 路径写错的规则永远不命中


@pytest.mark.parametrize("blocked", [True, False])
def test_apply_state_requires_admin_and_sends_nothing(rec, monkeypatch, blocked):
    monkeypatch.setattr(appnet.admin, "is_admin", lambda: False)
    ok, msg = appnet.apply_state(r"C:\a.exe", blocked)
    assert ok is False and "管理员" in msg
    assert rec.calls == []


def test_apply_state_creates_rule_when_absent(rec):
    p = r"C:\a.exe"
    ok, msg = appnet.apply_state(p, True)
    assert ok is True and msg == "已禁止联网"
    assert rec.fw.count(appnet.rule_name(p)) == 1             # 假防火墙里真的多了一条
    assert any("add rule" in t for t in rec.joined)
    assert not any("set rule" in t for t in rec.joined)


def test_apply_state_toggles_existing_rule_without_recreating(rec):
    p = r"C:\a.exe"
    appnet.apply_state(p, True)
    rec.reset()
    ok, msg = appnet.apply_state(p, False)
    assert ok is True and msg == "已允许联网"
    assert rec.fw.count(appnet.rule_name(p)) == 1             # 没建第二条
    assert any("set rule" in t and "enable=no" in t for t in rec.joined)
    assert not any("add rule" in t for t in rec.joined)


def test_apply_state_reports_command_failure(rec):
    p = r"C:\a.exe"
    appnet.apply_state(p, True)                                # 先建好，后续才走 set 分支
    rec.reset()
    rec.fw.fail("set", code=2, stderr="拒绝访问: 5")
    ok, msg = appnet.apply_state(p, False)
    assert ok is False and "拒绝访问" in msg


def test_apply_state_refuses_to_guess_when_probe_times_out(rec):
    """netsh show 超时 → 不能当成「规则不在」去 add：netsh 不查重，会留下同名两条。"""
    p = r"C:\a.exe"
    appnet.apply_state(p, True)
    rec.reset()
    rec.fw.show_timeout = True
    ok, msg = appnet.apply_state(p, False)
    assert ok is False and "超时" in msg
    assert rec.fw.count(appnet.rule_name(p)) == 1             # 没有第二条
    assert not any("add rule" in t for t in rec.joined)


def test_apply_state_does_not_report_success_when_rule_is_not_really_there(rec):
    """真机语义：写命令可以回显「没有与指定标准相匹配的规则。」且退出码 0，却什么都没做。

    探测时规则还在、下发瞬间已被外部删掉（TOCTOU）：netsh set 回 0，照着退出码报
    「已禁止联网」并写台账，用户得到的是一条根本不存在的保护。
    """
    p = r"C:\a.exe"
    name = appnet.rule_name(p)
    rec.respond_sub("show rule", stdout=f"规则名称:       {name}")   # 探测：在
    rec.respond_sub("set rule", code=0, stdout="没有与指定标准相匹配的规则。")
    # 假防火墙里始终没有这条规则 → PowerShell 回查得到 missing
    ok, msg = appnet.apply_state(p, False)
    assert ok is False and "查不到这条规则" in msg
    assert rec.fw.count(name) == 0                             # 也没有偷偷新建一条
    assert not any("add rule" in t for t in rec.joined)


def test_apply_state_reports_drifted_rule_instead_of_overwriting(rec):
    p = r"C:\a.exe"
    rec.fw.seed(appnet.rule_name(p), program=p, enabled=True, action="Allow")
    ok, msg = appnet.apply_state(p, True)                      # set enable=yes 不改 action
    assert ok is False and "不是「阻止」" in msg


def test_apply_state_drift_off_reports_achieved(rec):
    """动作已被外部改成「允许」时点「允许联网」：净效果已达成，如实说明而非报失败
    （旧文案要求「改回」，对用户是反的——代理 A 第 9 批 F7）。"""
    p = r"C:\a.exe"
    rec.fw.seed(appnet.rule_name(p), program=p, enabled=True, action="Allow")
    ok, msg = appnet.apply_state(p, False)                     # set enable=no，不覆盖 action
    assert ok is True and "外部改为「允许」" in msg


def test_apply_state_verifies_via_netsh_when_ps_down(rec):
    """补55 真机：netsh 是完整的读侧后端（netsh/PS 存储分歧的机器上尤其关键）——
    PS 挂了也能真验证，不再落入「未能回查」。"""
    p = r"C:\a.exe"
    rec.fw.seed(appnet.rule_name(p), program=p)
    rec.fw.ps_available = False
    ok, msg = appnet.apply_state(p, True)
    assert ok is True and "已禁止联网" in msg and "未能回查" not in msg


def test_apply_state_says_so_when_post_write_check_is_impossible(rec):
    """PS 与 netsh 兜底都读不出状态时：如实说「未能回查」，不假装验证过。"""
    p = r"C:\a.exe"
    name = appnet.rule_name(p)
    rec.fw.seed(appnet.rule_name(p), program=p)
    rec.respond_sub("show rule", stdout=f"规则名称:       {name}")   # 名字在、字段解析不出
    rec.fw.ps_available = False
    ok, msg = appnet.apply_state(p, True)
    assert ok is True and "未能回查" in msg


def test_delete_rule_requires_admin_and_sends_nothing(rec, monkeypatch):
    monkeypatch.setattr(appnet.admin, "is_admin", lambda: False)
    ok, msg = appnet.delete_rule(r"C:\a.exe")
    assert ok is False and "管理员" in msg
    assert rec.calls == []


def test_delete_rule_reports_failure_when_rule_survives(rec):
    p = r"C:\a.exe"
    appnet.apply_state(p, True)
    rec.reset()
    rec.respond_sub("delete rule", code=0, stdout="没有与指定标准相匹配的规则。")
    # 上面这条打桩让 delete 假装执行成功，而假防火墙里规则仍在（组策略保护的情形）
    ok, msg = appnet.delete_rule(p)
    assert ok is False and "仍在" in msg
    assert rec.fw.count(appnet.rule_name(p)) == 1


def test_delete_rule_says_so_when_post_write_check_is_impossible(rec):
    """PS 与 netsh 兜底都读不出状态时：如实说「未能回查」，不假装验证过删除。"""
    p = r"C:\a.exe"
    name = appnet.rule_name(p)
    rec.fw.seed(appnet.rule_name(p), program=p)
    rec.respond_sub("delete rule", code=0, stdout="确定。")          # 命令层成功（规则其实没删）
    rec.respond_sub("show rule", stdout=f"规则名称:       {name}")   # 读侧字段解析不出
    rec.fw.ps_available = False
    ok, msg = appnet.delete_rule(p)
    assert ok is True and "未能回查" in msg


def test_confirm_reports_unexpected_state_verbatim(monkeypatch):
    """回查返回状态机之外的值（PS 输出格式将来变了）：照实说，绝不静默当成功。"""
    monkeypatch.setattr(appnet, "actual_state", lambda p: "weird")
    ok, msg = appnet._confirm(r"C:\a.exe", "on", "已禁止联网")
    assert ok is False and "weird" in msg and "on" in msg


# ---------------- 台账 ----------------

def test_load_entries_tolerates_corrupt_and_partial_records(rec, monkeypatch):
    monkeypatch.setattr(appnet.stores, "load_json", lambda name, default: {"entries": [
        {"program": r"C:\a.exe", "blocked": True},           # 缺 rule/added → 现场推导
        {"program": "   "},                                   # 空路径 → 丢弃
        "垃圾",                                               # 非字典 → 丢弃
        None,
    ]})
    out = appnet.load_entries()
    assert len(out) == 1
    e = out[0]
    assert e["program"] == appnet.real_path(r"C:\a.exe")     # 台账保留原始大小写
    assert e["rule"] == appnet.rule_name(r"C:\a.exe")
    assert e["blocked"] is True and e["added"] == ""


def test_load_entries_survives_missing_file_shape(rec, monkeypatch):
    monkeypatch.setattr(appnet.stores, "load_json", lambda name, default: [])
    assert appnet.load_entries() == []
    monkeypatch.setattr(appnet.stores, "load_json", lambda name, default: None)
    assert appnet.load_entries() == []


def test_entries_round_trip_through_store(rec):
    entries = [{"program": appnet.real_path(r"C:\Program Files\A\a.exe"),
                "rule": appnet.rule_name(r"C:\a.exe"),
                "blocked": True, "added": "2026-09-19 10:00"}]
    assert appnet.save_entries(entries) is True
    assert appnet.load_entries() == entries
    # 大小写不同的同一程序仍算同一条（比对走 norm_path）
    assert appnet.find_entry(appnet.load_entries(), r"c:\program files\a\A.EXE") == 0


# ---------------- 面向 UI 的组合操作 ----------------

def test_add_program_creates_rule_then_records_ledger(rec, exe):
    ok, msg, idx = appnet.add_program(exe, blocked=True)
    assert ok is True and idx == 0 and msg == "已禁止联网"
    assert sum("add rule" in t for t in rec.joined) == 1     # 一次点击只建一条规则
    assert rec.fw.count(appnet.rule_name(exe)) == 1
    entries = appnet.load_entries()
    assert len(entries) == 1
    assert entries[0]["blocked"] is True
    assert entries[0]["rule"] == appnet.rule_name(exe)


def test_add_program_rejects_bad_path_without_touching_firewall(rec):
    ok, msg, idx = appnet.add_program(r"C:\nope\missing.exe")
    assert ok is False and idx is None
    assert rec.calls == []
    assert appnet.load_entries() == []


def test_add_program_does_not_record_ledger_when_firewall_fails(rec, exe, monkeypatch):
    rec.respond_sub("add rule", code=1, stderr="拒绝访问")
    ok, msg, idx = appnet.add_program(exe)
    assert ok is False and "拒绝访问" in msg and idx is None
    assert appnet.load_entries() == []                       # 没生效就不该出现在清单里


def test_add_program_does_not_record_ledger_when_post_write_check_fails(rec, exe):
    """写命令"成功"但回查说防火墙里没这条规则：台账一个字都不许写。"""
    rec.fw.ps_available = True
    rec.respond_sub("add rule", code=0, stdout="确定。")        # 假装加了，实际没加
    ok, msg, idx = appnet.add_program(exe)
    assert ok is False and "查不到这条规则" in msg and idx is None
    assert appnet.load_entries() == []


def test_add_program_twice_adjusts_state_instead_of_duplicating(rec, exe):
    assert appnet.add_program(exe)[0] is True
    n = len(rec.calls)
    ok, msg, idx = appnet.add_program(exe, blocked=False)
    assert ok is True and "已在清单" in msg and idx == 0
    assert len(appnet.load_entries()) == 1
    assert appnet.load_entries()[0]["blocked"] is False
    assert rec.fw.count(appnet.rule_name(exe)) == 1            # 没建第二条
    assert any("set rule" in t for t in rec.joined[n:])       # 只调状态，不新建规则


def test_add_program_uses_disabled_block_rule_for_allow(rec, exe):
    """新程序直接选「允许」= 建一条 enable=no 的块规则：wf.msc 里留着停用规则是刻意取舍。"""
    ok, msg, idx = appnet.add_program(exe, blocked=False)
    assert ok is True and rec.fw.of(appnet.rule_name(exe))["enabled"] is False
    assert rec.fw.count(appnet.rule_name(exe)) == 1


def test_add_program_again_reports_state_change_failure_without_touching_ledger(rec, exe):
    """已在清单里但状态调不动：整次操作算失败，台账原样保留。"""
    appnet.add_program(exe)
    rec.fw.fail("set")
    ok, msg, idx = appnet.add_program(exe, blocked=False)
    assert ok is False and idx is None and "拒绝访问" in msg
    assert appnet.load_entries()[0]["blocked"] is True


def test_add_program_again_admits_save_failure(rec, exe, monkeypatch):
    """已在清单里、规则也调好了，但台账写不进去：明说保存失败，别只报「一切正常」。"""
    appnet.add_program(exe)
    _break_save(monkeypatch)
    ok, msg, idx = appnet.add_program(exe, blocked=False)
    assert ok is True and idx == 0 and "已在清单" in msg and "保存失败" in msg
    assert rec.fw.of(appnet.rule_name(exe))["enabled"] is False


def test_set_program_blocked_updates_ledger(rec, exe):
    appnet.add_program(exe)
    ok, msg = appnet.set_program_blocked(exe, False)
    assert ok is True and appnet.load_entries()[0]["blocked"] is False
    ok, msg = appnet.set_program_blocked(r"C:\other.exe", True)
    assert ok is False and "不在清单" in msg


def test_set_program_blocked_keeps_ledger_when_firewall_fails(rec, exe):
    appnet.add_program(exe)
    rec.fw.fail("set")
    ok, msg = appnet.set_program_blocked(exe, False)
    assert ok is False
    assert appnet.load_entries()[0]["blocked"] is True       # 失败不得留下假状态


def test_drop_program_removes_rule_and_ledger(rec, exe):
    appnet.add_program(exe)
    ok, msg = appnet.drop_program(exe)
    assert ok is True and appnet.load_entries() == []
    assert rec.fw.count(appnet.rule_name(exe)) == 0
    assert any("delete rule" in t for t in rec.joined)


def test_drop_program_keeps_ledger_when_rule_deletion_fails(rec, exe):
    appnet.add_program(exe)
    rec.respond_sub("delete rule", code=1, stderr="拒绝访问")
    ok, msg = appnet.drop_program(exe)
    assert ok is False and "拒绝访问" in msg
    assert len(appnet.load_entries()) == 1                   # 规则还在就不许丢台账


def test_drop_program_reports_unknown_program(rec):
    ok, msg = appnet.drop_program(r"C:\never\added.exe")
    assert ok is False and "不在清单" in msg


# ---------------- 台账写盘失败：规则已生效也不能装作一切正常 ----------------

def _break_save(monkeypatch):
    """在台账已经写好之后才让 save_json 失败（模拟磁盘满 / 目录被占用）。"""
    monkeypatch.setattr(appnet.stores, "save_json", lambda name, data: False)


def test_add_program_reports_that_row_will_not_appear_when_save_fails(rec, exe, monkeypatch):
    _break_save(monkeypatch)
    ok, msg, idx = appnet.add_program(exe)
    assert ok is True and idx is None                          # 没有这一行，就别给出行号
    assert "保存失败" in msg and "本次界面" in msg and "wf.msc" in msg
    assert rec.fw.count(appnet.rule_name(exe)) == 1            # 规则确实已生效


def test_toggle_keeps_rule_and_admits_save_failure(rec, exe, monkeypatch):
    appnet.add_program(exe)
    _break_save(monkeypatch)
    ok, msg = appnet.set_program_blocked(exe, False)
    assert ok is True and "保存失败" in msg
    assert rec.fw.of(appnet.rule_name(exe))["enabled"] is False   # 防火墙已按目标改好


def test_drop_keeps_ledger_row_when_save_fails(rec, exe, monkeypatch):
    appnet.add_program(exe)
    _break_save(monkeypatch)
    ok, msg = appnet.drop_program(exe)
    assert ok is True and "保存失败" in msg
    assert rec.fw.count(appnet.rule_name(exe)) == 0            # 规则已删
    assert len(appnet.load_entries()) == 1                     # 台账清不掉就留着，别丢信息


# ---------------- 台账脏数据 ----------------

def test_load_entries_drops_case_duplicate_rows(rec, monkeypatch):
    """同一 exe 的大小写重复行必须合并：否则选中第二行点「移除」会 pop 掉第一行。"""
    monkeypatch.setattr(appnet.stores, "load_json", lambda name, default: {"entries": [
        {"program": r"C:\Tools\DL\dl.exe"},
        {"program": r"c:\tools\dl\DL.EXE"},
        {"program": r"C:\Other\other.exe"},
    ]})
    out = appnet.load_entries()
    assert [appnet.norm_path(e["program"]) for e in out] == [
        appnet.norm_path(r"C:\Tools\DL\dl.exe"),
        appnet.norm_path(r"C:\Other\other.exe")]
