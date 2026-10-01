"""网络监控仪表盘：统计卡 + 延迟/速率曲线 + 断网事件时间线 + 网络质量日报 + 流量统计。"""
import time

from PySide6.QtGui import QColor, QFont
from PySide6.QtWidgets import (QComboBox, QFrame, QGroupBox,
                               QHBoxLayout, QHeaderView, QLabel, QListWidget,
                               QListWidgetItem, QPushButton,
                               QTableWidgetItem, QTabWidget, QVBoxLayout, QWidget)

from . import theme
from .charts import BarChart, LineChart, human_bytes
from .. import netstats

STATE_TEXT = {"ok": "恢复在线", "no_net": "连接中断", "ncsi_false": "NCSI 误报",
              "offline": "无网络连接", "no_internet": "外网不可达"}

# 每张统计卡的强调色（左侧色条），让五张卡一眼可区分
CARD_ACCENTS = {
    "网关延迟": "#3576e0", "公网延迟": "#8e5ce6", "↓ 下载速率": "#2e9e44",
    "↑ 上传速率": "#ef6c00", "今日流量 ↓/↑": "#00838f",
}


def _stat_card(title):
    """统计卡：左侧强调色条 + 标题 + 大字数值（投影由 theme.add_shadow 提供）。"""
    frame = QFrame()
    frame.setObjectName("statCard")
    outer = QHBoxLayout(frame)
    outer.setContentsMargins(0, 0, 0, 0)
    outer.setSpacing(0)

    strip = QFrame()
    strip.setFixedWidth(4)
    accent = CARD_ACCENTS.get(title, "#3576e0")
    strip.setStyleSheet(
        f"background:{accent}; border:none;"
        "border-top-left-radius:11px; border-bottom-left-radius:11px;")
    outer.addWidget(strip)

    inner = QVBoxLayout()
    inner.setContentsMargins(12, 10, 12, 12)
    inner.setSpacing(2)
    t = QLabel(title)
    t.setObjectName("statTitle")
    v = QLabel("—")
    v.setFont(QFont("Microsoft YaHei UI", 15, QFont.Bold))
    v.setObjectName("statValue")
    inner.addWidget(t)
    inner.addWidget(v)
    outer.addLayout(inner, 1)

    frame.value_label = v
    theme.add_shadow(frame, blur=18, dy=3, alpha=30)
    return frame


class DashboardPage(QWidget):
    _GRANS = [("分钟（近 60 分）", "minute"), ("小时（近 48 时）", "hour"),
              ("天（近 30 天）", "day"), ("月（近 12 月）", "month")]

    def __init__(self, eventstore, parent=None, traffic=None, rate_stats=None):
        super().__init__(parent)
        self._eventstore = eventstore
        self._traffic = traffic            # TrafficStore：日报的按天流量列
        self._rate_stats = rate_stats      # RateStatsStore：流量统计（分钟/时/天/月 × WiFi）
        self._rate_wifi_items = []         # 当前 WiFi 下拉项（变更才重建，防选中跳动）
        self._rate_last_refresh = 0.0
        lay = QVBoxLayout(self)
        lay.setContentsMargins(14, 14, 14, 10)

        cards = QHBoxLayout()
        self.card_gw = _stat_card("网关延迟")
        self.card_pub = _stat_card("公网延迟")
        self.card_down = _stat_card("↓ 下载速率")
        self.card_up = _stat_card("↑ 上传速率")
        self.card_traffic = _stat_card("今日流量 ↓/↑")
        for c in (self.card_gw, self.card_pub, self.card_down, self.card_up, self.card_traffic):
            cards.addWidget(c, 1)
        lay.addLayout(cards)

        charts = QHBoxLayout()
        self.chart_lat = LineChart(
            "延迟（网关 vs 公网，3 秒/点）",
            series=[("网关", "@accent"), ("公网", "#8e5ce6")],
            unit="ms", y_fixed=(0, 120), fmt=lambda v: f"{v:.0f}ms")
        self.chart_speed = LineChart(
            "实时速率（1 秒/点）",
            series=[("下载", "@ok"), ("上传", "@warn")],
            fmt=human_bytes)
        charts.addWidget(self.chart_lat, 1)
        charts.addWidget(self.chart_speed, 1)
        lay.addLayout(charts, 3)

        self.chart_wifi = LineChart(
            "WiFi 信号强度（5 秒/点）",
            series=[("信号", "#8e5ce6")],
            y_fixed=(0, 100), fmt=lambda v: f"{v:.0f}%")
        self.chart_wifi.setMinimumHeight(110)
        lay.addWidget(self.chart_wifi, 1)

        grp = QGroupBox("断网 / 恢复事件时间线（近 30 条） · 网络质量日报")
        glay = QVBoxLayout(grp)
        self._hist_tabs = QTabWidget()
        # Tab1：事件时间线（属性名 list_events 保持不变，既有测试依赖）
        self.list_events = QListWidget()
        self.list_events.setAlternatingRowColors(True)
        self._hist_tabs.addTab(self.list_events, "时间线")
        # Tab2：近 7/30 天网络质量日报
        report_w = QWidget()
        rlay = QVBoxLayout(report_w)
        rlay.setContentsMargins(4, 4, 4, 4)
        days_row = QHBoxLayout()
        self._report_days = 7
        self.btn_report_7 = QPushButton("近 7 天")
        self.btn_report_7.setProperty("primary", True)
        self.btn_report_7.clicked.connect(lambda: self._set_report_days(7))
        self.btn_report_30 = QPushButton("近 30 天")
        self.btn_report_30.clicked.connect(lambda: self._set_report_days(30))
        days_row.addWidget(self.btn_report_7)
        days_row.addWidget(self.btn_report_30)
        days_row.addStretch(1)
        rlay.addLayout(days_row)
        self.tbl_report = theme.make_table(
            ["日期", "断网次数", "累计断网", "下载", "上传"], select_rows=True)
        self.tbl_report.horizontalHeader().setSectionResizeMode(0, QHeaderView.Stretch)
        rlay.addWidget(self.tbl_report, 1)
        self._hist_tabs.addTab(report_w, "日报")
        # Tab3：流量统计（秒=实时速率曲线已有；分/时/天/月 × 分 WiFi 归因）
        self._stats_w = self._build_rate_stats()
        self._hist_tabs.addTab(self._stats_w, "流量统计")
        glay.addWidget(self._hist_tabs)
        lay.addWidget(grp, 1)
        self.reload_events()

    def _build_rate_stats(self) -> QWidget:
        w = QWidget()
        slay = QVBoxLayout(w)
        srow = QHBoxLayout()
        self.cmb_rate_gran = QComboBox()
        self.cmb_rate_gran.addItems([label for label, _ in self._GRANS])
        self.cmb_rate_gran.setCurrentIndex(2)          # 默认「天」
        self.cmb_rate_gran.currentIndexChanged.connect(lambda *_: self._reload_rate_stats())
        self.cmb_rate_wifi = QComboBox()
        self.cmb_rate_wifi.currentIndexChanged.connect(lambda *_: self._reload_rate_stats())
        self.lbl_rate_total = QLabel("—")
        self.lbl_rate_total.setObjectName("hint")
        srow.addWidget(QLabel("粒度"))
        srow.addWidget(self.cmb_rate_gran)
        srow.addWidget(QLabel("网络"))
        srow.addWidget(self.cmb_rate_wifi, 1)
        srow.addWidget(self.lbl_rate_total)
        slay.addLayout(srow)
        self.chart_rate = BarChart("区间下载量（柱）· 上行/下行总量见右侧", fmt=human_bytes)
        slay.addWidget(self.chart_rate, 1)
        self.tbl_rate_wifi = theme.make_table(["网络（区间占用）", "下载", "上传"],
                                              select_rows=True)
        self.tbl_rate_wifi.horizontalHeader().setSectionResizeMode(0, QHeaderView.Stretch)
        self.tbl_rate_wifi.setMaximumHeight(150)
        slay.addWidget(self.tbl_rate_wifi)
        return w

    def _reload_rate_stats(self):
        """按粒度/网络筛选重绘流量统计；数据源缺失或异常时如实显示「—」。"""
        if self._rate_stats is None:
            return
        try:
            gran = self._GRANS[self.cmb_rate_gran.currentIndex()][1]
            snap = self._rate_stats.snapshot()
            ssid = self.cmb_rate_wifi.currentText()
            ssid = None if (ssid == "全部网络" or not ssid) else ssid
            res = netstats.range_series(gran, snap, ssid=ssid)
            self.chart_rate.set_bars(
                [(r["key"], r["recv"], "@accent" if r["recv"] < 0.5 * max(
                    (x["recv"] for x in res["rows"]), default=1) else "@err")
                 for r in res["rows"]])
            self.lbl_rate_total.setText(
                f"区间 ↓ {human_bytes(res['recv'])} · ↑ {human_bytes(res['sent'])}")
            self._reload_rate_wifi_table(snap, ssid)
            self._sync_rate_wifi_combo(snap)
            self._rate_last_refresh = time.time()
        except Exception:
            self.lbl_rate_total.setText("—")

    def _reload_rate_wifi_table(self, snap: dict, ssid):
        """各网络在所选粒度对应时间窗内的占用合计（不受 WiFi 筛选影响）。"""
        gran = self._GRANS[self.cmb_rate_gran.currentIndex()][1]
        totals = {}
        table = {"minute": snap["minutes"], "hour": snap["hours"]}.get(
            gran, snap["days"])
        win = netstats._GRAN_WINDOW[gran]
        if gran == "month":
            for key in sorted(table)[-win:]:
                mon = key[:7]
                agg = totals.setdefault("月汇总 " + mon, {"recv": 0, "sent": 0})
                for n, w in table[key].get("wifi", {}).items():
                    agg["recv"] += w.get("recv", 0)
                    agg["sent"] += w.get("sent", 0)
        else:
            for key in sorted(table)[-win:]:
                for n, w in table[key].get("wifi", {}).items():
                    agg = totals.setdefault(n, {"recv": 0, "sent": 0})
                    agg["recv"] += w.get("recv", 0)
                    agg["sent"] += w.get("sent", 0)
        rows = sorted(totals.items(), key=lambda kv: -kv[1]["recv"])
        self.tbl_rate_wifi.setRowCount(len(rows))
        for i, (name, agg) in enumerate(rows):
            self.tbl_rate_wifi.setItem(i, 0, QTableWidgetItem(name))
            self.tbl_rate_wifi.setItem(i, 1, QTableWidgetItem(human_bytes(agg["recv"])))
            self.tbl_rate_wifi.setItem(i, 2, QTableWidgetItem(human_bytes(agg["sent"])))

    def _sync_rate_wifi_combo(self, snap: dict):
        """WiFi 下拉随数据出现自动补充（保持当前选中；变更才重建防跳动）。"""
        names = ["全部网络"] + netstats.known_ssids(snap)
        if names == self._rate_wifi_items:
            return
        cur = self.cmb_rate_wifi.currentText()
        self.cmb_rate_wifi.blockSignals(True)
        self.cmb_rate_wifi.clear()
        self.cmb_rate_wifi.addItems(names)
        if cur in names:
            self.cmb_rate_wifi.setCurrentText(cur)
        self.cmb_rate_wifi.blockSignals(False)
        self._rate_wifi_items = names

    def refresh_rate_stats_if_visible(self):
        """统计 Tab 可见时按 ≥10s 节流刷新（update_tick 高频调用入口）。"""
        if (self._rate_stats is not None
                and self._hist_tabs.currentWidget() is self._stats_w
                and time.time() - self._rate_last_refresh >= 10):
            self._reload_rate_stats()

    def _set_report_days(self, days: int):
        self._report_days = days
        self.btn_report_7.setProperty("primary", days == 7)
        self.btn_report_30.setProperty("primary", days == 30)
        self.btn_report_7.style().unpolish(self.btn_report_7)
        self.btn_report_7.style().polish(self.btn_report_7)
        self.btn_report_30.style().unpolish(self.btn_report_30)
        self.btn_report_30.style().polish(self.btn_report_30)
        self._reload_report()

    @staticmethod
    def _fmt_duration(sec: int) -> str:
        if sec >= 3600:
            return f"{sec / 3600:.1f} 小时"
        if sec >= 60:
            return f"{sec // 60}分{sec % 60:02d}秒"
        return f"{sec}秒"

    def _reload_report(self):
        """近 N 天网络质量日报：从事件与流量数据聚合（netstats 纯逻辑）。"""
        traffic = self._traffic.days() if self._traffic is not None else None
        rows = netstats.daily_report(self._eventstore.items(),
                                     days=self._report_days, traffic=traffic)
        self.tbl_report.setRowCount(len(rows))
        for i, r in enumerate(rows):
            vals = [r["date"], str(r["outages"]),
                    self._fmt_duration(r["outage_sec"]) if r["has_events"] else "—",
                    human_bytes(r["recv"]) if r["has_traffic"] else "—",
                    human_bytes(r["sent"]) if r["has_traffic"] else "—"]
            for c, v in enumerate(vals):
                self.tbl_report.setItem(i, c, QTableWidgetItem(v))
        if rows:
            self.tbl_report.setRowHeight(0, 26)

    def reset_if_stale(self, max_gap_sec: float = 600.0):
        """页面隐藏期间采样被可见性守卫跳过：切回时若断流超过图表窗口，
        清空旧序列——避免「1 小时前」与现在被画成连续时间线。"""
        last = getattr(self, "_last_tick", None)
        if last is not None and time.time() - last > max_gap_sec:
            self.chart_lat.clear_data()
            self.chart_speed.clear_data()
            self.chart_wifi.clear_data()

    def reload_events(self):
        self.list_events.clear()
        p = theme.pal()
        for ev in self._eventstore.recent(30):
            color = p["ok"] if ev["state"] == "ok" else p["err"]
            text = f"{ev['ts']}　{STATE_TEXT.get(ev['state'], ev['state'])}"
            if ev.get("detail"):
                text += f"　— {ev['detail']}"
            item = QListWidgetItem(text)
            item.setForeground(QColor(color))
            self.list_events.addItem(item)
        self._reload_report()

    def update_tick(self, d: dict):
        self._last_tick = time.time()
        self.refresh_rate_stats_if_visible()     # 统计 Tab 可见时 ≥10s 节流刷新
        if "lat_gw" in d:
            # gateway 与 lat_gw 同 tick 成组出现（telemetry 保证）：
            # 无网关时 lat_gw=None 是"没得 ping"，不该显示成"超时/丢包"
            self._set_lat(self.card_gw, d["lat_gw"],
                          d.get("gateway", ""), no_gw=not d.get("gateway"))
            self._set_lat(self.card_pub, d["lat_pub"], "223.5.5.5")
            self.chart_lat.push([d["lat_gw"], d["lat_pub"]])
        if "down" in d:
            self.card_down.value_label.setText(f"{human_bytes(d['down'])}/s")
            self.card_up.value_label.setText(f"{human_bytes(d['up'])}/s")
            self.chart_speed.push([d["down"], d["up"]])
        if "wifi_signal" in d:
            self.chart_wifi.push([d["wifi_signal"]])
        if "today_sent" in d:
            self.card_traffic.value_label.setText(
                f"{human_bytes(d['today_recv'])} / {human_bytes(d['today_sent'])}")

    @staticmethod
    def _set_lat(card, ms, target, no_gw=False):
        p = theme.pal()
        if ms is None and no_gw:
            card.value_label.setText("无网关")
            card.value_label.setStyleSheet(
                f"color:{p['sub']}; border:none; background:transparent;")
        elif ms is None:
            card.value_label.setText("超时/丢包")
            card.value_label.setStyleSheet(
                f"color:{p['err']}; border:none; background:transparent;")
        else:
            card.value_label.setText(f"{ms:.0f} ms")
            color = p["ok"] if ms < 60 else p["warn"] if ms < 150 else p["err"]
            card.value_label.setStyleSheet(
                f"color:{color}; border:none; background:transparent;")
        card.setToolTip(target)
