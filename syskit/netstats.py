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

    # 断网配对：事件最新在前 → 先反转成时间正序，维护上一状态
    chrono = []
    for ev in reversed(events or []):
        st = _parse_ts(ev.get("ts"))
        if st is None:
            continue
        chrono.append((st, str(ev.get("state", ""))))
    prev_ts = None
    prev_down = False
    for st, state in chrono:
        key = time.strftime("%Y-%m-%d", st)
        day = by_date.get(key)
        if day is None:
            # 超出统计窗口的事件仍参与配对（作为前驱），但本身不计数
            prev_ts, prev_down = st, state != "ok"
            continue
        day["has_events"] = True
        now_down = state != "ok"
        if now_down:
            day["outages"] += 1
        elif prev_down and prev_ts is not None:
            # 断网恢复：时长归属断网开始日（prev 所在日）
            start_key = time.strftime("%Y-%m-%d", prev_ts)
            start_day = by_date.get(start_key)
            sec = max(0, int(time.mktime(st) - time.mktime(prev_ts)))
            if start_day is not None:
                start_day["outage_sec"] += sec
        prev_ts, prev_down = st, now_down
    return out
