"""网络监控仪表盘：统计卡 + 延迟/速率曲线 + 断网事件时间线 + 网络质量日报。"""
import time

from PySide6.QtGui import QColor, QFont
from PySide6.QtWidgets import (QAbstractItemView, QFrame, QGroupBox,
                               QHBoxLayout, QHeaderView, QLabel, QListWidget,
                               QListWidgetItem, QPushButton, QTableWidget,
                               QTableWidgetItem, QTabWidget, QVBoxLayout, QWidget)

from . import theme
from .charts import LineChart, human_bytes
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
    def __init__(self, eventstore, parent=None, traffic=None):
        super().__init__(parent)
        self._eventstore = eventstore
        self._traffic = traffic            # TrafficStore：日报的按天流量列
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
            series=[("网关", "#3576e0"), ("公网", "#8e5ce6")],
            unit="ms", y_fixed=(0, 120), fmt=lambda v: f"{v:.0f}ms")
        self.chart_speed = LineChart(
            "实时速率（1 秒/点）",
            series=[("下载", "#2e9e44"), ("上传", "#ef6c00")],
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
        self.tbl_report = QTableWidget(0, 5)
        self.tbl_report.setHorizontalHeaderLabels(
            ["日期", "断网次数", "累计断网", "下载", "上传"])
        self.tbl_report.horizontalHeader().setSectionResizeMode(0, QHeaderView.Stretch)
        self.tbl_report.setEditTriggers(QAbstractItemView.NoEditTriggers)
        self.tbl_report.setSelectionBehavior(QAbstractItemView.SelectRows)
        self.tbl_report.setAlternatingRowColors(True)
        self.tbl_report.verticalHeader().setVisible(False)
        rlay.addWidget(self.tbl_report, 1)
        self._hist_tabs.addTab(report_w, "日报")
        glay.addWidget(self._hist_tabs)
        lay.addWidget(grp, 1)
        self.reload_events()

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
