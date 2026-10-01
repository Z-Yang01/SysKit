"""多粒度流量统计：RateStatsStore 桶/归并/留存 + netstats 查询 + UI 冒烟。

时间依赖处理：add() 用真实 localtime（当前分钟/天桶）；小时归并用白盒改
_cur_hour 触发（跨整点场景），月派生直接注入跨月天桶键——均不需要 sleep。
"""
import time

import pytest

from syskit import netstats
from syskit.stores import RateStatsStore, EventStore
from syskit.ui.dashboard_page import DashboardPage
from syskit.ui.charts import human_bytes


@pytest.fixture()
def store():
    return RateStatsStore("rate_stats_test.json")


class TestRateStatsStore:
    def test_add_accumulates_minute_and_day_with_wifi(self, store):
        store.add(100, 200, "HomeWiFi")
        store.add(50, 80, "HomeWiFi")
        snap = store.snapshot()
        assert len(snap["minutes"]) == 1 and len(snap["days"]) == 1
        mrec = next(iter(snap["minutes"].values()))
        assert mrec["sent"] == 150 and mrec["recv"] == 280
        assert mrec["wifi"]["HomeWiFi"] == {"sent": 150, "recv": 280}
        assert next(iter(snap["days"].values()))["wifi"]["HomeWiFi"]["recv"] == 280

    def test_ssid_switch_within_minute_split(self, store):
        """分钟内换网：两个网络各自累计，总量为两者之和。"""
        store.add(100, 100, "A")
        store.add(30, 40, "B")
        mrec = next(iter(store.snapshot()["minutes"].values()))
        assert mrec["wifi"]["A"]["sent"] == 100
        assert mrec["wifi"]["B"]["sent"] == 30
        assert mrec["sent"] == 130

    def test_empty_ssid_goes_to_unknown_bucket(self, store):
        store.add(10, 10, "")
        store.add(5, 5, None)               # 缺省沿用 set_ssid/未知
        rec = next(iter(store.snapshot()["minutes"].values()))
        assert rec["wifi"] == {RateStatsStore.UNKNOWN: {"sent": 15, "recv": 15}}

    def test_zero_delta_not_marked_dirty(self, store):
        store.add(0, 0, "A")
        assert store._dirty is False        # 空转不写盘（与 TrafficStore 同语义）

    def test_hour_rollup_on_boundary(self, store):
        """滚动时钟无关：上一小时的遗留分钟桶在下一次 add 时被归并（含 wifi
        映射）；跨整点无流量/新小时冷启动同样补齐（每次 add 都扫，非仅边界）。"""
        store.add(100, 200, "HomeWiFi")                 # 当前小时的正常桶
        prev = time.localtime(time.mktime(time.localtime()) - 7200)
        pfx = time.strftime("%Y-%m-%dT%H", prev)        # 两小时前
        store._minutes[pfx + ":07"] = {"sent": 500, "recv": 700,
                                       "wifi": {"OldWifi": {"sent": 500, "recv": 700}}}
        store.add(1, 1, "NewWifi")                      # 触发滚动
        snap = store.snapshot()
        assert pfx in snap["hours"]                     # 遗留分钟桶归并进其小时桶
        hrec = snap["hours"][pfx]
        assert hrec["wifi"]["OldWifi"]["sent"] == 500
        cur_hour = time.strftime("%Y-%m-%dT%H")
        assert all(k.startswith(cur_hour) for k in snap["minutes"])  # 陈旧分钟桶已清
        assert snap["minutes"][cur_hour + time.strftime(":%M")]["wifi"]["NewWifi"]["sent"] == 1

    def test_retention_prune(self, store):
        cap = RateStatsStore.MINUTE_RETAIN
        for i in range(cap):
            store._minutes[f"m{i:05d}"] = {"sent": 0, "recv": 0, "wifi": {}}
        store.add(1, 1, "A")                     # +1 超限 → 剪回 cap（最旧先出）
        assert len(store._minutes) <= cap
        assert "m00000" not in store._minutes

    def test_persistence_roundtrip(self, store):
        store.add(123, 456, "HomeWiFi")
        store.save_if_dirty()
        again = RateStatsStore("rate_stats_test.json")
        snap = again.snapshot()
        assert next(iter(snap["minutes"].values()))["wifi"]["HomeWiFi"] == \
            {"sent": 123, "recv": 456}
        assert again._dirty is False        # 新实例不误判脏


class TestRangeSeries:
    def _snap(self):
        return {
            "minutes": {"2026-03-01T10:00": {"sent": 10, "recv": 100,
                                             "wifi": {"A": {"sent": 10, "recv": 100}}},
                        "2026-03-01T10:01": {"sent": 20, "recv": 200,
                                             "wifi": {"A": {"sent": 5, "recv": 50},
                                                      "B": {"sent": 15, "recv": 150}}}},
            "hours": {"2026-03-01T10": {"sent": 30, "recv": 300,
                                        "wifi": {"A": {"sent": 15, "recv": 150},
                                                 "B": {"sent": 15, "recv": 150}}}},
            "days": {"2026-03-01": {"sent": 30, "recv": 300,
                                    "wifi": {"A": {"sent": 15, "recv": 150}}},
                     "2026-02-01": {"sent": 1, "recv": 2,
                                    "wifi": {"A": {"sent": 1, "recv": 2}}},
                     "2026-01-15": {"sent": 7, "recv": 9,
                                    "wifi": {"B": {"sent": 7, "recv": 9}}}},
        }

    def test_minute_labels_and_totals(self):
        res = netstats.range_series("minute", self._snap())
        assert [r["key"] for r in res["rows"]] == ["10:00", "10:01"]
        assert res["recv"] == 300 and res["sent"] == 30

    def test_ssid_filter(self):
        res = netstats.range_series("minute", self._snap(), ssid="A")
        assert res["recv"] == 150 and res["sent"] == 15     # 只算 A 的份额
        assert res["rows"][-1]["recv"] == 50

    def test_ssid_absent_yields_zero(self):
        res = netstats.range_series("minute", self._snap(), ssid="不存在")
        assert res["sent"] == 0 and res["recv"] == 0

    def test_hour_and_day(self):
        assert netstats.range_series("hour", self._snap())["recv"] == 300
        day = netstats.range_series("day", self._snap())
        assert day["rows"][-1]["key"] == "03-01"
        assert day["recv"] == 311                           # 窗口内三天合计（30 天窗全含）

    def test_month_derived_from_days(self):
        res = netstats.range_series("month", self._snap())
        keys = [r["key"] for r in res["rows"]]
        assert keys == ["2026-01", "2026-02", "2026-03"]
        assert res["rows"][-1]["recv"] == 300
        assert res["recv"] == 311                           # 三个月合计

    def test_month_with_ssid_filter(self):
        res = netstats.range_series("month", self._snap(), ssid="B")
        assert res["rows"][-1]["recv"] == 0                 # 3 月无 B 流量
        assert res["rows"][0]["recv"] == 9

    def test_unknown_granularity_raises(self):
        with pytest.raises(ValueError):
            netstats.range_series("week", self._snap())

    def test_known_ssids_sorted_union(self):
        assert netstats.known_ssids(self._snap()) == ["A", "B"]


class TestStatsTabUI:
    @staticmethod
    def _page(qtbot, rate_stats):
        page = DashboardPage(EventStore(), rate_stats=rate_stats)
        qtbot.addWidget(page)
        page._hist_tabs.setCurrentWidget(page._stats_w)
        page._reload_rate_stats()
        return page

    def test_tab_renders_seeded_store(self, qtbot):
        store = RateStatsStore("ui_rate_test.json")
        store.add(1000, 50000, "HomeWiFi")
        store.add(2000, 8000, "Office5G")
        page = self._page(qtbot, store)
        tabs = [page._hist_tabs.tabText(i) for i in range(page._hist_tabs.count())]
        assert "流量统计" in tabs
        assert "↓" in page.lbl_rate_total.text() and "↑" in page.lbl_rate_total.text()
        assert page.cmb_rate_wifi.count() == 3              # 全部 + 两个网络
        assert page.tbl_rate_wifi.rowCount() == 2
        assert page.chart_rate._bars                        # 有柱

    def test_granularity_and_wifi_switch(self, qtbot):
        store = RateStatsStore("ui_rate_test2.json")
        store.add(10, 100, "A")
        page = self._page(qtbot, store)
        page.cmb_rate_gran.setCurrentIndex(0)               # 分钟
        assert ":" in page.chart_rate._bars[0][0]           # 分钟柱标签为 HH:MM
        page.cmb_rate_wifi.setCurrentText("A")              # 不可编辑框：项必须在列表内
        page._reload_rate_stats()
        assert page.lbl_rate_total.text() != "—"            # 切换后重算
        # wifi 表仍列全部网络（不受筛选影响）；切换全程不崩
        assert page.tbl_rate_wifi.rowCount() == 1

    def test_no_rate_stats_shows_placeholder(self, qtbot):
        page = self._page(qtbot, None)
        assert page.lbl_rate_total.text() == "—"            # 兼容旧签名（未接线）
        page._reload_rate_stats()                           # 不崩


def test_gran_human_bytes_sanity():
    assert human_bytes(1024) == "1.0KB" or "KB" in human_bytes(1024)
