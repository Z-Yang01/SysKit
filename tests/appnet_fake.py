"""appnet 测试共用的**有状态假防火墙**（不是 test_ 文件，pytest 不收集）。

为什么不是「逐条命令打桩返回预设文本」：本模块最要命的 bug 都出在
「写命令说成功了，防火墙里到底有没有」这条链上（真机已证实 netsh 查不到规则时
回显本地化文案且退出码 0）。静态打桩下 add/set/delete 各自返回什么由测试作者
随手写，规则的真实状态根本不存在，于是"写完回查""同名重复""规则被外部删除"
这类语义一律测不到。这里让 add/set/delete 真的改一份内存规则表、
`show rule` 与 PowerShell 读同一份表，测试就能验到端到端语义。

**模拟的是已在本机核实过的真实行为**（git 历史迭代日志 补充52/53）：
- `show rule name=不存在` → 退出码 0 + 中文「没有与指定标准相匹配的规则。」
- `set / delete rule name=不存在` → 同样退出码 0 + 「没有匹配」（净效果是啥也没干）
- `Get-NetFirewallRule -Name 'a','不存在'` → 输出正常但 powershell 退出码 1
- 同名的两条规则 netsh **允许并存**（所以"探测超时当不存在"会留下重复规则）
"""
import re

import pytest

from syskit import appnet
from syskit.proc import CmdResult

NO_MATCH = "没有与指定标准相匹配的规则。"



def _kv(cmd, key):
    """从 netsh 的 argv 里取 `key=value`（本项目一律逐元素传，不经 shell）。"""
    prefix = key + "="
    for part in cmd:
        if part.startswith(prefix):
            return part[len(prefix):]
    return ""


def _names_from_ps(script):
    """取 `-Name 'a','b'` 里的规则名字面量（单引号内 '' 是 PS 的转义）。"""
    m = re.search(r"-Name\s+(.+?)\s+\|", script)
    if not m:
        return []
    return re.findall(r"'((?:[^']|'')*)'", m.group(1))


class FakeFirewall:
    """一份内存规则表：netsh 写它、netsh show 与 PowerShell 读它。"""

    def __init__(self):
        self.rules = []          # [{name, program, enabled, action}]，允许同名重复
        self.ps_available = True
        self.show_timeout = False
        self.fail_next = {}      # 动作 → (code, stderr)：模拟"命令本身失败"（拒绝访问等）

    def fail(self, action, code=1, stderr="拒绝访问"):
        """让某个 netsh 动作（add/set/delete）失败一次。"""
        self.fail_next[action] = (code, stderr)
        return self

    def _failed(self, action):
        """取该动作的注入失败，返回 (code, stdout, stderr) 或 None。"""
        if action not in self.fail_next:
            return None
        code, stderr = self.fail_next.pop(action)
        return code, "", stderr

    # ---------------- 状态构造（测试里表达"防火墙现在长什么样"） ----------------

    def seed(self, name, program="", enabled=True, action="Block"):
        self.rules.append({"name": name, "program": program,
                           "enabled": enabled, "action": action})
        return self

    def names(self):
        return [r["name"] for r in self.rules]

    def count(self, name):
        return sum(1 for r in self.rules if r["name"] == name)

    def of(self, name):
        return next((r for r in self.rules if r["name"] == name), None)

    # ---------------- netsh 侧 ----------------

    def run(self, cmd):
        kind = _kv_action(cmd)
        name = _kv(cmd, "name")
        if kind == "add":
            if (boom := self._failed("add")) is not None:
                return boom
            self.rules.append({"name": name, "program": _kv(cmd, "program"),
                               "enabled": _kv(cmd, "enable") != "no",
                               "action": (_kv(cmd, "action") or "block").title()})
            return 0, "确定。", ""
        if kind == "set":
            hit = [r for r in self.rules if r["name"] == name]
            if (boom := self._failed("set")) is not None:
                return boom
            if not hit:
                return 1, NO_MATCH, ""          # 补55 真机对齐：no-match 退出码为 1
            for r in hit:
                en = _kv(cmd, "enable")
                if en == "yes":
                    r["enabled"] = True
                elif en == "no":
                    r["enabled"] = False
                if _kv(cmd, "action"):
                    r["action"] = _kv(cmd, "action").title()
            return 0, "确定。", ""
        if kind == "delete":
            if (boom := self._failed("delete")) is not None:
                return boom
            before = len(self.rules)
            self.rules = [r for r in self.rules if r["name"] != name]
            return (0, "确定。", "") if len(self.rules) < before else (1, NO_MATCH, "")
        if kind == "show":
            if self.show_timeout:
                return "timeout"
            hit = [r for r in self.rules if r["name"] == name]
            if not hit:
                return 1, NO_MATCH, ""
            body = "\n".join(f"规则名称:       {r['name']}\n"
                             f"操作:            {'阻止' if r['action'] == 'Block' else '允许'}\n"
                             f"已启用:          {'是' if r['enabled'] else '否'}\n"
                             f"程序:            {r['program']}" for r in hit)
            return 0, body, ""
        return 0, "", ""

    # ---------------- PowerShell 侧 ----------------

    def ps(self, script, probe):
        """返回 (code, stdout)：可用性哨兵 + 命中规则的 Name=Enabled=Action 行。"""
        if not self.ps_available:
            return 1, "", "Get-NetFirewallRule : 无法将_term识别为 cmdlet"
        names = _names_from_ps(script)
        lines = [probe]
        for n in names:
            r = self.of(n.replace("''", "'"))
            if r:
                lines.append(f"{r['name']}={r['enabled']}={r['action']}")
        # 真机：有名字查不到时 powershell 退出码 1，但上面的输出完全正常
        code = 1 if any(n.replace("''", "'") not in self.names() for n in names) else 0
        return code, "\r\n".join(lines) + "\r\n", ""


def _kv_action(cmd):
    """netsh advfirewall firewall <add|set|delete|show> rule 的动词。"""
    for i, part in enumerate(cmd):
        if part in ("add", "set", "delete", "show") and i + 1 < len(cmd) \
                and cmd[i + 1] == "rule":
            return part
    return ""


# ---------------- 共用 Recorder（单测与 UI 测试同一套语义） ----------------

def _cmd_str(cmd):
    return " ".join(cmd) if isinstance(cmd, list) else str(cmd)


class Recorder:
    """记录 proc.run / proc.run_ps；显式子串打桩优先，其余交给有状态假防火墙真实执行。

    「其余交给假防火墙」而不是「一律返回成功」是关键：默认成功会让测试作者随手写出
    与真实 netsh 语义相反的桩（比如 set 一条不存在的规则真的生效了），
    于是"写完回查""探测超时不敢 add"这些行为永远测不到。
    """

    def __init__(self):
        self.calls = []          # (kind, cmd, timeout, CmdResult)
        self._rules = []
        self.fw = FakeFirewall()

    def respond_sub(self, *subs, code=0, stdout="", stderr="", timeout=False):
        self._rules.append((subs, dict(code=code, stdout=stdout, stderr=stderr,
                                       timeout=timeout)))

    def reset(self):
        """清调用记录**与响应规则**——先注册的规则优先命中，只清 calls 会串味。"""
        self.calls.clear()
        self._rules = []

    def _dispatch(self, kind, cmd, timeout):
        text = _cmd_str(cmd)
        for subs, kw in self._rules:
            if all(s in text for s in subs):
                res = CmdResult(**kw)
                self.calls.append((kind, cmd, timeout, res))
                return res
        if kind == "run":
            out = self.fw.run(cmd if isinstance(cmd, list) else text.split())
            if out == "timeout":
                res = CmdResult(code=-1, stdout="", stderr="", timeout=True)
            else:
                code, stdout, stderr = out
                res = CmdResult(code=code, stdout=stdout, stderr=stderr)
        else:
            code, stdout, stderr = self.fw.ps(text, appnet.PS_PROBE)
            res = CmdResult(code=code, stdout=stdout, stderr=stderr)
        self.calls.append((kind, cmd, timeout, res))
        return res

    def run(self, cmd, timeout=20):
        return self._dispatch("run", cmd, timeout)

    def run_ps(self, script, timeout=40):
        return self._dispatch("ps", script, timeout)

    def patch(self, monkeypatch, is_admin=True, isfile=None):
        """装上桩；conftest 红线 1 的哨兵会击杀任何漏装的路径。

        isfile=None 表示不碰它——业务层单测要的是"这台机器上到底有没有这个文件"
        的真实判定，UI 测试才需要一律为真。
        """
        monkeypatch.setattr(appnet.proc, "run", self.run)
        monkeypatch.setattr(appnet.proc, "run_ps", self.run_ps)
        monkeypatch.setattr(appnet.admin, "is_admin", lambda: is_admin)
        if isfile is not None:
            monkeypatch.setattr(appnet.os.path, "isfile", lambda p: isfile)

    @property
    def texts(self):
        """所有命令文本（含 PowerShell 脚本）——断言「到底下发了什么」。"""
        return [_cmd_str(c[1]) for c in self.calls]

    @property
    def joined(self):
        return [_cmd_str(c[1]) for c in self.calls if c[0] == "run"]

    @property
    def argvs(self):
        return [c[1] for c in self.calls if c[0] == "run"]

    @property
    def ps_scripts(self):
        return [c[1] for c in self.calls if c[0] == "ps"]

    @property
    def write_cmds(self):
        """真正改变防火墙的命令（add/set/delete），show 与查询不算。"""
        return [t for t in self.joined if "add rule" in t or "set rule" in t
                or "delete rule" in t]


@pytest.fixture
def rec(monkeypatch):
    """管理员 + 文件存在的默认环境（各自的反例由专门用例覆盖）。"""
    r = Recorder()
    r.patch(monkeypatch)
    return r

