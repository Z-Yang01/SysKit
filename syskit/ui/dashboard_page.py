"""网络监控仪表盘：统计卡 + 延迟/速率曲线 + 断网事件时间线。"""
import time

from PySide6.QtCore import Qt
from PySide6.QtGui import QColor, QFont
from PySide6.QtWidgets import (QFrame, QGridLayout, QGroupBox, QHBoxLayout, QLabel,
                               QListWidget, QListWidgetItem, QVBoxLayout, QWidget)

from .charts import LineChart, human_bytes

STATE_TEXT = {"ok": "恢复在线", "no_net": "连接中断", "ncsi_false": "NCSI 误报",
              "offline": "无网络连接", "no_internet": "外网不可达"}


def _stat_card(title):
    frame = QFrame()
    frame.setObjectName("statCard")
    lay = QVBoxLayout(frame)
    lay.setContentsMargins(12, 8, 12, 10)
    t = QLabel(title)
    t.setStyleSheet("color:#8a93a0; font-size:9pt; border:none; background:transparent;")
    v = QLabel("—")
    v.setFont(QFont("Microsoft YaHei UI", 13, QFont.Bold))
    v.setStyleSheet("border:none; background:transparent;")
    lay.addWidget(t)
    lay.addWidget(v)
    frame.value_label = v
    return frame


class DashboardPage(QWidget):
    def __init__(self, eventstore, parent=None):
        super().__init__(parent)
        self._eventstore = eventstore
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

        grp = QGroupBox("断网 / 恢复事件时间线（近 30 条）")
        glay = QVBoxLayout(grp)
        self.list_events = QListWidget()
        self.list_events.setMaximumHeight(140)
        self.list_events.setAlternatingRowColors(True)
        glay.addWidget(self.list_events)
        lay.addWidget(grp, 1)
        self.reload_events()

    def reload_events(self):
        self.list_events.clear()
        for ev in self._eventstore.recent(30):
            color = {"ok": "#2e9e44"}.get(ev["state"], "#c62828")
            text = f"{ev['ts']}　{STATE_TEXT.get(ev['state'], ev['state'])}"
            if ev.get("detail"):
                text += f"　— {ev['detail']}"
            item = QListWidgetItem(text)
            item.setForeground(QColor(color))
            self.list_events.addItem(item)

    def update_tick(self, d: dict):
        if "lat_gw" in d:
            self._set_lat(self.card_gw, d["lat_gw"], d.get("gateway", ""))
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
    def _set_lat(card, ms, target):
        if ms is None:
            card.value_label.setText(f"超时/丢包")
            card.value_label.setStyleSheet(
                "color:#c62828; border:none; background:transparent;")
        else:
            card.value_label.setText(f"{ms:.0f} ms")
            color = "#2e9e44" if ms < 60 else "#ef6c00" if ms < 150 else "#c62828"
            card.value_label.setStyleSheet(
                f"color:{color}; border:none; background:transparent;")
        card.setToolTip(target)
