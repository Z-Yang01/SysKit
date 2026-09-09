"""修复确认对话框：展示动作说明 / 风险 / 还原方式。"""
from PySide6.QtWidgets import (QDialog, QDialogButtonBox, QLabel, QVBoxLayout,
                               QFrame, QHBoxLayout)

from ..repairs import RISK_CONFIG, RISK_AGGRESSIVE, RISK_LABEL

RISK_COLOR = {"safe": "#2e7d32", "config": "#ef6c00", "aggressive": "#c62828"}


class ConfirmDialog(QDialog):
    def __init__(self, action, param_desc="", parent=None):
        super().__init__(parent)
        self.setWindowTitle("确认修复")
        self.setMinimumWidth(460)
        lay = QVBoxLayout(self)

        title = QLabel(action.title)
        title.setStyleSheet("font-size:15px; font-weight:bold;")
        lay.addWidget(title)

        risk = QLabel(f"风险级别：{RISK_LABEL[action.risk]}")
        risk.setStyleSheet(f"color: {RISK_COLOR[action.risk]}; font-weight:bold;")
        lay.addWidget(risk)

        desc = QFrame()
        desc.setFrameShape(QFrame.StyledPanel)
        dlay = QVBoxLayout(desc)
        dlay.addWidget(QLabel("将执行的操作："))
        body = QLabel(action.description)
        body.setWordWrap(True)
        dlay.addWidget(body)
        if param_desc:
            p = QLabel("参数：" + param_desc)
            p.setWordWrap(True)
            p.setStyleSheet("color:#555;")
            dlay.addWidget(p)
        lay.addWidget(desc)

        restore = QLabel("还原方式：" + action.restore_hint)
        restore.setWordWrap(True)
        restore.setStyleSheet("color:#555;")
        lay.addWidget(restore)
        if action.need_reboot:
            rb = QLabel("⚠ 该操作需要重启电脑后生效")
            rb.setStyleSheet("color:#c62828; font-weight:bold;")
            lay.addWidget(rb)

        note = QLabel("改配置类动作执行前会自动备份原值（备份目录见日志）。")
        note.setStyleSheet("color:#777; font-size:12px;")
        lay.addWidget(note)

        btns = QDialogButtonBox(QDialogButtonBox.Yes | QDialogButtonBox.No)
        btns.button(QDialogButtonBox.Yes).setText("执行")
        btns.button(QDialogButtonBox.No).setText("取消")
        btns.accepted.connect(self.accept)
        btns.rejected.connect(self.reject)
        lay.addWidget(btns)
