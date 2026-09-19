"""syskit/netstats.py 纯逻辑聚合测试：断网配对/时长归属/跨零点/畸形 ts/流量 join。

红线：无 Qt、无网络、无系统调用；时间用固定字符串构造。
"""
import time

from syskit import netstats


def _ts(y, mo, d, h, mi, s):
    return time.strftime("%Y-%m-%d %H:%M:%S", time.strptime(
        f"{y:04d}-{mo:02d}-{d:02d} {h:02d}:{mi:02d}:{s:02d}", netstats._TS_FMT))


def test_empty_inputs():
    rows = netstats.daily_report([], days=7)
    assert len(rows) == 7
    assert rows[-1]["date"] == time.strftime("%Y-%m-%d")
    assert all(r["outages"] == 0 and not r["has_events"] and not r["has_traffic"]
               for r in rows)


def test_outage_pair_counts_once_with_duration():
    today = time.strftime("%Y-%m-%d")
    events = [  # 最新在前（EventStore 原始顺序）
        {"ts": _ts(*_today(10, 0, 5)), "state": "ok", "detail": ""},
        {"ts": _ts(*_today(10, 0, 0)), "state": "no_net", "detail": ""},
    ]
    rows = netstats.daily_report(events, days=3)
    last = rows[-1]
    assert last["date"] == today
    assert last["outages"] == 1
    assert last["outage_sec"] == 5
    assert last["has_events"] is True
    assert rows[-2]["has_events"] is False       # 断网归属开始日（同日场景下别的天为 0）


def _today(h, mi, s):
    t = time.localtime()
    return (t.tm_year, t.tm_mon, t.tm_mday, h, mi, s)


def test_cross_midnight_duration_attributed_to_start_day():
    t = time.localtime()
    y, mo, d = t.tm_year, t.tm_mon, t.tm_mday
    events = [
        {"ts": _ts(y, mo, d, 0, 5, 0), "state": "ok", "detail": ""},      # 今天 00:05 恢复
        {"ts": _ts(*_yesterday(23, 59, 50)), "state": "no_net", "detail": ""},  # 昨晚断
    ]
    rows = netstats.daily_report(events, days=3)
    yesterday = rows[-2]
    today_row = rows[-1]
    assert yesterday["outages"] == 1
    assert yesterday["outage_sec"] == 310     # 整段时长（23:59:50→00:05:00）计入开始日（昨天）
    assert today_row["outages"] == 0 and today_row["outage_sec"] == 0
    assert today_row["has_events"] is True    # 今天有事件（恢复）


def _yesterday(h, mi, s):
    t = time.localtime(time.time() - 86400)
    return (t.tm_year, t.tm_mon, t.tm_mday, h, mi, s)


def test_malformed_ts_skipped_but_others_counted():
    today = time.strftime("%Y-%m-%d")
    events = [
        {"ts": "not-a-date", "state": "ok", "detail": ""},               # 畸形 → 跳过
        {"ts": _ts(*_today(9, 0, 0)), "state": "no_net", "detail": ""},
        {"ts": _ts(*_today(8, 59, 30)), "state": "ok", "detail": ""},
        {"ts": _ts(*_today(8, 59, 0)), "state": "offline", "detail": ""},
    ]
    rows = netstats.daily_report(events, days=2)
    last = rows[-1]
    # 畸形行不参与配对；offline→ok（30s）计一次；no_net 无恢复不计时长但计次
    assert last["outages"] == 2
    assert last["outage_sec"] == 30


def test_outage_without_recovery_no_duration():
    today = time.strftime("%Y-%m-%d")
    events = [{"ts": _ts(*_today(8, 0, 0)), "state": "no_net", "detail": ""}]
    rows = netstats.daily_report(events, days=1)
    assert rows[0]["outages"] == 1 and rows[0]["outage_sec"] == 0


def test_first_event_no_predecessor_duration():
    today = time.strftime("%Y-%m-%d")
    events = [{"ts": _ts(*_today(8, 0, 0)), "state": "ok", "detail": ""}]
    rows = netstats.daily_report(events, days=1)
    assert rows[0]["outages"] == 0 and rows[0]["outage_sec"] == 0


def test_traffic_join_and_missing_days_not_interpolated():
    today = time.strftime("%Y-%m-%d")
    t = time.localtime(time.time() - 86400)
    ykey = time.strftime("%Y-%m-%d", t)
    traffic = {today: {"sent": 100, "recv": 900}}      # 昨天无记录（应用未运行）
    rows = netstats.daily_report([], days=2, traffic=traffic)
    assert rows[-1]["sent"] == 100 and rows[-1]["recv"] == 900
    assert rows[-1]["has_traffic"] is True
    assert rows[0]["has_traffic"] is False             # 无记录 ≠ 0
    assert rows[0]["date"] == ykey


def test_negative_traffic_clamped():
    today = time.strftime("%Y-%m-%d")
    rows = netstats.daily_report([], days=1,
                                 traffic={today: {"sent": -5, "recv": 10}})
    assert rows[0]["sent"] == 0 and rows[0]["recv"] == 10
