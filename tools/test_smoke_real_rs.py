r"""真机冒烟：Rust+Tauri 重写版（rust-rewrite 分支）——8 靶 + 2 边界。

============================================================================
⚠ 只能由人坐在真实桌面会话里运行：脚本会向真实桌面注入按键（Ctrl+Alt+V /
  Ctrl+Win+V / 方向键 / 回车）、聚焦/启动真实窗口、写真实剪贴板。
  严禁无人值守运行；运行期间不要操作键鼠。
============================================================================

运行说明
--------
1. 前置（一次性）：
   - 运行【Rust 版】exe：app-rs/src-tauri/target/release/SysKit.exe
     （npx tauri build 产物）。**Python 版与 Rust 版严禁同时运行**
     （共用数据目录）；冒烟前退出一切 SysKit 实例，只启动 Rust 版。
   - 设置页开启「监听剪贴板」（脚本靠真实复制让 Rust 版记录历史）。
   - pip install uiautomation pywin32（缺哪个，对应靶自动 skip）。
   - 可选环境变量 SYSKIT_DATA_DIR 指向数据目录（便携默认
     <exe目录>\data\config；不设则探测 %APPDATA%\SysKit）。
2. 运行：
     python -m pytest tools/test_smoke_real_rs.py --run-real -v
   （默认全数 skip；--run-real 时根 conftest 对 @pytest.mark.real 豁免系统
     调用哨兵——冒烟靶要真实 os.startfile 拉窗口。）
3. 边界靶说明：
   - 边界 a（热键回退链）在本驱动中 skip：Rust 版回退链与热键即时重注册
     在 Step 2 落地后补验（当前语义=注册失败仅日志降级、改键需重启）。
   - 边界 b（50ms 焦点漂移）自动重试 3 次，未命中 skip（时序概率）。
   - 边界 c / 靶 8 弹一次 UAC（管理员 PowerShell），人工点「是」。

设计取舍（与 Python 版驱动一致的披露）
------------------------------------
- magic 断言在驱动侧完成；SysKit 侧注键证据 = 目标窗口内容变化 + 行为分支。
- 托盘气泡文案进程外不可读：行为代理断言（取消 = 剪贴板有 token 且目标未变）。
- 全部等待为驱动进程内轮询；不碰被测进程 UI 线程；无 WH_KEYBOARD_LL。
"""
import ctypes
import os
import shutil
import sys
import time
from ctypes import wintypes

import pytest

RUN_REAL = "--run-real" in sys.argv

# conftest 顶层把 QT_QPA_PLATFORM 设为 offscreen（离屏测试需要）；本脚本必须
# 用真实平台，否则 QApplication 剪贴板不与真实系统剪贴板互通。仅在真机跑时
# 清除——默认 skip 路径（含 pytest 收集期的模块导入）不得改动会话平台设置，
# 否则 tools 与 tests/ 同场收集时会把离屏测试拖到真实窗口平台（审计修复）。
if RUN_REAL:
    os.environ.pop("QT_QPA_PLATFORM", None)

from PySide6.QtCore import QMimeData, QUrl          # noqa: E402
from PySide6.QtGui import QGuiApplication            # noqa: E402

from syskit import paste as sk                        # noqa: E402  复用 INPUT 结构/签名/magic

pytestmark = [
    pytest.mark.real,
    pytest.mark.skipif(
        not RUN_REAL,
        reason="真机冒烟默认跳过：python -m pytest tools/test_smoke_real.py --run-real（人工在场）"),
]

PASTE_MAGIC = sk.PASTE_MAGIC
VK_CTRL, VK_MENU, VK_WIN, VK_SHIFT, VK_V = 0x11, 0x12, 0x5B, 0x10, 0x56
VK_DOWN, VK_RETURN, VK_L = 0x28, 0x0D, 0x4C
_INVALID = 0xFFFFFFFF

# ---------------------------------------------------------------- Win32 基元


def _send_input(events) -> int:
    arr = (sk.INPUT * len(events))(*events)
    return int(sk._user32().SendInput(len(events), arr, ctypes.sizeof(sk.INPUT)))


def _key(vk, up=False):
    inp = sk.INPUT()
    inp.type = 1                                # INPUT_KEYBOARD
    inp.iu.ki.wVk = vk
    inp.iu.ki.dwFlags = sk._KEY_UP if up else 0
    inp.iu.ki.dwExtraInfo = PASTE_MAGIC          # 本脚本注入统一带 magic
    return inp


def wait_until(pred, timeout=6.0, step=0.05, desc=""):
    deadline = time.time() + timeout
    while time.time() < deadline:
        if pred():
            return True
        time.sleep(step)                          # 驱动进程轮询；不碰 SysKit UI 线程
    return False


def tap(vk):
    assert _send_input([_key(vk), _key(vk, True)]) == 2, f"SendInput 注入 {vk:#x} 失败"


def press_hotkey(mods, vk):
    """完整热键序列：修饰键 down → 主键 down/up → 修饰键 up（顺序可回放）。"""
    downs = [_key(m) for m in mods]
    ups = [_key(m, True) for m in reversed(mods)]
    seq = downs + [_key(vk), _key(vk, True)] + ups
    assert _send_input(seq) == len(seq), "热键 SendInput 计数断言失败（硬约束⑥同源）"


def press_quick_paste():
    press_hotkey([VK_CTRL, VK_MENU], VK_V)


def configured_panel_hotkey():
    """settings.json 实配的面板热键 → (mods, vk)；未设/非法回退 Ctrl+Win+V。"""
    mods, vk = 0x000A, VK_V  # Ctrl+Win+V 默认
    p = _find_settings_file()
    if p:
        try:
            import json as _json
            text = str(_json.load(open(p, encoding="utf-8")).get("clip_paste_hotkey", "") or "")
            for part in text.split("+"):
                pp = part.strip().lower()
                if pp in ("ctrl", "control"):
                    mods |= 0x0002
                elif pp == "alt":
                    mods |= 0x0001
                elif pp == "shift":
                    mods |= 0x0004
                elif pp == "win":
                    mods |= 0x0008
                elif pp and pp.isalnum() and len(pp) == 1:
                    vk = ord(pp.upper())
        except Exception:
            pass
    return mods, vk


def press_panel_paste():
    mods, vk = configured_panel_hotkey()
    press_hotkey([m for m in (VK_CTRL, VK_MENU, VK_SHIFT, VK_WIN) if mods & m], vk)


def hold_quick_paste_v(hold_sec=0.5):
    """按住 V 触发系统自动重复——MOD_NOREPEAT 下应只产生一次 WM_HOTKEY。"""
    seq = [_key(VK_CTRL), _key(VK_MENU), _key(VK_V)]
    assert _send_input(seq) == len(seq)
    time.sleep(hold_sec)                          # 驱动侧按住时长
    ups = [_key(VK_V, True), _key(VK_MENU, True), _key(VK_CTRL, True)]
    assert _send_input(ups) == len(ups)


def foreground() -> int:
    return int(sk._user32().GetForegroundWindow() or 0)


def focus(hwnd: int) -> bool:
    return sk.focus_window(hwnd)


def find_windows(class_name=None, title_sub=None, visible_only=True):
    out = []

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def _cb(hwnd, _lp):
        if visible_only and not sk._user32().IsWindowVisible(hwnd):
            return True
        if class_name:
            buf = ctypes.create_unicode_buffer(256)
            sk._user32().GetClassNameW(hwnd, buf, 256)
            if buf.value != class_name:
                return True
        if title_sub is not None:
            buf = ctypes.create_unicode_buffer(512)
            sk._user32().GetWindowTextW(hwnd, buf, 512)
            if title_sub not in buf.value:
                return True
        out.append(int(hwnd))
        return True

    sk._user32().EnumWindows(_cb, 0)
    return out


def window_title(hwnd) -> str:
    buf = ctypes.create_unicode_buffer(512)
    sk._user32().GetWindowTextW(hwnd, buf, 512)
    return buf.value


def syskit_pids() -> list:
    """按进程名枚举 SysKit.exe 的 pid（Toolhelp32 快照，只读）。"""
    TH32CS_SNAPPROCESS = 0x2
    k32 = ctypes.windll.kernel32

    class _PE(ctypes.Structure):
        _fields_ = [("dwSize", wintypes.DWORD), ("cntUsage", wintypes.DWORD),
                    ("th32ProcessID", wintypes.DWORD),
                    ("th32DefaultHeapID", ctypes.POINTER(wintypes.ULONG)),
                    ("th32ModuleID", wintypes.DWORD), ("cntThreads", wintypes.DWORD),
                    ("th32ParentProcessID", wintypes.DWORD), ("pcPriClassBase", ctypes.c_long),
                    ("dwFlags", wintypes.DWORD), ("szExeFile", ctypes.c_wchar * 260)]

    snap = k32.CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
    pids = []
    try:
        pe = _PE()
        pe.dwSize = ctypes.sizeof(_PE)
        ok = k32.Process32FirstW(snap, ctypes.byref(pe))
        while ok:
            if pe.szExeFile.lower() in ("syskit.exe", "syskit-rs.exe"):
                pids.append(pe.th32ProcessID)
            ok = k32.Process32NextW(snap, ctypes.byref(pe))
    finally:
        k32.CloseHandle(snap)
    return pids


def dismiss_syskit_dialog(timeout=8):
    """SysKit 的 QMessageBox（#32770 对话框）出现时按回车（确定）自动收掉。"""
    pids = set(syskit_pids())

    def _dialog_up():
        for hwnd in find_windows(class_name="#32770"):
            pid = wintypes.DWORD()
            sk._user32().GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
            if pid.value in pids and window_title(hwnd):
                return hwnd
        return 0

    if wait_until(lambda: _dialog_up(), timeout, desc="SysKit 弹窗"):
        hwnd = _dialog_up()
        if hwnd and foreground() == hwnd:
            tap(VK_RETURN)
            return True
        if hwnd:
            focus(hwnd)
            wait_until(lambda: foreground() == hwnd, 2)
            tap(VK_RETURN)
            return True
    return False


# ---------------------------------------------------------------- 剪贴板驱动

_qapp = None


def qapp():
    global _qapp
    if _qapp is None:
        _qapp = QGuiApplication.instance() or QGuiApplication(sys.argv)
    return _qapp


def clip_set_text(text):
    """原生 Win32 写剪贴板（**不用 Qt**）。

    Qt 的 Windows 剪贴板写入是**延迟渲染**：SetClipboardData(NULL) 占位，
    数据等其它进程索取时才由 Qt 窗口渲染（WM_RENDERFORMAT）。冒烟驱动的
    QApplication **从不跑事件循环**（测试体 time.sleep 阻塞主线程）→ 渲染
    请求永远得不到处理 → Rust 应用的剪贴板读取拿到空 → 「监听未开启」假象
    （上一轮 7 靶连挂的根因）。原生 ctypes 写入是立即数据，跨进程立即可读。
    """
    import ctypes
    CF_UNICODETEXT = 13
    GMEM_MOVEABLE = 0x0002
    k32 = ctypes.windll.kernel32
    u32 = ctypes.windll.user32
    # x64 句柄截断防御：显式声明 64 位 restype（本仓库已知坑）
    k32.GlobalAlloc.restype = ctypes.c_void_p
    k32.GlobalAlloc.argtypes = [ctypes.c_uint, ctypes.c_size_t]
    k32.GlobalLock.restype = ctypes.c_void_p
    k32.GlobalLock.argtypes = [ctypes.c_void_p]
    k32.GlobalUnlock.argtypes = [ctypes.c_void_p]
    u32.SetClipboardData.argtypes = [ctypes.c_uint, ctypes.c_void_p]
    u32.SetClipboardData.restype = ctypes.c_void_p
    if not u32.OpenClipboard(None):
        raise OSError("OpenClipboard 失败（被其它进程占用）")
    try:
        u32.EmptyClipboard()
        data = text.encode("utf-16-le") + bytes([0, 0])
        h = k32.GlobalAlloc(GMEM_MOVEABLE, len(data))
        ptr = k32.GlobalLock(h)
        ctypes.memmove(ptr, data, len(data))
        k32.GlobalUnlock(h)
        # 句柄所有权移交系统，不再释放
        if not u32.SetClipboardData(CF_UNICODETEXT, h):
            k32.GlobalFree(h)
            raise OSError("SetClipboardData 失败")
    finally:
        u32.CloseClipboard()


def clip_set_html(html, plain):
    mime = QMimeData()
    mime.setHtml(html)
    mime.setText(plain)
    qapp().clipboard().setMimeData(mime)


def clip_set_files(paths):
    mime = QMimeData()
    mime.setUrls([QUrl.fromLocalFile(p) for p in paths])
    qapp().clipboard().setMimeData(mime)


def clip_text() -> str:
    return qapp().clipboard().text()


def recorded_tokens() -> set:
    """从运行实例的 clipboard.json 读已记录条目文本（DPAPI 同用户可解）。"""
    p = _find_settings_file()
    if not p:
        return set()
    base = os.path.dirname(p)
    cj = os.path.join(base, "clipboard.json")
    if not os.path.exists(cj):
        return set()
    try:
        from syskit import secure_store
        data, _ = secure_store.load_sealed_json(cj, [])
        return {str(it.get("text", "")) for it in data if isinstance(it, dict)}
    except Exception:
        return set()


def copy_and_wait_recorded(token, settle=1.2):
    """真实复制 token → 等待 SysKit 监听入库；入库失败立刻 FAIL 并指明原因
    （监听未开启是最常见根因——本轮 7 靶连挂的教训，不再让它级联）。"""
    clip_set_text(token)
    ok = wait_until(lambda: token in recorded_tokens(), timeout=12.0, step=0.2,
                    desc="监听入库")
    if not ok and not wait_until(lambda: token in recorded_tokens(), timeout=12.0, step=0.2):
        pytest.fail(
            f"复制的内容 {token!r} 没有进入剪贴板历史——**监听未开启**。"
            "请到「工具箱→剪贴板历史」点「开启监听」后重跑（全部粘贴靶都依赖它）。"
        )


# ---------------------------------------------------------------- 日志采集

def _data_dir_candidates():
    cands = []
    override = os.environ.get("SYSKIT_DATA_DIR")
    if override:
        cands.append(override)
    appdata = os.environ.get("APPDATA", "")
    if appdata:
        cands.append(os.path.join(appdata, "SysKit"))
    for base in ("E:\\Software\\SysKit", "C:\\Program Files\\SysKit",
                 "C:\\Program Files (x86)\\SysKit"):
        cands.append(os.path.join(base, "data"))
    return cands


def find_syskit_log():
    """运行实例的 syskit.log：便携安装 <目录>\\data\\local\\logs，否则 %LOCALAPPDATA%。"""
    cands = []
    for d in _data_dir_candidates():
        cands.append(os.path.join(d, "local", "logs", "syskit.log"))
        cands.append(os.path.join(d, "config", "..", "local", "logs", "syskit.log"))
    la = os.environ.get("LOCALAPPDATA", "")
    if la:
        cands.append(os.path.join(la, "SysKit", "logs", "syskit.log"))
    for p in cands:
        if os.path.exists(p):
            return p
    return ""


class LogWatcher:
    """按位置增量采集 syskit.log 新增行（诊断 + 「注册失败」类断言）。"""

    def __init__(self):
        self.path = find_syskit_log()
        self.pos = os.path.getsize(self.path) if self.path else 0

    def new_lines(self):
        if not self.path:
            return []
        try:
            size = os.path.getsize(self.path)
            if size < self.pos:
                self.pos = 0                      # 日志轮转
            with open(self.path, "r", encoding="utf-8", errors="replace") as f:
                f.seek(self.pos)
                lines = f.read().splitlines()
                self.pos = f.tell()
            return lines
        except OSError:
            return []


# ---------------------------------------------------------------- 目标窗口

def _uia():
    try:
        import uiautomation as ui                 # noqa: F401
        return ui
    except ImportError:
        return None


def _read_win32_text(hwnd):
    """经典 Edit/RichEdit 子控件 WM_GETTEXT 读取（Win10 记事本等）。"""
    for klass in ("Edit", "RichEditD2DHWND", "RichEdit50W", "RichEdit20W"):
        child = sk._user32().FindWindowExW(hwnd, None, klass, None)
        if child:
            size = sk._user32().SendMessageW(child, 0x000E, 0, 0)   # WM_GETTEXTLENGTH
            buf = ctypes.create_unicode_buffer(size + 1)
            sk._user32().SendMessageW(child, 0x000D, size + 1, buf)  # WM_GETTEXT
            return buf.value
    return ""


def _read_uia_all_text(hwnd):
    """UIA 兜底读取：窗口内全部控件的 Name/Value/Text 拼接（Win11 记事本/终端）。"""
    ui = _uia()
    if ui is None:
        return None
    try:
        ctrl = ui.ControlFromHandle(hwnd)
        parts = []

        def _walk(c, depth=0):
            if depth > 12:
                return
            try:
                if c.Name:
                    parts.append(c.Name)
                pat = c.GetValuePattern() if c.GetPropertyValue(
                    c.IsValuePatternAvailableProperty) else None
                if pat is not None:
                    parts.append(pat.Value)
            except Exception:
                pass
            for ch in c.GetChildren():
                _walk(ch, depth + 1)

        _walk(ctrl)
        return "\n".join(parts)
    except Exception:
        return None


def launch_notepad():
    """启动记事本并聚焦；返回 hwnd（标题含 记事本/Notepad 的新可见窗口）。"""
    before = set(find_windows(title_sub=""))
    os.startfile("notepad.exe")

    def _new_notepad():
        hits = [x for x in find_windows()
                if x not in before and ("记事本" in window_title(x)
                                        or "Notepad" in window_title(x))]
        return hits[0] if hits else 0

    assert wait_until(lambda: _new_notepad(), 12), "记事本窗口未出现"
    hwnd = _new_notepad()
    focus(hwnd)
    wait_until(lambda: foreground() == hwnd, 3)
    return hwnd


def read_notepad(hwnd):
    text = _read_win32_text(hwnd)
    if text:
        return text
    uia_text = _read_uia_all_text(hwnd)
    return uia_text or ""


def launch_terminal(admin=False):
    """Windows Terminal 优先，退而 PowerShell。admin 走 ShellExecute runas（弹 UAC）。"""
    if admin:
        rc = ctypes.windll.shell32.ShellExecuteW(
            None, "runas", "powershell.exe", None, None, 1)  # SW_SHOWNORMAL
        if rc <= 32:
            pytest.skip("管理员 PowerShell 启动被拒（UAC 取消）")
        title_sub = "管理员"
    else:
        try:
            os.startfile("wt.exe")
            title_sub = None
        except OSError:
            os.startfile("powershell.exe")
            title_sub = "Windows PowerShell"
    def _find():
        hits = [h for h in find_windows()
                if (title_sub is None or title_sub in window_title(h))
                and ("Windows Terminal" in window_title(h)
                     or "PowerShell" in window_title(h))]
        return hits[0] if hits else 0
    assert wait_until(lambda: _find(), 15), "终端窗口未出现"
    hwnd = _find()
    focus(hwnd)
    wait_until(lambda: foreground() == hwnd, 3)
    return hwnd


def read_terminal(hwnd):
    text = _read_uia_all_text(hwnd)
    if text:
        return text
    return _read_win32_text(hwnd)                  # conhost 的 Text 控件


def word_session():
    """COM 起 Word 并新建文档；返回 (app, doc, hwnd)。缺 pywin32 → skip。"""
    try:
        import win32com.client                      # noqa: F401
    except ImportError:
        pytest.skip("需要 pywin32（pip install pywin32）")
    try:
        app = win32com.client.Dispatch("Word.Application")
    except Exception as e:
        pytest.skip(f"Word 不可用（未安装 Office？）：{e}")
    app.Visible = True
    doc = app.Documents.Add()
    hwnd = int(app.ActiveWindow.Hwnd)
    focus(hwnd)
    wait_until(lambda: foreground() == hwnd, 5)
    return app, doc, hwnd


def excel_session():
    try:
        import win32com.client
    except ImportError:
        pytest.skip("需要 pywin32")
    try:
        app = win32com.client.Dispatch("Excel.Application")
    except Exception as e:
        pytest.skip(f"Excel 不可用（未安装 Office？）：{e}")
    app.Visible = True
    wb = app.Workbooks.Add()
    hwnd = int(app.ActiveWindow.Hwnd)
    focus(hwnd)
    wait_until(lambda: foreground() == hwnd, 5)
    return app, wb, hwnd


def dialog_titles_present(sub):
    return [window_title(h) for h in find_windows() if sub in window_title(h)]


def close_com(app):
    try:
        app.DisplayAlerts = False
    except Exception:
        pass
    try:
        app.Quit()
    except Exception:
        pass


# ---------------------------------------------------------------- SysKit 前置

def require_syskit_running():
    if not syskit_pids():
        pytest.skip("SysKit 未运行（请用 22412e8 之后构建的新 exe 启动）")


def require_watch_hint():
    # 无法从进程外确认监听开关；历史为空时给出明确指引用户自查
    pass


TOKEN = "SMOKE-{}-{}"


def unique_token(tag):
    return TOKEN.format(tag, int(time.time() * 1000) % 100000)


# ================================================================ 用例

class TestTarget1Notepad:
    """靶 1：记事本——直贴 + 触发次数语义（NOREPEAT 相关，含规格校正说明）。"""

    def test_quick_paste_direct(self):
        require_syskit_running()
        token = unique_token("np")
        hwnd = launch_notepad()
        copy_and_wait_recorded(token)
        press_quick_paste()
        assert wait_until(lambda: token in read_notepad(hwnd), 5), \
            f"直贴失败：目标未见 token（剪贴板={clip_text()!r}）"

    def test_single_trigger_pastes_once(self):
        """单次触发恰好一次粘贴（防 double-SendInput / 双信号回归）。

        规格校正：SendInput 注入的 keydown **不会**产生硬件自动重复（重复由
        键盘栈对物理键生成），因此「按住 V」经注入只有一次 keydown——本用例
        验证一次触发只粘一次；MOD_NOREPEAT 标志位本身已由离屏单测
        test_register_sets_norepeat 钉住（RegisterHotKey 入参断言）。
        """
        require_syskit_running()
        token = unique_token("nr")
        hwnd = launch_notepad()
        copy_and_wait_recorded(token)
        hold_quick_paste_v(0.3)                      # 注入语义：仅一次 keydown
        assert wait_until(lambda: token in read_notepad(hwnd), 5)
        time.sleep(1.0)                              # 留给任何重复触发显形的时间窗
        text = read_notepad(hwnd)
        assert text.count(token) == 1, \
            f"单次触发出现 {text.count(token)} 次（预期恰好 1 次）"

    def test_triple_discrete_press_pastes_thrice(self):
        """规格校正用例：修饰键按住期间**离散**连按 V 三次 = 三次独立 WM_HOTKEY
        （NOREPEAT 只抑制按住自动重复，不抑制离散重按）→ 恰好粘 3 次。
        若实现误把 NOREPEAT 当「全局去重」，此用例会以 count==1 失败。"""
        require_syskit_running()
        token = unique_token("tp")
        hwnd = launch_notepad()
        copy_and_wait_recorded(token)
        for _ in range(3):
            press_hotkey([VK_CTRL, VK_MENU], VK_V)
            time.sleep(0.35)                         # 留出每条粘贴链执行间隔
        assert wait_until(lambda: (read_notepad(hwnd) or "").count(token) >= 1, 6)
        time.sleep(1.5)                              # 等最后一条链收尾
        count = (read_notepad(hwnd) or "").count(token)
        assert count == 3, f"三次离散按压应粘 3 次，实测 {count} 次"


class TestPanelFlowTargets:
    """靶 2/3/4：Ctrl+Win+V 弹面板 → 回车选最新 → 直贴。读取失败靶自动 skip。"""

    @staticmethod
    def _panel_paste_and_read(setup_target, reader, tag):
        if not panel_hotkey_configured():
            pytest.skip("面板热键未配置（settings.json clip_paste_hotkey 为空）")
        require_syskit_running()
        hwnd, _extra = setup_target()
        token = unique_token(tag)
        copy_and_wait_recorded(token)
        press_panel_paste()
        if not wait_until(lambda: _panel_visible(), 5):
            log_tail = chr(10).join(LogWatcher().new_lines()[-6:])
            hint = "面板热键被占用已禁用（见日志「被占，已回退为禁用」）——请在设置页改用其他组合"                 if "被占" in log_tail else "面板未弹出（热键未生效？）"
            pytest.fail(f"快捷粘贴面板未弹出：{hint}；日志尾部：{log_tail[-300:]}")
        time.sleep(0.4)
        tap(VK_RETURN)                                # 行 0 = 最新（刚复制的 token）
        ok = wait_until(lambda: token in (reader(hwnd) or ""), 6)
        if not ok:
            if reader(hwnd) is None:
                pytest.skip("目标窗口内容无法经 UIA 读取（靶结构性限制），链路行为见剪贴板断言")
            pytest.fail(f"面板粘贴未到达目标：token={token!r} "
                        f"目标内容尾部={(reader(hwnd) or '')[-200:]!r}")
        return token

    def test_browser(self):
        """前置：手动开一个 Chrome/Edge 并停在任意页面。流程：Ctrl+L 聚焦
        地址栏 → Ctrl+Win+V 面板 → 还焦后地址栏仍持焦 → 回车粘贴落入地址栏。"""
        hwnds = [h for h in find_windows(class_name="Chrome_WidgetWin_1")
                 if window_title(h)]
        if not hwnds:
            pytest.skip("未发现 Chrome/Edge 窗口（Chrome_WidgetWin_1）——请先打开浏览器")
        hwnd = hwnds[0]
        focus(hwnd)
        wait_until(lambda: foreground() == hwnd, 3)
        press_hotkey([VK_CTRL], VK_L)                 # 聚焦地址栏（面板还焦后仍持焦）
        time.sleep(0.4)
        self._panel_paste_and_read(lambda: (hwnd, None),
                                   lambda h: _read_uia_all_text(h), "br")

    def test_wechat(self):
        hwnds = [h for h in find_windows() if "微信" in window_title(h)
                 or "WeChat" in window_title(h)]
        if not hwnds:
            pytest.skip("未发现微信窗口——请先打开微信并停在任意聊天")
        hwnd = hwnds[0]
        focus(hwnd)
        wait_until(lambda: foreground() == hwnd, 3)
        token = self._panel_paste_and_read(lambda: (hwnd, None),
                                           lambda h: _read_uia_all_text(h), "wx")
        assert token

    def test_windows_terminal(self):
        hwnd = launch_terminal(admin=False)
        before = read_terminal(hwnd)
        token = self._panel_paste_and_read(lambda: (hwnd, None),
                                           lambda h: read_terminal(h), "wt")
        assert token and token in (read_terminal(hwnd) or "") and token not in (before or "")


def panel_hotkey_configured() -> bool:
    p = _find_settings_file()
    if not p:
        return True
    try:
        import json as _json
        return bool(str(_json.load(open(p, encoding="utf-8")).get("clip_paste_hotkey", "")).strip())
    except Exception:
        return True


def _panel_visible():
    # 面板是无边框 Qt.Tool 窗口：以「前台进程属于 SysKit」为面板可见代理
    fg = foreground()
    pid = wintypes.DWORD()
    sk._user32().GetWindowThreadProcessId(fg, ctypes.byref(pid))
    return pid.value in set(syskit_pids())


class TestTarget5Word:
    """靶 5：Word——html 富文本保真 + Ctrl+Alt+V 覆盖「选择性粘贴」。"""

    def test_html_fidelity_and_paste_special_override(self):
        require_syskit_running()
        app, doc, hwnd = word_session()
        try:
            token = unique_token("wd")
            bold_part = "<b>SMOKE-BOLD-" + token + "</b>"
            clip_set_html(bold_part, token)
            time.sleep(1.2)                           # SysKit 记录 kind=html
            press_quick_paste()                       # 直贴（顺带覆盖选择性粘贴）
            assert wait_until(lambda: token in doc.Content.Text, 6), \
                "Word 未收到粘贴（html 写回/注入失败）"
            # 富文本保真：token 所在 Range 应为粗体（setHtml 生效）
            rng = doc.Content.Duplicate
            rng.Find.Execute(FindText=token)
            assert rng.Find.Found, "token 范围未定位"
            assert rng.Font.Bold != 0, "粘贴为纯文本：html 粗体丢失（setHtml 未生效？）"
            # 覆盖「选择性粘贴」：1.5s 内不得出现该对话框
            time.sleep(1.5)
            assert not dialog_titles_present("选择性粘贴"), \
                "Word 弹出了选择性粘贴——SysKit 热键未拦截"
        finally:
            close_com(app)


class TestTarget6Excel:
    """靶 6：Excel——Ctrl+Alt+V 覆盖「选择性粘贴」。"""

    def test_paste_special_override(self):
        require_syskit_running()
        app, wb, hwnd = excel_session()
        try:
            token = unique_token("xl")
            copy_and_wait_recorded(token)
            press_quick_paste()
            sh = wb.ActiveSheet
            assert wait_until(lambda: token in str(sh.UsedRange.Text or "") or
                              token in str(sh.Range("A1").Text or ""), 6), \
                "Excel 未收到粘贴"
            time.sleep(1.5)
            assert not dialog_titles_present("选择性粘贴"), \
                "Excel 弹出了选择性粘贴——SysKit 热键未拦截"
        finally:
            close_com(app)


class TestTarget7Explorer:
    """靶 7：资源管理器——file 条目粘出真文件（uri-list，非路径文本）。"""

    def test_file_entry_true_paste(self, tmp_path):
        if not panel_hotkey_configured():
            pytest.skip("面板热键未配置")
        require_syskit_running()
        src = tmp_path / "smoke_src.txt"
        src.write_text("smoke-payload", encoding="utf-8")
        dst_dir = tmp_path / "dst"
        dst_dir.mkdir()
        os.startfile(str(dst_dir))
        assert wait_until(lambda: self._explorer_of(dst_dir) != 0, 12), \
            "目标文件夹窗口未出现"
        hwnd = self._explorer_of(dst_dir)
        focus(hwnd)
        wait_until(lambda: foreground() == hwnd, 3)
        clip_set_files([str(src)])
        time.sleep(1.2)                               # SysKit 记录 kind=file
        press_panel_paste()
        assert wait_until(lambda: _panel_visible(), 5), "面板未弹出"
        time.sleep(0.4)
        tap(VK_RETURN)                                # 行 0 = 最新（文件条目）
        dst = dst_dir / "smoke_src.txt"
        assert wait_until(lambda: dst.exists(), 8), \
            "文件未被粘贴（写回应走 QMimeData uri-list；若目标出现的是路径文本则违反硬约束⑦）"
        assert dst.read_text(encoding="utf-8") == "smoke-payload"
        # 反证：目标目录不得出现「以条目文本命名的 .txt 路径文件」
        strays = [p.name for p in dst_dir.iterdir() if p.name != "smoke_src.txt"]
        assert not strays, f"发现疑似路径文本落盘：{strays}"

    @staticmethod
    def _explorer_of(path):
        name = os.path.basename(str(path))
        for h in find_windows(class_name="CabinetWClass"):
            if name in window_title(h):
                return h
        return 0


class TestTarget8Admin:
    """靶 8 / 边界 c：管理员态终端——UIPI 拦截与降级行为。"""

    def test_uipi_degradation(self):
        require_syskit_running()
        elevated_syskit = _syskit_elevated()
        hwnd = launch_terminal(admin=True)            # 会弹 UAC，人工点「是」
        token = unique_token("adm")
        copy_and_wait_recorded(token)
        before = read_terminal(hwnd) or ""
        press_quick_paste()
        time.sleep(2.0)
        after = read_terminal(hwnd) or ""
        if elevated_syskit:
            if token in after and token not in before:
                pytest.skip("SysKit 以管理员运行（打包版 uac_admin 预期）：UIPI 不拦截，"
                            "粘贴直达——本靶仅在非管理员 SysKit 下有意义")
            pytest.fail("SysKit 为管理员态但粘贴未送达——超出本靶预期，人工排查")
        # 非管理员 SysKit：UIPI 拦截 → 目标不变 + 内容已在剪贴板（降级行为代理）
        assert token in clip_text(), "写回未发生（应先写回再注键）"
        assert token not in after, \
            "非管理员 SysKit 竟把内容贴进了管理员窗口——UIPI 未拦截？检查完整性"
        # 托盘气泡文案「已复制；自动粘贴未送达，请按 Ctrl+V」不可进程外读取，
        # 人工瞄一眼气泡作为清单项（见期望/失败特征表）。


def _syskit_elevated():
    """SysKit 主进程是否提权（TokenElevation 查询；查询失败按 False 处理）。"""
    advapi = ctypes.windll.advapi32
    k32 = ctypes.windll.kernel32
    try:
        for pid in syskit_pids():
            PROCESS_QUERY_LIMITED_INFORMATION = 0x1000
            h = k32.OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, False, pid)
            if not h:
                continue
            try:
                token = wintypes.HANDLE()
                if advapi32_open_process_token(advapi, h, ctypes.byref(token)):
                    elevation = wintypes.DWORD()
                    ret_len = wintypes.DWORD()
                    TokenElevation = 20
                    ok = advapi.GetTokenInformation(token, TokenElevation,
                                                    ctypes.byref(elevation),
                                                    ctypes.sizeof(elevation),
                                                    ctypes.byref(ret_len))
                    k32.CloseHandle(token)
                    if ok and elevation.value:
                        return True
            finally:
                k32.CloseHandle(h)
    except Exception:
        return False
    return False


def advapi32_open_process_token(advapi, proc_handle, out_token):
    TOKEN_QUERY = 0x8
    return bool(advapi.OpenProcessToken(proc_handle, TOKEN_QUERY,
                                        ctypes.byref(out_token)))


class TestEdgeAFallbackChain:
    """边界 a：占住 Ctrl+Win+V → 设置页改键保存 → 即时重挂回退链。

    Rust 版已实现即时重注册（Step 2）：sk_settings_set 保存热键键 →
    apply_hotkeys 走链（当前值 → Ctrl+Alt+V 预设(快速键未占时) → 禁用），
    回退结果持久化 settings.json。需人工配合：按提示在设置页操作。
    回退提示经 webview message 呈现（无原生对话框），断言只看 settings.json。
    """

    def test_fallback_chain_when_ctrl_win_v_occupied(self, tmp_path):
        require_syskit_running()
        settings_path = _find_settings_file()
        if not settings_path:
            pytest.skip("未定位到运行实例的 settings.json（用 SYSKIT_DATA_DIR 指定后重跑）")
        import json
        before = json.load(open(settings_path, encoding="utf-8"))
        panel_before = before.get("clip_paste_hotkey", "")
        if panel_before != "Ctrl+Win+V":
            pytest.skip(f"面板热键当前为 {panel_before!r}（需为 Ctrl+Win+V 再跑本靶）")

        log = LogWatcher()
        occupier = _Occupier()
        ahk = shutil.which("AutoHotkey.exe") or shutil.which("AutoHotkeyU64.exe")
        if ahk:
            import subprocess as _sp
            ahk_script = tmp_path / "occupy.ahk"
            ahk_script.write_text("^#v::Return" + chr(10), encoding="utf-8")
            ahk_proc = _sp.Popen([ahk, str(ahk_script)])
            time.sleep(1.0)
            def release():
                ahk_proc.terminate()   # 旧实现 startfile 起的 AHK 永不退出，
                                       # 占住 Ctrl+Win+V 毒化后续每一次运行
        else:
            occupier.acquire()
            release = occupier.release
        try:
            print(chr(10) + "[边界a] 请在 Rust 版设置页把面板粘贴热键改为 Ctrl+Win+V 并保存…")
            changed = wait_until(lambda: _panel_key_changed(settings_path, panel_before),
                                 90, step=0.5)
            if not changed:
                pytest.skip("90s 内未检测到保存动作（人工步骤未执行），本靶跳过")
            after = json.load(open(settings_path, encoding="utf-8"))
            panel_after = after.get("clip_paste_hotkey", "")
            quick_key = str(after.get("clip_quick_hotkey", "")).strip().lower()
            if quick_key == "ctrl+alt+v":
                assert panel_after == "",                     f"回退链未按预期禁用面板键：当前 {panel_after!r}（预期 ''）"
            else:
                assert panel_after == "Ctrl+Alt+V",                     f"回退链未回退到 Ctrl+Alt+V：当前 {panel_after!r}"
            lines = chr(10).join(log.new_lines())
            print(chr(10) + "[边界a] SysKit(Rust) 日志尾部：" + chr(10) + lines[-800:])
            print(chr(10) + "[边界a] 结果：面板键 " + repr(panel_before) + " → "
                  + repr(panel_after) + "；恢复方法：退出占用源后在设置页改回并保存。")
        finally:
            release()


def _Occupier():
    class _Occ:
        def acquire(self):
            MOD_CTRL_WIN = 0x0002 | 0x0008
            self.ok = bool(ctypes.windll.user32.RegisterHotKey(
                None, 0x5991, MOD_CTRL_WIN | 0x4000, VK_V))
            if not self.ok:
                pytest.skip("本进程注册 Ctrl+Win+V 失败（已被占？）——占位环境不成立")

        def release(self):
            try:
                ctypes.windll.user32.UnregisterHotKey(None, 0x5991)
            except Exception:
                pass
    return _Occ()


def _find_settings_file():
    for d in _data_dir_candidates():
        p = os.path.join(d, "config", "settings.json")
        if os.path.exists(p):
            return p
        p2 = os.path.join(d, "settings.json")
        if os.path.exists(p2):
            return p2
    return ""


def _panel_key_changed(settings_path, panel_before):
    try:
        import json
        cur = json.load(open(settings_path, encoding="utf-8")).get("clip_paste_hotkey", "")
        return cur != panel_before
    except Exception:
        return False


class TestEdgeBFocusDrift:
    """边界 b：热键触发后 50ms 守卫读前台前切走 → 取消注键（硬约束⑩）。

    判定（行为代理）：取消 = 剪贴板已有 token 且 1 号记事本未出现 token；
    若粘贴直达（未命中漂移窗口）自动重试，3 次未命中 skip（时序概率，不算失败）。
    """

    def test_focus_drift_cancels_paste(self):
        require_syskit_running()
        target = launch_notepad()
        third = launch_notepad()
        for attempt in range(3):
            token = unique_token(f"drift{attempt}")
            copy_and_wait_recorded(token)
            focus(target)
            wait_until(lambda: foreground() == target, 3)
            press_quick_paste()
            focus(third)                              # 立刻切走：抢在守卫读前台之前
            time.sleep(2.0)
            pasted = token in (read_notepad(target) or "")
            cancelled = (not pasted) and token in clip_text()
            if cancelled:
                return                                # 守卫正确放弃注键
            if pasted:
                continue                              # 未命中漂移窗口 → 重试
            pytest.fail("既未粘贴也未取消：写回/链路异常（剪贴板无 token）")
        pytest.skip("3 次未命中 50ms 漂移窗口（时序概率）；守卫行为已在离屏用例覆盖")
