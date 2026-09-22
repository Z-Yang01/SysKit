"""syskit/stores.py 单元测试：TrafficStore 按天流量累计 + EventStore 事件时间线（纯逻辑 + 临时目录落盘）。

Mock 边界（配合 conftest 哨兵双保险）：
- 持久化路径由 conftest.isolated_appdata 重定向到 pytest 临时目录，真实 %APPDATA% 不被触碰；
- "今天"通过替换 syskit.stores 的 time 模块引用固定（不动全局 time），验证跨天与 62 天修剪；
- 异常路径用损坏 JSON / 目录占位 / 目录阻塞 os.replace 构造，不发真实系统调用。

已修复行为（原迭代日志疑似 bug）：
1. TrafficStore.__init__ 对 _load 返回值做 isinstance(dict) 校验，非 object JSON 置空。
2. save_if_dirty 仅在 _save 成功时清 _dirty；写盘失败保留脏标记待下次重试。
"""
import json
import os
import types


from syskit import config, stores


# ---------------- 工具 ----------------

def _read_json(name):
    with open(os.path.join(config.APPDATA_DIR, name), "r", encoding="utf-8") as f:
        return json.load(f)


def _write_raw(name, text):
    os.makedirs(config.APPDATA_DIR, exist_ok=True)
    with open(os.path.join(config.APPDATA_DIR, name), "w", encoding="utf-8") as f:
        f.write(text)


def _write_json(name, data):
    _write_raw(name, json.dumps(data, ensure_ascii=False))


def fix_today(monkeypatch, day="2026-09-10"):
    """把 stores 模块内的 time 引用换成固定日期假模块（add/today 均读它）。"""
    monkeypatch.setattr(stores, "time", types.SimpleNamespace(strftime=lambda fmt: day))
    return day


# ---------------- _load / _save ----------------

class TestLoad:
    def test_missing_file_returns_default(self):
        assert stores._load("nope.json", {"d": 1}) == {"d": 1}

    def test_valid_json_roundtrip(self, tmp_path):
        _write_json("ok.json", {"k": "值", "n": 3})
        assert stores._load("ok.json", {}) == {"k": "值", "n": 3}

    def test_corrupt_json_returns_default(self, tmp_path):
        _write_raw("bad.json", "{oops not json")
        assert stores._load("bad.json", ["fallback"]) == ["fallback"]

    def test_directory_as_file_returns_default(self, tmp_path):
        """目标名是目录时 open 抛 OSError → 返回默认值而非崩溃。"""
        os.makedirs(os.path.join(config.APPDATA_DIR, "dir.json"))
        assert stores._load("dir.json", {"x": 0}) == {"x": 0}


class TestSave:
    def test_save_creates_file_and_no_tmp_left(self, tmp_path):
        stores._save("data.json", {"中文": "值"})
        assert _read_json("data.json") == {"中文": "值"}
        assert not os.path.exists(os.path.join(config.APPDATA_DIR, "data.json.tmp"))

    def test_save_oserror_swallowed(self, tmp_path):
        """os.replace 目标被目录占用 → PermissionError(OSError) 被静默吞掉。"""
        os.makedirs(os.path.join(config.APPDATA_DIR, "blocked.json"))
        stores._save("blocked.json", {"a": 1})          # 不应抛异常
        assert os.path.isdir(os.path.join(config.APPDATA_DIR, "blocked.json"))


# ---------------- TrafficStore ----------------

class TestTrafficStore:
    def test_init_loads_existing_data(self):
        _write_json("traffic.json", {"2026-09-09": {"sent": 11, "recv": 22}})
        s = stores.TrafficStore()
        assert s._data == {"2026-09-09": {"sent": 11, "recv": 22}}

    def test_add_accumulates_and_clamps_negative(self):
        s = stores.TrafficStore()
        s.add(100, 200)
        s.add(50, -5)                 # 负数被钳到 0
        assert s.today() == (150, 200)

    def test_add_coerces_non_int(self):
        s = stores.TrafficStore()
        s.add("70", 12.9)             # int("70")=70, int(12.9)=12
        assert s.today() == (70, 12)

    def test_today_empty(self):
        assert stores.TrafficStore().today() == (0, 0)

    def test_day_rollover_resets_today(self, monkeypatch):
        d1 = fix_today(monkeypatch, "2026-09-09")
        s = stores.TrafficStore()
        s.add(10, 10)
        fix_today(monkeypatch, "2026-09-10")
        assert s.today() == (0, 0)
        assert s._data[d1] == {"sent": 10, "recv": 10}

    def test_prune_keeps_at_most_62_days(self, monkeypatch):
        """>62 天时按日期序修剪最老的，当天记录保留。"""
        fix_today(monkeypatch, "2026-09-10")
        s = stores.TrafficStore()
        from datetime import date, timedelta
        base = date(2026, 7, 2)                      # 递增 70 天，最后一天 = 2026-09-09
        old_days = [str(base + timedelta(days=i)) for i in range(70)]
        for i, d in enumerate(old_days):
            s._data[d] = {"sent": i, "recv": i}
        s.add(1, 1)
        assert len(s._data) == 62
        assert old_days[0] not in s._data            # 最老的 9 天被删（70+1-62）
        assert old_days[8] not in s._data
        assert old_days[9] in s._data                # 第 10 老的开始保留
        assert "2026-09-10" in s._data

    def test_save_if_dirty_writes_and_resets(self, monkeypatch):
        day = fix_today(monkeypatch, "2026-09-10")  # 固定日期：断言不得依赖真实当天
        s = stores.TrafficStore()
        s.add(7, 8)
        s.save_if_dirty()
        assert _read_json("traffic.json") == {day: {"sent": 7, "recv": 8}}
        assert s._dirty is False
        # 不脏时不再写盘
        called = []
        monkeypatch.setattr(stores, "_save", lambda name, data: called.append(name))
        s.save_if_dirty()
        assert called == []

    def test_save_failure_keeps_dirty_for_retry(self, tmp_path):
        """写盘失败时保留 _dirty，下次 save_if_dirty 重试。"""
        os.makedirs(os.path.join(config.APPDATA_DIR, "traffic.json"))
        s = stores.TrafficStore()
        s.add(5, 5)
        s.save_if_dirty()                            # 不抛异常
        assert s._dirty is True                      # 失败不清脏标记

    def test_loaded_non_dict_resets_to_empty(self, tmp_path):
        """traffic.json 是合法 JSON list 时重置为空 dict，add() 不崩。"""
        _write_json("traffic.json", ["not", "a", "dict"])
        s = stores.TrafficStore()
        assert s._data == {}
        s.add(1, 1)
        assert s.today() == (1, 1)


# ---------------- EventStore ----------------

class TestEventStore:
    def test_init_empty(self):
        assert stores.EventStore().items() == []

    def test_init_loads_existing_list(self):
        _write_json("events.json", [{"ts": "t1", "state": "down"}])
        assert [e["state"] for e in stores.EventStore().items()] == ["down"]

    def test_init_non_list_resets(self, tmp_path):
        """events.json 是 dict（损坏）→ 视为空而不是崩溃。"""
        _write_json("events.json", {"bad": True})
        assert stores.EventStore().items() == []

    def test_add_newest_first_and_persisted(self, tmp_path):
        s = stores.EventStore()
        s.add("no_net", "detail-1")
        s.add("ok", "detail-2")
        items = s.items()
        assert [e["state"] for e in items] == ["ok", "no_net"]
        assert set(items[0]) == {"ts", "state", "detail"}
        assert _read_json("events.json") == items     # add 即落盘

    def test_add_truncates_detail_to_120(self, tmp_path):
        s = stores.EventStore()
        s.add("no_net", "x" * 300)
        assert len(s.items()[0]["detail"]) == 120

    def test_limit_enforced(self, tmp_path):
        s = stores.EventStore(limit=3)
        for i in range(5):
            s.add(f"s{i}")
        assert [e["state"] for e in s.items()] == ["s4", "s3", "s2"]

    def test_items_returns_copy(self, tmp_path):
        s = stores.EventStore()
        s.add("ok")
        got = s.items()
        got.append({"fake": True})
        assert len(s.items()) == 1

    def test_recent(self, tmp_path):
        s = stores.EventStore()
        for i in range(5):
            s.add(f"s{i}")
        assert [e["state"] for e in s.recent(2)] == ["s4", "s3"]
        assert len(s.recent()) == 5                   # 默认 30 > 总数
        assert s.recent(100) == s.items()


# ---------------- 公共 JSON 包装（speedtest.json 等新持久化文件） ----------------

def test_load_save_json_roundtrip_and_corrupt_fallback(tmp_path, monkeypatch):
    monkeypatch.setattr(config, "APPDATA_DIR", str(tmp_path))
    assert stores.load_json("speedtest.json", []) == []          # 缺省
    assert stores.save_json("speedtest.json", [{"mbps": 1.5}]) is True
    assert stores.load_json("speedtest.json", []) == [{"mbps": 1.5}]
    (tmp_path / "speedtest.json").write_text("{半截", encoding="utf-8")
    assert stores.load_json("speedtest.json", ["fallback"]) == ["fallback"]


def test_traffic_store_days_returns_copy():
    s = stores.TrafficStore()
    s.add(10, 20)
    days = s.days()
    assert set(days.keys()) == {stores.time.strftime("%Y-%m-%d")}
    days[stores.time.strftime("%Y-%m-%d")]["sent"] = 99999       # 改拷贝不影响内部
    assert s.days()[stores.time.strftime("%Y-%m-%d")]["sent"] == 10
