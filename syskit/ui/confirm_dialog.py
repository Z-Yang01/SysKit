"""修复确认对话框：展示动作说明 / 风险 / 还原方式。"""
from PySide6.QtWidgets import (QDialog, QDialogButtonBox, QLabel, QVBoxLayout,
                               QFrame, QHBoxLayout)

from ..repairs import RISK_LABEL
from . import theme

RISK_COLOR = {"safe": "#2e9e44", "config": "#ef6c00", "aggressive": "#c62828"}
RISK_ICON = {"safe": "●", "config": "▲", "aggressive": "■"}


class ConfirmDialog(QDialog):
    def __init__(self, action, param_desc="", parent=None):
        super().__init__(parent)
        self.setWindowTitle("确认执行")
        self.setMinimumWidth(480)
        lay = QVBoxLayout(self)
        lay.setSpacing(10)

        title = QLabel(action.title)
        title.setStyleSheet("font-size:15px; font-weight:bold; border:none;")
        lay.addWidget(title)

        risk = QLabel(f"{RISK_ICON.get(action.risk, '●')} 风险级别：{RISK_LABEL[action.risk]}")
        risk.setStyleSheet(
            f"color: {RISK_COLOR[action.risk]}; font-weight:bold; "
            f"background: transparent; border: none; padding: 0;")
        lay.addWidget(risk)

        desc = QFrame()
        desc.setFrameShape(QFrame.StyledPanel)
        dlay = QVBoxLayout(desc)
        dlay.setContentsMargins(10, 8, 10, 8)
        lbl_ops = QLabel("将执行的操作")
        lbl_ops.setStyleSheet("font-weight:bold; color: @accent; border:none;".replace(
            "@accent", theme.pal().get("accent", "#3576e0")))
        dlay.addWidget(lbl_ops)
        body = QLabel(action.description)
        body.setWordWrap(True)
        dlay.addWidget(body)
        if param_desc:
            p = QLabel("参数：" + param_desc)
            p.setWordWrap(True)
            p.setStyleSheet(f"color:{theme.pal().get('sub', '#7b8794')}; border:none;")
            dlay.addWidget(p)
        lay.addWidget(desc)

        restore = QLabel("还原方式：" + action.restore_hint)
        restore.setWordWrap(True)
        restore.setStyleSheet(
            f"color:{theme.pal().get('sub', '#7b8794')}; border:none;")
        lay.addWidget(restore)
        if action.need_reboot:
            rb = QLabel("⚠ 该操作需要重启电脑后生效")
            rb.setStyleSheet(
                f"color:{RISK_COLOR['aggressive']}; font-weight:bold; border:none;")
            lay.addWidget(rb)

        note = QLabel("改配置类动作执行前会自动备份原值（备份目录见日志页）。")
        note.setWordWrap(True)
        note.setStyleSheet(
            f"color:{theme.pal().get('sub', '#7b8794')}; font-size:12px; border:none;")
        lay.addWidget(note)

        btns = QDialogButtonBox(QDialogButtonBox.Yes | QDialogButtonBox.No)
        yes = btns.button(QDialogButtonBox.Yes)
        no = btns.button(QDialogButtonBox.No)
        yes.setText("执行")
        yes.setProperty("primary", True)
        no.setText("取消")
        yes.setToolTip("确认后立即在后台线程执行该修复动作")
        no.setToolTip("放弃本次操作，不做任何系统改动")
        btns.accepted.connect(self.accept)
        btns.rejected.connect(self.reject)
        lay.addWidget(btns)
