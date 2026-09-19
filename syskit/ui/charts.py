"""轻量自绘折线图/柱状图：QPainter 实现，多序列、自动量程、None 断线、图例。"""
from collections import deque

from PySide6.QtCore import QPointF, QRectF, Qt
from PySide6.QtGui import QColor, QFont, QPainter, QPen
from PySide6.QtWidgets import QWidget

from . import theme


def human_bytes(v: float) -> str:
    for unit in ("B", "KB", "MB", "GB", "TB"):
        if abs(v) < 1024 or unit == "TB":
            return f"{v:.0f}{unit}" if unit == "B" else f"{v:.1f}{unit}"
        v /= 1024
    return f"{v:.1f}TB"


class LineChart(QWidget):
    """每秒一个采样点。push(values) 追加；None 表示断线（丢包/无数据）。"""

    def __init__(self, title="", series=None, unit="", seconds=600,
                 y_fixed=None, fmt=None, parent=None):
        super().__init__(parent)
        self.title = title
        self.unit = unit
        self.seconds = seconds
        self.y_fixed = y_fixed          # (lo, hi) 固定量程；None 自动
        self.fmt = fmt or (lambda v: f"{v:.0f}")
        self._series = []               # [ (name, color, deque) ]
        self.setMinimumHeight(140)
        for name, color in (series or []):
            self.add_series(name, color)

    def add_series(self, name: str, color: str):
        self._series.append((name, QColor(color), deque([None] * self.seconds, maxlen=self.seconds)))

    def push(self, values: list):
        """values 与序列一一对应；None 跳过成断线。"""
        for s, v in zip(self._series, values):
            s[2].append(v)
        self.update()

    def clear_data(self):
        for s in self._series:
            for i in range(len(s[2])):
                s[2][i] = None
        self.update()               # 立即擦除旧折线（曾滞留到下一个采样才消失）

    def paintEvent(self, ev):
        pal = theme.pal()
        p = QPainter(self)
        p.setRenderHint(QPainter.Antialiasing)
        w, h = self.width(), self.height()
        ml, mr, mt, mb = 52, 12, 26, 20
        plot_w, plot_h = w - ml - mr, h - mt - mb

        p.setPen(QPen(QColor(pal["border"]), 1))
        p.setBrush(QColor(pal["card"]))
        p.drawRoundedRect(QRectF(0.5, 0.5, w - 1, h - 1), 8, 8)

        p.setPen(QColor(pal["text"]))
        p.setFont(QFont("Microsoft YaHei UI", 9, QFont.Bold))
        if self.title:
            p.drawText(QRectF(ml, 4, plot_w, 18), Qt.AlignLeft | Qt.AlignVCenter, self.title)

        vals = [v for _, _, dq in self._series for v in dq if v is not None]
        if self.y_fixed:
            lo, hi = self.y_fixed
        elif vals:
            hi = max(vals) * 1.2 or 1
            lo = 0
        else:
            lo, hi = 0, 1

        p.setFont(QFont("Microsoft YaHei UI", 7))
        p.setPen(QColor(pal["grid"]))
        for i in range(5):
            y = mt + plot_h * i / 4
            p.drawLine(QPointF(ml, y), QPointF(ml + plot_w, y))
            value = hi - (hi - lo) * i / 4
            p.setPen(QColor(pal["sub"]))
            p.drawText(QRectF(0, y - 8, ml - 6, 16), Qt.AlignRight | Qt.AlignVCenter,
                       self.fmt(value))
            p.setPen(QColor(pal["grid"]))

        p.setPen(QPen(QColor(pal["scrollbar"]), 1))
        p.drawLine(QPointF(ml, mt), QPointF(ml, mt + plot_h))
        p.drawLine(QPointF(ml, mt + plot_h), QPointF(ml + plot_w, mt + plot_h))

        n = self.seconds
        for name, color, dq in self._series:
            pen = QPen(color, 1.6)
            p.setPen(pen)
            prev = None
            for i, v in enumerate(dq):
                if v is None:
                    prev = None
                    continue
                x = ml + plot_w * i / max(1, n - 1)
                # y 夹紧到绘图区：y_fixed 窗口（如延迟 0-120ms）被超窗值击穿时，
                # 折线曾画出标题/图例区、与轴标签脱节
                frac = (v - lo) / max(1e-9, hi - lo)
                y = mt + plot_h * (0.0 if frac < 0 else 1.0 if frac > 1 else frac)
                if prev is not None:
                    p.drawLine(prev, QPointF(x, y))
                else:
                    p.drawPoint(QPointF(x, y))
                prev = QPointF(x, y)

        # 图例
        p.setFont(QFont("Microsoft YaHei UI", 8))
        x = w - mr
        for name, color, _dq in reversed(self._series):
            text_width = p.fontMetrics().horizontalAdvance(name) + 16
            x -= text_width
            p.setPen(Qt.NoPen)
            p.setBrush(color)
            p.drawRect(QRectF(x, 7, 9, 9))
            p.setPen(QColor(pal["header_text"]))
            p.drawText(QRectF(x + 12, 2, text_width, 18), Qt.AlignLeft | Qt.AlignVCenter, name)
        p.end()


class BarChart(QWidget):
    """信道占用柱状图：bars = [(label, value, color)]，value 为权重（AP 数/最强信号）。"""

    def __init__(self, title="", y_label="", fmt=None, parent=None):
        super().__init__(parent)
        self.title = title
        self.y_label = y_label
        self.fmt = fmt or (lambda v: str(int(v)))
        self._bars = []
        self.setMinimumHeight(160)

    def set_bars(self, bars: list):
        self._bars = bars
        self.update()

    def paintEvent(self, ev):
        pal = theme.pal()
        p = QPainter(self)
        p.setRenderHint(QPainter.Antialiasing)
        w, h = self.width(), self.height()
        ml, mr, mt, mb = 40, 10, 26, 22
        plot_w, plot_h = w - ml - mr, h - mt - mb

        p.setPen(QPen(QColor(pal["border"]), 1))
        p.setBrush(QColor(pal["card"]))
        p.drawRoundedRect(QRectF(0.5, 0.5, w - 1, h - 1), 8, 8)
        p.setPen(QColor(pal["text"]))
        p.setFont(QFont("Microsoft YaHei UI", 9, QFont.Bold))
        if self.title:
            p.drawText(QRectF(ml, 4, plot_w, 18), Qt.AlignLeft | Qt.AlignVCenter, self.title)

        if not self._bars:
            p.setPen(QColor(pal["sub"]))
            p.drawText(QRectF(0, 0, w, h), Qt.AlignCenter, "暂无数据，请先扫描")
            p.end()
            return

        hi = max(v for _l, v, _c in self._bars) or 1
        n = len(self._bars)
        slot = plot_w / n
        bar_w = max(6, slot * 0.6)
        p.setFont(QFont("Microsoft YaHei UI", 7))
        for i, (label, value, color) in enumerate(self._bars):
            bh = plot_h * value / hi
            x = ml + slot * i + (slot - bar_w) / 2
            y = mt + plot_h - bh
            p.setPen(Qt.NoPen)
            p.setBrush(QColor(color))
            p.drawRoundedRect(QRectF(x, y, bar_w, bh), 2, 2)
            p.setPen(QColor(pal["sub"]))
            p.drawText(QRectF(x - slot * 0.2, max(mt + 2, y - 14), bar_w + slot * 0.4, 12),
                       Qt.AlignCenter, self.fmt(value))
            p.drawText(QRectF(x - slot * 0.2, mt + plot_h + 2, bar_w + slot * 0.4, 14),
                       Qt.AlignCenter, str(label))
        p.setPen(QColor(pal["scrollbar"]))
        p.drawLine(QPointF(ml, mt + plot_h), QPointF(ml + plot_w, mt + plot_h))
        if self.y_label:
            p.setPen(QColor(pal["sub"]))
            p.drawText(QRectF(0, 4, ml - 6, 18), Qt.AlignRight | Qt.AlignVCenter, self.y_label)
        p.end()
