"""命令面板动作注册表与模糊匹配（纯逻辑，无 Qt）。

动作 dict 结构：{"id", "label", "hint", "keywords", "callback"}
- callback 由 UI 层注入（切换页面、运行修复等），执行后由 UI 负责关闭面板。
- build_actions(window) 生成内置动作；面板运行时再叠加动态条目
  （剪贴板最近条目、密码库全局搜索、URL / ping / dns 语法、算式直算）。
"""
import ast
import operator
import re

# 算式直算：仅 ASCII 数字与四则/括号/百分号构成（** 幂由两个 * 组成同样命中）。
# 用 [0-9] 而非 \d：ast.parse 只接受 ASCII 数字字面量，\d 放行阿拉伯-印度数字等
# 只会让 looks_like_math 与求值器口径不一致（审计第 9 批 D3）。
_MATH_RE = re.compile(r"^[0-9\s\+\-\*/\(\)\.%]+$")

_OPS = {
    ast.Add: operator.add, ast.Sub: operator.sub, ast.Mult: operator.mul,
    ast.Div: operator.truediv, ast.FloorDiv: operator.floordiv,
    ast.Mod: operator.mod, ast.Pow: operator.pow,
    ast.USub: operator.neg, ast.UAdd: operator.pos,
}


def looks_like_math(q: str) -> bool:
    """输入像算式：仅数字/运算符/括号构成，且至少含一个运算符与一个数字。"""
    q = (q or "").strip()
    if not _MATH_RE.match(q):
        return False
    return any(op in q for op in "+-*/%") and any(ch in "0123456789" for ch in q)


def safe_calc(expr: str):
    """AST 安全求值（绝不用 eval）：仅支持数字四则/幂/取模/括号。

    返回 int（整值）或 float；未知节点/溢出/超界幂抛 ValueError，语法不通抛
    SyntaxError（ast.parse 的行为，调用方两种都要接）。"""
    node = ast.parse((expr or "").strip(), mode="eval")

    def _ev(n):
        if isinstance(n, ast.Expression):
            return _ev(n.body)
        if isinstance(n, ast.Constant) and isinstance(n.value, (int, float)):
            return n.value
        if isinstance(n, ast.BinOp) and type(n.op) in _OPS:
            left, right = _ev(n.left), _ev(n.right)
            if isinstance(n.op, ast.Pow) and abs(right) > 1000:
                raise ValueError("指数过大")
            if (isinstance(n.op, ast.Pow) and isinstance(left, int)
                    and isinstance(right, int)
                    and left.bit_length() * abs(right) > 200_000):
                # 右操作数 ≤1000 只挡字面量指数；括号左积链 ((10**1000)**1000)**1000
                # 的右操作数全是 1000，结果却是 3.3M/3.3G bit——逐层在乘方**计算前**
                # 按 bit_length 估结果规模，超界即拒（面板在 GUI 线程同步求值，
                # 一条小时级大幂就是整个界面冻结，审计第 9 批 D1 实测）
                raise ValueError("结果过大")
            try:
                return _OPS[type(n.op)](left, right)
            except (OverflowError, ZeroDivisionError) as e:
                raise ValueError(str(e)) from e
        if isinstance(n, ast.UnaryOp) and type(n.op) in _OPS:
            return _OPS[type(n.op)](_ev(n.operand))
        raise ValueError(f"不支持的表达式元素: {type(n).__name__}")

    val = _ev(node)
    if isinstance(val, float) and (val != val or val in (float("inf"), float("-inf"))):
        raise ValueError("结果溢出")
    return int(val) if isinstance(val, float) and val.is_integer() else val


def fuzzy_score(query: str, text: str):
    """子序列匹配得分；不匹配返回 None。连续命中/前缀/词首加分，短文本微加分。"""
    q = (query or "").lower().replace(" ", "")
    t = (text or "").lower()
    if not q:
        return 0.0
    if not t:
        return None
    score, ti, streak = 0.0, 0, 0
    for ch in q:
        idx = t.find(ch, ti)
        if idx < 0:
            return None
        streak = streak + 1 if idx == ti else 1
        score += 1 + streak * 0.5
        if idx == 0:
            score += 2
        elif ti == 0 or t[ti - 1] in " -_./":
            score += 1          # 词首
        ti = idx + 1
    return score - len(t) * 0.01


def match_action(query: str, action: dict):
    """label 为主，keywords 辅助（半权）。返回得分或 None。"""
    s = fuzzy_score(query, action.get("label", ""))
    kw = " ".join(action.get("keywords") or [])
    if kw:
        s2 = fuzzy_score(query, kw)
        if s2 is not None:
            s = max(s if s is not None else 0.0, s2 * 0.5)
    return s


def rank_actions(query: str, actions: list) -> list:
    """按得分降序返回可行动作；query 为空时原样返回全部。"""
    if not (query or "").strip():
        return list(actions)
    scored = []
    for a in actions:
        s = match_action(query, a)
        if s is not None:
            scored.append((s, a))
    scored.sort(key=lambda x: -x[0])
    return [a for _, a in scored]


def load_custom_commands() -> list:
    """settings.palette_commands（JSON 字符串）→ [{name, type, value}]。

    type: "url"（浏览器打开）| "cmd"（确认后执行）。解析失败返回 []。
    """
    import json
    from . import settings
    raw = settings.load().get("palette_commands", "")
    try:
        data = json.loads(raw) if raw else []
    except Exception:
        return []
    out = []
    for it in data if isinstance(data, list) else []:
        if isinstance(it, dict) and it.get("name") and it.get("value"):
            out.append({"name": str(it["name"]), "type": str(it.get("type", "url")),
                        "value": str(it["value"])})
    return out


def build_actions(window) -> list:
    """内置动作注册表：页面跳转、页内 Tab、快捷诊断修复、自定义命令。

    window 需提供：stack / nav / page_*（见 main_window）。
    """
    actions = []

    # 页面跳转（与 nav 定义保持一致）
    pages = [
        ("🏠 概览", 0), ("📈 网络监控", 1), ("📶 WiFi 工具", 2), ("🔍 诊断", 3),
        ("🔧 修复", 4), ("⚡ 工具箱", 5), ("🧰 系统助手", 6),
        ("🔐 密码库", 7), ("⚙ 设置", 8), ("📋 日志", 9),
    ]
    for label, idx in pages:
        actions.append({
            "id": f"nav:{idx}", "label": f"打开 {label}", "hint": "页面",
            "keywords": ["goto", "页面", "打开"],
            "callback": (lambda i=idx: window.switch_page(i)),
        })

    # 页内 Tab（工具箱 / 系统助手 / WiFi）
    def _tab_actions(page, page_label, page_idx):
        tabs = getattr(page, "tabs", None)
        if not tabs:
            return
        for i in range(tabs.count()):
            tab_name = tabs.tabText(i)

            def _go(pi=page_idx, ti=i):
                window.switch_page(pi)
                w = window.stack.widget(pi)
                if getattr(w, "tabs", None):
                    w.tabs.setCurrentIndex(ti)

            actions.append({
                "id": f"tab:{page_idx}:{i}", "label": f"{page_label} · {tab_name}",
                "hint": "工具", "keywords": [tab_name],
                "callback": _go,
            })

    for obj, label, idx in ((getattr(window, "page_toolbox", None), "工具箱", 5),
                            (getattr(window, "page_system", None), "系统助手", 6),
                            (getattr(window, "page_vault", None), "密码库", 7)):
        if obj is not None:
            _tab_actions(obj, label, idx)

    # 快捷动作
    actions.append({
        "id": "quick:diag", "label": "立即快速诊断", "hint": "网络",
        "keywords": ["diagnose", "体检", "检测"],
        "callback": lambda: window.run_diagnosis(quick=True),
    })
    actions.append({
        "id": "quick:deep", "label": "深度诊断（全部检查）", "hint": "网络",
        "keywords": ["diagnose", "完整"],
        "callback": lambda: window.run_diagnosis(quick=False),
    })
    actions.append({
        "id": "quick:flush", "label": "刷新 DNS 缓存", "hint": "网络",
        "keywords": ["flush", "修复", "dns"],
        "callback": lambda: window.run_repair("flush_dns"),
    })
    actions.append({
        "id": "quick:show", "label": "显示主界面", "hint": "窗口",
        "keywords": ["show", "主窗口"],
        "callback": window.showNormal,
    })

    # 自定义命令（url / cmd）
    for i, cmd in enumerate(load_custom_commands()):
        if cmd["type"] == "cmd":
            actions.append({
                "id": f"cmd:{i}", "label": cmd["name"], "hint": "运行命令",
                "keywords": ["run", "脚本", "命令"],
                "callback": (lambda v=cmd["value"]: window.run_palette_command(v)),
            })
        else:
            actions.append({
                "id": f"url:{i}", "label": cmd["name"], "hint": "打开网址",
                "keywords": ["open", "url", "网站"],
                "callback": (lambda v=cmd["value"]: window.run_palette_command(v)),
            })
    return actions
