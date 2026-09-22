"""快捷粘贴原语：前台窗口记录/恢复 + SendInput 模拟 Ctrl+V（无 Qt）。

剪贴板历史面板呼出时会抢走焦点，直接模拟 Ctrl+V 会落在本应用窗口上；
因此呼出前用 foreground_window() 记下目标窗口，粘贴前 focus_window() 还焦，
并由调用方等一个短事件循环拍（激活消息处理完）再 send_ctrl_v()——
在 GUI 线程同步 sleep 会卡住 WM_ACTIVATE 派发，首拍按键可能被丢。
SetForegroundWindow 只对前台进程放行——面板显示期间本进程即前台，可安全调用；
跨线程输入队列仍可能拦截激活，故补 AttachThreadInput + BringWindowToTop 兜底。
所有 Win32 入口在导入期声明 ctypes 签名（建议④）：ctypes 默认 restype/argtype
为 c_int，x64 下会把 HWND 句柄截断/符号扩展成 32 位（clipboard.py 剪贴板排除
格式处有同款前科）。
非 Windows / 调用失败一律返回 False，调用方降级为「已复制，手动粘贴」。
"""
import ctypes
import sys
from ctypes import wintypes

from .logger import LOGGER

VK_CONTROL = 0x11
VK_V = 0x56
_KEY_UP = 0x0002
_INPUT_KEYBOARD = 1
_SW_RESTORE = 9


def _user32():
    return ctypes.windll.user32


def _kernel32():
    return ctypes.windll.kernel32


# SendInput 结构提为模块级：POINTER(INPUT) 签名要求类身份跨调用稳定
# （旧版每次调用内联重建一份类定义）；x64 下 union 天然补齐到 MOUSEINPUT 宽，
# sizeof(INPUT) 仍显式传给第三参。
class KEYBDINPUT(ctypes.Structure):
    _fields_ = [("wVk", wintypes.WORD), ("wScan", wintypes.WORD),
                ("dwFlags", wintypes.DWORD), ("time", wintypes.DWORD),
                ("dwExtraInfo", ctypes.c_size_t)]


class MOUSEINPUT(ctypes.Structure):
    _fields_ = [("dx", wintypes.LONG), ("dy", wintypes.LONG),
                ("mouseData", wintypes.DWORD), ("dwFlags", wintypes.DWORD),
                ("time", wintypes.DWORD), ("dwExtraInfo", ctypes.c_size_t)]


class HARDWAREINPUT(ctypes.Structure):
    _fields_ = [("uMsg", wintypes.DWORD), ("wParamL", wintypes.WORD),
                ("wParamH", wintypes.WORD)]


class INPUTUNION(ctypes.Union):
    _fields_ = [("ki", KEYBDINPUT), ("mi", MOUSEINPUT), ("hi", HARDWAREINPUT)]


class INPUT(ctypes.Structure):
    _fields_ = [("type", wintypes.DWORD), ("iu", INPUTUNION)]


def _declare_signatures():
    """Win32 入口签名声明（进程级：windll 缓存同一 CDLL 实例，全局生效）。

    GetForegroundWindow 返回 None 表 NULL——clipboard.foreground_process_name
    的 `if not hwnd` 与本模块的比较逻辑均兼容。
    """
    u = _user32()
    hwnd = wintypes.HWND
    u.GetForegroundWindow.restype = hwnd
    u.IsWindow.argtypes = [hwnd]
    u.IsIconic.argtypes = [hwnd]
    u.ShowWindow.argtypes = [hwnd, ctypes.c_int]
    u.BringWindowToTop.argtypes = [hwnd]
    u.SetForegroundWindow.argtypes = [hwnd]
    u.GetWindowThreadProcessId.argtypes = [hwnd, ctypes.POINTER(wintypes.DWORD)]
    u.GetWindowThreadProcessId.restype = wintypes.DWORD
    u.AttachThreadInput.argtypes = [wintypes.DWORD, wintypes.DWORD, wintypes.BOOL]
    u.SendInput.argtypes = [wintypes.UINT, ctypes.POINTER(INPUT), ctypes.c_int]
    u.SendInput.restype = wintypes.UINT
    _kernel32().GetCurrentThreadId.restype = wintypes.DWORD


if sys.platform == "win32":
    _declare_signatures()


def foreground_window() -> int:
    """当前前台窗口句柄；失败/非 Windows 返回 0。"""
    if sys.platform != "win32":
        return 0
    try:
        return int(_user32().GetForegroundWindow() or 0)
    except Exception:
        return 0


def focus_window(hwnd: int) -> bool:
    """把焦点还给 hwnd（最小化先还原）。返回是否确实成为前台窗口。"""
    if sys.platform != "win32" or not hwnd:
        return False
    try:
        u = _user32()
        if not u.IsWindow(hwnd):
            return False
        if u.IsIconic(hwnd):
            u.ShowWindow(hwnd, _SW_RESTORE)
        if u.GetForegroundWindow() == hwnd:
            return True
        # 前台窗口可能属于另一线程：附着输入队列解除 SetForegroundWindow 限制
        # GetCurrentThreadId 只在 kernel32（user32 没有该导出，曾致本分支必抛
        # AttributeError、跨进程还焦全部降级，第 2 轮真机冒烟抓到）
        cur_tid = _kernel32().GetCurrentThreadId()
        tgt_tid = u.GetWindowThreadProcessId(hwnd, None)
        attached = cur_tid != tgt_tid and bool(
            u.AttachThreadInput(cur_tid, tgt_tid, True))
        try:
            u.BringWindowToTop(hwnd)
            u.SetForegroundWindow(hwnd)
        finally:
            if attached:
                u.AttachThreadInput(cur_tid, tgt_tid, False)
        return u.GetForegroundWindow() == hwnd
    except Exception:
        LOGGER.debug("focus_window 失败 hwnd={}", hwnd, exc_info=True)
        return False


def send_ctrl_v() -> bool:
    """向当前焦点窗口注入一次 Ctrl+V（SendInput 键盘事件）。"""
    if sys.platform != "win32":
        return False
    try:
        def key(vk, up):
            inp = INPUT()
            inp.type = _INPUT_KEYBOARD
            inp.iu.ki.wVk = vk
            inp.iu.ki.dwFlags = _KEY_UP if up else 0
            return inp

        events = (key(VK_CONTROL, False), key(VK_V, False),
                  key(VK_V, True), key(VK_CONTROL, True))
        arr = (INPUT * len(events))(*events)
        sent = int(_user32().SendInput(len(events), arr, ctypes.sizeof(INPUT)))
        return sent == len(events)
    except Exception:
        LOGGER.debug("send_ctrl_v 失败", exc_info=True)
        return False
