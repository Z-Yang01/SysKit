"""命令面板动作注册表与模糊匹配（纯逻辑，无 Qt）。

动作 dict 结构：{"id", "label", "hint", "keywords", "callback"}
- callback 由 UI 层注入（切换页面、运行修复等），执行后由 UI 负责关闭面板。
- build_actions(window) 生成内置动作；面板运行时再叠加动态条目
  （剪贴板最近条目、密码库全局搜索、URL / ping / dns 语法）。
"""


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
