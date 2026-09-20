"""应用联网控制：按可执行文件用 Windows 防火墙出站规则禁止 / 放行某个软件联网。

为什么用防火墙而不是 Hosts / 代理：Hosts 只能骗域名、代理只管走代理的程序，
只有防火墙的 ALE 层 per-program 出站块对任意协议都生效；而且规则在
「高级安全 Windows 防火墙」(wf.msc) 里看得见、改得动，SysKit 不藏黑箱。

**读命令与写命令故意分开**：
- 写（增 / 改 / 删规则）走 `netsh advfirewall firewall`，参数一律以 list 逐元素传，
  不经 shell，杜绝把用户选的路径拼进命令行字符串的注入面；带空格的路径也按
  「单个 argv 元素」传（`program=C:\\Program Files\\...`），由 Python 负责加引号。
- 读（查规则在不在线、启没启用）走 PowerShell `Get-NetFirewallRule`：netsh 的输出
  字段名和「Active: 是 / No rules match」都随系统语言变，而 PS 的属性值
  `True` / `False` 与规则名一样恒为 ASCII，解析不受语言影响。PS 不可用时读侧
  降级为「未校验」，写侧不受影响。

规则标识：`SysKit-AppNet-<规范化路径的 sha1 前 10 位>`。**只用 ASCII**——规则名是
所有增删改查的唯一键，掺中文或空格就会把引号问题带进 netsh。人类可读信息
（程序名、完整路径）只写进 DisplayName / Description，写完就不再回读。
"""
import hashlib
import os
import re
import time

from . import admin, proc, stores
from .logger import LOGGER

RULE_PREFIX = "SysKit-AppNet"
STORE_NAME = "appnet.json"
ADMIN_MSG = "修改防火墙规则需要管理员权限。\n请以管理员身份运行 SysKit 后重试。"
# PS 输出里的可用性哨兵：只有它出现才说明 Get-NetFirewallRule 跑得动。
# 不含 '='，因此 parse_status_output 天然忽略它。
PS_PROBE = "SysKit-AppNet-PS-Available"


# ---------------- 纯函数：路径规范化与命令构造 ----------------

def norm_path(program: str) -> str:
    """规范化可执行文件路径：去引号 / 转绝对路径 / 折叠大小写与斜杠。

    这是**身份键**，只用于比对与哈希：同一个 exe 从文件选择器拿到
    `C:\\Program Files\\A\\a.exe`、从手输得到 `c:/program files/a/a.exe`，
    必须落到同一条规则上，否则用户会看到两条重复规则却谁也没生效。
    写进防火墙的路径另用 real_path()——Windows 防火墙对大小写不敏感，
    但把用户程序名改成小写会在 wf.msc 里显得像伪造的。
    """
    p = real_path(program)
    return os.path.normcase(p) if p else ""


def real_path(program: str) -> str:
    """去引号并转绝对路径，**保留原始大小写**（用于下发给 netsh 的值与展示）。"""
    p = (program or "").strip().strip('"').strip()
    if not p:
        return ""
    try:
        return os.path.abspath(p)
    except (ValueError, TypeError):
        # 非法路径（如含 NUL）不炸调用方，交给上层按「文件不存在」处理
        return p


def rule_name(program: str) -> str:
    return f"{RULE_PREFIX}-{hashlib.sha1(norm_path(program).encode('utf-8')).hexdigest()[:10]}"


def _description(program: str) -> str:
    return f"SysKit 应用联网控制 · {real_path(program)}"


def cmd_add(program: str, enable: bool = True) -> list:
    # 只用 netsh 文档参数（name/program/dir/action/enable/description）。
    # 补55 真机实锤：netsh advfirewall `add rule` **没有** group/displayName 参数
    #（那是 PowerShell New-NetFirewallRule 的概念），带上即报「不是这个指令的有效
    # 参数」——初版写路径因此在任何机器上都从未成功过，被桩测试掩盖。人类可读
    # 信息（路径）只写进 description。
    return ["netsh", "advfirewall", "firewall", "add", "rule",
            f"name={rule_name(program)}",
            f"program={real_path(program)}",
            "dir=out", "action=block",
            f"enable={'yes' if enable else 'no'}",
            f"description={_description(program)}"]


def cmd_set_enable(program: str, enable: bool) -> list:
    return ["netsh", "advfirewall", "firewall", "set", "rule",
            f"name={rule_name(program)}", "new",
            f"enable={'yes' if enable else 'no'}"]


def cmd_delete(program: str) -> list:
    return ["netsh", "advfirewall", "firewall", "delete", "rule",
            f"name={rule_name(program)}"]


def cmd_show(program: str) -> list:
    return ["netsh", "advfirewall", "firewall", "show", "rule",
            f"name={rule_name(program)}"]


def _ps_status_script(programs) -> str:
    """批量查规则在离线：先输出一行可用性哨兵，再每行输出 `<ASCII 规则名>=Enabled=Action`。

    三处都是真机踩出来的（见 PROGRESS 补充52/53）：
    - `-Name` 里有一个名字不存在时，powershell.exe **即使输出正常也返回退出码 1**，
      所以脚本末尾必须 `exit 0`，否则一次「查 3 条、1 条被外部删了」就被当成查询失败；
    - 光看退出码/空输出也分不清「NetSecurity 模块不可用」和「规则都不在」，
      故显式探测 cmdlet 是否存在并输出哨兵行 PS_PROBE——没有哨兵才是真查不动；
    - **只读 Enabled 不够**：文档承诺规则在 wf.msc 里可见可改，用户把「操作」从
      阻止改成允许后，Enabled 仍是 True，只看 Enabled 会把已经放行掉的程序报成
      「已禁止联网」——对这种安全控制属于实质性错报，所以连 Action 一起读。
    规则名由 rule_name() 生成（仅 ASCII 十六进制），仍对单引号做 PS 转义——
    不依赖「调用方一定传安全值」这个假设。
    """
    names = ",".join("'" + rule_name(p).replace("'", "''") + "'" for p in programs)
    return ("$ErrorActionPreference='SilentlyContinue';"
            "$ok=[bool](Get-Command Get-NetFirewallRule -ErrorAction SilentlyContinue);"
            "if($ok){ '" + PS_PROBE + "';"
            " Get-NetFirewallRule -Name " + names +
            ' | ForEach-Object { "$($_.Name)=$($_.Enabled)=$($_.Action)" } }; exit 0')


def parse_status_output(text: str) -> dict:
    """解析 PS 输出 → {规则名: 'on' | 'off' | 'drift'}。不认识的行忽略（哨兵与 PS 告警）。

    'on' = 启用且动作为阻止（真的在拦）；'off' = 规则在但已停用（放行，且是我们那条）；
    'drift' = 动作不再是 Block（多半是用户在 wf.msc 里把它改成了「允许」），
    这时无论启不启用都不拦——绝不能报「已禁止联网」。
    规则名由 rule_name() 生成、不含 '='，所以 split 出三段才是规则行；哨兵行不含 '='。
    """
    out = {}
    for line in (text or "").splitlines():
        parts = [x.strip() for x in line.strip().split("=")]
        if len(parts) != 3:
            continue
        name, enabled, action = parts
        if not name.startswith(RULE_PREFIX):
            continue
        if action.lower() != "block":
            out[name] = "drift"
        else:
            out[name] = "on" if enabled.lower() == "true" else "off"
    return out


# ---------------- 本地台账（stores JSON） ----------------

def load_entries() -> list:
    """受控程序清单：[{"program","rule","blocked","added"}]，按加入顺序。

    `program` 存**原始大小写**的绝对路径（展示与下发都用它）；比对一律走
    norm_path()，所以台账里的大小写与实际 exe 不一致也不影响命中。

    按 norm_path 去重（保留第一条）：json 可手工编辑、也可能来自大小写不同的旧版本，
    留着重复行会让表格出现两行同名程序——`find_entry` 恒命中第一条，用户选中第二行
    点「移除」会弹「已删除规则」却把另一条 pop 掉，提示骗人。
    """
    data = stores.load_json(STORE_NAME, {"entries": []})
    entries = data.get("entries") if isinstance(data, dict) else None
    result = []
    seen = set()
    for e in entries or []:
        if not isinstance(e, dict):
            continue
        program = real_path(e.get("program", ""))
        if not program:
            continue
        key = norm_path(program)
        if key in seen:
            LOGGER.warning("台账重复条目已忽略: {}", program)
            continue
        seen.add(key)
        result.append({"program": program,
                       "rule": e.get("rule") or rule_name(program),
                       "blocked": bool(e.get("blocked", True)),
                       "added": e.get("added", "")})
    return result


def save_entries(entries) -> bool:
    return stores.save_json(STORE_NAME, {"entries": list(entries)})


def find_entry(entries, program: str) -> int:
    target = norm_path(program)
    for i, e in enumerate(entries):
        if norm_path(e.get("program", "")) == target:
            return i
    return -1


# ---------------- 与防火墙交互 ----------------

def rule_exists(program: str):
    """netsh 回显里有没有我们的 ASCII 规则名——与系统语言无关的存在性判定。

    返回 True / False / **None（命令超时 = 不知道）**。netsh 查不到规则时的退出码
    随版本漂移（补52 记录为 0，补55 真机已实测为 1），所以既不能看退出码，也不能
    把超时折成 False：调用方一旦把"不知道"当成"不在"，就会 add 出第二条同名规则
    （netsh 不查重）。
    """
    r = proc.run(cmd_show(program), timeout=20)
    if r.timeout:
        return None
    return rule_name(program) in (r.stdout or "")


# netsh show 输出的状态字段（中英双语；字段值随系统语言变，但只在这两种内取值）
_NETSH_ENABLED_RE = re.compile(r"已启用\s*[:：]\s*(是|否)|Enabled\s*[:：]\s*(Yes|No)", re.I)
_NETSH_ACTION_RE = re.compile(r"操作\s*[:：]\s*(阻止|允许)|Action\s*[:：]\s*(Block|Allow)", re.I)


def netsh_state(program: str):
    """从 netsh show 输出解析规则态——PS 读侧的兜底。

    背景（补55 真机实测）：netsh 建的规则在这台机器的 Get-NetFirewallRule（CIM）
    视图里 **持续不可见**（实测 178s+ 不出现），而 netsh 自己立即可见——netsh 与
    CIM 存储存在分歧，PS 报 missing 不等于规则不存在。字段中英双语解析（本机中文
    输出「已启用: 是 / 操作: 阻止」，EN 机为 Enabled: Yes / Action: Block），
    只解析我们自己的 ASCII 规则名。
    返回 'on' / 'off' / 'drift' / 'missing' / None（读不到或字段解析不出）。
    """
    r = proc.run(cmd_show(program), timeout=20)
    if r.timeout:
        return None
    text = r.stdout or ""
    if rule_name(program) not in text:
        return "missing"
    en = _NETSH_ENABLED_RE.search(text)
    ac = _NETSH_ACTION_RE.search(text)
    if not en or not ac:
        LOGGER.warning("netsh 规则状态解析失败（字段不齐）：{}",
                       text.strip()[:160])
        return None
    enabled = (en.group(1) or en.group(2)) in ("是", "Yes")
    block = (ac.group(1) or ac.group(2)) in ("阻止", "Block")
    if not block:
        return "drift"          # 动作被外部改成允许：无论启停都不拦
    return "on" if enabled else "off"


def query_states(programs) -> dict:
    """{norm_path(程序): 'on' | 'off' | 'drift' | 'missing'}；PowerShell 不可用整体返回 None。

    返回 None 表示「没查动」而不是「规则都不在」——UI 据此显示「未校验」，
    绝不能把查询失败画成「已被外部删除」，那会诱导用户重复建规则。

    **键用 norm_path 而不是传入的原串**：查询与显示两边各拿一份台账路径，只要大小写
    或斜杠风格有一处不同，按原串做键就会整表查不到 → 全部显示「未校验」，
    看着像"查询失灵"。规范化键把这层耦合消掉。

    判据是**输出里有没有 PS_PROBE 哨兵**，不是退出码：真机上 `-Name` 只要有一个
    名字不存在，powershell.exe 就返回退出码 1 而 stdout 完全正常（脚本末尾用
    `exit 0` 兜住），拿退出码判失败恰好会把「规则被外部删了」这种最需要报告的
    情况误报成查询失败。
    """
    programs = [p for p in programs if p]
    if not programs:
        return {}
    r = proc.run_ps(_ps_status_script(programs), timeout=40)
    text = r.stdout or ""
    if r.timeout or PS_PROBE not in text:
        LOGGER.warning("防火墙规则状态查询不可用 code={} timeout={} out={} err={}",
                       r.code, r.timeout, text.strip()[:120], (r.stderr or "").strip()[:200])
        return None
    states = parse_status_output(text)
    return {norm_path(p): states.get(rule_name(p), "missing") for p in programs}


def actual_state(program: str) -> str | None:
    """回查单条规则的实际状态；两个读侧都给不出结论返回 None（=无法判定，不是「不在」）。

    PS（Get-NetFirewallRule）优先；它报 missing 或整体不可用时用 netsh 兜底——
    补55 真机实测 netsh 与 CIM 存储会分歧（netsh 建的规则 CIM 长期看不见），
    只信 PS 会把刚刚建好的规则误判成「被外部删了」。netsh 自己也读不出（None）
    时回到 PS 的结论：确定的事实优先于「不知道」。
    """
    states = query_states([program])
    ps_state = None if states is None else states.get(norm_path(program))
    if states is not None and ps_state != "missing":
        return ps_state
    net = netsh_state(program)
    if net is not None:
        return net
    return ps_state


def apply_state(program: str, blocked: bool) -> tuple:
    """把规则调到目标态：不在就新建，在就改 enable。幂等。返回 (ok, 消息)。"""
    if not admin.is_admin():
        return False, ADMIN_MSG
    enable = blocked
    exists = rule_exists(program)
    if exists is None:
        # 探测超时就当"不知道在不在"，绝不猜成不在然后 add——netsh 允许同名多条规则，
        # 猜错会留下两条同 exe 的块规则，之后 set/delete name= 同时命中两条，界面看不出来
        return False, "查询防火墙规则超时（服务可能正在启动），本次未改动任何规则，请稍后重试"
    cmd = cmd_set_enable(program, enable) if exists else cmd_add(program, enable)
    r = proc.run(cmd, timeout=20)
    if not r.ok:
        err = (r.stderr or r.stdout or "").strip()
        return False, f"防火墙命令失败：{err[:200] or f'退出码 {r.code}'}"
    return _confirm(program, "on" if blocked else "off",
                    "已禁止联网" if blocked else "已允许联网")


def _confirm(program: str, expect: str, ok_msg: str) -> tuple:
    """写完回查实际状态再报成败——netsh 的退出码不可信（见 PROGRESS 补充53）。

    真机已证实 netsh `show rule` 查不到时也返回退出码 0 且回显本地化文案，同一套
    语义下 `set rule name=<已不存在的规则>` 完全可能"成功"返回 0（规则被外部删了、
    或被第三方安全软件接管），此时若直接报「已禁止联网」并写台账，用户看到的是一条
    根本不存在的保护。回查不到（PS 不可用）时保留成功但**明说没回查**，不含糊。
    """
    actual = actual_state(program)
    if actual is None:
        return True, f"{ok_msg}（未能回查防火墙实际状态，可点「刷新状态」确认）"
    if actual == expect:
        return True, ok_msg
    if actual == "missing":
        return False, ("命令返回成功，但防火墙里查不到这条规则"
                       "（可能被安全软件 / 组策略接管），请用 wf.msc 检查")
    if actual == "drift":
        if expect == "off":
            # 动作已被外部改成「允许」= 本来就不拦，用户要的「放行」净效果已达成；
            # 保守起见不宣称这条规则归我们管，如实说明现状。
            return True, "已允许联网（该规则的动作此前已被外部改为「允许」）"
        return False, ("规则已存在但动作不是「阻止」（在 wf.msc 里被改成了允许），"
                       "SysKit 未覆盖该改动，请先删除或改回")
    return False, f"回查结果与目标不一致（期望 {expect}，实际 {actual}），请刷新状态后重试"


def delete_rule(program: str) -> tuple:
    if not admin.is_admin():
        return False, ADMIN_MSG
    r = proc.run(cmd_delete(program), timeout=20)
    if not r.ok:
        # 补55：netsh 无匹配时退出码已从 0 漂移为 1，且 CIM 分歧机器上「规则其实
        # 已被外部删掉」是常态——删除失败与否以读侧终态为准，不认退出码。
        if actual_state(program) == "missing":
            return True, "规则已不存在"
        err = (r.stderr or r.stdout or "").strip()
        return False, f"删除规则失败：{err[:200] or f'退出码 {r.code}'}"
    # 「本来就没有」与「删掉了」终态相同，所以这里只关心回查是否真的没了
    actual = actual_state(program)
    if actual is None:
        return True, "已删除规则（未能回查防火墙实际状态，可点「刷新状态」确认）"
    if actual != "missing":
        return False, (f"命令返回成功，但防火墙里这条规则仍在（实际 {actual}），"
                       "可能被安全软件或组策略保护")
    return True, "已删除规则"



# ---------------- 面向 UI 的组合操作 ----------------

def validate_program(program: str) -> str:
    """返回错误文本，空串表示可用。"""
    p = real_path(program)
    if not p:
        return "请先选择一个可执行文件。"
    if not norm_path(p).endswith(".exe"):
        return f"只支持可执行文件（.exe）：{os.path.basename(p)}"
    if not os.path.isfile(p):
        return f"文件不存在：{p}\n（路径写错的规则永远不会命中，等于没拦）"
    return ""


def add_program(program: str, blocked: bool = True) -> tuple:
    """加入清单并下发规则。失败不落台账——否则用户看到一条「已禁止」其实没生效。"""
    err = validate_program(program)
    if err:
        return False, err, None
    p = real_path(program)
    entries = load_entries()
    idx = find_entry(entries, p)
    if idx >= 0:
        # 已在清单里：不重复建条目，只把状态调到目标态（等价于「重新启用」）
        ok, msg = apply_state(p, blocked)
        if not ok:
            return False, msg, None
        entries[idx]["blocked"] = blocked
        if not save_entries(entries):
            return True, f"该程序已在清单中，{msg}（但清单保存失败）", idx
        return True, f"该程序已在清单中，{msg}", idx
    ok, msg = apply_state(p, blocked)
    if not ok:
        return False, msg, None
    entries.append({"program": p, "rule": rule_name(p), "blocked": blocked,
                    "added": time.strftime("%Y-%m-%d %H:%M")})
    if not save_entries(entries):
        # 规则已生效但台账没落盘：如实说清"这次界面也不会有这一行"，
        # 只说「下次启动不显示」会让人以为保存重试就能捞回来
        return True, (f"{msg}（但清单保存失败：规则已生效而本次界面不会显示这一行，"
                      "需到 wf.msc 手工处理）"), None
    return True, msg, len(entries) - 1


def set_program_blocked(program: str, blocked: bool, entries=None) -> tuple:
    """切换某个已入清单程序的目标态，成功后回写台账。返回 (ok, 消息)。"""
    p = real_path(program)
    own = entries is None
    if own:
        entries = load_entries()
    idx = find_entry(entries, p)
    if idx < 0:
        return False, "该程序不在清单中"
    ok, msg = apply_state(p, blocked)
    if not ok:
        return False, msg
    entries[idx]["blocked"] = blocked
    if own and not save_entries(entries):
        return True, f"{msg}（但清单保存失败）"
    return True, msg


def drop_program(program: str, entries=None) -> tuple:
    """从清单移除并删除规则。规则删不掉时保留台账（否则规则成了看不见的孤儿）。"""
    p = real_path(program)
    own = entries is None
    if own:
        entries = load_entries()
    idx = find_entry(entries, p)
    if idx < 0:
        return False, "该程序不在清单中"
    ok, msg = delete_rule(p)
    if not ok:
        return False, msg
    entries.pop(idx)
    if own and not save_entries(entries):
        return True, f"{msg}（但清单保存失败）"
    return True, msg
