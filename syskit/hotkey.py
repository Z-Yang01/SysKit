"""Windows 全局热键：RegisterHotKey 封装与热键字符串解析（无 Qt）。

注册需在主线程调用（与 Qt 消息泵同线程），WM_HOTKEY 由
QAbstractNativeEventFilter（ui/palette.py::HotkeyFilter）接收。
"""
import sys

MOD_ALT = 0x0001
MOD_CONTROL = 0x0002
MOD_SHIFT = 0x0004
MOD_WIN = 0x0008

_NAMED_KEYS = {
    "space": 0x20, "esc": 0x1B, "escape": 0x1B, "tab": 0x09, "enter": 0x0D,
    "return": 0x0D, "backspace": 0x08, "del": 0x2E, "delete": 0x2E,
    "ins": 0x2D, "insert": 0x2D, "home": 0x24, "end": 0x23,
    "pgup": 0x21, "pgdn": 0x22, "left": 0x25, "up": 0x26, "right": 0x27,
    "down": 0x28,
    "oem_3": 0xC0, "`": 0xC0, "~": 0xC0,
    "oem_plus": 0xBB, "=": 0xBB, "+": 0xBB,
    "oem_minus": 0xBD, "-": 0xBD,
    "oem_comma": 0xBC, ",": 0xBC,
    "oem_period": 0xBE, ".": 0xBE,
    "oem_2": 0xBF, "/": 0xBF,
    "oem_1": 0xBA, ";": 0xBA,
    "oem_4": 0xDB, "[": 0xDB,
    "oem_6": 0xDD, "]": 0xDD,
    "oem_7": 0xDE, "'": 0xDE,
}


def _key_to_vk(key: str):
    """单字符 / F1-F24 / 命名键 → 虚拟键码；未知返回 None。"""
    key = key.strip().lower()
    if not key:
        return None
    if len(key) == 1:
        ch = key.upper()
        if "A" <= ch <= "Z" or "0" <= ch <= "9":
            return ord(ch)
    if key.startswith("f") and key[1:].isdigit():
        n = int(key[1:])
        if 1 <= n <= 24:
            return 0x70 + (n - 1)
    return _NAMED_KEYS.get(key)


def parse_hotkey(text: str):
    """'Ctrl+Alt+K' → (mods, vk)；无法解析返回 None（至少要有一个主键）。"""
    mods = 0
    key = None
    for part in str(text or "").split("+"):
        p = part.strip().lower()
        if not p:
            continue
        if p in ("ctrl", "control"):
            mods |= MOD_CONTROL
        elif p == "alt":
            mods |= MOD_ALT
        elif p == "shift":
            mods |= MOD_SHIFT
        elif p == "win":
            mods |= MOD_WIN
        else:
            key = p
    vk = _key_to_vk(key) if key else None
    if vk is None:
        return None
    return mods, vk


def hotkey_display(text: str) -> str:
    """规范化显示（Ctrl+Alt+K → Ctrl+Alt+K；k → K）。非法返回 ''。"""
    parsed = parse_hotkey(text)
    if parsed is None:
        return ""
    mods, vk = parsed
    names = []
    if mods & MOD_CONTROL:
        names.append("Ctrl")
    if mods & MOD_ALT:
        names.append("Alt")
    if mods & MOD_SHIFT:
        names.append("Shift")
    if mods & MOD_WIN:
        names.append("Win")
    for name, code in _NAMED_KEYS.items():
        if code == vk and len(name) > 1 and not name.startswith("oem"):
            names.append(name.capitalize())
            break
    else:
        if 0x70 <= vk <= 0x87:                  # F1-F24
            names.append(f"F{vk - 0x70 + 1}")
        else:
            names.append(chr(vk))
    return "+".join(names)


def register(mods: int, vk: int, hotkey_id: int = 1) -> bool:
    """注册全局热键（主线程调用）。失败返回 False（调用方应静默降级）。"""
    if sys.platform != "win32":
        return False
    try:
        import ctypes
        return bool(ctypes.windll.user32.RegisterHotKey(None, hotkey_id, mods, vk))
    except Exception:
        return False


def unregister(hotkey_id: int = 1) -> bool:
    if sys.platform != "win32":
        return False
    try:
        import ctypes
        return bool(ctypes.windll.user32.UnregisterHotKey(None, hotkey_id))
    except Exception:
        return False
