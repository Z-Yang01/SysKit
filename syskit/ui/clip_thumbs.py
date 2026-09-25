"""剪贴板图片条目的行内缩略图（单源）：系统助手页列表与快捷粘贴面板共用。

PNG 载荷按 md5 内容寻址（clipboard.save_image_bytes），同名 ⇒ 同内容，
缓存按文件名永久保留即可、无需失效逻辑；加载失败同样记入缓存（False），
避免每次 reload/呼出对已丢失文件反复读盘。返回 None 由调用方决定占位——
列表行回退 🖼 emoji，详情区回退文字提示。
"""
import os

from PySide6.QtCore import Qt
from PySide6.QtGui import QIcon, QPixmap

from .. import clipboard as clipboard_mod

_cache = {}     # {(文件名, 高度px): QIcon | False}——False = 已确认不可加载


def icon_for(name: str, px: int = 28):
    """图片载荷的行内缩略图 QIcon（px 为缩放后高度）；无载荷/文件丢失/损坏返回 None。"""
    if not name:
        return None
    key = (name, px)
    if key in _cache:
        icon = _cache[key]
        return icon if icon else None
    path = clipboard_mod.image_path(name)
    pix = QPixmap(path) if path and os.path.exists(path) else QPixmap()
    if pix.isNull():
        _cache[key] = False
        return None
    icon = QIcon(pix.scaledToHeight(px, Qt.SmoothTransformation))
    _cache[key] = icon
    return icon
