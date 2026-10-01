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
_CACHE_CAP = 256    # 托盘长驻数天、截图载荷各异时防无界增长（每次弹出面板都进缓存）

def _cache_put(key, value):
    """插入并按插入序淘汰最旧条目（载荷按 md5 内容寻址，淘汰仅损缓存命中率，
    不损正确性——下次命中重新读盘重建）。"""
    if len(_cache) >= _CACHE_CAP:
        for k in list(_cache)[: len(_cache) - _CACHE_CAP + 1]:
            _cache.pop(k, None)
    _cache[key] = value


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
        _cache_put(key, False)
        return None
    icon = QIcon(pix.scaledToHeight(px, Qt.SmoothTransformation))
    _cache_put(key, icon)
    return icon
