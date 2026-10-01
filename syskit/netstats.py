"""网络质量日报聚合：从断网事件与按天流量推导日粒度统计（纯逻辑，无 Qt）。

数据来源与语义：
- events：EventStore 的状态切换记录 [{ts: "%Y-%m-%d %H:%M:%S", state, detail}]，
  最新在前；相邻「非 ok → ok」事件对即一次断网，时长为两 ts 差。
- traffic：TrafficStore.days() 的 {"YYYY-MM-DD": {"sent", "recv"}}；应用未运行
  的日子无键（不插值，显示层显示「—」）。

边界约定：
- 断网时长归属「断网开始日」（跨零点整段计入起始日）；
- 首条事件无前驱、畸形/不可解析 ts：跳过该段时长（次数照记）；
- state != "ok" 一律视为断网态（ncsi_false/offline 均如此）。
"""
import time

_TS_FMT = "%Y-%m-%d %H:%M:%S"


def _parse_ts(ts: str):
    """事件时间戳 → 本地时间元组；畸形返回 None。"""
    try:
        return time.strptime(str(ts), _TS_FMT)
    except (TypeError, ValueError):
        return None


def daily_report(events: list, days: int = 7, traffic: dict = None) -> list:
    """最近 days 天（含今天）的日粒度统计，旧 → 新排序。

    返回 [{date, outages, outage_sec, sent, recv, has_events, has_traffic}]；
    has_events/has_traffic 标记该日是否有真实数据（区分「0 次」与「无记录」）。
    """
    traffic = traffic or {}
    out = []
    today = time.localtime()
    day_keys = []
    for offset in range(days - 1, -1, -1):
        d = time.localtime(time.mktime((today.tm_year, today.tm_mon, today.tm_mday,
                                        0, 0, 0, 0, 0, -1)) - offset * 86400)
        key = time.strftime("%Y-%m-%d", d)
        day_keys.append(key)
        out.append({"date": key, "outages": 0, "outage_sec": 0,
                    "sent": 0, "recv": 0, "has_events": False, "has_traffic": False})
    by_date = {r["date"]: r for r in out}

    # 流量列
    for key, day in by_date.items():
        t = traffic.get(key)
        if isinstance(t, dict):
            day["sent"] = max(0, int(t.get("sent", 0)))
            day["recv"] = max(0, int(t.get("recv", 0)))
            day["has_traffic"] = True

    # 断网配对：事件最新在前 → 先反转成时间正序，以「事故」为单位维护断网段。
    # （审计第 9 批 C-1：no_net/offline 互切是同一次故障——自动修复拍打网卡会让
    # 两态交替——原实现各自 +1 且时长只计最后一段，一次断网被计成多次/短计。）
    chrono = []
    for ev in reversed(events or []):
        st = _parse_ts(ev.get("ts"))
        if st is None:
            continue
        chrono.append((st, str(ev.get("state", ""))))
    prev_down = False
    down_start = None
    for st, state in chrono:
        key = time.strftime("%Y-%m-%d", st)
        day = by_date.get(key)                 # None = 超出统计窗口：参与配对、不计数
        now_down = state != "ok"
        if now_down:
            if not prev_down:
                down_start = st                # 本次事故的开始时刻（含超窗前驱）
                if day is not None:
                    day["outages"] += 1
            if day is not None:
                day["has_events"] = True
        else:
            if day is not None:
                day["has_events"] = True
            if prev_down and down_start is not None:
                # 断网恢复：整段时长归属断网开始日
                start_key = time.strftime("%Y-%m-%d", down_start)
                start_day = by_date.get(start_key)
                sec = max(0, int(time.mktime(st) - time.mktime(down_start)))
                if start_day is not None:
                    start_day["outage_sec"] += sec
            down_start = None
        prev_down = now_down
    return out


# ---------------- 多粒度流量查询（RateStatsStore 快照 → 柱状序列） ----------------

_GRAN_WINDOW = {"minute": 60, "hour": 48, "day": 30, "month": 12}


def known_ssids(stats: dict) -> list:
    """快照中出现过的网络名（含「以太网/未知」），名称排序。"""
    names = set()
    for table in (stats.get("minutes", {}), stats.get("hours", {}),
                  stats.get("days", {})):
        for rec in table.values():
            names.update(str(n) for n in rec.get("wifi", {}))
    return sorted(names)


def range_series(granularity: str, stats: dict, ssid=None) -> dict:
    """粒度区间序列（时间正序）+ 区间总量；ssid=None 全部，否则只取该网络。

    granularity：minute(近60分) / hour(近48时) / day(近30天) / month(近12月，
    由天桶按 YYYY-MM 派生，不另存)。返回
    {"rows": [{"key","sent","recv"}], "sent": 总上行, "recv": 总下行}。
    """
    if granularity not in _GRAN_WINDOW:
        raise ValueError(f"未知粒度: {granularity}")
    win = _GRAN_WINDOW[granularity]
    if granularity == "minute":
        table = stats.get("minutes", {})
    elif granularity == "hour":
        table = stats.get("hours", {})
    else:                                       # day / month（月由天桶派生）
        table = stats.get("days", {})

    def _pick(rec: dict):
        if ssid is None:
            return max(0, int(rec.get("sent", 0))), max(0, int(rec.get("recv", 0)))
        w = rec.get("wifi", {}).get(str(ssid)) or {}
        return max(0, int(w.get("sent", 0))), max(0, int(w.get("recv", 0)))

    if granularity == "month":
        grouped = {}
        for key in sorted(table):
            s, r = _pick(table[key])
            agg = grouped.setdefault(key[:7], {"sent": 0, "recv": 0})
            agg["sent"] += s
            agg["recv"] += r
        rows = [{"key": k, "sent": grouped[k]["sent"], "recv": grouped[k]["recv"]}
                for k in sorted(grouped)[-win:]]
    else:
        # 柱标签：分钟取 "HH:MM"、小时取 "HH时"、天取 "MM-DD"（月已含年月）
        def _label(key: str) -> str:
            if granularity == "minute":
                return key[-5:]
            if granularity == "hour":
                return key[11:] + "时"
            return key[5:]

        rows = [{"key": _label(key), "sent": s, "recv": r}
                for key in sorted(table)[-win:]
                for s, r in [_pick(table[key])]]
    total_s = sum(r["sent"] for r in rows)
    total_r = sum(r["recv"] for r in rows)
    return {"rows": rows, "sent": total_s, "recv": total_r}
